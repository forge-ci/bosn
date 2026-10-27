//! The daemon-held engine (#349 owned Act engine) that the machine's shared
//! engine (#547) is: its intent and frozen creation profile are durable before
//! it exists, it carries the ownership labels, and the daemon prepares it
//! under its own execution claim, running only bosn's fixed scripts. Runs
//! never claim it; they lease slots in it ([`super::shared_engine`]). It is
//! retired the same way, with proof, and startup recovery treats one a dead
//! daemon left exactly like any other non-terminal engine.

use std::time::Duration;

use bosn_registry::act::{ActEngineIntent, ActEngineObservation, ActRunOutcome};
use kernal_api::async_engine::{self, CancellationToken};

use super::{
    engine::{ActArtifact, ActEngineBackend, CacheVolume},
    lifecycle::{self, Claim, Clock},
};
use crate::RegistryActor;

/// A prepared engine, held under the daemon's claim `token`.
#[derive(Clone, Debug)]
pub struct Spare {
    pub intent: ActEngineIntent,
    pub observed: ActEngineObservation,
    pub token: String,
}

/// What the shared engine (#547) is made from.
#[derive(Clone, Debug)]
pub struct SparePlan {
    pub intent: ActEngineIntent,
    pub act: ActArtifact,
    pub cache: CacheVolume,
    /// How its immutable CPU cap was chosen.
    pub cpu_source: super::reply::CiCpuSource,
    /// Select/enroll the machine's native tools under its claim before creation.
    /// The payload bound is logical bytes, not a whole-machine storage ceiling.
    pub native_tools: Option<u64>,
    /// Outer phase limits, frozen when the run is planned.
    pub timeouts: super::config::CiTimeouts,
}

impl SparePlan {
    pub(super) fn validate_native_policy(&self) -> Result<(), String> {
        if let Some(max) = self.native_tools {
            if max == 0 || max > i64::MAX as u64 {
                return Err("invalid native payload bound".into());
            }
            if self
                .intent
                .creation_profile
                .as_ref()
                .is_some_and(|profile| profile.docker_socket.is_some())
            {
                return Err("native enrollment refuses historical Docker socket layout".into());
            }
        }
        if self.native_tools.is_some()
            && crate::act_engine::frozen_limits(&self.intent)
                .map_err(|e| e.to_string())?
                .storage
                != crate::act_engine::EngineStorage::Disk
        {
            return Err("native tool enrollment requires disk-backed engine storage".into());
        }
        Ok(())
    }

    pub(super) fn accepts(&self, identity: &super::machine::EngineIdentity) -> bool {
        let Some(max) = self.native_tools else {
            return super::machine::EngineIdentity::of(&self.intent).as_ref() == Some(identity);
        };
        let Some(binding) = identity.tool_generation() else {
            return false;
        };
        if binding.max_payload_bytes != max {
            return false;
        }
        self.with_generation(binding.clone())
            .ok()
            .and_then(|plan| super::machine::EngineIdentity::of(&plan.intent))
            .as_ref()
            == Some(identity)
    }

    pub(super) fn with_generation(
        &self,
        binding: bosn_registry::act::ActToolGenerationBinding,
    ) -> Result<Self, String> {
        binding.validate().map_err(|e| e.to_string())?;
        let mut plan = self.clone();
        let limits = crate::act_engine::frozen_limits(&plan.intent).map_err(|e| e.to_string())?;
        if limits.storage != crate::act_engine::EngineStorage::Disk {
            return Err("native tool enrollment requires disk-backed engine storage".into());
        }
        plan.intent.creation_profile = Some(
            crate::act_engine::creation_profile_with_tools(
                limits,
                Some(plan.cache.mount()),
                Some(binding),
            )
            .map_err(|e| e.to_string())?,
        );
        plan.native_tools = None;
        Ok(plan)
    }
}

/// Create, claim and prepare the engine. Any failure removes what was
/// created (or leaves its record `cleanup_required` for startup recovery).
pub(super) async fn prepare(
    registry: &RegistryActor,
    backend: &dyn ActEngineBackend,
    plan: SparePlan,
    cancellation: &CancellationToken,
    progress: &mut (dyn FnMut(super::shared_engine::LeaseUpdate) + Send),
) -> Result<Spare, String> {
    let owner = lifecycle::registry_owner(registry).await?;
    let mut state = PreparationState {
        clock: Clock(plan.intent.created_at),
        token: None,
    };
    let held = prepare_held(
        registry,
        backend,
        &owner,
        &plan,
        cancellation,
        &mut state,
        progress,
    )
    .await;
    match held {
        Ok(observed) => Ok(Spare {
            intent: plan.intent,
            observed,
            token: state.token.unwrap_or_default(),
        }),
        Err(error) => {
            let removed = lifecycle::cleanup(
                registry,
                backend,
                &owner,
                &plan.intent.run_id,
                state.token.as_deref(),
                ActRunOutcome::Cancelled,
                &mut state.clock,
            )
            .await;
            Err(match removed {
                Ok(()) => error,
                Err(removal) => format!("{error}; its removal failed: {removal}"),
            })
        }
    }
}

struct PreparationState {
    clock: Clock,
    token: Option<String>,
}

async fn prepare_held(
    registry: &RegistryActor,
    backend: &dyn ActEngineBackend,
    owner: &str,
    plan: &SparePlan,
    cancellation: &CancellationToken,
    state: &mut PreparationState,
    progress: &mut (dyn FnMut(super::shared_engine::LeaseUpdate) + Send),
) -> Result<ActEngineObservation, String> {
    backend.ensure_cache(&plan.cache).await?;
    let observed = backend
        .create(registry, &plan.intent, owner, state.clock.now())
        .await?;
    let held = Claim::commit(registry, &plan.intent, observed, state.clock.now()).await?;
    state.token = Some(held.token.clone());
    held.verify().await?;
    prepare_engine(backend, held.engine(), plan, cancellation, progress).await?;
    use super::shared_engine::LeaseUpdate;
    use super::wire::{StageEvent, StageKind, StageState};
    held.verify().await?;
    if plan
        .intent
        .creation_profile
        .as_ref()
        .is_some_and(|profile| profile.tool_generation.is_some())
    {
        let window =
            super::native_lifetime::Window::new(plan.intent.created_at, lifecycle::now_seconds())?;
        let kind = StageKind::RecoveryReservation;
        progress(LeaseUpdate::Stage(StageEvent::Started {
            kind,
            outer_limit_secs: plan.timeouts.prepare_secs,
            command_limit_secs: Some(plan.timeouts.reservation_secs),
            run_remaining_ms: None,
        }));
        let reserved_at = std::time::Instant::now();
        let reserved = async_engine::timeout(
            Duration::from_secs(plan.timeouts.prepare_secs),
            async_engine::cancellable(
                cancellation,
                backend.reserve_native_recovery(
                    registry,
                    &plan.intent.run_id,
                    &held.token,
                    window.created,
                    window.expires,
                    Duration::from_secs(plan.timeouts.reservation_secs),
                ),
            ),
        )
        .await;
        let state = match &reserved {
            Err(_) => StageState::TimedOut,
            Ok(Err(_)) => StageState::Cancelled,
            Ok(Ok(Err(_))) => StageState::Failed,
            Ok(Ok(Ok(_))) => StageState::Succeeded,
        };
        progress(LeaseUpdate::Stage(StageEvent::Finished {
            kind,
            state,
            elapsed_ms: reserved_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }));
        match reserved {
            Err(_) => return Err(format!("native recovery reservation timed out after {:.3}s (outer limit {}s)", reserved_at.elapsed().as_secs_f64(), plan.timeouts.prepare_secs)),
            Ok(Err(_)) => return Err("native source preparation was cancelled".into()),
            Ok(Ok(result)) => result.map_err(|error| format!("native recovery reservation failed after {:.3}s (command limit {}s; outer limit {}s; act2 may enforce a shorter internal limit): {error}", reserved_at.elapsed().as_secs_f64(), plan.timeouts.reservation_secs, plan.timeouts.prepare_secs))?,
        }
        held.verify().await?;
    }
    Ok(held.observed.clone())
}

async fn prepare_engine(
    backend: &dyn ActEngineBackend,
    engine: &str,
    plan: &SparePlan,
    cancellation: &CancellationToken,
    progress: &mut (dyn FnMut(super::shared_engine::LeaseUpdate) + Send),
) -> Result<(), String> {
    use super::shared_engine::LeaseUpdate;
    use super::wire::{StageEvent, StageKind, StageState};
    let kind = StageKind::EnginePrepare;
    progress(LeaseUpdate::Stage(StageEvent::Started {
        kind,
        outer_limit_secs: plan.timeouts.prepare_secs,
        command_limit_secs: None,
        run_remaining_ms: None,
    }));
    let prepared_at = std::time::Instant::now();
    let prepared = async_engine::timeout(
        Duration::from_secs(plan.timeouts.prepare_secs),
        async_engine::cancellable(cancellation, backend.prepare_engine(engine, plan.act)),
    )
    .await;
    let state = match &prepared {
        Err(_) => StageState::TimedOut,
        Ok(Err(_)) => StageState::Cancelled,
        Ok(Ok(Err(_))) => StageState::Failed,
        Ok(Ok(Ok(_))) => StageState::Succeeded,
    };
    progress(LeaseUpdate::Stage(StageEvent::Finished {
        kind,
        state,
        elapsed_ms: prepared_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }));
    match prepared {
        Err(_) => {
            return Err(format!(
                "engine preparation timed out after {:.3}s (outer limit {}s)",
                prepared_at.elapsed().as_secs_f64(),
                plan.timeouts.prepare_secs
            ));
        }
        Ok(Err(_)) => return Err("preparing the engine was cancelled".into()),
        Ok(Ok(result)) => result.map_err(|error| {
            format!(
                "engine preparation failed after {:.3}s (outer limit {}s): {error}",
                prepared_at.elapsed().as_secs_f64(),
                plan.timeouts.prepare_secs
            )
        })?,
    }
    Ok(())
}

/// Retire the engine as its holder.
pub(super) async fn retire(
    registry: &RegistryActor,
    backend: &dyn ActEngineBackend,
    spare: &Spare,
) -> Result<(), String> {
    let owner = lifecycle::registry_owner(registry).await?;
    lifecycle::cleanup(
        registry,
        backend,
        &owner,
        &spare.intent.run_id,
        Some(&spare.token),
        ActRunOutcome::Cancelled,
        &mut Clock(spare.intent.created_at),
    )
    .await
}
