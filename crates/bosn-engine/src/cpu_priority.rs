//! Low-weight cgroup placement for every container Bosn creates (#645).
//!
//! `--cpu-shares` (#643) only ranks a container against its siblings, and Docker puts
//! every container under `system.slice`, which competes with the desktop's `user.slice`
//! at equal weight. A top-level `bosn.slice` with a low `CPUWeight` moves Bosn's whole
//! share below the desktop. Setting a slice's weight needs root, so Bosn only uses the
//! slice when an operator provisioned it (see `docs/cpu-priority.md`); otherwise the
//! per-container weight alone applies.
//!
//! Placement is added where the command runs, never where it is described: a setup
//! container's name is a digest of its create arguments, and provisioning the slice must
//! not rename every container.
use std::{
    ffi::OsString,
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};

/// The CPU weight every container Bosn creates runs at (#643).
///
/// A quarter of Docker's default 1024 (cgroup v2 `cpu.weight` about 10 instead of
/// 100): under contention Bosn's builds yield so the desktop stays responsive, and
/// on an idle machine they still use every CPU their cap allows.
pub const LOW_CPU_PRIORITY_ARG: &str = "--cpu-shares=256";

/// The slice Bosn's containers join when it is provisioned with a low weight.
pub const BOSN_SLICE: &str = "bosn.slice";

/// The `cgroup v2` default weight. A provisioned slice must sit below it.
const DEFAULT_CPU_WEIGHT: u64 = 100;

/// How long one probe command may run. A hung engine or systemd is `WeightOnly`.
const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// Local engine sockets. A remote or rootless engine never joins a host slice.
const LOCAL_HOSTS: [&str; 2] = ["unix:///var/run/docker.sock", "unix:///run/docker.sock"];

/// What the host probe found, decided once per process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Placement {
    /// `bosn.slice` is provisioned below the default weight: containers join it.
    Slice,
    /// No usable slice: the per-container `--cpu-shares` weight alone applies.
    WeightOnly,
}

impl Placement {
    /// Decide from the probe's observations. Pure, so every branch is testable.
    #[must_use]
    pub fn decide(docker_host: Option<&str>, cgroup_driver: &str, slice_weight: &str) -> Self {
        let local = docker_host.is_none_or(|host| LOCAL_HOSTS.contains(&host));
        let low = slice_weight
            .trim()
            .parse::<u64>()
            .is_ok_and(|weight| weight < DEFAULT_CPU_WEIGHT);
        if local && cgroup_driver.trim() == "systemd" && low {
            Self::Slice
        } else {
            Self::WeightOnly
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slice => "bosn.slice",
            Self::WeightOnly => "weight-only",
        }
    }
}

/// The host's placement, probed on first use. A probe that cannot run is `WeightOnly`.
#[must_use]
pub fn placement() -> Placement {
    static PLACEMENT: OnceLock<Placement> = OnceLock::new();
    *PLACEMENT.get_or_init(|| {
        let host = std::env::var("DOCKER_HOST").ok();
        let driver = probe("docker", &["info", "--format", "{{.CgroupDriver}}"]);
        let weight = probe(
            "systemctl",
            &["show", BOSN_SLICE, "--property=CPUWeight", "--value"],
        );
        Placement::decide(host.as_deref(), &driver, &weight)
    })
}

/// One probe command's stdout, or empty when it fails or outlives its deadline.
fn probe(program: &str, args: &[&str]) -> String {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if started.elapsed() < PROBE_DEADLINE => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return String::new();
            }
        }
    }
    // The probes print one short line, far below any pipe buffer, so nothing blocks.
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stdout);
    }
    stdout
}

/// Insert the slice placement right after the low-priority weight, when both apply.
///
/// Only a `create` or `run` that already asks for Bosn's low weight is placed, so any
/// other command, `docker update` included, is never touched.
///
/// The host is probed only for such a create, and each probe is bounded, so no other
/// command ever waits on `docker info` or `systemctl`.
pub(crate) fn place(prefix: &mut Vec<OsString>, placement: impl FnOnce() -> Placement) {
    let parent = OsString::from(format!("--cgroup-parent={BOSN_SLICE}"));
    if prefix.contains(&parent) {
        return;
    }
    let Some(index) = prefix.iter().position(|arg| arg == LOW_CPU_PRIORITY_ARG) else {
        return;
    };
    // `docker update` takes the same weight but no parent: only a creation is placed.
    if prefix[..index]
        .iter()
        .any(|arg| arg == "create" || arg == "run")
        && placement() == Placement::Slice
    {
        prefix.insert(index + 1, parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: Option<&str> = Some("unix:///var/run/docker.sock");

    #[test]
    fn a_low_weight_slice_on_a_local_systemd_engine_is_used() {
        assert_eq!(
            Placement::decide(None, "systemd\n", "10\n"),
            Placement::Slice
        );
        assert_eq!(Placement::decide(LOCAL, "systemd", "10"), Placement::Slice);
    }

    #[test]
    fn anything_less_falls_back_to_the_weight_alone() {
        for (host, driver, weight) in [
            (None, "systemd", "[not set]"),
            (None, "systemd", "100"),
            (None, "systemd", ""),
            (None, "cgroupfs", "10"),
            (Some("tcp://10.0.0.2:2375"), "systemd", "10"),
            (Some("unix:///run/user/1000/docker.sock"), "systemd", "10"),
        ] {
            assert_eq!(
                Placement::decide(host, driver, weight),
                Placement::WeightOnly,
                "{host:?} {driver} {weight}"
            );
        }
    }

    #[test]
    fn placement_follows_the_low_weight_and_only_once() {
        let mut args: Vec<OsString> = ["create", LOW_CPU_PRIORITY_ARG, "img"]
            .map(OsString::from)
            .into();
        place(&mut args, || Placement::Slice);
        place(&mut args, || Placement::Slice);
        let expected = [
            "create",
            LOW_CPU_PRIORITY_ARG,
            "--cgroup-parent=bosn.slice",
            "img",
        ];
        assert_eq!(args, expected.map(OsString::from));
    }

    #[test]
    fn other_commands_and_weight_only_hosts_are_untouched() {
        let mut ps: Vec<OsString> = ["ps", "-q"].map(OsString::from).into();
        place(&mut ps, || unreachable!("ps is never probed"));
        assert_eq!(ps.len(), 2);
        let mut update: Vec<OsString> = ["update", LOW_CPU_PRIORITY_ARG, "c1"]
            .map(OsString::from)
            .into();
        place(&mut update, || unreachable!("an update is never probed"));
        assert_eq!(update.len(), 3);
        let mut create: Vec<OsString> = ["create", LOW_CPU_PRIORITY_ARG].map(OsString::from).into();
        place(&mut create, || Placement::WeightOnly);
        assert_eq!(create.len(), 2);
    }
}
