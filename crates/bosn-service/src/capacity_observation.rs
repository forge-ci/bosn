//! Read-only, point-in-time host capacity. This is evidence, never admission.

use crate::host_probe::{self, CpuTicks, parse_cpu_ticks, parse_loadavg, parse_meminfo};
use kernal_api::async_engine;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const ADVISORY: &str = "Point-in-time; may be stale before work starts. An empty Bosn registry does not mean an idle host.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    PermissionDenied,
    NotFound,
    Timeout,
    Unsupported,
    ParseError,
}

impl Reason {
    pub(crate) fn from_io(error: &io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::TimedOut => Self::Timeout,
            _ => Self::ParseError,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unavailable {
    pub field: String,
    pub reason: Reason,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Full,
    Partial,
    #[default]
    None,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Coverages {
    pub bosn: Coverage,
    pub host: Coverage,
    pub docker: Coverage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Daemon {
    Running,
    #[default]
    Absent,
    Unreachable,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Counts {
    pub queued: Option<usize>,
    pub running: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Slots {
    pub task_used: Option<usize>,
    pub task_limit: Option<usize>,
    pub ci_used: Option<usize>,
    pub ci_admission_limit: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Engine {
    pub cpus: Option<u64>,
    pub cpu_source: Option<String>,
    pub memory_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Bosn {
    pub daemon: Daemon,
    pub jobs: Counts,
    pub ci_runs: Counts,
    pub slots: Slots,
    pub engine: Engine,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Psi {
    pub cpu_some_avg10: Option<f64>,
    pub memory_some_avg10: Option<f64>,
    pub memory_full_avg10: Option<f64>,
    pub io_full_avg10: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Memory {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Swap {
    pub total_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskRole {
    DockerRoot,
    StateDir,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Disk {
    pub role: DiskRole,
    pub path: PathBuf,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Host {
    pub logical_cpus: Option<usize>,
    pub cpu_quota: Option<f64>,
    pub cpu_percent: Option<f64>,
    pub io_delay_percent: Option<f64>,
    pub load: [Option<f64>; 3],
    pub psi: Psi,
    pub memory: Memory,
    pub swap: Swap,
    pub disk: Vec<Disk>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LabelCount {
    pub key: String,
    pub value: String,
    pub count: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct External {
    pub containers_running_outside_bosn: Option<usize>,
    pub by_label: Option<Vec<LabelCount>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub schema: u8,
    pub sampled_at_ms: Option<u64>,
    pub age_ms: Option<u64>,
    pub coverage: Coverages,
    pub bosn: Bosn,
    pub host: Host,
    pub external: External,
    pub unavailable: Vec<Unavailable>,
    pub advisory: String,
}

impl Default for Observation {
    fn default() -> Self {
        Self {
            schema: 1,
            sampled_at_ms: None,
            age_ms: None,
            coverage: Coverages::default(),
            bosn: Bosn::default(),
            host: Host::default(),
            external: External::default(),
            unavailable: Vec::new(),
            advisory: ADVISORY.into(),
        }
    }
}

/// File input stays injectable, including IO errors (not just missing text).
pub struct HostSample {
    pub observation: Observation,
    previous: Option<CpuTicks>,
    started: Instant,
}

impl HostSample {
    pub fn begin(state_dir: &Path) -> Self {
        Self::with_reader(state_dir, |name| {
            std::fs::read_to_string(Path::new("/proc").join(name))
        })
    }

    fn with_reader(state_dir: &Path, mut read: impl FnMut(&str) -> io::Result<String>) -> Self {
        let started = Instant::now();
        let mut observation = Observation {
            sampled_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| u64::try_from(d.as_millis()).ok()),
            ..Observation::default()
        };
        let unavailable = &mut observation.unavailable;
        let mut probe = |name: &str| {
            read(name)
                .map_err(|error| {
                    unavailable.push(Unavailable {
                        field: format!("host.{name}"),
                        reason: Reason::from_io(&error),
                    });
                })
                .ok()
        };
        let previous = probe("stat").and_then(|s| parse_cpu_ticks(&s));
        let memory = probe("meminfo").map(|s| parse_meminfo(&s));
        let load = probe("loadavg").and_then(|s| parse_loadavg(&s));
        let cpu = probe("pressure/cpu");
        let memory_psi = probe("pressure/memory");
        let io_psi = probe("pressure/io");
        observation.host.logical_cpus = crate::capacity::host_cpus();
        observation.host.load = load.map_or([None; 3], |load| load.map(Some));
        if let Some(memory) = memory {
            observation.host.memory = Memory {
                total_bytes: memory.mem_total,
                available_bytes: memory.mem_available,
            };
            observation.host.swap = Swap {
                total_bytes: memory.swap_total,
                used_bytes: memory.swap_used(),
            };
        }
        observation.host.psi = Psi {
            cpu_some_avg10: cpu
                .as_deref()
                .and_then(|s| host_probe::pressure::avg10(s, "some")),
            memory_some_avg10: memory_psi
                .as_deref()
                .and_then(|s| host_probe::pressure::avg10(s, "some")),
            memory_full_avg10: memory_psi
                .as_deref()
                .and_then(|s| host_probe::pressure::avg10(s, "full")),
            io_full_avg10: io_psi
                .as_deref()
                .and_then(|s| host_probe::pressure::avg10(s, "full")),
        };
        check_parsed(
            &mut observation,
            previous.is_some(),
            memory.is_some(),
            load.is_some(),
        );
        // No existing cgroup quota reader: do not invent a second CPU resolver.
        observation.unavailable.push(Unavailable {
            field: "host.cpu_quota".into(),
            reason: Reason::Unsupported,
        });
        observe_disk(&mut observation, state_dir);
        Self {
            observation,
            previous,
            started,
        }
    }

    /// The runtime owns the sampling wait and bounded Docker read.
    pub async fn finish(mut self, docker: bool, timeout: Duration) -> Observation {
        if timeout.saturating_sub(self.started.elapsed()) >= Duration::from_millis(250) {
            async_engine::sleep(Duration::from_millis(250)).await;
            let later = async_engine::launch_blocking(|| {
                host_probe::read_proc("stat").and_then(|s| parse_cpu_ticks(&s))
            })
            .await
            .ok()
            .flatten();
            let usage = self.previous.zip(later).and_then(|(a, b)| a.usage_until(b));
            self.observation.host.cpu_percent = usage.map(|u| u.0);
            self.observation.host.io_delay_percent = usage.map(|u| u.1);
            if usage.is_none() {
                self.missing("host.cpu_percent", Reason::ParseError);
            }
        } else {
            self.missing("host.cpu_percent", Reason::Timeout);
        }
        if docker {
            let remaining = timeout.saturating_sub(self.started.elapsed());
            let result = if remaining.is_zero() {
                Err(Reason::Timeout)
            } else {
                host_probe::external::read(remaining).await
            };
            match result {
                Ok(external) => {
                    self.observation.external = external;
                    self.observation.coverage.docker = Coverage::Full;
                }
                Err(reason) => self.missing("external", reason),
            }
        } else {
            self.missing("external", Reason::Unsupported);
        }
        self.observation.coverage.host = host_coverage(&self.observation);
        self.observation.age_ms = u64::try_from(self.started.elapsed().as_millis()).ok();
        self.observation
    }

    fn missing(&mut self, field: &str, reason: Reason) {
        self.observation.unavailable.push(Unavailable {
            field: field.into(),
            reason,
        });
    }
}

fn check_parsed(observation: &mut Observation, stat: bool, memory: bool, load: bool) {
    let psi = &observation.host.psi;
    let checks = [
        ("stat", stat),
        (
            "meminfo",
            memory
                && observation.host.memory.total_bytes.is_some()
                && observation.host.memory.available_bytes.is_some()
                && observation.host.swap.used_bytes.is_some(),
        ),
        ("loadavg", load),
        ("pressure/cpu", psi.cpu_some_avg10.is_some()),
        (
            "pressure/memory",
            psi.memory_some_avg10.is_some() && psi.memory_full_avg10.is_some(),
        ),
        ("pressure/io", psi.io_full_avg10.is_some()),
        ("logical_cpus", observation.host.logical_cpus.is_some()),
    ];
    for (field, valid) in checks {
        let field = format!("host.{field}");
        if !valid && !observation.unavailable.iter().any(|u| u.field == field) {
            observation.unavailable.push(Unavailable {
                field,
                reason: Reason::ParseError,
            });
        }
    }
}

fn observe_disk(observation: &mut Observation, path: &Path) {
    // Discovering a Docker data root needs `docker info`, which this command forbids.
    // The existing state-directory fallback is the only safe known path here.
    let mut disk = Disk {
        role: DiskRole::StateDir,
        path: path.to_owned(),
        total_bytes: None,
        free_bytes: None,
    };
    match kernal_api::resources_total_space(path)
        .and_then(|total| kernal_api::resources_available_space(path).map(|free| (total, free)))
    {
        Ok((total, free)) => {
            disk.total_bytes = Some(total);
            disk.free_bytes = Some(free);
        }
        Err(error) => observation.unavailable.push(Unavailable {
            field: "host.disk".into(),
            reason: Reason::from_io(&error),
        }),
    }
    observation.host.disk.push(disk);
}

fn host_coverage(observation: &Observation) -> Coverage {
    if !observation
        .unavailable
        .iter()
        .any(|u| u.field.starts_with("host."))
    {
        Coverage::Full
    } else if observation.host.logical_cpus.is_some()
        || observation.host.memory.total_bytes.is_some()
    {
        Coverage::Partial
    } else {
        Coverage::None
    }
}

#[cfg(test)]
#[path = "capacity_observation_tests.rs"]
mod tests;
