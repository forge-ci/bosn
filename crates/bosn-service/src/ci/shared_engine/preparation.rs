//! Keep the machine claim until an interrupted preparation is physically gone.

use std::sync::Arc;

use bosn_registry::act::{ActEngineIntent, ActRunOutcome};
use kernal_api::async_engine;

use crate::{
    RegistryActor,
    ci::{
        engine::ActEngineBackend,
        lifecycle::{self, Clock},
    },
};

pub(super) struct PreparationGuard {
    registry: RegistryActor,
    backend: Arc<dyn ActEngineBackend>,
    intent: ActEngineIntent,
    claim: Option<String>,
}

impl PreparationGuard {
    pub(super) fn new(
        registry: RegistryActor,
        backend: Arc<dyn ActEngineBackend>,
        intent: ActEngineIntent,
        claim: String,
    ) -> Self {
        Self {
            registry,
            backend,
            intent,
            claim: Some(claim),
        }
    }

    pub(super) fn disarm(mut self) -> String {
        self.claim.take().unwrap_or_default()
    }
}

impl Drop for PreparationGuard {
    fn drop(&mut self) {
        let Some(claim) = self.claim.take() else {
            return;
        };
        let registry = self.registry.clone();
        let backend = Arc::clone(&self.backend);
        let intent = self.intent.clone();
        // If shutdown prevents this task from finishing, keep the claim. A peer
        // must remove its named engine before taking over that claim.
        async_engine::launch(async move {
            if let Err(error) =
                retire_preparation(&registry, backend.as_ref(), &intent, &claim).await
            {
                daemon_log!("bosn ci: interrupted preparation retains its machine claim: {error}");
            }
        })
        .detach();
    }
}

async fn retire_preparation(
    registry: &RegistryActor,
    backend: &dyn ActEngineBackend,
    intent: &ActEngineIntent,
    claim: &str,
) -> Result<(), String> {
    let owner = lifecycle::registry_owner(registry).await?;
    // Read past any queued Begin/Claim command; cancellation can occur before
    // the caller receives the committed execution token.
    let record = lifecycle::record(registry, &intent.run_id).await?;
    let token = record
        .as_ref()
        .and_then(|record| record.execution_claim.as_deref());
    let cleaned = lifecycle::cleanup(
        registry,
        backend,
        &owner,
        &intent.run_id,
        token,
        ActRunOutcome::Cancelled,
        &mut Clock(intent.created_at),
    )
    .await;
    let machine = backend
        .machine()
        .ok_or("shared engine backend unavailable")?;
    if machine.inspect(&intent.engine_name()).await?.is_some() {
        cleaned?;
        return Err("preparation engine still exists after cleanup".into());
    }
    // The claim fences the machine's one engine container, nothing else (#737).
    // Once that container is proven gone, cleanup the record still owes (a
    // recovery source retained until its reservation is acknowledged or
    // expires) stays with the record and startup recovery, and no longer holds
    // every daemon's next engine behind this claim.
    if let Err(error) = cleaned {
        daemon_log!(
            "bosn ci: engine {} is gone; releasing its machine claim, record cleanup deferred: {}",
            intent.engine_name(),
            error.chars().take(512).collect::<String>()
        );
    }
    machine.remove_claim(claim).await
}

/// Retry cleanup after a dropped preparation could not finish its first pass.
pub(super) async fn resume(
    registry: &RegistryActor,
    backend: &dyn ActEngineBackend,
    claim: &crate::ci::machine::EngineClaim,
) -> Result<(), String> {
    let run = claim
        .engine
        .strip_prefix("bosn-act-")
        .ok_or("invalid claimed engine name")?;
    match lifecycle::record(registry, run).await? {
        Some(record) if record.intent.engine_name() == claim.engine => {
            retire_preparation(registry, backend, &record.intent, &claim.id).await
        }
        Some(_) => Err("claimed engine differs from its registry record".into()),
        None => {
            let machine = backend
                .machine()
                .ok_or("shared engine backend unavailable")?;
            if machine.inspect(&claim.engine).await?.is_some() {
                return Err("claimed engine exists without a durable preparation record".into());
            }
            machine.remove_claim(&claim.id).await
        }
    }
}
