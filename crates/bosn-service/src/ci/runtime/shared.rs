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
            let (widget_release, widget_presence) = self.widget_report();
            return Ok(DoctorReply {
                engine: None,
                report: None,
                engine_capacity,
                admission_limit,
                task_capacity: self.task_capacity.get().cloned(),
                daemon_release: Some(self.release.to_string()),
                widget_release,
                widget_presence: Some(widget_presence),
            });
        };
        let report = self
            .backend
            .doctor(&engine)
            .await
            .map_err(|error| CiError::new("internal", error))?;
        let (widget_release, widget_presence) = self.widget_report();
        Ok(DoctorReply {
            engine: Some(engine),
            report: Some(report),
            engine_capacity,
            admission_limit,
            task_capacity: self.task_capacity.get().cloned(),
            daemon_release: Some(self.release.to_string()),
            widget_release,
            widget_presence: Some(widget_presence),
        })
    }

    /// Retire the shared engine once no run has used it for
    /// `[engine] idle_retire_secs` (read on every check).
    pub(super) fn start_shared_reaper(&self) {
        let state_dir = self.state_dir.clone();
        let source = std::sync::Mutex::new(IdleRetire::default());
        self.shared.spawn_reaper(move || {
            let loaded = super::super::config::load_engine(&state_dir);
            let mut source = source
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (ttl, complaint) = source.observe(loaded.map(|config| config.idle_retire_secs));
            if let Some(complaint) = complaint {
                daemon_log!(
                    "bosn ci: cannot read [engine] idle_retire_secs: {complaint}; \
                     keeping {}s",
                    ttl.as_secs()
                );
            }
            ttl
        });
    }

    /// Retire an idle shared engine and lease no more (daemon shutdown).
    pub async fn close_shared(&self) {
        self.shared.close().await;
    }
}

/// The idle limit across config reads: a config that stops parsing keeps the
/// last good limit rather than quietly returning to the default.
#[derive(Debug)]
struct IdleRetire {
    current: Duration,
    last_error: Option<String>,
}

impl Default for IdleRetire {
    fn default() -> Self {
        Self {
            current: DEFAULT_IDLE_RETIRE,
            last_error: None,
        }
    }
}

impl IdleRetire {
    /// The limit to use, and the error to log if this one is new.
    fn observe(&mut self, loaded: Result<Option<u64>, String>) -> (Duration, Option<String>) {
        match loaded {
            Ok(secs) => {
                self.last_error = None;
                self.current = secs.map_or(DEFAULT_IDLE_RETIRE, Duration::from_secs);
                (self.current, None)
            }
            Err(error) if self.last_error.as_ref() == Some(&error) => (self.current, None),
            Err(error) => {
                self.last_error = Some(error.clone());
                (self.current, Some(error))
            }
        }
    }
}

#[cfg(test)]
mod idle_retire_tests {
    use super::*;

    #[test]
    fn a_bad_config_keeps_the_last_good_limit_and_complains_once_per_error() {
        let mut source = IdleRetire::default();
        let two_hours = Duration::from_secs(7200);
        assert_eq!(source.observe(Ok(Some(7200))), (two_hours, None));
        let bad = || Err("unknown field `cpu`".to_owned());
        assert_eq!(
            source.observe(bad()),
            (two_hours, Some("unknown field `cpu`".to_owned()))
        );
        assert_eq!(source.observe(bad()), (two_hours, None));
        assert_eq!(source.observe(Ok(None)), (DEFAULT_IDLE_RETIRE, None));
        assert_eq!(
            source.observe(bad()),
            (DEFAULT_IDLE_RETIRE, Some("unknown field `cpu`".to_owned()))
        );
    }
}
