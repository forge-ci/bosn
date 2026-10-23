//! Permit compact windows only with installed compositor-owned dock placement.

use std::{
    future::Future,
    io::Read,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

const READY: &str = "bosn-widget-corner-ready-v1";

#[derive(Clone, Copy)]
enum Query {
    Loaded,
    Pulse,
}

async fn bus(query: Query) -> Option<String> {
    let name = match query {
        Query::Loaded => "bosn-widget-corner",
        Query::Pulse => READY,
    };
    let output = kernal_api::run_bounded_command_async(
        kernal_api::SpawnSpec::new("busctl").args([
            "--user",
            "--timeout=0.2",
            "call",
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting",
            "isScriptLoaded",
            "s",
            name,
        ]),
        Duration::from_millis(250),
        512,
    )
    .await
    .ok()?;
    (output.exit.raw_code() == 0)
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

fn loaded(reply: Option<String>) -> Option<bool> {
    match reply.as_deref().map(str::trim) {
        Some("b true") => Some(true),
        Some("b false") => Some(false),
        _ => None,
    }
}

/// Outcome of one bounded readiness check of the compositor placement script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Readiness {
    /// Placement is not installed/enabled, or KWin reports the script unloaded.
    Unavailable,
    /// A KWin reply was missing or malformed.
    NoReply,
    /// KWin reports the script loaded, but its readiness pulse never changed.
    StalePulse,
    /// The script was observed executing.
    Ready,
}

impl Readiness {
    fn diagnostic(self) -> Option<&'static str> {
        (self == Self::StalePulse).then_some(
            "KWin reports placement script bosn-widget-corner loaded, but its readiness \
             pulse did not change within 12 probes (1.2s); the script is likely stopped. \
             Compact panel refused. Recover with `bosn widget install`, which reloads only \
             the owned placement script.",
        )
    }
}

async fn live_ready<F, Fut>(configured: bool, mut query: F) -> Readiness
where
    F: FnMut(Query) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    if !configured {
        return Readiness::Unavailable;
    }
    match loaded(query(Query::Loaded).await) {
        Some(true) => {}
        Some(false) => return Readiness::Unavailable,
        None => return Readiness::NoReply,
    }
    let Some(initial) = loaded(query(Query::Pulse).await) else {
        return Readiness::NoReply;
    };
    // A cached marker is insufficient: require fresh execution after geometry
    // readback. Every call is bounded; at most twelve pulse probes are issued.
    for _ in 0..12 {
        kernal_api::async_engine::sleep(Duration::from_millis(100)).await;
        let Some(current) = loaded(query(Query::Pulse).await) else {
            return Readiness::NoReply;
        };
        if current != initial {
            return match loaded(query(Query::Loaded).await) {
                Some(true) => Readiness::Ready,
                Some(false) => Readiness::Unavailable,
                None => Readiness::NoReply,
            };
        }
    }
    Readiness::StalePulse
}

fn enabled(desktop: &str, wayland: bool, script: bool, config: &str) -> bool {
    if !wayland
        || !script
        || !desktop
            .split(':')
            .any(|part| part.eq_ignore_ascii_case("KDE"))
    {
        return false;
    }
    let mut plugins = false;
    let mut active = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            plugins = line == "[Plugins]";
        } else if plugins && let Some(value) = line.strip_prefix("bosn-widget-cornerEnabled=") {
            active = value == "true";
        }
    }
    active
}

fn installed_configuration() -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let mut text = String::new();
    let Ok(file) = std::fs::File::open(config.join("kwinrc")) else {
        return false;
    };
    if file.take(262145).read_to_string(&mut text).is_err() || text.len() > 262144 {
        return false;
    }
    enabled(
        &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        data.join("kwin/scripts/bosn-widget-corner/contents/code/main.js")
            .is_file(),
        &text,
    )
}

/// How long a refusal is reused before KWin is probed again (#692). Every
/// compact-window step asks; without this each one spawned up to fourteen
/// `busctl` calls and printed the same line, ~25,000 times an hour.
const REFUSAL_TTL: Duration = Duration::from_secs(30);

/// The last verdict and the last diagnostic printed. A refusal is reused for
/// [`REFUSAL_TTL`]; `Ready` is always re-proved, so the gate fails closed.
#[derive(Default)]
struct Gate {
    refused: Option<(Instant, Readiness)>,
    reported: Option<Readiness>,
}

impl Gate {
    fn cached(&self, now: Instant) -> Option<Readiness> {
        self.refused
            .filter(|(at, _)| now.saturating_duration_since(*at) < REFUSAL_TTL)
            .map(|(_, readiness)| readiness)
    }

    /// Record a probed verdict; the diagnostic is returned only on the
    /// transition into its state, never again while the state persists.
    fn record(&mut self, now: Instant, readiness: Readiness) -> Option<&'static str> {
        self.refused = (readiness != Readiness::Ready).then_some((now, readiness));
        let changed = self.reported != Some(readiness);
        self.reported = Some(readiness);
        changed.then(|| readiness.diagnostic()).flatten()
    }
}

static GATE: Mutex<Gate> = Mutex::new(Gate {
    refused: None,
    reported: None,
});

fn gate() -> std::sync::MutexGuard<'static, Gate> {
    GATE.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub async fn configured() -> bool {
    if let Some(readiness) = gate().cached(Instant::now()) {
        return readiness == Readiness::Ready;
    }
    let readiness = live_ready(installed_configuration(), bus).await;
    if let Some(message) = gate().record(Instant::now(), readiness) {
        eprintln!("bosn-widget: {message}");
    }
    readiness == Readiness::Ready
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_enabled_plugin_but_unloaded_fails_closed() {
        let configured = enabled(
            "KDE",
            true,
            true,
            "[Plugins]\nbosn-widget-cornerEnabled=true\n",
        );
        assert!(configured);
        let runtime = kernal_api::async_engine::RuntimeBuilder::current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            runtime.run(live_ready(configured, |_| std::future::ready(Some(
                "b false\n".into()
            )))),
            Readiness::Unavailable
        );
    }

    #[test]
    fn live_execution_requires_fresh_marker_and_valid_replies() {
        let runtime = kernal_api::async_engine::RuntimeBuilder::current_thread()
            .enable_all()
            .build()
            .unwrap();
        // #484: loaded script with a stale pulse is diagnosed, not silent.
        let stale = runtime.run(live_ready(true, |_| {
            std::future::ready(Some("b true".into()))
        }));
        assert_eq!(stale, Readiness::StalePulse);
        assert!(stale.diagnostic().unwrap().contains("bosn widget install"));
        assert_eq!(
            runtime.run(live_ready(true, |_| std::future::ready(None))),
            Readiness::NoReply
        );
        assert_eq!(
            runtime.run(live_ready(true, |_| std::future::ready(Some(
                "garbage".into()
            )))),
            Readiness::NoReply
        );
        assert_eq!(
            runtime.run(live_ready(false, |_| std::future::ready(None))),
            Readiness::Unavailable
        );
        let mut pulse = false;
        let ready = runtime.run(live_ready(true, |query| {
            let value = match query {
                Query::Loaded => true,
                Query::Pulse => {
                    pulse = !pulse;
                    pulse
                }
            };
            std::future::ready(Some(format!("b {value}")))
        }));
        assert_eq!(ready, Readiness::Ready);
        assert_eq!(ready.diagnostic(), None);
    }

    #[test]
    fn repeated_refusals_probe_once_per_ttl_and_report_once() {
        // #692: N compact-panel attempts against a stale pulse.
        let mut gate = Gate::default();
        let start = Instant::now();
        let (mut probes, mut reports) = (0, 0);
        for attempt in 0..1000u64 {
            let now = start + Duration::from_millis(attempt * 100);
            if gate.cached(now).is_some() {
                continue;
            }
            probes += 1;
            reports += usize::from(gate.record(now, Readiness::StalePulse).is_some());
        }
        // 100 s of attempts: one probe per 30 s TTL, one diagnostic in total.
        assert_eq!(probes, 4);
        assert_eq!(reports, 1);
        assert_eq!(gate.cached(start), Some(Readiness::StalePulse));
        // Ready is never cached, and a later stale pulse is a new transition.
        let later = start + Duration::from_secs(200);
        assert_eq!(gate.record(later, Readiness::Ready), None);
        assert_eq!(gate.cached(later), None);
        assert!(gate.record(later, Readiness::StalePulse).is_some());
    }

    #[test]
    fn compact_windows_require_enabled_kde_dock_placement() {
        let config = "[Plugins]\nbosn-widget-cornerEnabled=true\n";
        assert!(enabled("KDE:Plasma", true, true, config));
        assert!(!enabled("GNOME", true, true, config));
        assert!(!enabled("KDE", false, true, config));
        assert!(!enabled("KDE", true, false, config));
        assert!(!enabled(
            "KDE",
            true,
            true,
            "[Other]\nbosn-widget-cornerEnabled=true"
        ));
        assert!(!enabled(
            "KDE",
            true,
            true,
            "[Plugins]\nbosn-widget-cornerEnabled=true\nbosn-widget-cornerEnabled=false"
        ));
    }
}
