//! Per-daemon CI admission: one FIFO queue, one live concurrency limit, and
//! coalescing of identical submissions onto the run already queued/running.
//! Pure state; each CI actor owns its instance and performs effects. Separate
//! state roots do not share this limit or establish an aggregate CPU budget.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[cfg(any(test, target_os = "linux"))]
use std::path::Path;

/// Everything that makes two submissions the same execution.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RunKey {
    pub sha: String,
    pub dirty: Option<String>,
    pub workflow: String,
    pub job: Option<String>,
    pub trigger: String,
    pub mode: String,
    pub provider: String,
    pub engine: String,
    /// Branch, PR number and repository all shape the event payload.
    pub payload_sha256: String,
    pub timeout_secs: u64,
    pub secrets: Vec<String>,
    /// Inputs, matrix filter and env (#430): a different run when they differ.
    pub params: super::params::RunParams,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission {
    /// A new run was queued under the caller's ID.
    Queued(String),
    /// An identical run is already queued or running; share its ID.
    Coalesced(String),
}

#[derive(Debug)]
pub struct Scheduler {
    limit: usize,
    drained: bool,
    queue: VecDeque<String>,
    running: BTreeSet<String>,
    live: BTreeMap<RunKey, String>,
    keys: BTreeMap<String, RunKey>,
}

/// Default limit: physical cores / 4, at least one.
pub fn default_limit(cores: usize) -> usize {
    (cores / 4).max(1)
}

/// Count unique package/core pairs in the process's allowed CPU set. A CPU
/// quota smaller than that set does not identify which cores are available,
/// so use half the quota instead of counting the whole host.
#[cfg(any(test, target_os = "linux"))]
fn physical_cores_linux(root: &Path, logical: usize, allowed: &str) -> usize {
    let fallback = (logical / 2).max(1);
    let Some(ids) = allowed_cpu_ids(allowed) else {
        return fallback;
    };
    if ids.len() > logical {
        return fallback;
    }
    let mut cores = BTreeSet::new();
    for index in ids {
        let topology = root.join(format!("cpu{index}/topology"));
        let pair = ["physical_package_id", "core_id"].map(|file| {
            std::fs::read_to_string(topology.join(file))
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
        });
        let [Some(package), Some(core)] = pair else {
            return fallback;
        };
        cores.insert((package, core));
    }
    cores.len().max(1)
}

/// Linux's `Cpus_allowed_list` grammar: comma-separated IDs or inclusive
/// ranges. Refuse malformed or implausibly large intervals before allocating.
#[cfg(any(test, target_os = "linux"))]
fn allowed_cpu_ids(list: &str) -> Option<BTreeSet<usize>> {
    let mut ids = BTreeSet::new();
    for part in list.trim().split(',') {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let first: usize = first.parse().ok()?;
        let last: usize = last.parse().ok()?;
        if last < first || last - first > 4096 {
            return None;
        }
        ids.extend(first..=last);
    }
    (!ids.is_empty()).then_some(ids)
}

/// Probe host topology off the async daemon lane. The scheduler's CPU quota
/// view caps the result when affinity or a cgroup exposes fewer logical CPUs.
pub async fn detected_physical_cores() -> usize {
    let logical = std::thread::available_parallelism().map_or(1, usize::from);
    kernal_api::async_engine::launch_blocking(move || {
        #[cfg(target_os = "linux")]
        {
            let allowed = std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|status| {
                    status.lines().find_map(|line| {
                        line.strip_prefix("Cpus_allowed_list:")
                            .map(str::trim)
                            .map(str::to_owned)
                    })
                })
                .unwrap_or_default();
            physical_cores_linux(Path::new("/sys/devices/system/cpu"), logical, &allowed)
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("sysctl")
                .args(["-n", "hw.physicalcpu"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .and_then(|text| text.trim().parse::<usize>().ok())
                .filter(|count| *count > 0)
                .map_or((logical / 2).max(1), |count| count.min(logical))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            (logical / 2).max(1)
        }
    })
    .await
    .unwrap_or((logical / 2).max(1))
}

impl Scheduler {
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            drained: false,
            queue: VecDeque::new(),
            running: BTreeSet::new(),
            live: BTreeMap::new(),
            keys: BTreeMap::new(),
        }
    }

    pub fn submit(&mut self, key: RunKey, id: String) -> Admission {
        if let Some(existing) = self.live.get(&key) {
            return Admission::Coalesced(existing.clone());
        }
        self.live.insert(key.clone(), id.clone());
        self.keys.insert(id.clone(), key);
        self.queue.push_back(id.clone());
        Admission::Queued(id)
    }

    /// Runs to start now, in FIFO order. Callers must start every returned run.
    pub fn admit(&mut self) -> Vec<String> {
        let mut started = Vec::new();
        while !self.drained && self.running.len() < self.limit {
            let Some(id) = self.queue.pop_front() else {
                break;
            };
            self.running.insert(id.clone());
            started.push(id);
        }
        started
    }

    /// A run reached a terminal state: free its slot and its coalescing key.
    pub fn finish(&mut self, id: &str) {
        self.running.remove(id);
        self.queue.retain(|q| q != id);
        if let Some(key) = self.keys.remove(id) {
            self.live.remove(&key);
        }
    }

    /// Remove a queued run. Returns false when it is running or unknown.
    pub fn cancel_queued(&mut self, id: &str) -> bool {
        if !self.queue.iter().any(|q| q == id) {
            return false;
        }
        self.finish(id);
        true
    }

    /// Lowering the limit never stops a running job; raising it lets
    /// waiting runs start on the next [`Self::admit`].
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
    }

    pub fn set_drained(&mut self, drained: bool) {
        self.drained = drained;
    }

    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn drained(&self) -> bool {
        self.drained
    }
    pub fn running(&self) -> usize {
        self.running.len()
    }
    pub fn queued(&self) -> usize {
        self.queue.len()
    }
    pub fn is_running(&self, id: &str) -> bool {
        self.running.contains(id)
    }
    pub fn queue_position(&self, id: &str) -> Option<usize> {
        self.queue.iter().position(|q| q == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: usize) -> RunKey {
        RunKey {
            sha: format!("{n:040x}"),
            dirty: None,
            workflow: ".github/workflows/ci.yml".into(),
            job: None,
            trigger: "push".into(),
            mode: "minimal".into(),
            provider: "github".into(),
            engine: "act".into(),
            payload_sha256: "p".into(),
            timeout_secs: 60,
            secrets: Vec::new(),
            params: Default::default(),
        }
    }

    #[test]
    fn default_admission_uses_physical_cores_on_a_hyperthreaded_host() {
        let root = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
        for logical in 0..16 {
            let topology = root.path().join(format!("cpu{logical}/topology"));
            std::fs::create_dir_all(&topology).unwrap();
            std::fs::write(topology.join("physical_package_id"), "0\n").unwrap();
            std::fs::write(topology.join("core_id"), format!("{}\n", logical / 2)).unwrap();
        }
        let physical = physical_cores_linux(root.path(), 16, "0-15");
        assert_eq!(physical, 8);
        assert_eq!(default_limit(physical), 2);
        assert_eq!(
            physical_cores_linux(root.path(), 8, "0,2,4,6,8,10,12,14"),
            8
        );
        assert_eq!(physical_cores_linux(root.path(), 8, "0-15"), 4);
    }

    #[test]
    fn fifty_submissions_with_ten_keys_make_ten_executions() {
        let mut s = Scheduler::new(3);
        let mut ids = BTreeMap::new();
        for i in 0..50 {
            let k = key(i % 10);
            let id = match s.submit(k.clone(), format!("run-{i}")) {
                Admission::Queued(id) => id,
                Admission::Coalesced(id) => id,
            };
            // Every submitter of one key gets that key's single run ID.
            assert_eq!(ids.entry(i % 10).or_insert(id.clone()), &id);
        }
        assert_eq!(ids.len(), 10);
        let mut executed = BTreeSet::new();
        loop {
            let started = s.admit();
            assert!(s.running() <= 3, "limit respected");
            if started.is_empty() && s.running() == 0 {
                break;
            }
            for id in started {
                executed.insert(id.clone());
                s.finish(&id);
            }
        }
        assert_eq!(executed.len(), 10);
        assert_eq!(executed, ids.values().cloned().collect());
    }

    #[test]
    fn limit_changes_apply_live_and_never_stop_running_work() {
        let mut s = Scheduler::new(1);
        for i in 0..4 {
            s.submit(key(i), format!("r{i}"));
        }
        assert_eq!(s.admit(), ["r0"]);
        s.set_limit(3);
        assert_eq!(s.admit(), ["r1", "r2"], "raising starts waiting runs");
        s.set_limit(1);
        assert!(s.admit().is_empty());
        assert_eq!(s.running(), 3, "lowering kills nothing");
        s.finish("r0");
        s.finish("r1");
        assert!(s.admit().is_empty(), "still above the lowered limit");
        s.finish("r2");
        assert_eq!(s.admit(), ["r3"]);
    }

    #[test]
    fn drain_holds_the_queue_and_finished_keys_can_run_again() {
        let mut s = Scheduler::new(2);
        s.set_drained(true);
        s.submit(key(1), "a".into());
        assert!(s.admit().is_empty());
        s.set_drained(false);
        assert_eq!(s.admit(), ["a"]);
        assert_eq!(
            s.submit(key(1), "b".into()),
            Admission::Coalesced("a".into())
        );
        s.finish("a");
        assert_eq!(s.submit(key(1), "b".into()), Admission::Queued("b".into()));
        assert!(s.cancel_queued("b"));
        assert!(!s.cancel_queued("b"));
        assert_eq!(default_limit(16), 4);
        assert_eq!(default_limit(2), 1);
    }
}
