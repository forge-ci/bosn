//! Copy-on-start source mounts; host paths are visible only to the copy helper.
use crate::ci::engine::{
    CacheHelperActive, CacheHelperIdentity, CacheHelperTracker, DockerActBackend,
};
use crate::*;
use bosn_core::manifest::{OverlayUpper, Stack};
use bosn_registry::cache_helper::{
    CacheHelperIntent, CacheHelperRecord, CacheHelperRole, CacheHelperState,
};

#[derive(Clone, Debug)]
pub(crate) struct OverlaySource {
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) volume: String,
    pub(crate) temporary: bool,
}

#[derive(Debug, thiserror::Error)]
enum OverlayRefusal {
    #[error("overlay source mounts cannot be used with keep = true")]
    Kept,
    #[error("overlay source mounts require an ephemeral task; use bosn run --task NAME")]
    Ensure,
}

pub(crate) fn require_task(sources: &[OverlaySource]) -> Result<(), String> {
    if sources.is_empty() {
        Ok(())
    } else {
        Err(OverlayRefusal::Ensure.to_string())
    }
}

pub(crate) fn plan(
    stack: &Stack,
    mounts: &mut Vec<bosn_core::WorkspaceMount>,
    volumes: &[ManifestVolumeResource],
    keep: bool,
) -> Result<Vec<OverlaySource>, String> {
    let mut result = Vec::new();
    for mount in &stack.mounts {
        let SourceBinding::Overlay { upper } = manifest_source_binding(mount) else {
            continue;
        };
        if keep {
            return Err(OverlayRefusal::Kept.to_string());
        }
        let resolved = mounts
            .iter()
            .find(|m| m.target == mount.destination)
            .ok_or("overlay source was not resolved")?;
        let (volume, temporary) = match upper {
            OverlayUpper::Tmpfs => (format!("bosn-overlay-run-{}", mount.name), true),
            OverlayUpper::Volume(name) => {
                let index = stack
                    .volumes
                    .iter()
                    .position(|v| v.name == name)
                    .ok_or("overlay upper is not a declared volume")?;
                (volumes[index].name.clone(), false)
            }
        };
        result.push(OverlaySource {
            source: resolved.source.clone(),
            target: resolved.target.clone(),
            volume,
            temporary,
        });
    }
    mounts.retain(|m| !result.iter().any(|s| s.target == m.target));
    Ok(result)
}

pub(crate) fn bind_plan(plan: &mut SetupPlan, sources: &[OverlaySource]) {
    for source in sources {
        let labels = plan
            .named_volumes
            .iter()
            .find(|v| v.name == source.volume)
            .map(|v| v.labels.clone())
            .unwrap_or_else(|| volume_labels(&source.volume, &plan.content_sha256));
        plan.named_volumes.push(SetupNamedVolume {
            name: source.volume.clone(),
            target: source.target.clone(),
            labels,
        });
    }
}

pub(crate) fn task_workdir(plan: &mut SetupPlan, sources: &[OverlaySource]) {
    let Some(workdir) = plan.app.workdir.as_deref() else {
        return;
    };
    let selected = sources
        .iter()
        .filter_map(|source| {
            let suffix = if source.source == "." {
                Some(if workdir == "." { "" } else { workdir })
            } else if workdir == source.source {
                Some("")
            } else {
                workdir.strip_prefix(&format!("{}/", source.source))
            }?;
            Some((source, suffix))
        })
        .max_by_key(|(source, _)| source.source.len());
    if let Some((source, suffix)) = selected {
        // A deeper bind keeps its original workdir mapping.
        if plan.app.mounts.iter().any(|mount| {
            mount.source.len() > source.source.len()
                && (workdir == mount.source || workdir.starts_with(&format!("{}/", mount.source)))
        }) {
            return;
        }
        let destination = if suffix.is_empty() {
            source.target.clone()
        } else {
            format!("{}/{suffix}", source.target.trim_end_matches('/'))
        };
        for task in plan.tasks.values_mut() {
            task.command = format!(
                "cd {} || exit; {}",
                shell_single_quote(&destination),
                task.command
            );
        }
        plan.app.workdir = None;
    }
}

fn volume_labels(name: &str, hash: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("com.zackees.bosn.setup-managed".into(), "v1".into()),
        ("com.zackees.bosn.setup-content-sha256".into(), hash.into()),
        ("com.zackees.bosn.setup-container".into(), name.into()),
    ])
}

pub(crate) async fn prepare(
    engine: &DockerEngine,
    plan: &mut SetupPlan,
    input: (&[OverlaySource], &PreparedImage),
    context: (&dyn ManifestAppTaskSessionRecorder, &str),
    cancel: &async_engine::CancellationToken,
    deadline: async_engine::Deadline,
    temporary: &mut Vec<String>,
) -> Result<(), String> {
    let (sources, image) = input;
    let (session, task) = context;
    let owner = session.registry_id().await?;
    if sources.is_empty() {
        return Ok(());
    }
    let mut resolved = sources.to_vec();
    for source in &mut resolved {
        if source.temporary {
            let name = format!(
                "bosn-overlay-run-{}",
                crate::ci::new_uuid().await.map_err(|e| e.to_string())?
            );
            let mut labels = ResourceLabels::new(
                &owner,
                bosn_core::ResourceKind::Volume,
                "overlay-source",
                &plan.content_sha256,
                Scope::Spec,
                &plan.workspace_root.to_string_lossy(),
                &crate::ci::lifecycle::now_seconds().to_string(),
                None,
            )
            .map_err(|e| format!("overlay volume labels: {e:?}"))?
            .to_map()
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect::<BTreeMap<String, String>>();
            labels.extend(volume_labels(&name, &plan.content_sha256));
            labels.insert(bosn_core::LABEL_TASK.into(), task.into());
            for volume in &mut plan.named_volumes {
                if volume.name == source.volume && volume.target == source.target {
                    volume.name = name.clone();
                    volume.labels = labels.clone();
                }
            }
            source.volume = name;
            temporary.push(source.volume.clone());
        }
    }
    let (events, _receiver) = async_engine::channel(64);
    bosn_setup::prepare_task_container(
        engine,
        &CoreSetupEnsureRequest {
            plan,
            workspace_root: plan.workspace_root.clone(),
            prepared_image: image,
            options: RunOptions::bounded(deadline.remaining(), 1024 * 1024),
            cancellation: cancel,
            events: &events,
        },
        "overlay-volume-preparation",
    )
    .await
    .map_err(|e| e.to_string())?;
    for source in &resolved {
        let (registry, _) = session.registry_authority()?;
        copy(
            engine,
            registry,
            plan,
            source,
            (&owner, task),
            cancel,
            deadline,
        )
        .await?;
    }
    Ok(())
}

async fn copy(
    engine: &DockerEngine,
    registry: &RegistryActor,
    plan: &SetupPlan,
    source: &OverlaySource,
    authority: (&str, &str),
    cancel: &async_engine::CancellationToken,
    deadline: async_engine::Deadline,
) -> Result<(), String> {
    let workspace = plan.workspace_root.clone();
    let path = source.source.clone();
    let lower = async_engine::launch_blocking(move || {
        let resolved = manifest_workspace_member(&workspace, &path)?;
        let lower = if resolved == "." {
            workspace
        } else {
            workspace.join(resolved)
        };
        Ok::<_, String>(lower.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())??;
    let intent = CacheHelperIntent {
        registry_id: authority.0.into(),
        nonce: crate::ci::new_uuid().await.map_err(|e| e.to_string())?,
        volume: source.volume.clone(),
        image: crate::ci::engine::engine_image(),
        created_at: crate::ci::lifecycle::now_seconds(),
        role: Some(CacheHelperRole::OverlaySourceV1 {
            source: lower.clone(),
        }),
    };
    let identity = CacheHelperIdentity::from_intent(&intent)?;
    let tracker = CacheHelperTracker::new(registry, &intent.nonce);
    let backend = DockerActBackend::new(engine.clone());
    let _active = CacheHelperActive::claim(&backend, &intent.nonce);
    tracker.begin(intent.clone()).await?;
    let mut created_id = None;
    let outcome = async_engine::timeout_at(
        deadline,
        async_engine::cancellable(cancel, async {
            checked(
                engine,
                helper_args(&identity, &intent, &lower, authority.1),
                deadline,
            )
            .await?;
            let document = checked(
                engine,
                vec!["inspect".into(), identity.name.clone()],
                deadline,
            )
            .await?;
            let id = identity.verify(&document, &intent.volume)?;
            created_id = Some(id.clone());
            tracker.register(&id).await?;
            let output = checked(
                engine,
                vec!["start".into(), "--attach".into(), id],
                deadline,
            )
            .await?;
            crate::daemon_log!(
                "bosn overlay source: {}: copied bytes and entries: {}",
                source.volume,
                output.split_whitespace().collect::<Vec<_>>().join(" ")
            );
            Ok::<_, String>(())
        }),
    )
    .await;
    // Cleanup is outside cancellation and the task budget. Pending creates keep
    // the existing durable late-create journal until exact recovery proves removal.
    let state = if created_id.is_some() {
        CacheHelperState::Created
    } else {
        CacheHelperState::Pending
    };
    let record = CacheHelperRecord {
        schema_version: 1,
        intent,
        container_id: created_id,
        state,
        updated_at: crate::ci::lifecycle::now_seconds(),
    };
    backend.reconcile_measurement(registry, &record).await?;
    outcome
        .map_err(|_| "overlay copy deadline exhausted".to_string())?
        .map_err(|_| "overlay copy cancelled".to_string())?
}

// A closed helper profile, like volume_seed/helper.rs: only its source bind and
// destination volume differ. Recovery verifies the frozen command and mounts.
fn helper_args(
    identity: &CacheHelperIdentity,
    intent: &CacheHelperIntent,
    lower: &str,
    task: &str,
) -> Vec<String> {
    let mut args = vec![
        "create".into(),
        "--rm".into(),
        bosn_engine::LOW_CPU_PRIORITY_ARG.into(),
        "--name".into(),
        identity.name.clone(),
        "--label".into(),
        identity.label(),
        "--pull=never".into(),
        "--network=none".into(),
        "--read-only".into(),
        "--tmpfs".into(),
        format!(
            "/var/lib/docker:{}",
            bosn_registry::cache_helper::VOLUME_SEED_TMPFS
        ),
        "--init=false".into(),
        "--restart=no".into(),
        "--memory=512m".into(),
        "--cpus=2".into(),
        "--pids-limit=128".into(),
        "--mount".into(),
        format!("type=bind,source={lower},target=/lower,readonly"),
        "--mount".into(),
        format!(
            "type=volume,source={},target=/upper,volume-nocopy",
            intent.volume
        ),
        "--entrypoint".into(),
        "/bin/sh".into(),
        intent.image.clone(),
        "-ec".into(),
        bosn_registry::cache_helper::OVERLAY_SOURCE_COMMAND.into(),
    ];
    args.splice(
        1..1,
        [
            "--label".into(),
            format!("{}={task}", bosn_core::LABEL_TASK),
        ],
    );
    args.splice(1..1, identity.ownership_args());
    args
}

async fn checked(
    engine: &DockerEngine,
    args: Vec<String>,
    deadline: async_engine::Deadline,
) -> Result<String, String> {
    let result = engine
        .with_args(args)
        .capture_async(RunOptions::bounded(deadline.remaining(), 1024 * 1024))
        .await
        .map_err(|e| e.to_string())?;
    result.require_complete().map_err(|e| e.to_string())?;
    if !result.ok() {
        return Err(format!(
            "overlay Docker action failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    String::from_utf8(result.stdout).map_err(|e| e.to_string())
}

pub(crate) async fn remove_volumes(
    engine: &DockerEngine,
    volumes: &[String],
) -> Result<(), String> {
    for volume in volumes {
        let result = engine
            .with_args(["volume", "rm", volume])
            .capture_async(RunOptions::bounded(Duration::from_secs(30), 1024 * 1024))
            .await
            .map_err(|e| e.to_string())?;
        result.require_complete().map_err(|e| e.to_string())?;
        if !result.ok() && !result.reports_missing() {
            return Err(format!(
                "overlay volume {volume} cleanup failed: {}",
                String::from_utf8_lossy(&result.stderr)
            ));
        }
    }
    Ok(())
}
