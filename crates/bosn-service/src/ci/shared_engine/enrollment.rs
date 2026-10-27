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
    async fn enroll_phase(
        &self,
        engine: &Spare,
        phase: EnrollmentPhase,
        max: u64,
        limit_secs: u64,
        cancellation: &CancellationToken,
        progress: &mut (dyn FnMut(super::LeaseUpdate) + Send),
    ) -> Result<Option<bosn_registry::act::ActToolGenerationBinding>, String> {
        use crate::ci::wire::{StageEvent, StageKind, StageState};
        let kind = match phase {
            EnrollmentPhase::Fence => StageKind::EnrollmentFence,
            EnrollmentPhase::Complete => StageKind::EnrollmentComplete,
        };
        progress(super::LeaseUpdate::Stage(StageEvent::Started {
            kind,
            outer_limit_secs: limit_secs,
            command_limit_secs: None,
            run_remaining_ms: None,
        }));
        let started = std::time::Instant::now();
        let result = async_engine::timeout(
            Duration::from_secs(limit_secs),
            async_engine::cancellable(
                cancellation,
                self.backend.enroll_native_tools(
                    &engine.observed.engine_id,
                    phase,
                    max,
                    Duration::from_secs(limit_secs),
                ),
            ),
        )
        .await;
        let state = match &result {
            Err(_) => StageState::TimedOut,
            Ok(Err(_)) => StageState::Cancelled,
            Ok(Ok(Err(_))) => StageState::Failed,
            Ok(Ok(Ok(_))) => StageState::Succeeded,
        };
        progress(super::LeaseUpdate::Stage(StageEvent::Finished {
            kind,
            state,
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }));
        match result {
            Err(_) => Err(format!(
                "native enrollment {phase:?} timed out after {:.3}s (outer limit {limit_secs}s)",
                started.elapsed().as_secs_f64()
            )),
            Ok(Err(_)) => Err(format!(
                "native enrollment cancelled during {phase:?} after {:.3}s",
                started.elapsed().as_secs_f64()
            )),
            Ok(Ok(Err(error))) => Err(format!(
                "native enrollment {phase:?} failed after {:.3}s (outer limit {limit_secs}s; act2 may enforce a shorter internal limit): {error}",
                started.elapsed().as_secs_f64()
            )),
            Ok(Ok(Ok(result))) => Ok(result),
        }
    }

    pub(super) async fn enroll_prepared(
        &self,
        want: &SparePlan,
        engine: Spare,
        guard: PreparationGuard,
        max: u64,
        cancellation: &CancellationToken,
        progress: &mut (dyn FnMut(super::LeaseUpdate) + Send),
    ) -> Result<Result<Held, Step>, String> {
        let held = Claim::held(&self.registry, &engine);
        held.verify().await?;
        self.enroll_phase(
            &engine,
            EnrollmentPhase::Fence,
            max,
            want.timeouts.enrollment_secs,
            cancellation,
            progress,
        )
        .await?;
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
        let binding = self
            .enroll_phase(
                &engine,
                EnrollmentPhase::Complete,
                max,
                want.timeouts.enrollment_secs,
                cancellation,
                progress,
            )
            .await?
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
        Box::pin(self.make(self.machine()?, &actual, &owner, cancellation, progress)).await
    }
}
