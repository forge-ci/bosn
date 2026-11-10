//! Session recorders, request digests and the client-gone sweep used by the job actor.

use super::*;

#[derive(Clone)]
pub(crate) struct ActorSetupAppTaskSessionRecorder {
    pub(crate) actor: RegistryActor,
    pub(crate) job_id: u64,
}

impl SetupAppTaskSessionRecorder for ActorSetupAppTaskSessionRecorder {
    fn begin<'a>(
        &'a self,
        managed_container_identity: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.actor
                .begin_setup_app_task_session(self.job_id, managed_container_identity)
                .await
                .map_err(|_| "registry session start failed".into())
        })
    }
    fn finish<'a>(
        &'a self,
        outcome: &'static str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.actor
                .finish_setup_app_task_session(self.job_id, outcome)
                .await
                .map_err(|_| "registry session finish failed".into())
        })
    }
}

#[derive(Clone)]
pub(crate) struct ActorManifestAppTaskSessionRecorder {
    pub(crate) actor: RegistryActor,
    pub(crate) job_id: u64,
    pub(crate) run: Option<RunContext>,
}
impl ManifestAppTaskSessionRecorder for ActorManifestAppTaskSessionRecorder {
    fn registry_id(&self) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async {
            self.actor
                .status()
                .await
                .map(|s| s.registry_id)
                .map_err(|e| e.to_string())
        })
    }

    fn run_context(&self) -> Option<&RunContext> {
        self.run.as_ref()
    }
    fn stop_retired_generations<'a>(
        &'a self,
        workspace: &'a str,
        stack: &'a str,
    ) -> Pin<Box<dyn Future<Output = Vec<String>> + Send + 'a>> {
        Box::pin(async move {
            match stop_retired_stack_containers(&self.actor, workspace, stack).await {
                Ok(stops) => stops
                    .stopped
                    .iter()
                    .map(|name| format!("[manifest-app-task] stopped retired generation {name}"))
                    .chain(stops.failed.iter().map(|(name, error)| {
                        format!("[manifest-app-task] warning: retired generation {name} was not stopped: {error}")
                    }))
                    .collect(),
                Err(error) => vec![format!(
                    "[manifest-app-task] warning: retired generations of stack {stack} were not listed: {error}"
                )],
            }
        })
    }
    fn begin<'a>(
        &'a self,
        managed_container_identity: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.actor
                .begin_manifest_app_task_session(self.job_id, managed_container_identity)
                .await
                .map_err(|_| "registry session start failed".into())
        })
    }
    fn finish<'a>(
        &'a self,
        outcome: &'static str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.actor
                .finish_manifest_app_task_session(self.job_id, outcome)
                .await
                .map_err(|_| "registry session finish failed".into())
        })
    }
}

pub(crate) fn bounded_log_line(value: &str) -> String {
    if value.len() <= jobs::MAX_LOG_LINE_BYTES {
        return value.to_owned();
    }
    let mut end = jobs::MAX_LOG_LINE_BYTES.saturating_sub(3);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = value[..end].to_owned();
    bounded.push_str("...");
    bounded
}

pub(crate) fn setup_prepare_digest(request: &SetupPrepareRequest) -> String {
    let mut material = Vec::new();
    let workspace = request.workspace.to_string_lossy();
    let policy = request.policy.wire().to_le_bytes();
    let deadline = request.deadline.as_millis().to_le_bytes();
    let output_limit = (request.output_limit as u64).to_le_bytes();
    for part in [
        b"bosn.setup-prepare.v1".as_slice(),
        workspace.as_bytes(),
        request.config.as_bytes(),
        &policy,
        &deadline,
        &output_limit,
    ] {
        material.extend_from_slice(&(part.len() as u64).to_le_bytes());
        material.extend_from_slice(part);
    }
    format!(
        "setup:{}",
        kernal_api::hash::blake3_bytes(&material).to_hex()
    )
}

pub(crate) fn setup_task_digest(request: &SetupTaskJobRequest) -> String {
    let mut material = Vec::new();
    let workspace = request.workspace.to_string_lossy();
    let policy = request.policy.wire().to_le_bytes();
    let deadline = request.deadline.as_millis().to_le_bytes();
    let output_limit = (request.output_limit as u64).to_le_bytes();
    for part in [
        b"bosn.setup-task.v1".as_slice(),
        workspace.as_bytes(),
        request.config.as_bytes(),
        &policy,
        request.task_name.as_bytes(),
        &deadline,
        &output_limit,
    ] {
        material.extend_from_slice(&(part.len() as u64).to_le_bytes());
        material.extend_from_slice(part);
    }
    format!(
        "setup:{}",
        kernal_api::hash::blake3_bytes(&material).to_hex()
    )
}

pub(crate) fn setup_app_task_digest(request: &SetupAppTaskJobRequest) -> String {
    let mut material = Vec::new();
    let workspace = request.workspace.to_string_lossy();
    let policy = request.policy.wire().to_le_bytes();
    let deadline = request.deadline.as_millis().to_le_bytes();
    let output_limit = (request.output_limit as u64).to_le_bytes();
    for part in [
        b"bosn.setup-app-task.v1".as_slice(),
        workspace.as_bytes(),
        request.config.as_bytes(),
        &policy,
        request.task_name.as_bytes(),
        &deadline,
        &output_limit,
    ] {
        material.extend_from_slice(&(part.len() as u64).to_le_bytes());
        material.extend_from_slice(part);
    }
    format!(
        "setup:{}",
        kernal_api::hash::blake3_bytes(&material).to_hex()
    )
}

pub(crate) fn setup_ensure_digest(request: &SetupEnsureJobRequest) -> String {
    let mut material = Vec::new();
    let workspace = request.workspace.to_string_lossy();
    let policy = request.policy.wire().to_le_bytes();
    let deadline = request.deadline.as_millis().to_le_bytes();
    let output_limit = (request.output_limit as u64).to_le_bytes();
    for part in [
        b"bosn.setup-ensure.v1".as_slice(),
        workspace.as_bytes(),
        request.config.as_bytes(),
        &policy,
        &deadline,
        &output_limit,
    ] {
        material.extend_from_slice(&(part.len() as u64).to_le_bytes());
        material.extend_from_slice(part);
    }
    format!(
        "setup:{}",
        kernal_api::hash::blake3_bytes(&material).to_hex()
    )
}

pub(crate) fn manifest_ensure_digest(request: &ManifestEnsureJobRequest) -> String {
    let mut material = Vec::new();
    let workspace = request.workspace.to_string_lossy();
    let deadline = request.deadline.as_millis().to_le_bytes();
    let output_limit = (request.output_limit as u64).to_le_bytes();
    for part in [
        b"bosn.manifest-ensure.v1".as_slice(),
        workspace.as_bytes(),
        request.manifest.as_bytes(),
        request.stack.as_bytes(),
        &deadline,
        &output_limit,
    ] {
        material.extend_from_slice(&(part.len() as u64).to_le_bytes());
        material.extend_from_slice(part);
    }
    format!(
        "manifest:{}",
        kernal_api::hash::blake3_bytes(&material).to_hex()
    )
}
pub(crate) fn manifest_converge_digest(request: &ManifestConvergeJobRequest) -> String {
    let mut material = Vec::new();
    let workspace = request.workspace.to_string_lossy();
    let deadline = request.deadline.as_millis().to_le_bytes();
    let output_limit = (request.output_limit as u64).to_le_bytes();
    for part in [
        b"bosn.manifest-converge.v1".as_slice(),
        workspace.as_bytes(),
        request.manifest.as_bytes(),
        &deadline,
        &output_limit,
    ] {
        material.extend_from_slice(&(part.len() as u64).to_le_bytes());
        material.extend_from_slice(part);
    }
    format!(
        "manifest-converge:{}",
        kernal_api::hash::blake3_bytes(&material).to_hex()
    )
}
pub(crate) fn manifest_app_task_digest(request: &ManifestAppTaskJobRequest) -> String {
    let mut material = Vec::new();
    let workspace = request.workspace.to_string_lossy();
    let deadline = request.deadline.as_millis().to_le_bytes();
    let output_limit = (request.output_limit as u64).to_le_bytes();
    for part in [
        b"bosn.manifest-app-task.v1".as_slice(),
        workspace.as_bytes(),
        request.manifest.as_bytes(),
        request.stack.as_bytes(),
        request.task_name.as_bytes(),
        &deadline,
        &output_limit,
    ] {
        material.extend_from_slice(&(part.len() as u64).to_le_bytes());
        material.extend_from_slice(part);
    }
    for arg in &request.args {
        material.extend_from_slice(&(arg.len() as u64).to_le_bytes());
        material.extend_from_slice(arg.as_bytes());
    }
    format!(
        "manifest:{}",
        kernal_api::hash::blake3_bytes(&material).to_hex()
    )
}

/// A job ends only when every recorded PID plus creation key is proven gone.
/// Heartbeat silence, restart and sleep never prove owner death (#777).
pub(crate) fn clients_gone(jobs: &mut Jobs, sweep: bool) -> Vec<(u64, String)> {
    if !sweep {
        return Vec::new();
    }
    jobs.orphaned(ClientOwner::liveness)
        .into_iter()
        .map(|(id, pids)| (id, format!("[bosn] {}", owner::orphaned_message(&pids))))
        .collect()
}

pub(crate) fn manifest_task_key(request: &ManifestAppTaskJobRequest) -> String {
    if request.args.is_empty() {
        format!("manifest-app-task:{}", request.stack)
    } else {
        format!(
            "manifest-app-task:{}:{}:{}",
            request.stack,
            request.task_name,
            serde_json::to_string(&request.args).expect("string arguments serialize")
        )
    }
}

pub(crate) fn admit_manifest_task(
    jobs: &mut Jobs,
    requests: &mut BTreeMap<u64, SetupJobRequest>,
    request: ManifestAppTaskJobRequest,
) -> Result<(u64, bool), Error> {
    let digest = manifest_app_task_digest(&request);
    let workspace = request.workspace.to_string_lossy();
    let submission = jobs
        .submit_class(
            &workspace,
            &manifest_task_key(&request),
            &digest,
            jobs::JobClass::Runner,
        )
        .map_err(|_| Error::Protocol("manifest app task job admission"))?;
    let id = match submission {
        Submission::Joined(id) => return Ok((id, false)),
        Submission::Started(id) | Submission::Queued(id) => id,
        Submission::Superseded { replacement, .. } => replacement,
    };
    jobs.set_args(id, request.args.clone());
    requests.insert(id, SetupJobRequest::ManifestAppTask(request));
    Ok((id, true))
}
