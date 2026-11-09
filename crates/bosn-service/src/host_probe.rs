//! Shared read-only host probes. Missing observations remain unknown.

use bosn_core::LABEL_STACK;
use bosn_engine::{DockerEngine, RunOptions};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

pub(crate) const DOCKER_DEADLINE: Duration = Duration::from_secs(5);
pub(crate) const DOCKER_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

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
    pub(crate) labels: String,
}

pub(crate) fn read_proc(name: &str) -> Option<String> {
    std::fs::read_to_string(Path::new("/proc").join(name)).ok()
}

pub(crate) fn read_containers() -> Option<(Vec<ContainerInfo>, usize)> {
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
pub(crate) fn other_running(text: &str) -> Option<usize> {
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

/// Only Bosn's own labels leave the host's Docker: an image's labels are not
/// ours to publish. Docker joins them with commas.
pub(crate) fn bosn_labels(labels: &str) -> Vec<String> {
    let mut kept: Vec<String> = bosn_label_entries(labels).map(str::to_owned).collect();
    kept.sort();
    kept
}

/// The entries of Docker's joined label text whose key is Bosn's. Another
/// container's label value may mention our prefix; that does not make it ours.
pub(crate) fn bosn_label_entries(labels: &str) -> impl Iterator<Item = &str> {
    labels
        .split(',')
        .filter(|label| label.starts_with("com.zackees.bosn."))
}

/// One JSON document per line; a malformed line refuses the whole listing.
/// Only containers carrying a Bosn label are kept: setup containers have their
/// own label family and no registry label, so no single `--filter` covers them.
pub(crate) fn parse_containers(text: &str) -> Option<Vec<ContainerInfo>> {
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
pub(crate) struct CpuTicks {
    total: u64,
    idle: u64,
    iowait: u64,
}

impl CpuTicks {
    /// (busy %, IO-delay %) between two readings; `None` if no time passed.
    /// Busy excludes IO wait, which is reported as its own series.
    pub(crate) fn usage_until(self, later: Self) -> Option<(f64, f64)> {
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

pub(crate) fn parse_cpu_ticks(stat: &str) -> Option<CpuTicks> {
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
pub(crate) struct Memory {
    pub(crate) mem_total: Option<u64>,
    pub(crate) mem_available: Option<u64>,
    pub(crate) swap_total: Option<u64>,
    pub(crate) swap_free: Option<u64>,
}

impl Memory {
    pub(crate) fn used(self) -> Option<u64> {
        Some(self.mem_total?.saturating_sub(self.mem_available?))
    }

    pub(crate) fn swap_used(self) -> Option<u64> {
        Some(self.swap_total?.saturating_sub(self.swap_free?))
    }
}

pub(crate) fn parse_meminfo(text: &str) -> Memory {
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

pub(crate) fn parse_loadavg(text: &str) -> Option<[f64; 3]> {
    let mut fields = text.split_whitespace().map(str::parse::<f64>);
    Some([
        fields.next()?.ok()?,
        fields.next()?.ok()?,
        fields.next()?.ok()?,
    ])
}

pub(crate) fn parse_uptime(text: &str) -> Option<u64> {
    let seconds = text.split_whitespace().next()?.parse::<f64>().ok()?;
    Some(seconds as u64)
}

pub(crate) fn parse_cpu_model(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, model)| model.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_invalid_probes_remain_unknown() {
        assert_eq!(parse_cpu_ticks(""), None);
        assert_eq!(parse_loadavg("invalid 0 0"), None);
        assert_eq!(parse_meminfo("").used(), None);
        assert_eq!(other_running(""), Some(0));
        assert_eq!(other_running("not json"), None);
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
}
