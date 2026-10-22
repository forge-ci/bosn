//! Cut over under a genuine machine claim; the preparation engine admits no run.
use super::{Held, SharedEngine, Step, preparation::PreparationGuard};
use crate::ci::{
    engine::EnrollmentPhase,
    lifecycle::Claim,
    spare::{self, Spare, SparePlan},
};
use kernal_api::async_engine::{self, CancellationToken};
use std::time::Duration;

impl SharedEngine {
    pub(super) async fn enroll_prepared(
        &self,
        want: &SparePlan,
        engine: Spare,
        guard: PreparationGuard,
        max: u64,
        cancellation: &CancellationToken,
    ) -> Result<Result<Held, Step>, String> {
        let held = Claim::held(&self.registry, &engine);
        held.verify().await?;
        async_engine::cancellable(
            cancellation,
            self.backend.enroll_native_tools(
                &engine.observed.engine_id,
                EnrollmentPhase::Fence,
                max,
            ),
        )
        .await
        .map_err(|_| "native enrollment cancelled during source fence")??;
        // Open directory descriptors survive the atomic source exchange. Drain
        // every other engine after installing the fence before publishing.
        loop {
            held.verify().await?;
            let busy = self
                .drain_legacy(self.machine()?, Some(&engine.intent.engine_name()))
                .await?;
            if busy.is_empty() {
                break;
            }
            async_engine::cancellable(cancellation, async_engine::sleep(Duration::from_secs(2)))
                .await
                .map_err(|_| "native enrollment cancelled while draining writers")?;
        }
        let binding = async_engine::cancellable(
            cancellation,
            self.backend.enroll_native_tools(
                &engine.observed.engine_id,
                EnrollmentPhase::Complete,
                max,
            ),
        )
        .await
        .map_err(|_| "native enrollment cancelled during completion")??
        .ok_or("native enrollment returned no generation")?;
        held.verify().await?;
        if binding.max_payload_bytes != max {
            return Err("native enrollment changed payload bound".into());
        }
        let mut actual = want.with_generation(binding)?;
        actual.intent.run_id = crate::ci::wire::new_uuid().await.map_err(|e| e.message)?;
        actual.intent.created_at = crate::ci::lifecycle::now_seconds();
        spare::retire(&self.registry, self.backend.as_ref(), &engine).await?;
        if self
            .machine()?
            .inspect(&engine.intent.engine_name())
            .await?
            .is_some()
        {
            return Err("native preparation engine survived retirement".into());
        }
        self.machine()?.remove_claim(&guard.disarm()).await?;
        let owner = crate::ci::lifecycle::registry_owner(&self.registry).await?;
        // The source refusal file remains across these two genuine claims.
        Box::pin(self.make(self.machine()?, &actual, &owner, cancellation)).await
    }
}
