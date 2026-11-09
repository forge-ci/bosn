//! `bosn capacity`: an observation, with no admission or lifecycle side effects.

use bosn_service::capacity_observation::{HostSample, Observation, Reason, Unavailable};
use kernal_api::async_engine::RuntimeBuilder;
use std::{
    ffi::OsString,
    fmt::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

const USAGE: &str = "bosn capacity [--json] [--no-docker] [--timeout-ms N] [--state-dir STATE_DIR]";
struct Arguments {
    json: bool,
    docker: bool,
    timeout: Duration,
    state: PathBuf,
}

fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Arguments, ()> {
    let mut json = false;
    let mut docker = true;
    let mut timeout = None;
    let mut state = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--json") if !json => json = true,
            Some("--no-docker") if docker => docker = false,
            Some("--timeout-ms") if timeout.is_none() => {
                let value = args
                    .next()
                    .ok_or(())?
                    .to_str()
                    .ok_or(())?
                    .parse::<u64>()
                    .map_err(|_| ())?;
                if value == 0 {
                    return Err(());
                }
                timeout = Some(Duration::from_millis(value));
            }
            Some("--state-dir") if state.is_none() => {
                let value = args.next().ok_or(())?;
                if value.is_empty() {
                    return Err(());
                }
                state = Some(PathBuf::from(value));
            }
            _ => return Err(()),
        }
    }
    Ok(Arguments {
        json,
        docker,
        timeout: timeout.unwrap_or(Duration::from_secs(5)),
        state: state.unwrap_or_else(bosn_service::mcp::default_state_dir),
    })
}

pub fn run(args: impl Iterator<Item = OsString>) {
    let args = parse(args).unwrap_or_else(|()| {
        eprintln!("usage: {USAGE}");
        std::process::exit(2)
    });
    match observe(&args) {
        Ok(observation) => {
            let text = if args.json {
                serde_json::to_string(&observation).ok()
            } else {
                Some(human(&observation))
            };
            let Some(text) = text else { fail() };
            use std::io::Write as _;
            let _ = writeln!(std::io::stdout().lock(), "{text}");
        }
        Err(_) => fail(),
    }
}

fn fail() -> ! {
    eprintln!("bosn capacity: observation failed");
    std::process::exit(1)
}

fn observe(args: &Arguments) -> Result<Observation, std::io::Error> {
    let started = Instant::now();
    let pending = super::capacity_daemon::start(args.state.clone(), args.timeout)?;
    let state = args.state.clone();
    let docker = args.docker;
    let timeout = args.timeout.saturating_sub(started.elapsed());
    let mut observation = host_within(timeout, docker, move || {
        let host = HostSample::begin(&state);
        let runtime = RuntimeBuilder::current_thread().enable_all().build()?;
        // Leave time for the bounded ps process to reap before the outer deadline.
        Ok(runtime.run(host.finish(docker, timeout.saturating_sub(Duration::from_millis(100)))))
    })?;
    pending.finish(&mut observation);
    observation.age_ms = u64::try_from(started.elapsed().as_millis()).ok();
    Ok(observation)
}

/// Filesystem IO can stall too. Keep it off the CLI boundary and bound the
/// entire host worker, including runtime teardown and the second stat read.
fn host_within(
    timeout: Duration,
    docker: bool,
    probe: impl FnOnce() -> std::io::Result<Observation> + Send + 'static,
) -> Result<Observation, std::io::Error> {
    let sampled_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok());
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("capacity-host".into())
        .spawn(move || {
            let result = probe();
            let _ = sender.send(result);
        })?;
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(Observation {
            sampled_at_ms,
            unavailable: vec![
                Unavailable {
                    field: "host".into(),
                    reason: Reason::Timeout,
                },
                Unavailable {
                    field: "external".into(),
                    reason: if docker {
                        Reason::Timeout
                    } else {
                        Reason::Unsupported
                    },
                },
            ],
            ..Observation::default()
        }),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(std::io::Error::other("host worker failed"))
        }
    }
}

fn value<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".into(), |v| v.to_string())
}

fn human(o: &Observation) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "Bosn:");
    let _ = writeln!(text, "  Daemon:      {:?}", o.bosn.daemon);
    let _ = writeln!(
        text,
        "  Jobs:        {} queued, {} running",
        value(o.bosn.jobs.queued),
        value(o.bosn.jobs.running)
    );
    let _ = writeln!(
        text,
        "  CI runs:     {} queued, {} running",
        value(o.bosn.ci_runs.queued),
        value(o.bosn.ci_runs.running)
    );
    let _ = writeln!(
        text,
        "  Task slots:  {} / {}",
        value(o.bosn.slots.task_used),
        value(o.bosn.slots.task_limit)
    );
    let _ = writeln!(
        text,
        "  CI slots:    {} / {}",
        value(o.bosn.slots.ci_used),
        value(o.bosn.slots.ci_admission_limit)
    );
    let _ = writeln!(
        text,
        "  Engine:      {} CPUs, {} bytes memory",
        value(o.bosn.engine.cpus),
        value(o.bosn.engine.memory_bytes)
    );
    human_host(&mut text, o);
    human_external(&mut text, o);
    let _ = writeln!(text, "Unavailable:");
    if o.unavailable.is_empty() {
        let _ = writeln!(text, "  none");
    }
    for unavailable in &o.unavailable {
        let _ = writeln!(text, "  {}: {:?}", unavailable.field, unavailable.reason);
    }
    let _ = write!(text, "{}", o.advisory);
    text
}

fn human_host(text: &mut String, o: &Observation) {
    let _ = writeln!(text, "Host:");
    let _ = writeln!(
        text,
        "  CPU:         {} logical, {}% busy, {}% IO delay",
        value(o.host.logical_cpus),
        value(o.host.cpu_percent.map(|v| format!("{v:.1}"))),
        value(o.host.io_delay_percent.map(|v| format!("{v:.1}")))
    );
    let _ = writeln!(
        text,
        "  Load:        {} / {} / {}",
        value(o.host.load[0]),
        value(o.host.load[1]),
        value(o.host.load[2])
    );
    let _ = writeln!(
        text,
        "  Memory:      {} / {} bytes available",
        value(o.host.memory.available_bytes),
        value(o.host.memory.total_bytes)
    );
    let _ = writeln!(
        text,
        "  Swap:        {} / {} bytes used",
        value(o.host.swap.used_bytes),
        value(o.host.swap.total_bytes)
    );
    let _ = writeln!(
        text,
        "  PSI avg10:   CPU some {}, memory some/full {}/{}, IO full {}",
        value(o.host.psi.cpu_some_avg10),
        value(o.host.psi.memory_some_avg10),
        value(o.host.psi.memory_full_avg10),
        value(o.host.psi.io_full_avg10)
    );
    for disk in &o.host.disk {
        let _ = writeln!(
            text,
            "  Disk:        {}: {} / {} bytes free",
            disk.path.display(),
            value(disk.free_bytes),
            value(disk.total_bytes)
        );
    }
}

fn human_external(text: &mut String, o: &Observation) {
    let labels = o
        .external
        .by_label
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|label| {
            let prefix = if label.key == "com.docker.compose.project" {
                "compose:"
            } else if label.key == "com.docker.compose.service" {
                "service:"
            } else {
                ""
            };
            format!("{prefix}{}×{}", escape(&label.value), value(label.count))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        text,
        "Outside Bosn: {} running containers ({labels})",
        value(o.external.containers_running_outside_bosn)
    );
}

fn escape(value: &str) -> String {
    value.chars().flat_map(char::escape_default).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stalled_host_probe_cannot_hold_the_cli_past_its_deadline() {
        let (release, wait) = std::sync::mpsc::channel();
        let started = Instant::now();
        let report = host_within(Duration::from_millis(20), false, move || {
            let _ = wait.recv();
            Ok(Observation::default())
        })
        .unwrap();
        release.send(()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            report
                .unavailable
                .iter()
                .any(|u| u.field == "host" && u.reason == Reason::Timeout)
        );
    }
}
