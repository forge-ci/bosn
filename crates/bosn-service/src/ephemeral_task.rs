//! Named task-container lifetime, independent of persistent setup generations.

use crate::*;
use bosn_core::{ResourceKind, ResourceLabels, Scope};
use serde::Deserialize;

#[path = "ephemeral_task_wait.rs"]
mod waiting;
#[cfg(test)]
use waiting::{WaitDecision, busy_after, wait_decision};

#[derive(Debug, thiserror::Error)]
pub(crate) enum TaskContainerError {
    #[error("task container {0} is busy{suffix}", suffix = waiting::suffix(.1))]
    TaskContainerBusy(String, Option<Duration>),
    #[error("task container {0} is foreign or lacks exact Bosn ownership labels")]
    TaskContainerForeign(String),
    #[error("{0}")]
    Operation(String),
}

impl From<String> for TaskContainerError {
    fn from(error: String) -> Self {
        Self::Operation(error)
    }
}

impl From<&str> for TaskContainerError {
    fn from(error: &str) -> Self {
        Self::Operation(error.into())
    }
}

#[derive(Debug, PartialEq)]
enum Leftover {
    Remove,
    Busy,
    Refuse,
}

fn leftover(running: bool, ours: bool) -> Leftover {
    if !ours {
        Leftover::Refuse
    } else if running {
        Leftover::Busy
    } else {
        Leftover::Remove
    }
}

fn task_name(task: &str, identity: &str) -> String {
    format!("{task}-{}", runners::short_hash(identity.as_bytes(), 16))
}

pub(crate) struct TaskContainer {
    pub(crate) name: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspection {
    id: String,
    state: ContainerState,
    config: ContainerConfig,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerState {
    running: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerConfig {
    labels: Option<BTreeMap<String, String>>,
}

async fn control(engine: &DockerEngine, args: Vec<String>) -> Result<CommandResult, String> {
    engine
        .with_args(args)
        .capture_async(RunOptions::bounded(Duration::from_secs(30), 1024 * 1024))
        .await
        .map_err(|e| e.to_string())
}

async fn inspect(engine: &DockerEngine, name: &str) -> Result<Option<Inspection>, String> {
    let result = control(
        engine,
        vec!["container".into(), "inspect".into(), name.into()],
    )
    .await?;
    if result.reports_missing() {
        return Ok(None);
    }
    if !result.ok() {
        return Err("task container inspection failed".into());
    }
    let mut rows: Vec<Inspection> =
        serde_json::from_slice(&result.stdout).map_err(|e| e.to_string())?;
    if rows.len() != 1 {
        return Err("task container inspection returned an invalid inventory".into());
    }
    Ok(rows.pop())
}

fn ours(observed: &Inspection, expected: &ResourceLabels, name: &str, task: &str) -> bool {
    let Some(raw) = &observed.config.labels else {
        return false;
    };
    let Ok(labels) = ResourceLabels::parse(raw) else {
        return false;
    };
    labels.registry == expected.registry
        && labels.kind == ResourceKind::Container
        && labels.stack == expected.stack
        && labels.workspace == expected.workspace
        && labels.scope == Scope::Stack
        && raw
            .get("com.zackees.bosn.setup-managed")
            .is_some_and(|v| v == "v1")
        && raw
            .get("com.zackees.bosn.setup-container")
            .is_some_and(|v| v == name)
        && raw.get(bosn_core::LABEL_TASK).is_some_and(|v| v == task)
}

async fn clear_leftover(
    engine: &DockerEngine,
    name: &str,
    task: &str,
    labels: &ResourceLabels,
    wait: bool,
) -> Result<(), TaskContainerError> {
    let Some(observed) = inspect(engine, name).await? else {
        return Ok(());
    };
    match leftover(observed.state.running, ours(&observed, labels, name, task)) {
        Leftover::Busy => Err(TaskContainerError::TaskContainerBusy(name.into(), None)),
        Leftover::Refuse => Err(TaskContainerError::TaskContainerForeign(name.into())),
        Leftover::Remove => {
            let result =
                control(engine, vec!["container".into(), "rm".into(), observed.id]).await?;
            if result.ok() || result.reports_missing() {
                Ok(())
            } else if wait {
                // The holder can remove its stopped container between our inspect and rm.
                match inspect(engine, name).await? {
                    None => Ok(()),
                    Some(current) if ours(&current, labels, name, task) => {
                        Err(TaskContainerError::TaskContainerBusy(name.into(), None))
                    }
                    Some(_) => Err(TaskContainerError::TaskContainerForeign(name.into())),
                }
            } else {
                Err("stopped task container removal failed".into())
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "task boundary carries existing execution budgets and authority"
)]
pub(crate) async fn prepare(
    engine: &DockerEngine,
    plan: &SetupPlan,
    image: &PreparedImage,
    request: &ManifestAppTaskJobRequest,
    clean_context: (bool, &Path, &[ManifestVolumeResource]),
    session: &dyn ManifestAppTaskSessionRecorder,
    cancellation: &async_engine::CancellationToken,
    events: &async_engine::Sender<EngineEvent>,
    remaining: Duration,
    lifetime: &mut Option<TaskContainer>,
) -> Result<SetupEnsureResult, TaskContainerError> {
    let started = Instant::now();
    loop {
        let result = prepare_once(
            engine,
            plan,
            image,
            request,
            clean_context,
            session,
            cancellation,
            events,
            remaining.saturating_sub(started.elapsed()),
            lifetime,
        )
        .await;
        match result {
            Err(TaskContainerError::TaskContainerBusy(name, _))
                if request.wait && !clean_context.0 =>
            {
                if started.elapsed() >= remaining {
                    return Err(waiting::busy_after(&name, started.elapsed()));
                }
                if cancellation.is_cancelled() {
                    return Err("manifest task cancelled while waiting on container".into());
                }
                async_engine::sleep(
                    Duration::from_millis(250).min(remaining.saturating_sub(started.elapsed())),
                )
                .await;
            }
            other => return other,
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "task boundary carries existing execution budgets and authority"
)]
async fn prepare_once(
    engine: &DockerEngine,
    plan: &SetupPlan,
    image: &PreparedImage,
    request: &ManifestAppTaskJobRequest,
    clean_context: (bool, &Path, &[ManifestVolumeResource]),
    session: &dyn ManifestAppTaskSessionRecorder,
    cancellation: &async_engine::CancellationToken,
    events: &async_engine::Sender<EngineEvent>,
    remaining: Duration,
    lifetime: &mut Option<TaskContainer>,
) -> Result<SetupEnsureResult, TaskContainerError> {
    let (keep, state, volumes) = clean_context;
    let remaining = if request.wait && !keep {
        waiting::before_prepare(engine, plan, request, session, cancellation, remaining).await?
    } else {
        remaining
    };
    if keep && request.clean != CleanVolumes::None {
        reset_kept_container(
            engine,
            CoreSetupEnsureRequest {
                plan,
                workspace_root: request.workspace.clone(),
                prepared_image: image,
                options: RunOptions::streaming(remaining, 8 * 1024 * 1024),
                cancellation,
                events,
            },
            state,
        )
        .await?;
    }
    crate::managed_retention::clean::execute(engine, state, volumes, request.clean, cancellation)
        .await
        .map_err(|e| TaskContainerError::Operation(e.to_string()))?;
    let cleaning = request.clean != CleanVolumes::None;
    if cleaning {
        session.task_volumes(volumes, false).await?;
    }
    let result = prepare_container(
        engine,
        CoreSetupEnsureRequest {
            plan,
            workspace_root: request.workspace.clone(),
            prepared_image: image,
            options: RunOptions::streaming(remaining, 8 * 1024 * 1024),
            cancellation,
            events,
        },
        request,
        keep,
        session,
        remaining,
        lifetime,
    )
    .await?;
    if cleaning {
        session.task_volumes(volumes, true).await?;
    }
    Ok(result)
}

async fn prepare_container(
    engine: &DockerEngine,
    setup: CoreSetupEnsureRequest<'_>,
    request: &ManifestAppTaskJobRequest,
    keep: bool,
    session: &dyn ManifestAppTaskSessionRecorder,
    remaining: Duration,
    lifetime: &mut Option<TaskContainer>,
) -> Result<SetupEnsureResult, TaskContainerError> {
    if keep {
        return if request.clean == CleanVolumes::None {
            adopt_running(engine, setup).await
        } else {
            bosn_setup::ensure_setup_app(engine, setup)
                .await
                .map_err(|e| e.to_string().into())
        };
    }
    let CoreSetupEnsureRequest {
        plan,
        prepared_image: image,
        cancellation,
        events,
        ..
    } = setup;
    if plan.macos_guest.is_some() {
        return Err("ephemeral macOS guest tasks are unsupported; set keep = true".into());
    }
    let workspace = plan.workspace_root.clone();
    let identity = async_engine::launch_blocking(move || workspace_identity::observe(&workspace))
        .await
        .map_err(|_| "task workspace identity worker failed")?
        .map_err(|e| e.to_string())?;
    let name = task_name(&request.task_name, &identity);
    let registry = session.registry_id().await?;
    let labels = task_labels(&registry, plan, request)?;
    clear_leftover(engine, &name, &request.task_name, &labels, request.wait).await?;
    if cancellation.is_cancelled() {
        return Err("manifest task cancelled before container creation".into());
    }
    let mut command = bosn_setup::prepare_task_container(
        engine,
        &CoreSetupEnsureRequest {
            plan,
            workspace_root: request.workspace.clone(),
            prepared_image: image,
            options: RunOptions::streaming(remaining, 8 * 1024 * 1024),
            cancellation,
            events,
        },
        &name,
    )
    .await
    .map_err(|e| e.to_string())?;
    let bosn_setup::SetupEnsureCommand::Create { labels: raw, .. } = &mut command else {
        return Err("task container derivation did not return a create command".into());
    };
    raw.extend(labels.to_map().into_iter().map(|(k, v)| (k.into(), v)));
    raw.insert(bosn_core::LABEL_TASK.into(), request.task_name.clone());
    let result = create(engine, command, events).await?;
    if !result.ok() {
        return Err(create_failure(engine, &name, &request.task_name, &labels).await);
    }
    let id = String::from_utf8(result.stdout)
        .map_err(|e| e.to_string())?
        .trim()
        .to_owned();
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid task container creation receipt".into());
    }
    *lifetime = Some(TaskContainer {
        name: name.clone(),
        id: id.clone(),
    });
    let result = control(engine, vec!["container".into(), "start".into(), id.clone()]).await?;
    if !result.ok() {
        return Err("task container start failed".into());
    }
    Ok(SetupEnsureResult {
        container_name: name,
        container_id: id,
        image_identity: image.observed_identity.clone(),
        created: true,
        started: true,
        running: true,
    })
}

fn task_labels(
    registry: &str,
    plan: &SetupPlan,
    request: &ManifestAppTaskJobRequest,
) -> Result<ResourceLabels, TaskContainerError> {
    ResourceLabels::new(
        registry,
        ResourceKind::Container,
        &request.stack,
        &plan.content_sha256,
        Scope::Stack,
        plan.workspace_root
            .to_str()
            .ok_or("workspace is not UTF-8")?,
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs()
            .to_string(),
        None,
    )
    .map_err(|e| TaskContainerError::Operation(format!("invalid task ownership labels: {e:?}")))
}

/// Prove the old attachments before reset can remove any named storage.
async fn reset_kept_container(
    engine: &DockerEngine,
    setup: CoreSetupEnsureRequest<'_>,
    state: &Path,
) -> Result<(), TaskContainerError> {
    let name =
        bosn_setup::setup_container_name(setup.plan, &setup.workspace_root, setup.prepared_image)
            .map_err(|e| e.to_string())?;
    let cancellation = setup.cancellation.clone();
    let observed = adopt_setup_app(engine, setup)
        .await
        .map_err(|e| e.to_string())?;
    if observed.running {
        return Err(TaskContainerError::TaskContainerBusy(name, None));
    }
    crate::managed_retention::clean::remove_kept(engine, state, observed, &cancellation)
        .await
        .map_err(|e| e.to_string().into())
}

async fn adopt_running(
    engine: &DockerEngine,
    request: CoreSetupEnsureRequest<'_>,
) -> Result<SetupEnsureResult, TaskContainerError> {
    let observed = adopt_setup_app(engine, request)
        .await
        .map_err(|e| e.to_string())?;
    if !observed.running {
        return Err(
            "manifest app task requires the exact managed application to be running".into(),
        );
    }
    Ok(observed)
}

async fn create(
    engine: &DockerEngine,
    command: bosn_setup::SetupEnsureCommand,
    events: &async_engine::Sender<EngineEvent>,
) -> Result<CommandResult, String> {
    // Once issued, finish the bounded create even if the task is cancelled,
    // so its immutable ID can be removed by the run's cleanup.
    let response = bosn_setup::SetupEnsureEngine::stream(
        engine,
        command,
        RunOptions::bounded(Duration::from_secs(30), 1024 * 1024),
        &async_engine::CancellationSource::new().token(),
        events,
    )
    .await
    .map_err(|e| e.to_string())?;
    match response {
        bosn_setup::SetupEnsureResponse::Command(result) => Ok(result),
        _ => Err("task create returned an inspection response".into()),
    }
}

async fn create_failure(
    engine: &DockerEngine,
    name: &str,
    task: &str,
    labels: &ResourceLabels,
) -> TaskContainerError {
    match inspect(engine, name).await {
        Ok(Some(observed)) if ours(&observed, labels, name, task) => {
            TaskContainerError::TaskContainerBusy(name.into(), None)
        }
        Ok(Some(_)) => TaskContainerError::TaskContainerForeign(name.into()),
        _ => format!("task container {name} creation failed").into(),
    }
}

pub(crate) async fn finish(
    engine: &DockerEngine,
    container: Option<TaskContainer>,
    session: &dyn ManifestAppTaskSessionRecorder,
    result: Result<String, String>,
) -> Result<String, String> {
    let Some(container) = container else {
        return result;
    };
    let removed = control(
        engine,
        vec![
            "container".into(),
            "rm".into(),
            "--force".into(),
            container.id,
        ],
    )
    .await;
    match removed {
        Ok(output) if output.ok() || output.reports_missing() => {
            session
                .finish(if result.is_ok() {
                    "succeeded"
                } else {
                    "stopped"
                })
                .await?;
            result
        }
        other => {
            let error = format!(
                "task container {} cleanup failed: {other:?}",
                container.name
            );
            daemon_log!("{error}");
            Err(format!("{}; {error}", result.err().unwrap_or_default()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_name_is_deterministic_and_checkout_specific() {
        assert_eq!(task_name("build", "/a"), task_name("build", "/a"));
        assert_ne!(task_name("build", "/a"), task_name("build", "/b"));
        assert!(task_name("build", "/a").starts_with("build-"));
    }

    #[test]
    fn leftovers_require_exact_ownership_and_never_remove_running_containers() {
        assert_eq!(leftover(false, true), Leftover::Remove);
        assert_eq!(leftover(true, true), Leftover::Busy);
        assert_eq!(leftover(false, false), Leftover::Refuse);
        assert_eq!(leftover(true, false), Leftover::Refuse);
    }
    #[test]
    fn ownership_requires_this_registry_workspace_and_task() {
        let expected = ResourceLabels::new(
            "registry",
            ResourceKind::Container,
            "stack",
            "old-generation",
            Scope::Stack,
            "/workspace",
            "1",
            None,
        )
        .unwrap();
        let mut raw: BTreeMap<String, String> = expected
            .to_map()
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        raw.insert("com.zackees.bosn.setup-managed".into(), "v1".into());
        raw.insert(
            "com.zackees.bosn.setup-container".into(),
            "task-checkout".into(),
        );
        raw.insert(bosn_core::LABEL_TASK.into(), "task".into());
        let mut observed = Inspection {
            id: "id".into(),
            state: ContainerState { running: false },
            config: ContainerConfig {
                labels: Some(raw.clone()),
            },
        };
        assert!(ours(&observed, &expected, "task-checkout", "task"));
        for key in [
            bosn_core::LABEL_REGISTRY,
            bosn_core::LABEL_WORKSPACE,
            bosn_core::LABEL_TASK,
        ] {
            let mut foreign = raw.clone();
            foreign.insert(key.into(), "foreign".into());
            observed.config.labels = Some(foreign);
            assert!(!ours(&observed, &expected, "task-checkout", "task"));
        }
        observed.config.labels = None;
        assert!(!ours(&observed, &expected, "task-checkout", "task"));
    }
}

#[cfg(test)]
mod wait_tests {
    use super::*;
    #[test]
    fn wait_decision_respects_ownership_and_deadline() {
        assert_eq!(wait_decision(true, true, false), WaitDecision::Wait);
        assert_eq!(wait_decision(true, false, false), WaitDecision::Refuse);
        assert_eq!(wait_decision(true, true, true), WaitDecision::Expired);
        assert_eq!(wait_decision(false, true, false), WaitDecision::Ready);
        assert!(
            busy_after("probe", Duration::from_secs(3))
                .to_string()
                .contains("waited 3s")
        );
    }
}
