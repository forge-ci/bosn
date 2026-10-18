//! The desktop widget's daemon side: presence, dismissal, the command queue
//! the dashboard fills (toggle, open, open-external, quit) and the auto-launch
//! decision. The widget is a separate user-session process that talks to the
//! daemon only through typed CI requests; the daemon stays headless.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

/// A poll older than this means the widget is gone (crashed or quit).
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
/// At most one launch attempt per this interval.
pub const LAUNCH_DEBOUNCE: Duration = Duration::from_secs(10);
const MAX_QUEUED: usize = 64;

fn release_key(release: &str) -> Option<(u64, u64, u64)> {
    let core = release.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(str::parse::<u64>);
    let key = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    parts.next().is_none().then_some(key)
}

/// A missing release is a protocol-1 widget. Unparsable or newer releases
/// remain connected and visible in status instead of being downgraded.
pub fn release_is_older(widget: Option<&str>, daemon: &str) -> bool {
    match widget {
        None => true,
        Some(widget) => matches!((release_key(widget), release_key(daemon)),
            (Some(widget), Some(daemon)) if widget < daemon),
    }
}

vocabulary!(
    /// When the daemon brings the widget up on its own.
    AutoLaunch, "auto_launch" {
        Always => "always",
        OnActivity => "on-activity",
        Never => "never",
    }
);

// The `vocabulary!` macro cannot carry `#[default]` for one enum only.
#[allow(clippy::derivable_impls)]
impl Default for AutoLaunch {
    fn default() -> Self {
        Self::Always
    }
}

vocabulary!(
    /// Whether a widget is running, gone, or deliberately quit.
    WidgetPresence, "presence" {
        Connected => "connected",
        Absent => "absent",
        Dismissed => "dismissed",
    }
);

/// What the widget process should do next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum WidgetCommand {
    /// Show or hide the panel.
    Toggle,
    /// Show the bubble (a second `bosn widget` asked for it).
    Show,
    /// Open or focus the full-view window on a dashboard path.
    Open { path: String },
    /// Open an allowlisted external URL with the OS opener.
    OpenExternal { url: String },
    /// Quit deliberately, as closing the bubble does (dismiss, then exit).
    Quit,
}

/// Why the daemon considered launching the widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchTrigger {
    DaemonStart,
    Activity,
}

#[derive(Debug, Default)]
pub struct WidgetState {
    seen: Option<WidgetSeen>,
    /// A menu quit in this graphical session suppresses auto-launch.
    dismissed_session: Option<String>,
    queue: VecDeque<WidgetCommand>,
    last_launch: Option<Instant>,
    /// A replacement quit must not count as a user dismissal.
    replacing_pid: Option<u32>,
    replacement_ready: bool,
    replacement_quit: bool,
}

#[derive(Debug)]
struct WidgetSeen {
    at: Instant,
    pid: u32,
    session: String,
    release: Option<String>,
}

impl WidgetState {
    pub fn presence(&self, now: Instant) -> WidgetPresence {
        match &self.seen {
            Some(seen) if now.duration_since(seen.at) < HEARTBEAT_TIMEOUT => {
                WidgetPresence::Connected
            }
            _ if self.dismissed_session.is_some() => WidgetPresence::Dismissed,
            _ => WidgetPresence::Absent,
        }
    }

    pub fn release(&self, now: Instant) -> Option<&str> {
        (self.presence(now) == WidgetPresence::Connected)
            .then(|| self.seen.as_ref().and_then(|seen| seen.release.as_deref()))
            .flatten()
    }

    /// A widget process starting. `explicit` (`bosn widget` typed by the
    /// user) clears a dismissal; an auto-launched one in the dismissed
    /// session is told to exit. A new session always clears it.
    pub fn hello(&mut self, now: Instant, pid: u32, session: &str, explicit: bool) -> bool {
        let dismissed_here = self.dismissed_session.as_deref() == Some(session);
        if dismissed_here && !explicit {
            return false;
        }
        self.dismissed_session = None;
        // A normal queued quit addressed the previous connected widget.
        self.queue.retain(|command| *command != WidgetCommand::Quit);
        // A new widget can register before systemd's restart returns. Keep
        // the old process's replacement handoff until its own poll or exit.
        if self.replacing_pid.is_none() {
            self.replacing_pid = None;
            self.replacement_ready = false;
            self.replacement_quit = false;
        }
        self.seen = Some(WidgetSeen {
            at: now,
            pid,
            session: session.into(),
            release: None,
        });
        true
    }

    /// A release mismatch asks the old process to quit. Its later dismiss
    /// message is an upgrade exit, not a user's choice to suppress relaunch.
    pub fn hello_release(
        &mut self,
        now: Instant,
        pid: u32,
        session: &str,
        explicit: bool,
        release: Option<&str>,
        daemon_release: &str,
    ) -> bool {
        let same_replacing_pid = self.replacing_pid == Some(pid);
        if !self.hello(now, pid, session, explicit) {
            return false;
        }
        if let Some(seen) = &mut self.seen {
            seen.release = release.map(str::to_owned);
        }
        if release_is_older(release, daemon_release) && !same_replacing_pid {
            self.replacing_pid = Some(pid);
            self.replacement_ready = false;
            self.replacement_quit = false;
        } else if !release_is_older(release, daemon_release) && same_replacing_pid {
            self.cancel_replacement(pid);
        }
        true
    }

    /// Called only after a matching replacement is installed and its launch
    /// path has been updated. The old widget stays usable if repair fails.
    pub fn ready_to_replace(&mut self, pid: u32) -> bool {
        if self.replacing_pid == Some(pid) {
            self.replacement_ready = true;
            return true;
        }
        false
    }

    pub fn replacement_pending(&self, pid: u32) -> bool {
        self.replacing_pid == Some(pid)
    }

    pub fn cancel_replacement(&mut self, pid: u32) {
        if self.replacing_pid == Some(pid) {
            self.replacing_pid = None;
            self.replacement_ready = false;
            self.replacement_quit = false;
        }
    }

    /// The heartbeat; returns (and clears) the pending commands.
    pub fn poll(&mut self, now: Instant, pid: u32) -> Vec<WidgetCommand> {
        let deliver_replacement = self.replacing_pid == Some(pid) && self.replacement_ready;
        if deliver_replacement {
            self.replacement_ready = false;
            self.replacement_quit = true;
            if self.seen.as_ref().is_some_and(|seen| seen.pid != pid) {
                return vec![WidgetCommand::Quit];
            }
        }
        if let Some(seen) = &mut self.seen
            && seen.pid == pid
        {
            seen.at = now;
            let mut commands: Vec<_> = self.queue.drain(..).collect();
            if deliver_replacement {
                commands.push(WidgetCommand::Quit);
            }
            return commands;
        }
        Vec::new()
    }

    /// A deliberate quit from the widget's menu (a crash is not one).
    pub fn dismiss(&mut self, session: &str, pid: Option<u32>) {
        if self
            .seen
            .as_ref()
            .is_some_and(|seen| seen.session != session)
        {
            return;
        }
        if self.replacement_quit && (pid.is_none() || pid == self.replacing_pid) {
            let current_was_replaced = self
                .seen
                .as_ref()
                .is_some_and(|seen| Some(seen.pid) == self.replacing_pid);
            self.replacing_pid = None;
            self.replacement_ready = false;
            self.replacement_quit = false;
            if current_was_replaced {
                self.seen = None;
                self.queue.clear();
            }
            return;
        }
        if (pid.is_none() || pid == self.replacing_pid)
            && self.replacing_pid.is_some()
            && self
                .seen
                .as_ref()
                .is_some_and(|seen| Some(seen.pid) != self.replacing_pid)
        {
            // A legacy old process quit before receiving its replacement
            // command. Preserve the new process's connection, but remember
            // the user's dismissal for the next auto-launch decision.
            self.dismissed_session = Some(session.into());
            self.replacing_pid = None;
            self.replacement_ready = false;
            self.replacement_quit = false;
            return;
        }
        if pid.is_some() && self.seen.as_ref().is_some_and(|seen| Some(seen.pid) != pid) {
            return;
        }
        self.dismissed_session = Some(session.into());
        self.seen = None;
        self.queue.clear();
        self.replacing_pid = None;
        self.replacement_ready = false;
        self.replacement_quit = false;
    }

    /// Queue a command for the widget; the oldest are dropped when full.
    pub fn enqueue(&mut self, command: WidgetCommand) {
        if self.queue.len() == MAX_QUEUED {
            self.queue.pop_front();
        }
        self.queue.push_back(command);
    }

    /// Should the daemon try to launch the widget now? Records the attempt.
    pub fn should_launch(
        &mut self,
        now: Instant,
        policy: AutoLaunch,
        trigger: LaunchTrigger,
    ) -> bool {
        let wanted = !matches!(
            (policy, trigger),
            (AutoLaunch::Never, _) | (AutoLaunch::OnActivity, LaunchTrigger::DaemonStart)
        );
        let recently = self
            .last_launch
            .is_some_and(|at| now.duration_since(at) < LAUNCH_DEBOUNCE);
        if !wanted || recently || self.presence(now) != WidgetPresence::Absent {
            return false;
        }
        self.last_launch = Some(now);
        true
    }
}

/// External links the widget may open: `https` to an allowlisted host only.
pub fn external_allowed(url: &str, extra_hosts: &[String]) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.contains(['@', ':']) || host.is_empty() {
        return false;
    }
    ["github.com", "gitlab.com"]
        .iter()
        .copied()
        .chain(extra_hosts.iter().map(String::as_str))
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
}

/// The platform command that starts the installed widget service, if any.
pub fn launch_command() -> Option<(&'static str, Vec<String>)> {
    if cfg!(target_os = "linux") {
        Some((
            "systemctl",
            vec![
                "--user".into(),
                "start".into(),
                "bosn-widget.service".into(),
            ],
        ))
    } else if cfg!(target_os = "macos") {
        let uid = kernal_api::platform::ipc::current_user_id().ok()?;
        Some((
            "launchctl",
            vec!["kickstart".into(), format!("gui/{uid}/dev.bosn.widget")],
        ))
    } else if cfg!(windows) {
        Some((
            "schtasks",
            vec!["/Run".into(), "/TN".into(), "bosn-widget".into()],
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_follows_heartbeats_and_a_crash_is_not_a_dismissal() {
        let t0 = Instant::now();
        let mut w = WidgetState::default();
        assert_eq!(w.presence(t0), WidgetPresence::Absent);
        assert!(w.hello(t0, 7, "s1", false));
        assert_eq!(w.presence(t0), WidgetPresence::Connected);
        // No heartbeat for longer than the timeout: gone, not dismissed.
        assert_eq!(w.presence(t0 + HEARTBEAT_TIMEOUT), WidgetPresence::Absent);
    }

    #[test]
    fn an_older_widget_is_replaced_without_recording_a_user_dismissal() {
        let now = Instant::now();
        let mut state = WidgetState::default();
        assert!(state.hello_release(now, 7, "desktop", false, Some("0.1.22"), "0.1.23"));
        assert_eq!(state.release(now), Some("0.1.22"));
        assert!(state.ready_to_replace(7));
        assert_eq!(state.poll(now, 7), [WidgetCommand::Quit]);
        state.dismiss("desktop", Some(7));
        assert_eq!(state.presence(now), WidgetPresence::Absent);
        assert!(state.hello_release(now, 8, "desktop", false, Some("0.1.23"), "0.1.23"));
        assert_eq!(state.release(now), Some("0.1.23"));
        assert!(state.hello_release(now, 9, "desktop", false, None, "0.1.23"));
        assert!(state.ready_to_replace(9), "a protocol-1 widget is older");
        assert_eq!(state.poll(now, 9), [WidgetCommand::Quit]);
    }

    #[test]
    fn failed_repair_keeps_an_old_widget_connected_and_its_quit_choice_meaningful() {
        let now = Instant::now();
        let mut state = WidgetState::default();
        assert!(state.hello_release(now, 7, "desktop", false, Some("0.1.22"), "0.1.23"));
        state.cancel_replacement(7);
        assert!(state.poll(now, 7).is_empty());
        assert_eq!(state.release(now), Some("0.1.22"));
        state.dismiss("desktop", Some(7));
        assert_eq!(state.presence(now), WidgetPresence::Dismissed);
    }

    #[test]
    fn a_user_quit_before_replacement_quit_is_delivered_still_dismisses() {
        let now = Instant::now();
        let mut state = WidgetState::default();
        assert!(state.hello_release(now, 7, "desktop", false, Some("0.1.22"), "0.1.23"));
        assert!(state.ready_to_replace(7));
        state.dismiss("desktop", Some(7));
        assert_eq!(state.presence(now), WidgetPresence::Dismissed);
        assert!(state.poll(now, 7).is_empty());
    }

    #[test]
    fn a_new_widget_can_register_before_the_old_process_receives_quit() {
        let now = Instant::now();
        let mut state = WidgetState::default();
        assert!(state.hello_release(now, 7, "desktop", false, Some("0.1.22"), "0.1.23"));
        assert!(state.hello_release(now, 8, "desktop", false, Some("0.1.23"), "0.1.23"));
        assert!(state.ready_to_replace(7));
        assert!(state.poll(now, 8).is_empty(), "the new widget stays up");
        assert_eq!(state.poll(now, 7), [WidgetCommand::Quit]);
        state.dismiss("desktop", None); // old protocol-1 widget
        assert_eq!(state.release(now), Some("0.1.23"));
        assert_eq!(state.presence(now), WidgetPresence::Connected);
        state.dismiss("desktop", Some(8));
        assert_eq!(state.presence(now), WidgetPresence::Dismissed);
    }

    #[test]
    fn an_old_widgets_rehello_keeps_its_in_flight_replacement() {
        let now = Instant::now();
        let mut state = WidgetState::default();
        assert!(state.hello_release(now, 7, "desktop", false, Some("0.1.22"), "0.1.23"));
        assert!(state.ready_to_replace(7));
        assert!(state.hello_release(now, 7, "desktop", false, Some("0.1.22"), "0.1.23"));
        assert_eq!(state.poll(now, 7), [WidgetCommand::Quit]);
    }

    #[test]
    fn a_newer_widget_is_never_downgraded_by_an_older_daemon() {
        let now = Instant::now();
        let mut state = WidgetState::default();
        assert!(state.hello_release(now, 7, "desktop", false, Some("0.1.24"), "0.1.23"));
        assert!(!state.ready_to_replace(7));
        assert!(state.poll(now, 7).is_empty());
        assert_eq!(state.release(now), Some("0.1.24"));
    }

    #[test]
    fn a_menu_quit_holds_until_a_new_session_or_an_explicit_start() {
        let t0 = Instant::now();
        let mut w = WidgetState::default();
        w.hello(t0, 7, "s1", false);
        w.dismiss("s1", Some(7));
        assert_eq!(w.presence(t0), WidgetPresence::Dismissed);
        assert!(!w.should_launch(t0, AutoLaunch::Always, LaunchTrigger::Activity));
        assert!(
            !w.hello(t0, 8, "s1", false),
            "auto-launch in the dismissed session exits"
        );
        assert!(
            w.hello(t0, 9, "s1", true),
            "`bosn widget` typed by the user clears it"
        );
        w.dismiss("s1", Some(9));
        assert!(w.hello(t0, 10, "s2", false), "a new login clears it");
    }

    #[test]
    fn launches_are_debounced_and_follow_the_policy() {
        let t0 = Instant::now();
        let mut w = WidgetState::default();
        assert!(!w.should_launch(t0, AutoLaunch::Never, LaunchTrigger::Activity));
        assert!(!w.should_launch(t0, AutoLaunch::OnActivity, LaunchTrigger::DaemonStart));
        assert!(w.should_launch(t0, AutoLaunch::OnActivity, LaunchTrigger::Activity));
        // 20 concurrent submissions inside the debounce: one attempt only.
        assert!((0..20).all(|_| !w.should_launch(t0, AutoLaunch::Always, LaunchTrigger::Activity)));
        assert!(w.should_launch(
            t0 + LAUNCH_DEBOUNCE,
            AutoLaunch::Always,
            LaunchTrigger::Activity
        ));
        w.hello(t0 + LAUNCH_DEBOUNCE, 3, "s", false);
        assert!(
            !w.should_launch(
                t0 + LAUNCH_DEBOUNCE * 3,
                AutoLaunch::Always,
                LaunchTrigger::Activity
            ) || w.presence(t0 + LAUNCH_DEBOUNCE * 3) == WidgetPresence::Absent
        );
    }

    #[test]
    fn commands_reach_only_the_registered_widget_and_stay_bounded() {
        let t0 = Instant::now();
        let mut w = WidgetState::default();
        w.hello(t0, 7, "s", false);
        for _ in 0..100 {
            w.enqueue(WidgetCommand::Toggle);
        }
        assert!(w.poll(t0, 99).is_empty(), "an unknown pid gets nothing");
        assert_eq!(w.poll(t0, 7).len(), MAX_QUEUED);
        assert!(w.poll(t0, 7).is_empty(), "drained once");
    }

    #[test]
    fn a_quit_addresses_only_the_widget_that_was_running() {
        let t0 = Instant::now();
        let mut w = WidgetState::default();
        w.hello(t0, 7, "s", false);
        w.enqueue(WidgetCommand::Quit);
        w.enqueue(WidgetCommand::Open { path: "/".into() });
        // Widget 7 died before its next poll; its replacement must not quit.
        assert!(w.hello(t0, 8, "s", false));
        assert_eq!(w.poll(t0, 8), [WidgetCommand::Open { path: "/".into() }]);
    }

    #[test]
    fn only_https_allowlisted_hosts_open_externally() {
        let extra = ["git.example.org".to_string()];
        assert!(external_allowed(
            "https://github.com/zackees/bosn/pull/1",
            &extra
        ));
        assert!(external_allowed("https://GitLab.com/x", &extra));
        assert!(external_allowed("https://git.example.org/x", &extra));
        for url in [
            "http://github.com/x",
            "https://github.com.evil.example/x",
            "https://evil.example/github.com",
            "https://user@github.com/x",
            "https://github.com:444/x",
            "javascript:alert(1)",
            "file:///etc/passwd",
        ] {
            assert!(!external_allowed(url, &extra), "{url}");
        }
    }
}
