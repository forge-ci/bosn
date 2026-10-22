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

/// How long preparing the engine may take.
const PREPARE_DEADLINE: Duration = Duration::from_secs(30 * 60);

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
}

/// Create, claim and prepare the engine. Any failure removes what was
/// created (or leaves its record `cleanup_required` for startup recovery).
pub(super) async fn prepare(
    registry: &RegistryActor,
    backend: &dyn ActEngineBackend,
    plan: SparePlan,
    cancellation: &CancellationToken,
) -> Result<Spare, String> {
    let owner = lifecycle::registry_owner(registry).await?;
    let mut clock = Clock(plan.intent.created_at);
    let mut token = None;
    let held = prepare_held(
        registry,
        backend,
        &owner,
        &plan,
        cancellation,
        &mut clock,
        &mut token,
    )
    .await;
    match held {
        Ok(observed) => Ok(Spare {
            intent: plan.intent,
            observed,
            token: token.unwrap_or_default(),
        }),
        Err(error) => {
            let removed = lifecycle::cleanup(
                registry,
                backend,
                &owner,
                &plan.intent.run_id,
                token.as_deref(),
                ActRunOutcome::Cancelled,
                &mut clock,
            )
            .await;
            Err(match removed {
                Ok(()) => error,
                Err(removal) => format!("{error}; its removal failed: {removal}"),
            })
        }
    }
}

async fn prepare_held(
    registry: &RegistryActor,
    backend: &dyn ActEngineBackend,
    owner: &str,
    plan: &SparePlan,
    cancellation: &CancellationToken,
    clock: &mut Clock,
    token: &mut Option<String>,
) -> Result<ActEngineObservation, String> {
    backend.ensure_cache(&plan.cache).await?;
    let observed = backend
        .create(registry, &plan.intent, owner, clock.now())
        .await?;
    let held = Claim::commit(registry, &plan.intent, observed, clock.now()).await?;
    *token = Some(held.token.clone());
    held.verify().await?;
    let prepared = async_engine::timeout(
        PREPARE_DEADLINE,
        async_engine::cancellable(
            cancellation,
            backend.prepare_engine(held.engine(), plan.act),
        ),
    )
    .await;
    match prepared {
        Err(_) => return Err("preparing the engine timed out".into()),
        Ok(Err(_)) => return Err("preparing the engine was cancelled".into()),
        Ok(Ok(result)) => result?,
    }
    held.verify().await?;
    if plan
        .intent
        .creation_profile
        .as_ref()
        .is_some_and(|profile| profile.tool_generation.is_some())
    {
        let window =
            super::native_lifetime::Window::new(plan.intent.created_at, lifecycle::now_seconds())?;
        backend
            .reserve_native_recovery(
                registry,
                &plan.intent.run_id,
                &held.token,
                window.created,
                window.expires,
            )
            .await?;
        held.verify().await?;
    }
    Ok(held.observed.clone())
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
