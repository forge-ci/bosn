//! Desktop notifications and the external-link opener, from the user
//! session (the daemon is headless). Notify on every failure, and on the
//! completion of runs a human started; agent successes stay silent.

use std::{collections::BTreeSet, sync::Mutex};

use bosn_service::{
    Client,
    ci::{Conclusion, RunState, RunView},
};

#[derive(Default)]
pub struct Notifier {
    /// Finished runs already seen (the first poll only records, never notifies).
    seen: BTreeSet<String>,
    primed: bool,
}

/// Whether a finished run deserves a notification.
pub fn worth_notifying(run: &RunView) -> bool {
    match run.record.conclusion {
        Some(Conclusion::Success) => run.record.actor == "human",
        Some(_) => true,
        None => false,
    }
}

impl Notifier {
    pub async fn check(&mut self, client: &Client) {
        let Ok(list) = client.ci_list(None, Some(RunState::Done), Some(20)).await else {
            return;
        };
        for run in list.runs {
            if self.seen.insert(run.record.id.clone()) && self.primed && worth_notifying(&run) {
                notify(&run);
            }
        }
        self.primed = true;
    }
}

fn notify(run: &RunView) {
    let conclusion = run.record.conclusion.map_or("done", Conclusion::as_str);
    let summary = format!("bosn ci: {conclusion}");
    let body = format!(
        "{} · {} · {}",
        run.record.workflow.trim_start_matches(".github/workflows/"),
        &run.record.sha[..12.min(run.record.sha.len())],
        run.record.actor
    );
    // org.freedesktop.Notifications through libnotify's CLI until a
    // kernal-api notification facade exists.
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=bosn", &summary, &body])
        .spawn();
}

/// The last dashboard refusal reported, so one that persists across every
/// window attempt is shown once, and again only after a grant succeeds.
#[derive(Default)]
struct Refusals {
    last: Option<String>,
}

impl Refusals {
    fn first(&mut self, message: &str) -> bool {
        if self.last.as_deref() == Some(message) {
            return false;
        }
        self.last = Some(message.to_owned());
        true
    }
}

static REFUSALS: Mutex<Refusals> = Mutex::new(Refusals { last: None });

fn refusals() -> std::sync::MutexGuard<'static, Refusals> {
    REFUSALS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The text shown when the daemon refuses a dashboard page.
fn refusal_body(message: &str) -> String {
    format!("{message}. `bosn widget install` turns the dashboard on for this state directory.")
}

/// The daemon refused a dashboard page: say so, with the fix, once per
/// distinct refusal.
pub fn ui_refused(message: &str) {
    if !refusals().first(message) {
        return;
    }
    let body = refusal_body(message);
    eprintln!("bosn-widget: dashboard unavailable: {body}");
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=bosn", "bosn: dashboard unavailable", &body])
        .spawn();
}

/// A grant succeeded: a later refusal is a new transition worth reporting.
pub fn ui_granted() {
    refusals().last = None;
}

/// Open an allowlisted URL (the daemon already checked the allowlist).
pub fn open_external(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(url).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_persisting_refusal_is_reported_once_with_its_fix() {
        let mut refusals = Refusals::default();
        let refused = "refused: the dashboard is disabled; set `[ui] enabled = true` in <state>/config.toml and restart the daemon";
        let reports = (0..100).filter(|_| refusals.first(refused)).count();
        assert_eq!(reports, 1);
        assert!(refusals.first("refused: path must be a local dashboard path"));
        refusals.last = None;
        assert!(refusals.first(refused), "a refusal after a grant is new");
        let body = refusal_body(refused);
        assert!(body.contains("[ui] enabled = true") && body.contains("bosn widget install"));
    }

    #[test]
    fn failures_always_notify_successes_only_for_humans() {
        let view = |conclusion, actor: &str| {
            let mut value: serde_json::Value =
                serde_json::from_str(include_str!("../tests/run_view.json")).unwrap();
            value["conclusion"] = serde_json::to_value(conclusion).unwrap();
            value["actor"] = actor.into();
            serde_json::from_value::<RunView>(value).unwrap()
        };
        assert!(worth_notifying(&view(Some(Conclusion::Failure), "agent:x")));
        assert!(worth_notifying(&view(Some(Conclusion::Success), "human")));
        assert!(!worth_notifying(&view(
            Some(Conclusion::Success),
            "agent:x"
        )));
        assert!(!worth_notifying(&view(None::<Conclusion>, "human")));
    }
}
