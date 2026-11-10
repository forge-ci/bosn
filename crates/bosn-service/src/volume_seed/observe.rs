//! Bounded blocking observations; uncertainty always leaves a new volume cold.
use super::{Candidate, locks, select};
use crate::managed_retention::registered::RegisteredOwnership;
use crate::{ManifestVolumeResource, git_common_dir::common_dir};
use bosn_core::{ResourceKind, Scope, unmanaged::InspectedVolume};
use bosn_engine::{DockerEngine, RunOptions};
use std::path::Path;
use std::time::Duration;

pub(super) const OPTIONS: RunOptions = RunOptions::bounded(Duration::from_secs(30), 1024 * 1024);

pub(super) fn inspect(
    engine: &DockerEngine,
    name: &str,
) -> Result<Option<InspectedVolume>, String> {
    let result = engine
        .with_args(["volume", "inspect", name])
        .capture(OPTIONS)
        .map_err(|e| e.to_string())?;
    if result.reports_missing() {
        return Ok(None);
    }
    result.require_complete().map_err(|e| e.to_string())?;
    if !result.ok() {
        return Err("seed volume inspection failed".into());
    }
    let mut entries: Vec<InspectedVolume> =
        serde_json::from_slice(&result.stdout).map_err(|e| e.to_string())?;
    if entries.len() != 1 || entries[0].name != name {
        return Err("ambiguous volume inspection".into());
    }
    Ok(entries.pop())
}

pub(super) fn family(workspace: &str) -> Option<String> {
    common_dir(Path::new(workspace)).and_then(|p| crate::workspace_identity::observe(&p).ok())
}

pub(super) fn unmounted(engine: &DockerEngine, name: &str) -> Result<bool, String> {
    engine
        .container_ids_using_volume(name, OPTIONS)
        .map(|ids| ids.is_empty())
        .map_err(|e| e.to_string())
}

pub(super) fn source(
    state: &Path,
    engine: &DockerEngine,
    target: &ManifestVolumeResource,
) -> Result<
    Option<(
        bosn_registry::Resource,
        kernal_api::platform::fs::OwnedFileLock,
    )>,
    String,
> {
    // Never modify an existing volume, even if unregistered or unlabelled.
    if inspect(engine, &target.name)?.is_some() {
        return Ok(None);
    }
    let Some(family) = family(&target.workspace) else {
        return Ok(None);
    };
    let Some(volume) = &target.seed_name else {
        return Ok(None);
    };
    let ownership = RegisteredOwnership::load(state)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs_f64();
    let mut candidates = Vec::new();
    for resource in &ownership.resources {
        if resource.kind != ResourceKind::Volume
            || resource.scope != Scope::Stack
            || resource.stack != target.stack
            || resource.workspace == target.workspace
        {
            continue;
        }
        let identity = crate::workspace_identity::observe(Path::new(&resource.workspace))
            .map_err(|e| e.to_string())?;
        let digest =
            crate::manifest_runtime::manifest_volume_identity(&resource.stack, volume, &identity);
        if resource.name != format!("bosn-v-stack-{}", &digest[..24]) {
            continue;
        }
        let source_family = self::family(&resource.workspace);
        if source_family.as_deref() != Some(&family) {
            continue;
        }
        let Some(observed) = inspect(engine, &resource.name)? else {
            continue;
        };
        let Some(proven) =
            ownership.normalize(ResourceKind::Volume, &resource.name, &observed.labels, now)
        else {
            continue;
        };
        candidates.push(Candidate {
            resource: resource.clone(),
            family: source_family,
            volume: volume.clone(),
            idle: proven.idle_seconds > 0.0,
            leased: proven.protected,
            mounted: !unmounted(engine, &resource.name)?,
        });
    }
    let Some(chosen) = select(Some(&family), target, &candidates, false) else {
        if candidates.iter().any(|c| c.leased) {
            return Err("matching sibling is leased".into());
        }
        if candidates.iter().any(|c| c.mounted) {
            return Err("matching sibling is mounted".into());
        }
        return Ok(None);
    };
    let Some(lock) = locks::try_lock(&crate::runners::cache_lock_dir(), &chosen.resource.name)?
    else {
        return Err(format!(
            "source {} has an exclusive lease",
            chosen.resource.name
        ));
    };
    Ok(Some((chosen.resource.clone(), lock)))
}
