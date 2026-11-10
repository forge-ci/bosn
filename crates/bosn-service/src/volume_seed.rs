//! Seed only newly created stack volumes; source selection is pure.
use crate::setup_adopt::{PreparedSetupEnsure, SetupEnsurePipeline, execute_setup_ensure_pipeline};
use crate::{ManifestVolumeResource, RegistryActor, manifest_volume_registry::Command};
use bosn_core::{ResourceKind, Scope};
use bosn_engine::{DockerEngine, EngineEvent};
use kernal_api::async_engine;
use std::path::Path;

mod copy;
mod locks;
mod observe;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct Candidate {
    resource: bosn_registry::Resource,
    family: Option<String>,
    volume: String,
    idle: bool,
    leased: bool,
    mounted: bool,
}

fn select<'a>(
    family: Option<&str>,
    target: &ManifestVolumeResource,
    candidates: &'a [Candidate],
    exists: bool,
) -> Option<&'a Candidate> {
    let family = family?;
    let volume = target.seed_name.as_ref()?;
    if exists || target.scope != Scope::Stack {
        return None;
    }
    candidates
        .iter()
        .filter(|c| {
            c.resource.kind == ResourceKind::Volume
                && c.resource.scope == Scope::Stack
                && c.family.as_deref() == Some(family)
                && c.resource.stack == target.stack
                && c.volume == *volume
                && c.resource.name != target.name
                && c.resource.workspace != target.workspace
                && c.idle
                && !c.leased
                && !c.mounted
                && c.resource.last_used.is_finite()
        })
        .max_by(|a, b| {
            a.resource
                .last_used
                .total_cmp(&b.resource.last_used)
                .then_with(|| a.resource.name.cmp(&b.resource.name))
        })
}

pub(crate) struct SeedContext<'a> {
    pub state_dir: &'a Path,
    pub engine: &'a DockerEngine,
    pub registry: &'a RegistryActor,
    pub volumes: &'a [ManifestVolumeResource],
}
impl SeedContext<'_> {
    pub async fn execute(
        &self,
        pipeline: &SetupEnsurePipeline<'_>,
        cancellation: &async_engine::CancellationToken,
        events: &async_engine::Sender<EngineEvent>,
        logs: &crate::raw_run_log::JobLogSink,
    ) -> Result<PreparedSetupEnsure, String> {
        // A destination ensure waits asynchronously for another ensure/seed of
        // that exact volume. Source selection itself always uses a nonblocking try.
        let locks = locks::destinations(
            self.state_dir,
            self.volumes,
            pipeline.deadline,
            cancellation,
        )
        .await;
        let _locks = match locks {
            Ok(locks) => Some(locks),
            Err(error) => {
                crate::daemon_log!(
                    "bosn volume seed: cold create: volume locking unavailable: {error}"
                );
                None
            }
        };
        for volume in self.volumes.iter().filter(|_| _locks.is_some()) {
            if let Err(error) = self.seed(volume, pipeline.deadline).await {
                crate::daemon_log!("bosn volume seed: {}: cold create: {error}", volume.name);
            }
        }
        execute_setup_ensure_pipeline(self.engine, pipeline, cancellation, events, logs).await
    }

    async fn seed(
        &self,
        target: &ManifestVolumeResource,
        deadline: &async_engine::Deadline,
    ) -> Result<(), String> {
        if target.seed_name.is_none() {
            return Ok(());
        }
        let state = self.state_dir.to_owned();
        let engine = self.engine.clone();
        let target = target.clone();
        let worker_target = target.clone();
        let selected =
            async_engine::launch_blocking(move || observe::source(&state, &engine, &worker_target))
                .await
                .map_err(|e| e.to_string())??;
        let Some((source, source_lock)) = selected else {
            return Ok(());
        };
        let id = format!("volume-seed:{}", target.name);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs_f64();
        let lease = bosn_registry::Lease {
            id: id.clone(),
            resource_id: source.id.clone(),
            pid: std::process::id(),
            proc_start: None,
            acquired_at: now,
            heartbeat_at: now,
            ttl_seconds: 0.0,
        };
        if !self
            .registry
            .manifest_volume_command(Command::AcquireSeed {
                source: Box::new(source.clone()),
                lease,
            })
            .await
            .map_err(|e| e.to_string())?
        {
            return Err(format!("source {} is leased or changed", source.name));
        }
        // The cleanup driver outlives a caller dropped by an outer deadline.
        // Its blocking worker owns the file lease until Docker cleanup finishes.
        let engine = self.engine.clone();
        let state = self.state_dir.to_owned();
        let registry = self.registry.clone();
        let budget = deadline.remaining();
        async_engine::launch(async move {
            let outcome = async_engine::launch_blocking(move || {
                let _source_lock = source_lock;
                copy::seed(&engine, &source, &target, &state, budget)
            })
            .await
            .map_err(|e| e.to_string())?;
            if outcome.helper_uncertain {
                return Err("seed helper cleanup uncertain; source lease retained".into());
            }
            registry
                .manifest_volume_command(Command::ReleaseSeed(id))
                .await
                .map_err(|e| format!("seed lease release failed (source stays protected): {e}"))?;
            outcome.result
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
