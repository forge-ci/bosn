//! Explicit reset uses retention's ownership/liveness/pin proofs and removal path.
//! Only the age gate is waived by the caller's explicit --clean request.
use super::*;
use crate::{CleanVolumes, ManifestVolumeResource};
use kernal_api::async_engine;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CleanError {
    #[error("cannot clean volume {volume}: held by {holder}")]
    Held { volume: String, holder: String },
    #[error("cannot clean volumes: {0}")]
    Observation(String),
}

impl From<String> for CleanError {
    fn from(value: String) -> Self {
        Self::Observation(value)
    }
}

fn held(volume: &str, holder: impl Into<String>) -> CleanError {
    CleanError::Held {
        volume: volume.into(),
        holder: holder.into(),
    }
}

fn policy() -> RetentionPolicy {
    RetentionPolicy {
        volume_ttl: Duration::ZERO,
        container_ttl: Duration::ZERO,
        ..RetentionPolicy::default()
    }
}

pub(crate) async fn execute(
    engine: &DockerEngine,
    state: &Path,
    volumes: &[ManifestVolumeResource],
    mode: CleanVolumes,
    cancellation: &async_engine::CancellationToken,
) -> Result<(), CleanError> {
    if mode == CleanVolumes::None {
        return Ok(());
    }
    let targets: Vec<_> = volumes
        .iter()
        .filter(|v| mode.selects(v.scope))
        .cloned()
        .collect();
    let engine = engine.clone();
    let state = state.to_owned();
    let cancellation = cancellation.clone();
    async_engine::launch_blocking(move || {
        let mut locks = Vec::new();
        for target in &targets {
            let lock = crate::volume_seed::locks::try_lock(
                &crate::runners::cache_lock_dir(),
                &target.name,
            )?
            .ok_or_else(|| held(&target.name, format!("exclusive lock {}", target.name)))?;
            locks.push(lock);
            check(&engine, &state, target)?;
        }
        for target in targets {
            if cancellation.is_cancelled() {
                return Err(held(&target.name, "cancelled client"));
            }
            // Repeat all durable and engine proofs immediately before removal.
            if let Some((volume, containers)) = check(&engine, &state, &target)? {
                let owner = registry_owner(&state)?;
                for container in containers {
                    revalidate(&engine, &state, &container, Some(&owner), policy())?;
                    if cancellation.is_cancelled() {
                        return Err(held(&target.name, "cancelled client"));
                    }
                    remove_owned(&engine, &container)?;
                }
                check_lease(&state, &target)?;
                revalidate(&engine, &state, &volume, Some(&owner), policy())?;
                if cancellation.is_cancelled() {
                    return Err(held(&target.name, "cancelled client"));
                }
                remove_owned(&engine, &volume)?;
                crate::daemon_log!("bosn task clean: removed volume {}", target.name);
            }
        }
        drop(locks);
        Ok(())
    })
    .await
    .map_err(|e| CleanError::Observation(e.to_string()))?
}

fn registry_owner(state: &Path) -> Result<String, String> {
    bosn_registry::Registry::open_read_only(state.join("registry.sqlite3"))
        .and_then(|r| r.registry_id())
        .map_err(|e| e.to_string())
}

fn check_lease(state: &Path, target: &ManifestVolumeResource) -> Result<(), CleanError> {
    let registry = bosn_registry::Registry::open_read_only(state.join("registry.sqlite3"))
        .map_err(|e| e.to_string())?;
    for lease in registered::read_all(|offset| registry.leases(offset, 256))? {
        if lease.resource_id == target.id || lease.resource_id == target.name {
            return Err(held(
                &target.name,
                format!("lease {} (pid {})", lease.id, lease.pid),
            ));
        }
    }
    Ok(())
}

fn verdict(
    name: &str,
    kind: ResourceKind,
    proven: authority::Proven,
    owner: &str,
) -> Result<RetentionVerdict, CleanError> {
    let artifact = bosn_core::ObservedArtifact {
        id: name.into(),
        kind,
        labels: proven.labels,
        signals: signals(proven.in_use),
        bytes: None,
        age_seconds: Some(proven.age),
    };
    let verdict = classify_managed(&artifact, Some(owner), policy());
    if let Some(reason) = verdict.hold {
        return Err(held(name, reason.as_str()));
    }
    Ok(verdict)
}

fn check(
    engine: &DockerEngine,
    state: &Path,
    target: &ManifestVolumeResource,
) -> Result<Option<(RetentionVerdict, Vec<RetentionVerdict>)>, CleanError> {
    check_lease(state, target)?;
    let Some(observed) = crate::volume_seed::observe::inspect(engine, &target.name)? else {
        return Ok(None);
    };
    if target
        .labels
        .iter()
        .any(|(k, v)| observed.labels.get(k) != Some(v))
    {
        return Err(held(&target.name, "foreign ownership labels"));
    }
    let owner = registry_owner(state)?;
    if observed
        .labels
        .get(bosn_core::LABEL_REGISTRY)
        .is_some_and(|r| r != &owner)
    {
        return Err(held(&target.name, "foreign registry"));
    }
    if target.retention == bosn_core::Retention::Pinned {
        return Err(held(&target.name, "pinned manifest volume"));
    }
    let authority = authority::Authority::load(state, false);
    let proven = authority.prove(
        ResourceKind::Volume,
        &target.name,
        observed.labels,
        0.0,
        false,
    );
    let volume = verdict(&target.name, ResourceKind::Volume, proven, &owner)?;
    let options = RunOptions::bounded(RETENTION_READ_DEADLINE, RETENTION_OUTPUT_LIMIT);
    let ids = engine
        .container_ids_using_volume(&target.name, options)
        .map_err(|e| e.to_string())?;
    let mut containers = Vec::new();
    for id in ids {
        let entries =
            parse_read::<ContainerDetail>(engine.inspect_containers(&[id.clone()], options))
                .ok_or_else(|| held(&target.name, format!("unreadable container {id}")))?;
        for entry in entries {
            let holder = format!("container {} ({id})", entry.engine_name());
            if entry.running() {
                return Err(held(&target.name, holder));
            }
            if entry
                .labels()
                .get(bosn_core::LABEL_REGISTRY)
                .is_some_and(|r| r != &owner)
            {
                return Err(held(&target.name, holder));
            }
            let proven = authority.prove(
                ResourceKind::Container,
                entry.engine_name(),
                entry.labels(),
                0.0,
                false,
            );
            if proven
                .labels
                .get(bosn_core::LABEL_WORKSPACE)
                .map(String::as_str)
                != Some(&target.workspace)
            {
                return Err(held(&target.name, holder));
            }
            containers.push(
                verdict(&id, ResourceKind::Container, proven, &owner)
                    .map_err(|_| held(&target.name, holder))?,
            );
        }
    }
    Ok(Some((volume, containers)))
}

#[cfg(test)]
mod tests;
