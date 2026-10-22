//! Typed forwarding of act2's persistent archive enrollment protocol.
use super::machine_policy::PolicyAgreement;
use super::{DockerActBackend, ENGINE_CACHE, RunOptions, legacy_lease, owned};
use crate::ci::{cache_cohort::Namespace, cache_policy::CachePolicy};
use kernal_api::async_engine::{self, CancellationToken};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveEnrollmentPhase {
    Fence,
    Complete,
    Require,
}

pub struct ArchiveEnrollmentRequest<'a> {
    pub engine: &'a str,
    pub namespace: &'a Namespace,
    pub policy: CachePolicy,
    pub phase: ArchiveEnrollmentPhase,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ArchiveEnrollmentStatus {
    Ready,
    Fenced,
    Retry,
    Refused(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema_version: u32,
    source: String,
    root: String,
    namespace: String,
    #[serde(default)]
    snapshot: String,
    max_bytes: i64,
    source_fenced: bool,
    ready: bool,
    partial: bool,
    retryable: bool,
    #[serde(default)]
    error: String,
}

impl ArchiveEnrollmentRequest<'_> {
    fn args(&self) -> Vec<String> {
        let mut args = owned(&["exec", self.engine]);
        args.extend(match self.phase {
            ArchiveEnrollmentPhase::Require => legacy_lease::command(None),
            _ => legacy_lease::migration_command(),
        });
        args.extend(owned(&["cache", "archive-enroll", "--from"]));
        args.push(self.namespace.legacy_path());
        args.extend(owned(&[
            "--namespace",
            self.namespace.as_str(),
            "--cache-server-path",
        ]));
        args.push(crate::ci::cache_cohort::root());
        args.extend([
            "--max-bytes".into(),
            self.policy.repository_max_bytes.to_string(),
        ]);
        args.extend(match self.phase {
            ArchiveEnrollmentPhase::Fence => owned(&["--apply", "--fence-only"]),
            ArchiveEnrollmentPhase::Complete => owned(&["--apply", "--source-quiescent"]),
            ArchiveEnrollmentPhase::Require => owned(&["--require-ready"]),
        });
        args
    }

    fn parse(&self, bytes: &[u8], success: bool) -> Result<ArchiveEnrollmentStatus, String> {
        if bytes.len() > 8192 {
            return Err("archive enrollment report exceeds bound".into());
        }
        let report: Report =
            serde_json::from_slice(bytes).map_err(|e| format!("archive enrollment report: {e}"))?;
        let prefix = format!(
            "{ENGINE_CACHE}/actcache/.{}.archive-enrollment-",
            self.namespace.as_str()
        );
        let valid_snapshot = report.snapshot.strip_prefix(&prefix).is_some_and(|id| {
            id.len() == 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        if report.schema_version != 1
            || report.source != self.namespace.legacy_path()
            || report.root != crate::ci::cache_cohort::root()
            || report.namespace != self.namespace.as_str()
            || report.max_bytes != self.policy.repository_max_bytes
            || report.error.chars().count() > 256
            || (!report.snapshot.is_empty() && !valid_snapshot)
            || (report.source_fenced && !valid_snapshot)
            || success == report.partial
            || (report.retryable && !report.partial)
            || (report.ready && (!report.source_fenced || report.partial))
            || (!report.partial && !report.error.is_empty())
            || (report.partial && report.error.is_empty())
        {
            return Err("archive enrollment report identity or lifecycle mismatch".into());
        }
        if report.partial {
            return Ok(if report.retryable {
                ArchiveEnrollmentStatus::Retry
            } else {
                ArchiveEnrollmentStatus::Refused(report.error)
            });
        }
        if report.ready {
            return Ok(ArchiveEnrollmentStatus::Ready);
        }
        if self.phase == ArchiveEnrollmentPhase::Fence && report.source_fenced {
            return Ok(ArchiveEnrollmentStatus::Fenced);
        }
        Err("archive enrollment acknowledged the wrong phase".into())
    }
}

impl DockerActBackend {
    pub(super) async fn enroll_archives(
        &self,
        request: ArchiveEnrollmentRequest<'_>,
    ) -> Result<ArchiveEnrollmentStatus, String> {
        let agreement = match request.phase {
            ArchiveEnrollmentPhase::Fence => Some(
                self.try_agree_cache_policy(request.engine, request.policy)
                    .await?,
            ),
            ArchiveEnrollmentPhase::Complete => Some(
                self.try_require_cache_policy(request.engine, request.policy)
                    .await?,
            ),
            ArchiveEnrollmentPhase::Require => None,
        };
        if agreement == Some(PolicyAgreement::Busy) {
            return Ok(ArchiveEnrollmentStatus::Retry);
        }
        let result = self
            .run_bounded(
                request.args(),
                RunOptions::bounded(Duration::from_secs(150), 8192),
            )
            .await?;
        // The trusted migration wrapper refuses its nonblocking lease before
        // starting act. Application refusals carry typed JSON even on exit 1.
        if result.exit_code == 75 && result.stdout.is_empty() {
            return Ok(ArchiveEnrollmentStatus::Retry);
        }
        let status = request.parse(&result.stdout, result.ok())?;
        if status == ArchiveEnrollmentStatus::Ready
            && self
                .try_require_cache_policy(request.engine, request.policy)
                .await?
                == PolicyAgreement::Busy
        {
            return Ok(ArchiveEnrollmentStatus::Retry);
        }
        Ok(status)
    }
    /// Return a terminal execution result only if policy waiting ends the run.
    /// Missing or changed agreement is an error, never permission to recreate it.
    pub(super) async fn guard_archive_execution(
        &self,
        engine: &str,
        policy: CachePolicy,
        deadline: async_engine::Deadline,
        cancellation: &CancellationToken,
    ) -> Result<Option<super::ExecEnd>, String> {
        let wait = async {
            loop {
                match self.try_require_cache_policy(engine, policy).await? {
                    PolicyAgreement::Agreed => return Ok(()),
                    PolicyAgreement::Busy => async_engine::sleep(Duration::from_millis(250)).await,
                }
            }
        };
        match async_engine::timeout_at(deadline, async_engine::cancellable(cancellation, wait))
            .await
        {
            Err(_) => Ok(Some(super::ExecEnd::TimedOut)),
            Ok(Err(_)) => Ok(Some(super::ExecEnd::Cancelled)),
            Ok(Ok(result)) => result.map(|()| None),
        }
    }
}

#[cfg(test)]
mod tests;
