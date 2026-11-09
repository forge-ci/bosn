//! Host metrics for the `/system` page: a background sampler keeps one hour of
//! bounded persistent history, so a page load never touches the host. Linux reads
//! `/proc`; a metric a platform cannot report is `null`, never a guessed zero.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bosn_core::LABEL_STACK;
use bosn_engine::{DockerEngine, RunOptions};
use kernal_api::async_engine;
use serde::{Deserialize, Serialize};

pub mod history;
mod store;

use history::{History, Point, Range};

const INTERVAL: Duration = Duration::from_secs(10);
/// One hour at [`INTERVAL`].
#[cfg(test)]
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
    /// Running containers without a Bosn label, from the same read: an idle Bosn
    /// is not an idle host (#797). A count only; their names are not ours to publish.
    pub other_running_containers: Option<usize>,
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

/// What one selected container reports. Stats are `None` unless it is running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ContainerDetail {
    pub labels: Vec<String>,
    pub stats: Option<ContainerStats>,
}

/// Docker's own formatted figures for one container.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStats {
    pub cpu: String,
    pub memory: String,
    pub memory_percent: String,
    pub pids: String,
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
    pub range: Range,
    pub samples: Vec<Point>,
    pub latest: Option<Sample>,
}

struct MonitorState {
    host: HostInfo,
    latest: Option<Sample>,
    history: History,
}
struct Inner {
    state: Mutex<MonitorState>,
    store: Mutex<Option<store::Store>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let store = self
            .store
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let mut history = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .history
            .clone();
        if let Some(mut store) = store {
            async_engine::launch_blocking(move || {
                if let Err(error) = store.flush(&mut history, now_ms()) {
                    crate::daemon_log!("bosn system history: final flush failed: {error}");
                }
            })
            .detach();
        }
    }
}

/// Shared with the listener's handlers; dropping the server stops the sampler.
#[derive(Clone)]
pub struct SystemMonitor {
    inner: Arc<Inner>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl SystemMonitor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(MonitorState {
                    host: HostInfo::default(),
                    latest: None,
                    history: History::default(),
                }),
                store: Mutex::new(None),
            }),
        }
    }

    pub async fn open(state_dir: PathBuf) -> Self {
        let monitor = Self::new();
        let worker = monitor.clone();
        if let Err(error) = async_engine::launch_blocking(move || {
            *worker
                .inner
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(store::Store::new(&state_dir));
            worker.flush_blocking();
        })
        .await
        {
            crate::daemon_log!("bosn system history: startup worker failed: {error}");
        }
        monitor
    }

    #[must_use]
    pub fn reply(&self) -> SystemReply {
        self.reply_range(Range::Hour)
    }

    #[must_use]
    pub fn reply_range(&self, range: Range) -> SystemReply {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        SystemReply {
            interval_secs: history::TIERS[range.tier()].0 / 1000,
            range,
            host: state.host.clone(),
            latest: state.latest.clone(),
            samples: state.history.points(range, now_ms()),
        }
    }

    pub async fn flush(&self) {
        let worker = self.clone();
        if let Err(error) = async_engine::launch_blocking(move || worker.flush_blocking()).await {
            crate::daemon_log!("bosn system history: flush worker failed: {error}");
        }
    }

    fn flush_blocking(&self) {
        let mut store = self
            .inner
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(store) = store.as_mut() else { return };
        let mut history = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .history
            .clone();
        if let Err(error) = store.flush(&mut history, now_ms()) {
            crate::daemon_log!("bosn system history: flush failed: {error}");
        }
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .history = history;
    }

    /// One listed container's details, or `None` for anything else: this is
    /// not a way to read arbitrary containers. The sampler pays nothing for
    /// this; the page asks only while a container is selected, and the Docker
    /// call is bounded by [`DOCKER_DEADLINE`].
    pub async fn container_detail(&self, id: &str) -> Option<ContainerDetail> {
        let container = self
            .reply()
            .host
            .containers?
            .into_iter()
            .find(|c| c.id == id)?;
        let stats = if container.state == "running" {
            let id = container.id.clone();
            async_engine::launch_blocking(move || read_container_stats(&id))
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        Some(ContainerDetail {
            labels: bosn_labels(&container.labels),
            stats,
        })
    }

    fn record(&self, host: HostInfo, sample: Sample) {
        let _writer = self
            .inner
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.host = host;
        state.history.record(&sample);
        state.latest = Some(sample);
    }

    /// Sample until the returned task is dropped. Reads run off the async lane.
    pub fn spawn_sampler(
        &self,
        disk_path: PathBuf,
        stop: async_engine::CancellationToken,
    ) -> async_engine::Task<()> {
        let monitor = self.clone();
        async_engine::launch(async move {
            let mut previous = None;
            let mut last_flush = std::time::Instant::now();
            // The disk retention watches; the state directory until Docker answers.
            let mut docker_root = None;
            while !stop.is_cancelled() {
                let started = std::time::Instant::now();
                let path = disk_path.clone();
                let known_root = docker_root.clone();
                let worker = monitor.clone();
                let observed = async_engine::launch_blocking(move || {
                    let ticks = read_proc("stat").and_then(|s| parse_cpu_ticks(&s));
                    let root = known_root.or_else(|| {
                        crate::unmanaged::docker_root_dir(&DockerEngine::docker())
                            .filter(|root| kernal_api::resources_total_space(root).is_ok())
                    });
                    let (host, sample) = observe(root.as_deref().unwrap_or(&path), previous, ticks);
                    worker.record(host, sample);
                    (ticks, root)
                })
                .await;
                // A lost worker leaves a gap in the history, never an invented point.
                if let Ok((ticks, root)) = observed {
                    docker_root = root;
                    previous = ticks;
                }
                if last_flush.elapsed() >= Duration::from_secs(60) {
                    monitor.flush().await;
                    last_flush = std::time::Instant::now();
                }
                if async_engine::cancellable(
                    &stop,
                    async_engine::sleep(INTERVAL.saturating_sub(started.elapsed())),
                )
                .await
                .is_err()
                {
                    break;
                }
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
    let (containers, other_running_containers) = read_containers().unzip();
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
        containers,
        other_running_containers,
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

fn read_containers() -> Option<(Vec<ContainerInfo>, usize)> {
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
    if !result.ok() {
        return None;
    }
    let text = String::from_utf8_lossy(&result.stdout);
    Some((parse_containers(&text)?, other_running(&text)?))
}

/// How many listed containers are running without a Bosn label.
fn other_running(text: &str) -> Option<usize> {
    let containers: Vec<ContainerInfo> = text
        .lines()
        .map(|line| serde_json::from_str(line).ok())
        .collect::<Option<_>>()?;
    Some(
        containers
            .iter()
            .filter(|c| c.state == "running" && bosn_label_entries(&c.labels).next().is_none())
            .count(),
    )
}

fn read_container_stats(id: &str) -> Option<ContainerStats> {
    let format = concat!(
        r#"{"cpu":{{json .CPUPerc}},"memory":{{json .MemUsage}},"#,
        r#""memory_percent":{{json .MemPerc}},"pids":{{json .PIDs}}}"#,
    );
    let result = DockerEngine::docker()
        .with_args(["stats", "--no-stream", "--format", format, id])
        .capture(RunOptions::bounded(DOCKER_DEADLINE, DOCKER_OUTPUT_LIMIT))
        .ok()?;
    result
        .ok()
        .then(|| serde_json::from_slice(&result.stdout).ok())
        .flatten()
}

/// Only Bosn's own labels leave the host's Docker: an image's labels are not
/// ours to publish. Docker joins them with commas.
fn bosn_labels(labels: &str) -> Vec<String> {
    let mut kept: Vec<String> = bosn_label_entries(labels).map(str::to_owned).collect();
    kept.sort();
    kept
}

/// The entries of Docker's joined label text whose key is Bosn's. Another
/// container's label value may mention our prefix; that does not make it ours.
fn bosn_label_entries(labels: &str) -> impl Iterator<Item = &str> {
    labels
        .split(',')
        .filter(|label| label.starts_with("com.zackees.bosn."))
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
            .filter(|c| bosn_label_entries(&c.labels).next().is_some())
            .map(|mut c| {
                if c.kind.is_empty()
                    && bosn_label_entries(&c.labels)
                        .any(|label| label.starts_with("com.zackees.bosn.setup-managed="))
                {
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
    fn details_publish_only_bosn_labels_and_parse_dockers_figures() {
        assert_eq!(
            bosn_labels("maintainer=me,com.zackees.bosn.stack=act,com.zackees.bosn.a=b"),
            ["com.zackees.bosn.a=b", "com.zackees.bosn.stack=act"]
        );
        let stats: ContainerStats = serde_json::from_str(
            r#"{"cpu":"1.5%","memory":"1MiB / 2GiB","memory_percent":"0.05%","pids":"3"}"#,
        )
        .unwrap();
        assert_eq!(stats.pids, "3");
    }

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
        let mentions = r#"{"id":"c","name":"x","image":"i","state":"running","status":"Up","kind":"","labels":"note=com.zackees.bosn.setup-managed=v1"}"#;
        assert_eq!(parse_containers(mentions), Some(Vec::new()));
        let parsed = parse_containers(&format!("{line}\n{foreign}\n")).unwrap();
        assert_eq!(
            parsed.len(),
            1,
            "a container without a Bosn label is not listed"
        );
        assert_eq!(parsed[0].kind, "setup");
        assert_eq!(parse_containers(""), Some(Vec::new()));
        assert_eq!(other_running(""), Some(0));
        assert_eq!(other_running("not json"), None);
        let running = |labels: &str, state: &str| {
            format!(
                r#"{{"id":"a","name":"n","image":"i","state":"{state}","status":"s","labels":"{labels}","kind":""}}"#
            )
        };
        let listing = [
            running("x=y", "running"),
            running("x=y", "exited"),
            running("com.zackees.bosn.stack=act", "running"),
            running("note=com.zackees.bosn.stack", "running"),
        ]
        .join("\n");
        assert_eq!(other_running(&listing), Some(2));
        assert_eq!(parse_containers(&format!("{line}\nnot json\n")), None);
    }

    #[test]
    fn history_is_bounded_to_the_newest_hour() {
        let monitor = SystemMonitor::new();
        for i in 0..HISTORY * 2 {
            monitor.record(
                HostInfo::default(),
                Sample {
                    at_ms: now_ms() + i as u64 * 10_000,
                    ..Sample::default()
                },
            );
        }
        assert_eq!(
            monitor.inner.state.lock().unwrap().history.tiers[0]
                .iter()
                .flatten()
                .count(),
            HISTORY
        );
    }
    #[test]
    fn monitor_reloads_history_and_flushes_on_drop() {
        crate::ci::lifecycle::tests::with_registry(|_registry, dir| async move {
            let monitor = SystemMonitor::open(dir.clone()).await;
            monitor.record(
                HostInfo::default(),
                Sample {
                    at_ms: now_ms(),
                    cpu_percent: Some(12.0),
                    ..Sample::default()
                },
            );
            monitor.flush().await;
            monitor.record(
                HostInfo::default(),
                Sample {
                    at_ms: now_ms(),
                    cpu_percent: Some(24.0),
                    ..Sample::default()
                },
            );
            drop(monitor);
            // The final flush runs on a blocking worker. Opening retries a held
            // lock on each flush, exactly as the handoff sampler does.
            let restored = SystemMonitor::open(dir).await;
            for _ in 0..100 {
                restored.flush().await;
                if restored
                    .reply()
                    .samples
                    .iter()
                    .any(|p| p.avg.cpu_percent == Some(24.0))
                {
                    break;
                }
                async_engine::sleep(Duration::from_millis(10)).await;
            }
            assert!(
                restored
                    .reply()
                    .samples
                    .iter()
                    .any(|p| p.avg.cpu_percent == Some(24.0))
            );
        });
    }
}
