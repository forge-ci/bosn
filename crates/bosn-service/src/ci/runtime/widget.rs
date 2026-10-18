//! The runtime's widget operations: presence, the command queue and the
//! debounced auto-launch (the decisions themselves live in `ci::widget`).

use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use super::*;
use crate::ci::widget::{self as policy, WidgetCommand};

impl CiRuntime {
    pub(super) fn widget_state(&self) -> MutexGuard<'_, WidgetState> {
        self.widget.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether this is the daemon's first failed widget launch: one "the
    /// widget cannot be launched" line per daemon, not one per run.
    pub(crate) fn note_launch_failure(&self) -> bool {
        !self.launch_failure_logged.swap(true, Ordering::Relaxed)
    }

    pub(crate) fn widget_presence(&self) -> WidgetPresence {
        self.widget_state().presence(Instant::now())
    }

    /// Apply the `[widget]` configuration (daemon start).
    pub fn configure_widget(&self, config: WidgetConfig) {
        *self.widget_config.lock().unwrap_or_else(|e| e.into_inner()) = config;
    }

    fn reply(&self, allowed: bool, commands: Vec<WidgetCommand>) -> WidgetReply {
        WidgetReply {
            presence: self.widget_presence(),
            allowed,
            commands,
        }
    }

    pub(super) fn widget_hello(
        &self,
        pid: u32,
        session: &str,
        explicit: bool,
        release: Option<&str>,
    ) -> WidgetReply {
        let allowed = self.widget_state().hello_release(
            Instant::now(),
            pid,
            session,
            explicit,
            release,
            &self.release,
        );
        if allowed && policy::release_is_older(release, &self.release) {
            self.repair_widget(pid);
        }
        self.reply(allowed, Vec::new())
    }

    /// An older widget stays connected while the matching artifact is
    /// installed and the user unit is rewritten. The child process performs
    /// the downloads; the daemon awaits it without blocking its executor.
    fn repair_widget(&self, pid: u32) {
        if !cfg!(target_os = "linux") || self.widget_repairing.swap(true, Ordering::AcqRel) {
            return;
        }
        let runtime = self.clone();
        async_engine::launch(async move {
            let result = repair_widget_unit(&runtime.state_dir).await;
            match result {
                Ok(()) => {
                    if !runtime.widget_state().replacement_pending(pid) {
                        runtime.widget_repairing.store(false, Ordering::Release);
                        return;
                    }
                    match restart_widget_unit().await {
                        Ok(()) => {
                            // The service manager has accepted the new launch.
                            // A still-connected old widget also gets a Quit
                            // on its next poll, without a user dismissal.
                            runtime.widget_state().ready_to_replace(pid);
                        }
                        Err(error) => {
                            runtime.widget_state().cancel_replacement(pid);
                            daemon_log!("bosn ci: widget restart failed: {error}");
                        }
                    }
                }
                Err(error) => {
                    runtime.widget_state().cancel_replacement(pid);
                    daemon_log!("bosn ci: widget replacement deferred: {error}");
                }
            }
            runtime.widget_repairing.store(false, Ordering::Release);
        })
        .detach();
    }

    pub(super) fn widget_poll(&self, pid: u32) -> WidgetReply {
        let commands = self.widget_state().poll(Instant::now(), pid);
        self.reply(true, commands)
    }

    pub(super) fn widget_dismiss(&self, session: &str, pid: Option<u32>) -> WidgetReply {
        self.widget_state().dismiss(session, pid);
        self.reply(false, Vec::new())
    }

    /// Queue a command; paths must be local, external links allowlisted, and
    /// a quit needs a running widget (a queued one would outlive it).
    pub(super) fn widget_command(&self, command: WidgetCommand) -> Result<WidgetReply, CiError> {
        match &command {
            WidgetCommand::Open { path }
                if !path.starts_with('/') || path.starts_with("//") || path.len() > 256 =>
            {
                return Err(CiError::refused("open needs a local dashboard path"));
            }
            WidgetCommand::OpenExternal { url } => {
                let hosts = self
                    .widget_config
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .external_hosts
                    .clone();
                if !policy::external_allowed(url, &hosts) {
                    daemon_log!("bosn ci: refused to open a non-allowlisted external link");
                    return Err(CiError::refused(
                        "only https links to github.com, gitlab.com or a configured host open",
                    ));
                }
            }
            WidgetCommand::Quit if self.widget_presence() != WidgetPresence::Connected => {
                return Err(CiError::refused("no widget is running"));
            }
            _ => {}
        }
        self.widget_state().enqueue(command);
        Ok(self.reply(true, Vec::new()))
    }

    /// Bring the widget up when the policy allows and none is running. Only
    /// with the dashboard serving (the widget shows its pages); debounced.
    pub(crate) fn maybe_launch_widget(&self, trigger: LaunchTrigger) {
        if self.ui.get().is_none() {
            return;
        }
        let policy = self
            .widget_config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .auto_launch;
        if !self
            .widget_state()
            .should_launch(Instant::now(), policy, trigger)
        {
            return;
        }
        let Some((program, args)) = policy::launch_command() else {
            return;
        };
        let runtime = self.clone();
        async_engine::launch(async move {
            let spec = args.iter().fold(
                kernal_api::SpawnSpec::new(program)
                    .stdin(kernal_api::StreamMode::Null)
                    .stdout(kernal_api::StreamMode::Null)
                    .stderr(kernal_api::StreamMode::Piped),
                |spec, arg| spec.arg(arg),
            );
            let started =
                kernal_api::run_bounded_command_async(spec, Duration::from_secs(10), 4096).await;
            let ok = started.as_ref().is_ok_and(|out| out.exit.raw_code() == 0);
            if !ok && runtime.note_launch_failure() {
                daemon_log!(
                    "bosn ci: widget: not launched ({program} failed; no graphical session or the \
                     bosn-widget service is not installed)"
                );
            }
        })
        .detach();
    }
}

async fn repair_widget_unit(state_dir: &std::path::Path) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let output = kernal_api::run_bounded_command_async(
        kernal_api::SpawnSpec::new(executable)
            .arg("widget")
            .arg("install")
            .arg("--state-dir")
            .arg(state_dir),
        Duration::from_secs(6 * 60),
        4096,
    )
    .await
    .map_err(|error| error.to_string())?;
    (output.exit.raw_code() == 0)
        .then_some(())
        .ok_or_else(|| format!("widget installer exited {}", output.exit.raw_code()))
}

async fn restart_widget_unit() -> Result<(), String> {
    let output = kernal_api::run_bounded_command_async(
        kernal_api::SpawnSpec::new("systemctl")
            .arg("--user")
            .arg("restart")
            .arg("bosn-widget.service"),
        Duration::from_secs(15),
        4096,
    )
    .await
    .map_err(|error| error.to_string())?;
    (output.exit.raw_code() == 0)
        .then_some(())
        .ok_or_else(|| format!("systemctl restart exited {}", output.exit.raw_code()))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::Ordering};

    use super::*;
    use crate::ci::{
        CiRequest,
        lifecycle::tests::{FakeBackend, with_registry},
    };

    #[test]
    fn older_widget_hello_receives_replacement_quit_after_repair() {
        with_registry(|registry, dir| async move {
            let runtime = CiRuntime::start_with_release(
                &dir,
                registry,
                Arc::new(FakeBackend::default()),
                1,
                "0.1.23",
            );
            // Keep the real installer idle; completion below represents a
            // successful artifact install and user-unit handoff.
            runtime.widget_repairing.store(true, Ordering::Release);
            let hello: WidgetReply = serde_json::from_value(
                runtime
                    .handle(CiRequest::WidgetHello {
                        pid: 42,
                        session: "desktop".into(),
                        explicit: false,
                        release: Some("0.1.0".into()),
                    })
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert!(hello.allowed);
            assert_eq!(
                runtime.listing().runners.widget_release.as_deref(),
                Some("0.1.0")
            );
            runtime
                .handle(CiRequest::WidgetHello {
                    pid: 43,
                    session: "desktop".into(),
                    explicit: false,
                    release: Some("0.1.23".into()),
                })
                .await
                .unwrap();
            assert!(runtime.widget_state().ready_to_replace(42));
            let poll: WidgetReply = serde_json::from_value(
                runtime
                    .handle(CiRequest::WidgetPoll { pid: 42 })
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(poll.commands, [WidgetCommand::Quit]);
            runtime
                .handle(CiRequest::WidgetDismiss {
                    session: "desktop".into(),
                    pid: Some(42),
                })
                .await
                .unwrap();
            assert_eq!(runtime.widget_presence(), WidgetPresence::Connected);
            assert_eq!(
                runtime.listing().runners.widget_release.as_deref(),
                Some("0.1.23")
            );
        });
    }
}
