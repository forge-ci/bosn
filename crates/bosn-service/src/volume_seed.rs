//! Seed only newly created stack volumes; source selection is pure.
use crate::setup_adopt::{PreparedSetupEnsure, SetupEnsurePipeline, execute_setup_ensure_pipeline};
use crate::{ManifestVolumeResource, RegistryActor};
use bosn_core::{ResourceKind, Scope};
use bosn_engine::{DockerEngine, EngineEvent};
use kernal_api::async_engine;
use std::path::Path;

mod copy;
mod gate;
pub(crate) use gate::authorize_plan;
mod helper;
pub(crate) mod locks;
pub(crate) mod observe;
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
        let _locks = locks::destinations(
            self.state_dir,
            self.volumes,
            pipeline.deadline,
            cancellation,
        )
        .await?;
        for target in self.volumes {
            // The gate runs even when seed=false: opt-out never bypasses an unfinished copy.
            if gate::ensure_safe(self, target)
                .await
                .map_err(|e| e.to_string())?
            {
                continue;
            }
            let Some(query_budget) = budget(pipeline.deadline.remaining()) else {
                if target.seed_name.is_some() {
                    crate::daemon_log!(
                        "bosn volume seed: {}: cold create: ensure floor reserved",
                        target.name
                    );
                }
                continue;
            };
            if target.seed_name.is_none() {
                continue;
            }
            let state = self.state_dir.to_owned();
            let engine = self.engine.clone();
            let target_copy = target.clone();
            let selected = async_engine::timeout_at(
                async_engine::Deadline::after(query_budget.min(std::time::Duration::from_secs(15))),
                async_engine::launch_blocking(move || {
                    observe::source(&state, &engine, &target_copy)
                }),
            )
            .await;
            let selected = match selected {
                Ok(worker) => worker.map_err(|e| e.to_string())?,
                Err(_) => {
                    crate::daemon_log!(
                        "bosn volume seed: {}: cold create: source observation budget exhausted",
                        target.name
                    );
                    continue;
                }
            };
            match selected {
                Ok(Some((source, source_lock))) => {
                    let _source_lock = source_lock;
                    copy::seed(
                        self,
                        target,
                        source,
                        match budget(pipeline.deadline.remaining()) {
                            Some(budget) => budget,
                            None => {
                                crate::daemon_log!(
                                    "bosn volume seed: {}: cold create: ensure floor reserved",
                                    target.name
                                );
                                continue;
                            }
                        },
                        cancellation,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                Ok(None) => (),
                Err(error) => {
                    crate::daemon_log!("bosn volume seed: {}: cold create: {error}", target.name)
                }
            }
        }
        self.guarded_pipeline(pipeline, cancellation, events, logs)
            .await
    }
    async fn guarded_pipeline(
        &self,
        pipeline: &SetupEnsurePipeline<'_>,
        cancellation: &async_engine::CancellationToken,
        events: &async_engine::Sender<EngineEvent>,
        logs: &crate::raw_run_log::JobLogSink,
    ) -> Result<PreparedSetupEnsure, String> {
        let mut plan = pipeline.plan.clone();
        authorize_plan(Some(self.state_dir), &mut plan).await?;
        let guarded = SetupEnsurePipeline {
            plan: &plan,
            workspace: pipeline.workspace.clone(),
            deadline: pipeline.deadline,
            prepare_output: pipeline.prepare_output,
            ensure_output: pipeline.ensure_output,
        };
        execute_setup_ensure_pipeline(self.engine, &guarded, cancellation, events, logs).await
    }
}
pub(super) const ENSURE_FLOOR: std::time::Duration = std::time::Duration::from_secs(60);
fn budget(remaining: std::time::Duration) -> Option<std::time::Duration> {
    let budget = remaining
        .checked_sub(ENSURE_FLOOR)?
        .min(std::time::Duration::from_secs(600));
    // Reserve control time inside the seed budget for teardown and a cold reset.
    (budget > std::time::Duration::from_secs(30)).then_some(budget)
}

pub(crate) async fn recover_startup(registry: &RegistryActor, state_dir: &Path) {
    let engine = DockerEngine::docker();
    let context = SeedContext {
        state_dir,
        engine: &engine,
        registry,
        volumes: &[],
    };
    match gate::snapshot(&context).await {
        Ok(snapshot) => {
            for seed in snapshot
                .seeds
                .iter()
                .filter(|s| s.state == bosn_registry::volume_seed::SeedState::Copying)
            {
                let Some(intent) = snapshot.intents.iter().find(|i| i.name == seed.destination)
                else {
                    crate::daemon_log!(
                        "bosn volume seed: {}: recovery held: destination intent missing",
                        seed.destination
                    );
                    continue;
                };
                let name = seed.destination.clone();
                let _destination_lock = match async_engine::launch_blocking(move || {
                    locks::try_lock(&crate::runners::cache_lock_dir(), &name)
                })
                .await
                {
                    Ok(Ok(Some(lock))) => lock,
                    Ok(Ok(None)) => continue, // A newly admitted ensure owns this attempt.
                    Ok(Err(error)) => {
                        crate::daemon_log!(
                            "bosn volume seed: startup recovery lock unavailable: {error}"
                        );
                        continue;
                    }
                    Err(error) => {
                        crate::daemon_log!(
                            "bosn volume seed: startup recovery worker failed: {error}"
                        );
                        continue;
                    }
                };
                let target = ManifestVolumeResource {
                    id: format!("manifest-volume:{}", intent.name),
                    name: intent.name.clone(),
                    stack: intent.stack.clone(),
                    generation: intent.generation.clone(),
                    scope: intent.scope,
                    workspace: intent.workspace.clone(),
                    retention: bosn_core::Retention::Warm,
                    target: String::new(),
                    labels: intent.labels.clone(),
                    seed_name: None,
                };
                if let Err(error) = gate::ensure_safe(&context, &target).await {
                    crate::daemon_log!("bosn volume seed: {}: recovery held: {error}", target.name);
                }
            }
        }
        Err(error) => crate::daemon_log!("bosn volume seed: startup recovery held: {error}"),
    }
}
