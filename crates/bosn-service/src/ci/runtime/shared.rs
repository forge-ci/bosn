//! The daemon's side of the shared engine (#547): running a planned run in
//! it, idle retirement, and retirement at shutdown.

use std::time::Duration;

use super::super::{lifecycle::EngineReport, shared_engine::DEFAULT_IDLE_RETIRE};
use super::*;

impl CiRuntime {
    /// Run a planned run in the machine's shared engine (#547).
    pub(super) async fn run_planned(
        &self,
        plan: EnginePlan,
        cancellation: &async_engine::CancellationToken,
        observer: &mut RunObserver,
    ) -> EngineReport {
        lifecycle::shared::run_on_shared(
            self.backend.as_ref(),
            &self.shared,
            plan.shared.clone(),
            &plan,
            cancellation,
            observer,
        )
        .await
    }

    /// `act doctor` in the shared engine this daemon holds, if any.
    pub(super) async fn doctor(&self) -> Result<DoctorReply, CiError> {
        let engine_capacity = self.engine_capacity().await;
        let admission_limit = Some(self.lock().scheduler.limit());
        let Some((engine, _)) = self.shared.status().await else {
            return Ok(DoctorReply {
                engine: None,
                report: None,
                engine_capacity,
                admission_limit,
                task_capacity: self.task_capacity.get().cloned(),
            });
        };
        let report = self
            .backend
            .doctor(&engine)
            .await
            .map_err(|error| CiError::new("internal", error))?;
        Ok(DoctorReply {
            engine: Some(engine),
            report: Some(report),
            engine_capacity,
            admission_limit,
            task_capacity: self.task_capacity.get().cloned(),
        })
    }

    /// Retire the shared engine once no run has used it for
    /// `[engine] idle_retire_secs` (read on every check).
    pub(super) fn start_shared_reaper(&self) {
        let state_dir = self.state_dir.clone();
        self.shared.spawn_reaper(move || idle_retire(&state_dir));
    }

    /// Retire an idle shared engine and lease no more (daemon shutdown).
    pub async fn close_shared(&self) {
        self.shared.close().await;
    }
}

/// `[engine] idle_retire_secs`, or [`DEFAULT_IDLE_RETIRE`].
fn idle_retire(state_dir: &std::path::Path) -> Duration {
    super::super::config::load_engine(state_dir)
        .ok()
        .and_then(|config| config.idle_retire_secs)
        .map_or(DEFAULT_IDLE_RETIRE, Duration::from_secs)
}
