//! Host metrics for the `/system` page: a background sampler keeps one hour of
//! history in memory, so a page load never touches the host. Linux reads
//! `/proc`; a metric a platform cannot report is `null`, never a guessed zero.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bosn_core::LABEL_STACK;
use bosn_engine::{DockerEngine, RunOptions};
use kernal_api::async_engine;
use serde::{Deserialize, Serialize};

const INTERVAL: Duration = Duration::from_secs(10);
/// One hour at [`INTERVAL`].
const HISTORY: usize = 360;
const DOCKER_DEADLINE: Duration = Duration::from_secs(5);
const DOCKER_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

/// The facts that describe the host, refreshed with every sample.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HostInfo {
    pub hostname: Option<String>,
    pub kernel: Option<String>,
    pub cpu_model: Option<String>,
    pub logical_cpus: usize,
    pub uptime_secs: Option<u64>,
    pub load: Option<[f64; 3]>,
    pub mem_total_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub disk_path: PathBuf,
    pub disk_total_bytes: Option<u64>,
    /// Bosn-labelled containers; `None` when Docker could not be read.
    pub containers: Option<Vec<ContainerInfo>>,
}

/// One Bosn-labelled container, as `docker ps` reports it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    /// The stack label (`act`, `ci-source-context`, ...), or `setup`. No
    /// workspace here: a CI container's label names the state directory or
    /// `machine`, not the repository it serves.
    pub kind: String,
    #[serde(skip_serializing)]
    labels: String,
}

/// One point on the charts.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Sample {
    pub at_ms: u64,
    pub cpu_percent: Option<f64>,
    pub io_delay_percent: Option<f64>,
    pub load1: Option<f64>,
    pub mem_used_bytes: Option<u64>,
    pub swap_used_bytes: Option<u64>,
    pub disk_used_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemReply {
    pub interval_secs: u64,
    pub host: HostInfo,
    pub samples: VecDeque<Sample>,
}

/// Shared with the listener's handlers; dropping the server stops the sampler.
#[derive(Clone)]
pub struct SystemMonitor {
    state: Arc<Mutex<SystemReply>>,
}

impl SystemMonitor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SystemReply {
                interval_secs: INTERVAL.as_secs(),
                host: HostInfo::default(),
                samples: VecDeque::with_capacity(HISTORY),
            })),
        }
    }

    #[must_use]
    pub fn reply(&self) -> SystemReply {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn record(&self, host: HostInfo, sample: Sample) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.host = host;
        if state.samples.len() == HISTORY {
            state.samples.pop_front();
        }
        state.samples.push_back(sample);
    }

    /// Sample until the returned task is dropped. Reads run off the async lane.
    pub fn spawn_sampler(&self, disk_path: PathBuf) -> async_engine::Task<()> {
        let monitor = self.clone();
        async_engine::launch(async move {
            let mut previous = None;
            loop {
                let path = disk_path.clone();
                let observed = async_engine::launch_blocking(move || {
                    let ticks = read_proc("stat").and_then(|s| parse_cpu_ticks(&s));
                    let (host, sample) = observe(&path, previous, ticks);
                    (host, sample, ticks)
                })
                .await;
                // A lost worker leaves a gap in the history, never an invented point.
                if let Ok((host, sample, ticks)) = observed {
                    monitor.record(host, sample);
                    previous = ticks;
                }
                async_engine::sleep(INTERVAL).await;
            }
        })
    }
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

fn read_proc(name: &str) -> Option<String> {
    std::fs::read_to_string(Path::new("/proc").join(name)).ok()
}

fn observe(
    disk_path: &Path,
    previous: Option<CpuTicks>,
    ticks: Option<CpuTicks>,
) -> (HostInfo, Sample) {
    let memory = read_proc("meminfo").map(|m| parse_meminfo(&m));
    let load = read_proc("loadavg").and_then(|l| parse_loadavg(&l));
    let disk_total = kernal_api::resources_total_space(disk_path).ok();
    let disk_free = kernal_api::resources_available_space(disk_path).ok();
    let usage = previous.zip(ticks).and_then(|(a, b)| a.usage_until(b));
    let trimmed = |name| read_proc(name).map(|s: String| s.trim().to_owned());
    let host = HostInfo {
        hostname: trimmed("sys/kernel/hostname"),
        kernel: trimmed("sys/kernel/osrelease"),
        cpu_model: read_proc("cpuinfo").and_then(|c| parse_cpu_model(&c)),
        logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
        uptime_secs: read_proc("uptime").and_then(|u| parse_uptime(&u)),
        load,
        mem_total_bytes: memory.and_then(|m| m.mem_total),
        swap_total_bytes: memory.and_then(|m| m.swap_total),
        disk_path: disk_path.to_owned(),
        disk_total_bytes: disk_total,
        containers: read_containers(),
    };
    let sample = Sample {
        at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        cpu_percent: usage.map(|u| u.0),
        io_delay_percent: usage.map(|u| u.1),
        load1: load.map(|l| l[0]),
        mem_used_bytes: memory.and_then(Memory::used),
        swap_used_bytes: memory.and_then(Memory::swap_used),
        disk_used_bytes: disk_total
            .zip(disk_free)
            .map(|(total, free)| total.saturating_sub(free)),
    };
    (host, sample)
}

fn read_containers() -> Option<Vec<ContainerInfo>> {
    let format = format!(
        concat!(
            r#"{{"id":{{{{json .ID}}}},"name":{{{{json .Names}}}},"image":{{{{json .Image}}}},"#,
            r#""state":{{{{json .State}}}},"status":{{{{json .Status}}}},"labels":{{{{json .Labels}}}},"#,
            r#""kind":{{{{json (.Label "{stack}")}}}}}}"#,
        ),
        stack = LABEL_STACK,
    );
    let result = DockerEngine::docker()
        .with_args(["ps", "-a", "--format", &format])
        .capture(RunOptions::bounded(DOCKER_DEADLINE, DOCKER_OUTPUT_LIMIT))
        .ok()?;
    result
        .ok()
        .then(|| parse_containers(&String::from_utf8_lossy(&result.stdout)))
        .flatten()
}

/// One JSON document per line; a malformed line refuses the whole listing.
/// Only containers carrying a Bosn label are kept: setup containers have their
/// own label family and no registry label, so no single `--filter` covers them.
fn parse_containers(text: &str) -> Option<Vec<ContainerInfo>> {
    let containers: Vec<ContainerInfo> = text
        .lines()
        .map(|line| serde_json::from_str(line).ok())
        .collect::<Option<_>>()?;
    Some(
        containers
            .into_iter()
            .filter(|c| c.labels.contains("com.zackees.bosn."))
            .map(|mut c| {
                if c.kind.is_empty() && c.labels.contains("com.zackees.bosn.setup-managed=") {
                    c.kind = "setup".into();
                }
                c
            })
            .collect(),
    )
}

/// Cumulative jiffies from the aggregate `cpu` line of `/proc/stat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CpuTicks {
    total: u64,
    idle: u64,
    iowait: u64,
}

impl CpuTicks {
    /// (busy %, IO-delay %) between two readings; `None` if no time passed.
    /// Busy excludes IO wait, which is reported as its own series.
    fn usage_until(self, later: Self) -> Option<(f64, f64)> {
        let total = later.total.checked_sub(self.total).filter(|t| *t > 0)?;
        let idle = later.idle.saturating_sub(self.idle);
        let iowait = later.iowait.saturating_sub(self.iowait);
        let percent = |part: u64| part as f64 * 100.0 / total as f64;
        Some((
            percent(total.saturating_sub(idle + iowait)),
            percent(iowait),
        ))
    }
}

fn parse_cpu_ticks(stat: &str) -> Option<CpuTicks> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    // user nice system idle iowait irq softirq steal; guest time is inside user.
    let fields = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse)
        .collect::<Result<Vec<u64>, _>>()
        .ok()?;
    Some(CpuTicks {
        total: fields.iter().sum(),
        idle: *fields.get(3)?,
        iowait: *fields.get(4)?,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Memory {
    mem_total: Option<u64>,
    mem_available: Option<u64>,
    swap_total: Option<u64>,
    swap_free: Option<u64>,
}

impl Memory {
    fn used(self) -> Option<u64> {
        Some(self.mem_total?.saturating_sub(self.mem_available?))
    }

    fn swap_used(self) -> Option<u64> {
        Some(self.swap_total?.saturating_sub(self.swap_free?))
    }
}

fn parse_meminfo(text: &str) -> Memory {
    let mut memory = Memory::default();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let bytes = rest
            .split_whitespace()
            .next()
            .and_then(|kib| kib.parse::<u64>().ok())
            .map(|kib| kib.saturating_mul(1024));
        match name {
            "MemTotal" => memory.mem_total = bytes,
            "MemAvailable" => memory.mem_available = bytes,
            "SwapTotal" => memory.swap_total = bytes,
            "SwapFree" => memory.swap_free = bytes,
            _ => {}
        }
    }
    memory
}

fn parse_loadavg(text: &str) -> Option<[f64; 3]> {
    let mut fields = text.split_whitespace().map(str::parse::<f64>);
    Some([
        fields.next()?.ok()?,
        fields.next()?.ok()?,
        fields.next()?.ok()?,
    ])
}

fn parse_uptime(text: &str) -> Option<u64> {
    let seconds = text.split_whitespace().next()?.parse::<f64>().ok()?;
    Some(seconds as u64)
}

fn parse_cpu_model(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, model)| model.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_usage_separates_busy_from_io_delay() {
        let a = parse_cpu_ticks("cpu  100 0 100 800 0 0 0 0 5 0\ncpu0 1 1 1 1 1 1 1 1\n").unwrap();
        let b = parse_cpu_ticks("cpu  130 0 120 830 20 0 0 0 9 0\n").unwrap();
        assert_eq!(a.usage_until(b), Some((50.0, 20.0)));
        assert_eq!(a.usage_until(a), None, "no time passed is not 0% busy");
        assert_eq!(parse_cpu_ticks("cpu  x y\n"), None);
    }

    #[test]
    fn memory_uses_available_and_a_missing_line_stays_unknown() {
        let memory = parse_meminfo(
            "MemTotal: 1000 kB\nMemFree: 1 kB\nMemAvailable: 400 kB\nSwapTotal: 10 kB\n",
        );
        assert_eq!(memory.used(), Some(600 * 1024));
        assert_eq!(memory.swap_used(), None);
    }

    #[test]
    fn load_uptime_and_model_parse_or_stay_unknown() {
        assert_eq!(
            parse_loadavg("0.50 1.25 2.00 1/2 3\n"),
            Some([0.5, 1.25, 2.0])
        );
        assert_eq!(parse_loadavg("0.5"), None);
        assert_eq!(parse_uptime("123.9 4.0\n"), Some(123));
        assert_eq!(
            parse_cpu_model("vendor: x\nmodel name\t: Xeon\n").as_deref(),
            Some("Xeon")
        );
    }

    #[test]
    fn container_lines_parse_and_a_malformed_line_refuses_the_listing() {
        let line = r#"{"id":"a","name":"n","image":"i","state":"running","status":"Up","kind":"","labels":"com.zackees.bosn.setup-managed=v1"}"#;
        let foreign = r#"{"id":"b","name":"x","image":"i","state":"running","status":"Up","kind":"","labels":"other=1"}"#;
        let parsed = parse_containers(&format!("{line}\n{foreign}\n")).unwrap();
        assert_eq!(
            parsed.len(),
            1,
            "a container without a Bosn label is not listed"
        );
        assert_eq!(parsed[0].kind, "setup");
        assert_eq!(parse_containers(""), Some(Vec::new()));
        assert_eq!(parse_containers(&format!("{line}\nnot json\n")), None);
    }

    #[test]
    fn history_is_bounded_to_the_newest_hour() {
        let monitor = SystemMonitor::new();
        for at_ms in 0..=HISTORY as u64 {
            monitor.record(
                HostInfo::default(),
                Sample {
                    at_ms,
                    ..Sample::default()
                },
            );
        }
        let reply = monitor.reply();
        assert_eq!(reply.samples.len(), HISTORY);
        assert_eq!(reply.samples.front().map(|s| s.at_ms), Some(1));
    }
}
