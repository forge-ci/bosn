//! Preparation uses the task's existing lease; it never reacquires or releases it.
use super::*;

pub(super) async fn prepare_plan(
    engine: &DockerEngine,
    context: (&Path, &[ManifestVolumeResource]),
    input: (&SetupPlan, CleanVolumes),
    session: &dyn ManifestAppTaskSessionRecorder,
    remaining: Duration,
    cancellation: &async_engine::CancellationToken,
) -> Result<SetupPlan, TaskContainerError> {
    let (state, volumes) = context;
    let (plan, clean) = input;
    let (registry, _) = session.registry_authority()?;
    if clean == CleanVolumes::None {
        crate::volume_seed::SeedContext {
            state_dir: state,
            engine,
            registry,
            volumes,
        }
        .seed(&async_engine::Deadline::after(remaining), cancellation)
        .await?;
    }
    let mut plan = plan.clone();
    crate::volume_seed::authorize_plan(Some(state), &mut plan).await?;
    Ok(plan)
}

pub(super) async fn record_kept(
    plan: &SetupPlan,
    image: &PreparedImage,
    request: &ManifestAppTaskJobRequest,
    context: (bool, &[ManifestVolumeResource]),
    observed: &SetupEnsureResult,
    session: &dyn ManifestAppTaskSessionRecorder,
) -> Result<(), TaskContainerError> {
    let (autostart, volumes) = context;
    let (registry, job) = session.registry_authority()?;
    let workspace = plan.workspace_root.to_string_lossy().into_owned();
    let generation = format!("sha256:{}", plan.content_sha256);
    let kind = if plan.macos_guest.is_some() {
        "manifest-guest"
    } else {
        "manifest-container"
    };
    let execution = SetupEnsureExecution {
        receipt: format!("ensured task stack {}", request.stack),
        resource: SetupEnsureResource {
            id: setup_container_resource_id(kind, &request.stack, &observed.container_name),
            name: observed.container_name.clone(),
            stack: request.stack.clone(),
            generation,
            workspace: workspace.clone(),
        },
        image: SetupEnsureImageResource {
            id: format!("manifest-image:{}", image.observed_identity),
            name: format!("manifest-image:{}", image.observed_identity),
            stack: request.stack.clone(),
            generation: image.observed_identity.clone(),
            workspace,
        },
        volumes: volumes.to_vec(),
        manifest_autostart: autostart,
    };
    let ensure_request = ManifestEnsureJobRequest {
        workspace: request.workspace.clone(),
        manifest: request.manifest.clone(),
        stack: request.stack.clone(),
        deadline: request.deadline,
        output_limit: request.output_limit,
    };
    let contract = manifest_recovery_contract(&ensure_request, &execution, job)?;
    registry
        .record_manifest_ensure(job, execution, contract)
        .await
        .map_err(|e| e.to_string().into())
}

pub(super) async fn remove(
    engine: &DockerEngine,
    container: &TaskContainer,
) -> Result<bosn_engine::CommandResult, String> {
    control(
        engine,
        vec![
            "container".into(),
            "rm".into(),
            "--force".into(),
            container.id.clone(),
        ],
    )
    .await
}

pub(super) fn retry_cleanup(engine: DockerEngine, lifetime: TaskLifetime) {
    async_engine::launch(async move {
        let Some(container) = &lifetime.container else {
            return;
        };
        loop {
            async_engine::sleep(Duration::from_secs(1)).await;
            if let Ok(output) = remove(&engine, container).await
                && (output.ok() || output.reports_missing())
            {
                daemon_log!(
                    "bosn task cleanup: removed {} and released volume leases",
                    container.name
                );
                return;
            }
        }
    })
    .detach();
}
