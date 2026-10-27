//! CI wire types: requests, run records and conclusions, shared by the
//! daemon, the CLI, MCP and the Python client.

use std::{path::Path, time::Duration};

use crate::github_ci_policy::GithubWrites;
use serde::{Deserialize, Serialize};

use super::{
    model::RunTree,
    params::RunParams,
    provider::{self, Mode, Provider, Trigger},
    scheduler::RunKey,
    snapshot::BaseRef,
};

pub const SCHEMA_VERSION: u32 = 1;
/// Default per-call caps; agents never receive unbounded output.
pub const DEFAULT_LOG_PAGE_RECORDS: usize = 500;
pub const MAX_LOG_PAGE_BYTES: usize = 256 * 1024;
pub const DEFAULT_LOG_PAGE_BYTES: usize = 48 * 1024;
pub const DEFAULT_REPORT_TAIL: usize = 40;
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
pub(super) const MAX_RUN_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);

vocabulary!(
    /// Where a run is in its life.
    RunState, "state" { Queued => "queued", Running => "running", Done => "done" }
);

vocabulary!(
    /// Run conclusion. Exit codes: success 0, failure/error 1,
    /// cancelled/timed_out 2, refused/incomplete 3.
    Conclusion, "conclusion" {
        Success => "success",
        Failure => "failure",
        Cancelled => "cancelled",
        TimedOut => "timed_out",
        /// Some jobs could not run here, or none succeeded; never a pass.
        Incomplete => "incomplete",
        Refused => "refused",
        /// The engine or its cleanup failed; never a pass.
        Error => "error",
    }
);

impl Conclusion {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Failure | Self::Error => 1,
            Self::Cancelled | Self::TimedOut => 2,
            Self::Refused | Self::Incomplete => 3,
        }
    }
}

/// A submission. Built by [`crate::Client::ci_submit`] after it snapshots the
/// workspace into the daemon's staging area.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitRequest {
    pub staging: String,
    pub workspace: String,
    pub provider: Provider,
    pub engine: String,
    pub workflow: String,
    pub job: Option<String>,
    pub trigger: Trigger,
    pub mode: Mode,
    pub actor: String,
    pub sha: String,
    pub branch: Option<String>,
    pub tree_digest: String,
    /// Git tree of the effective frozen checkout, absent in legacy receipts.
    /// Source identity does not authorize cache writers or compatible outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_tree: Option<String>,
    pub dirty: bool,
    /// The synthetic commit holding the uncommitted work (dirty runs only):
    /// the job checks it out, and the event payload names it.
    #[serde(default)]
    pub commit: Option<String>,
    /// The base branch a `pr` run's snapshot carries as `origin/<branch>`;
    /// the event payload names it (#403).
    #[serde(default)]
    pub base: Option<BaseRef>,
    pub origin: Option<String>,
    pub pr_number: Option<u64>,
    pub timeout_secs: Option<u64>,
    /// Opt-in daemon-owned secrets by name (only `github_token`).
    #[serde(default)]
    pub secrets: Vec<String>,
    /// `--github-proxy` (#650): a per-run fake `GITHUB_TOKEN` swapped for the
    /// host credential by the daemon, with this writes mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_proxy: Option<GithubWrites>,
    /// Workflow inputs, a matrix filter and extra env (#430).
    #[serde(default, skip_serializing_if = "RunParams::is_empty")]
    pub params: RunParams,
}

impl SubmitRequest {
    /// Semantic checks the daemon applies before touching any file. The
    /// staging directory itself is checked by the runtime.
    pub fn validate(&self) -> Result<(), CiError> {
        let refuse = |m: &str| Err(CiError::refused(m));
        if !valid_uuid(&self.staging) {
            return refuse("invalid staging ID");
        }
        if !valid_sha(&self.sha) || !valid_hex(&self.tree_digest, 64) {
            return refuse("invalid SHA or tree digest");
        }
        if self
            .git_tree
            .as_deref()
            .is_some_and(|tree| !valid_sha(tree))
        {
            return refuse("invalid Git tree identity");
        }
        if self
            .commit
            .as_deref()
            .is_some_and(|c| !self.dirty || !valid_sha(c))
        {
            return refuse("a synthetic commit is a SHA, and only for a dirty tree");
        }
        if self
            .base
            .as_ref()
            .is_some_and(|b| self.trigger != Trigger::Pr || !b.is_valid())
        {
            return refuse("a base is a branch and a SHA, and only for a pr run");
        }
        provider::require_supported(self.provider).map_err(CiError::refused)?;
        if self.engine != "act" {
            return refuse("only the act engine is supported");
        }
        if !Path::new(&self.workspace).is_absolute() || self.workspace.len() > 4096 {
            return refuse("workspace must be an absolute path");
        }
        if !valid_workflow(&self.workflow) {
            return refuse("workflow must be a file under .github/workflows/");
        }
        if self.job.as_deref().is_some_and(|j| !valid_job(j)) {
            return refuse("invalid job ID");
        }
        if self.actor.is_empty() || self.actor.len() > 200 {
            return refuse("invalid actor");
        }
        if self.secrets.len() > 4
            || self
                .secrets
                .iter()
                .any(|s| crate::secrets::secret_env_name(s).is_none())
        {
            return refuse("unknown secret (only github_token is supported)");
        }
        if self.github_proxy.is_some() && !self.secrets.is_empty() {
            return refuse("--github-proxy replaces --github-token; pass one of them");
        }
        self.params
            .validate(self.trigger)
            .map_err(CiError::refused)?;
        provider::validate(self.trigger, self.mode, self.dirty).map_err(CiError::refused)
    }

    pub fn timeout(&self) -> Duration {
        self.timeout_secs
            .map_or(DEFAULT_RUN_TIMEOUT, Duration::from_secs)
            .min(MAX_RUN_TIMEOUT)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunnerAction {
    List,
    Drain,
    Resume,
    SetLimit {
        limit: usize,
    },
    PruneCache {
        older_than_secs: Option<u64>,
        max_bytes: Option<u64>,
    },
    /// Measure the machine-wide cache volume.
    CacheUsage,
    /// Remove the machine-wide cache volume (refused while a run executes).
    ClearCache,
}

/// Every CI operation. Each maps to one typed handler; there is no generic
/// command surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum CiRequest {
    Submit {
        request: Box<SubmitRequest>,
    },
    List {
        workspace: Option<String>,
        state: Option<RunState>,
        limit: Option<usize>,
    },
    Show {
        run: String,
        tree: Option<bool>,
    },
    Logs {
        run: String,
        job: Option<String>,
        section: Option<String>,
        since_seq: Option<u64>,
        limit: Option<usize>,
        max_bytes: Option<usize>,
    },
    Cancel {
        run: String,
    },
    /// The submitting client is exiting: cancel the run unless another
    /// owner still holds it. The owner is the request's own (see
    /// [`crate::ClientOwner`]); detached runs are never released.
    Release {
        run: String,
    },
    Retry {
        run: String,
        job: Option<String>,
    },
    Report {
        run: String,
        tail: Option<usize>,
    },
    Runners {
        action: RunnerAction,
    },
    /// act2's `act doctor` inside the machine's shared engine (#547).
    Doctor,
    /// A widget process starting (`explicit` = typed by the user).
    WidgetHello {
        pid: u32,
        session: String,
        explicit: bool,
        /// None from a widget built before the release handshake.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release: Option<String>,
    },
    /// The widget's heartbeat; the reply carries its pending commands.
    WidgetPoll {
        pid: u32,
    },
    /// A deliberate quit from the widget's menu.
    WidgetDismiss {
        session: String,
        /// Missing from widgets predating the release handoff.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
    },
    /// Queue a command for the widget (from the dashboard or `bosn ui`).
    WidgetCommand {
        command: super::widget::WidgetCommand,
    },
    /// A single-use dashboard link (only from the owner-only socket).
    UiGrant {
        path: Option<String>,
    },
}

/// How long a client waits for a CI reply. Most requests answer from
/// memory; the slow ones copy a source tree (retry), scan a whole log
/// (report) or measure every run (a size-bounded prune).
pub const REPLY_DEADLINE: Duration = Duration::from_secs(30);
pub const SLOW_REPLY_DEADLINE: Duration = Duration::from_secs(10 * 60);

impl CiRequest {
    pub fn reply_deadline(&self) -> Duration {
        match self {
            Self::Retry { .. }
            | Self::Report { .. }
            | Self::Doctor
            | Self::Runners {
                action:
                    RunnerAction::PruneCache { .. }
                    | RunnerAction::CacheUsage
                    | RunnerAction::ClearCache,
            } => SLOW_REPLY_DEADLINE,
            _ => REPLY_DEADLINE,
        }
    }
}

/// A bounded stage observation. Wall-clock start is informational; completed
/// durations are measured with a monotonic clock. Interrupted observations have
/// no elapsed duration because that clock does not survive daemon restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StageTiming {
    pub kind: StageKind,
    pub state: StageState,
    pub started_at: f64,
    pub elapsed_ms: Option<u64>,
    /// Bounded classification, never raw command output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<StageReason>,
    pub outer_limit_secs: u64,
    /// Separately bounded command inside the phase, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_limit_secs: Option<u64>,
    /// Whole-run budget left at phase start, when observed by a live runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_remaining_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    EnginePrepare,
    RecoveryReservation,
    EnrollmentFence,
    EnrollmentComplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StageReason {
    OuterDeadline,
    BackendError,
    Cancelled,
    RunDeadline,
    Interrupted,
}

impl StageReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OuterDeadline => "outer_deadline",
            Self::BackendError => "backend_error",
            Self::Cancelled => "cancelled",
            Self::RunDeadline => "run_deadline",
            Self::Interrupted => "interrupted",
        }
    }
}

impl StageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnginePrepare => "engine_prepare",
            Self::RecoveryReservation => "recovery_reservation",
            Self::EnrollmentFence => "enrollment_fence",
            Self::EnrollmentComplete => "enrollment_complete",
        }
    }
}

impl StageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

/// In-process observation; elapsed values are measured by the phase producer.
#[derive(Clone, Copy, Debug)]
pub enum StageEvent {
    Started {
        kind: StageKind,
        outer_limit_secs: u64,
        command_limit_secs: Option<u64>,
        run_remaining_ms: Option<u64>,
    },
    Finished {
        kind: StageKind,
        state: StageState,
        elapsed_ms: u64,
    },
    Aborted {
        state: StageState,
        reason: StageReason,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunRecord {
    pub schema_version: u32,
    pub id: String,
    pub provider: Provider,
    pub engine: String,
    pub workspace: String,
    pub repository: String,
    pub sha: String,
    pub branch: Option<String>,
    /// Tree digest when the run used uncommitted work (`sha + dirty`).
    pub dirty: Option<String>,
    /// The synthetic commit the job checked out when `dirty`: `sha` plus the
    /// uncommitted work, so a workflow cleaning its tree still builds it.
    #[serde(default)]
    pub commit: Option<String>,
    pub tree_digest: String,
    /// Git tree of the effective frozen checkout, absent in legacy receipts.
    /// Source identity does not authorize cache writers or compatible outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_tree: Option<String>,
    pub workflow: String,
    pub job: Option<String>,
    pub trigger: Trigger,
    pub mode: Mode,
    pub actor: String,
    pub event: String,
    pub payload_sha256: String,
    pub state: RunState,
    /// Accepted cancel request; teardown still owns the scheduler slot until
    /// the run reaches `done`. Optional for older peers and saved records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_requested_at: Option<f64>,
    pub conclusion: Option<Conclusion>,
    pub reason: Option<String>,
    pub created_at: f64,
    pub started_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub act_exit_code: Option<i32>,
    pub cleanup: Option<String>,
    pub engine_id: Option<String>,
    pub act_version: String,
    /// Pins captured by the daemon at submission; absent in legacy receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub execution_pins: Option<bosn_core::act::ActPins>,
    pub runner_image: String,
    pub submitters: u32,
    pub retry_of: Option<String>,
    pub timeout_secs: u64,
    /// Latest bounded setup phase observations; absent in legacy records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<StageTiming>,
    /// Opt-in secret names; values never leave the daemon's secret store.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// `--github-proxy` and its writes mode (#650); no value is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_proxy: Option<GithubWrites>,
    pub log_records: u64,
    #[serde(default)]
    pub tree: RunTree,
    /// Workflow inputs, matrix filter and extra env (#430); omitted when none.
    #[serde(default, skip_serializing_if = "RunParams::is_empty")]
    pub params: RunParams,
}

impl RunRecord {
    /// A queued record for a validated submission.
    pub fn queued(id: String, request: &SubmitRequest, event: &str, payload: &[u8]) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id,
            provider: request.provider,
            engine: request.engine.clone(),
            workspace: request.workspace.clone(),
            repository: provider::repository(request.origin.as_deref()),
            sha: request.sha.clone(),
            branch: request.branch.clone(),
            dirty: request.dirty.then(|| request.tree_digest.clone()),
            commit: request.commit.clone(),
            tree_digest: request.tree_digest.clone(),
            git_tree: request.git_tree.clone(),
            workflow: request.workflow.clone(),
            job: request.job.clone(),
            trigger: request.trigger,
            mode: request.mode,
            actor: request.actor.clone(),
            event: event.into(),
            payload_sha256: sha256_hex(payload),
            state: RunState::Queued,
            cancel_requested_at: None,
            conclusion: None,
            reason: None,
            created_at: super::lifecycle::now_seconds(),
            started_at: None,
            finished_at: None,
            act_exit_code: None,
            cleanup: None,
            engine_id: None,
            act_version: super::engine::ACT_VERSION.into(),
            execution_pins: super::pins::execution_pins(),
            runner_image: super::engine::RUNNER_IMAGE.into(),
            submitters: 1,
            retry_of: None,
            timeout_secs: request.timeout().as_secs(),
            stages: Vec::new(),
            secrets: request.secrets.clone(),
            github_proxy: request.github_proxy,
            log_records: 0,
            tree: RunTree::default(),
            params: request.params.clone(),
        }
    }

    /// A queued re-run of a finished run, optionally narrowed to one job.
    pub fn retry(&self, id: String, job: Option<String>) -> Self {
        Self {
            id,
            job: job.or_else(|| self.job.clone()),
            state: RunState::Queued,
            cancel_requested_at: None,
            conclusion: None,
            reason: None,
            created_at: super::lifecycle::now_seconds(),
            started_at: None,
            finished_at: None,
            act_exit_code: None,
            cleanup: None,
            engine_id: None,
            submitters: 1,
            retry_of: Some(self.id.clone()),
            log_records: 0,
            tree: RunTree::default(),
            stages: Vec::new(),
            act_version: super::engine::ACT_VERSION.into(),
            execution_pins: super::pins::execution_pins(),
            runner_image: super::engine::RUNNER_IMAGE.into(),
            ..self.clone()
        }
    }

    /// The act cache-server namespace: one store per repository identity
    /// (the origin repository, else the checkout path).
    pub fn cache_namespace(&self) -> String {
        let identity = if self.repository == super::provider::LOCAL_REPOSITORY {
            &self.workspace
        } else {
            &self.repository
        };
        sha256_hex(identity.as_bytes())[..16].to_string()
    }

    pub(crate) fn stage_started(
        &mut self,
        kind: StageKind,
        started_at: f64,
        outer_limit_secs: u64,
        command_limit_secs: Option<u64>,
        run_remaining_ms: Option<u64>,
    ) {
        if let Some(previous) = self
            .stages
            .last_mut()
            .filter(|stage| stage.state == StageState::Running)
        {
            previous.state = StageState::Interrupted;
            previous.reason = Some(StageReason::Interrupted);
        }
        // The setup path can repeat after retirement; cap records independent of run duration.
        if self.stages.len() == 16 {
            self.stages.remove(0);
        }
        self.stages.push(StageTiming {
            kind,
            state: StageState::Running,
            started_at,
            elapsed_ms: None,
            reason: None,
            outer_limit_secs,
            command_limit_secs,
            run_remaining_ms,
        });
    }

    pub(crate) fn stage_finished(&mut self, kind: StageKind, state: StageState, elapsed_ms: u64) {
        if let Some(stage) = self
            .stages
            .last_mut()
            .filter(|stage| stage.kind == kind && stage.state == StageState::Running)
        {
            stage.state = state;
            stage.reason = match state {
                StageState::TimedOut => Some(StageReason::OuterDeadline),
                StageState::Failed => Some(StageReason::BackendError),
                StageState::Cancelled => Some(StageReason::Cancelled),
                StageState::Interrupted => Some(StageReason::Interrupted),
                StageState::Running | StageState::Succeeded => None,
            };
            stage.elapsed_ms = Some(elapsed_ms);
        }
    }

    pub(crate) fn stage_aborted(
        &mut self,
        state: StageState,
        reason: StageReason,
        elapsed_ms: Option<u64>,
    ) {
        if let Some(stage) = self
            .stages
            .last_mut()
            .filter(|stage| stage.state == StageState::Running)
        {
            stage.state = state;
            stage.reason = Some(reason);
            stage.elapsed_ms = elapsed_ms;
        }
    }

    /// Mark the run done. Unfinished jobs and steps are cancelled.
    pub fn finish(&mut self, conclusion: Conclusion, reason: Option<String>) {
        self.state = RunState::Done;
        self.conclusion = Some(conclusion);
        self.reason = reason;
        self.finished_at = Some(super::lifecycle::now_seconds());
        self.tree.cancel_unfinished();
        if let Some(stage) = self
            .stages
            .last_mut()
            .filter(|stage| stage.state == StageState::Running)
        {
            stage.state = StageState::Interrupted;
            stage.reason = Some(StageReason::Interrupted);
        }
    }

    /// The visible state distinguishes teardown after a cancel request while
    /// retaining the `running` wire state older clients understand.
    pub fn display_state(&self) -> &'static str {
        if self.state == RunState::Running && self.cancel_requested_at.is_some() {
            "cancelling"
        } else {
            self.state.as_str()
        }
    }

    pub(crate) fn key(&self) -> RunKey {
        RunKey {
            sha: self.sha.clone(),
            dirty: self.dirty.clone(),
            workflow: self.workflow.clone(),
            job: self.job.clone(),
            trigger: self.trigger.as_str().into(),
            mode: self.mode.as_str().into(),
            provider: self.provider.as_str().into(),
            engine: self.engine.clone(),
            payload_sha256: self.payload_sha256.clone(),
            timeout_secs: self.timeout_secs,
            secrets: self.secrets.clone(),
            github_proxy: self.github_proxy,
            params: self.params.clone(),
        }
    }
}

#[derive(Debug)]
pub struct CiError {
    pub code: &'static str,
    pub message: String,
}
impl CiError {
    pub fn refused(message: impl Into<String>) -> Self {
        Self::new("refused", message)
    }
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl From<CiError> for crate::Error {
    fn from(error: CiError) -> Self {
        Self::Ci {
            code: error.code.into(),
            message: error.message,
        }
    }
}

impl std::fmt::Display for CiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

pub(crate) fn valid_uuid(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(part, len)| valid_hex(part, len))
}

pub(crate) fn valid_sha(value: &str) -> bool {
    valid_hex(value, 40)
}

pub(crate) fn valid_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A relative workflow file under `.github/workflows/`, no traversal.
pub(crate) fn valid_workflow(value: &str) -> bool {
    valid_name(value, 512)
        && value.starts_with(".github/workflows/")
        && !value.split('/').any(|part| part.is_empty() || part == "..")
}

/// A job ID act receives as an argument value: never option-shaped.
pub(crate) fn valid_job(value: &str) -> bool {
    valid_name(value, 128) && !value.starts_with('-')
}

pub(crate) fn valid_name(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':'))
}

/// A fresh random v4 UUID (lowercase).
pub async fn new_uuid() -> Result<String, CiError> {
    let bytes = kernal_api::random::SecureRandom::new(1, Duration::from_secs(3))
        .map_err(|_| CiError::new("internal", "randomness unavailable"))?
        .bytes(16)
        .await
        .map_err(|_| CiError::new("internal", "randomness unavailable"))?;
    Ok(crate::uuid(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = kernal_api::hash::Sha256Hasher::new();
    hasher.update(bytes);
    hasher.finalize().to_hex()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_stage_timeouts_interrupted_stage_does_not_fabricate_elapsed() {
        let mut record = crate::ci::tests::sample_record("stage-test");
        record.stage_started(StageKind::EnrollmentFence, 123.0, 30, None, None);
        assert_eq!(record.stages.len(), 1);
        assert_eq!(record.stages[0].elapsed_ms, None);
        let report = crate::ci::report::report(&record, None, |_, _| Vec::new());
        assert_eq!(report.stages, record.stages);
        let encoded = serde_json::to_value(&report).unwrap();
        assert_eq!(encoded["stages"][0]["kind"], "enrollment_fence");
        record.finish(Conclusion::Error, Some("daemon interrupted".into()));
        assert_eq!(record.stages[0].state, StageState::Interrupted);
        assert_eq!(record.stages[0].elapsed_ms, None);
        assert!(record.retry("new-run".into(), None).stages.is_empty());
    }

    #[test]
    fn slow_requests_get_the_long_reply_deadline() {
        let run = || "r".to_string();
        let slow = [
            CiRequest::Retry {
                run: run(),
                job: None,
            },
            CiRequest::Report {
                run: run(),
                tail: None,
            },
            CiRequest::Runners {
                action: RunnerAction::PruneCache {
                    older_than_secs: None,
                    max_bytes: Some(1),
                },
            },
        ];
        for request in slow {
            assert_eq!(request.reply_deadline(), SLOW_REPLY_DEADLINE, "{request:?}");
        }
        let quick = [
            CiRequest::Show {
                run: run(),
                tree: None,
            },
            CiRequest::Cancel { run: run() },
            CiRequest::Runners {
                action: RunnerAction::Drain,
            },
        ];
        for request in quick {
            assert_eq!(request.reply_deadline(), REPLY_DEADLINE, "{request:?}");
        }
        assert!(
            REPLY_DEADLINE > Duration::from_secs(3),
            "longer than a plain daemon call"
        );
    }

    #[test]
    fn actions_cache_is_namespaced_per_repository_identity() {
        let namespace = |workspace: &str, origin: Option<&str>| {
            let mut record = crate::ci::tests::sample_record("r");
            record.workspace = workspace.into();
            record.repository = super::super::provider::repository(origin);
            record.cache_namespace()
        };
        let https = namespace("/a", Some("https://github.com/o/r.git"));
        assert_eq!(
            https,
            namespace("/b", Some("git@github.com:o/r.git")),
            "one repository shares its cache across clones and transports"
        );
        assert_ne!(
            https,
            namespace("/a", Some("https://github.com/o/other.git"))
        );
        assert_ne!(
            namespace("/a", None),
            namespace("/b", None),
            "without an origin each checkout path is its own repository"
        );
        assert_eq!(namespace("/a", None), namespace("/a", None));
    }
}
