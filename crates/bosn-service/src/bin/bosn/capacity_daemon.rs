//! Existing daemon list operations only; never startup or per-job enrichment.

use bosn_service::{
    Error,
    capacity_observation::{
        Bosn, Counts, Coverage, Daemon, Observation, Reason, Slots, Unavailable,
    },
};
use kernal_api::async_engine::{self, RuntimeBuilder};
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Jobs {
    capacity: Capacity,
    lanes: Lanes,
}
#[derive(Deserialize)]
struct Capacity {
    runner_slots: usize,
}
#[derive(Deserialize)]
struct Lanes {
    runner: Lane,
    control: Lane,
}
#[derive(Deserialize)]
struct Lane {
    queued: usize,
    running: usize,
}

pub struct Pending {
    receiver: mpsc::Receiver<DaemonReport>,
    started: Instant,
    deadline: Duration,
}
struct DaemonReport {
    bosn: Bosn,
    coverage: Coverage,
    unavailable: Vec<Unavailable>,
}

pub fn start(state: PathBuf, timeout: Duration) -> std::io::Result<Pending> {
    let (sender, receiver) = mpsc::channel();
    let started = Instant::now();
    let deadline = timeout.min(Duration::from_secs(2));
    std::thread::Builder::new()
        .name("capacity-daemon".into())
        .spawn(move || {
            let _ = sender.send(observe(state, started, deadline));
        })?;
    Ok(Pending {
        receiver,
        started,
        deadline,
    })
}

impl Pending {
    pub fn finish(self, observation: &mut Observation) {
        match self
            .receiver
            .recv_timeout(self.deadline.saturating_sub(self.started.elapsed()))
        {
            Ok(report) => {
                observation.bosn = report.bosn;
                observation.coverage.bosn = report.coverage;
                observation.unavailable.extend(report.unavailable);
            }
            Err(_) => {
                observation.bosn.daemon = Daemon::Unreachable;
                observation.unavailable.push(Unavailable {
                    field: "bosn".into(),
                    reason: Reason::Timeout,
                });
            }
        }
    }
}

fn observe(state: PathBuf, started: Instant, deadline: Duration) -> DaemonReport {
    let mut report = DaemonReport {
        bosn: Bosn::default(),
        coverage: Coverage::None,
        unavailable: Vec::new(),
    };
    let result = read_jobs(&state, started, deadline);
    let (jobs, client, runtime) = match result {
        Ok(values) => values,
        Err(error) => {
            let (daemon, reason) = failure(&error);
            report.bosn.daemon = daemon;
            report.unavailable.push(Unavailable {
                field: "bosn".into(),
                reason,
            });
            return report;
        }
    };
    apply_jobs(&mut report, jobs);
    let remaining = deadline.saturating_sub(started.elapsed());
    match runtime.run(async_engine::timeout(
        remaining,
        client.ci_list(None, None, Some(0)),
    )) {
        Ok(Ok(ci)) => {
            report.bosn.ci_runs = Counts {
                queued: Some(ci.runners.queued),
                running: Some(ci.runners.running),
            };
            report.bosn.slots.ci_used = Some(ci.runners.running);
            report.bosn.slots.ci_admission_limit = Some(ci.runners.limit);
            // Runners::List saves settings and kicks the dispatcher. The pure
            // list reply has counters but no engine limits; never trigger repair.
            report.unavailable.push(Unavailable {
                field: "bosn.engine".into(),
                reason: Reason::Unsupported,
            });
        }
        result => report.unavailable.push(Unavailable {
            field: "bosn.ci_runs".into(),
            reason: if result.is_err() {
                Reason::Timeout
            } else {
                Reason::ParseError
            },
        }),
    }
    report
}

fn apply_jobs(report: &mut DaemonReport, jobs: Jobs) {
    report.bosn.daemon = Daemon::Running;
    report.coverage = Coverage::Partial;
    report.bosn.jobs = Counts {
        queued: jobs
            .lanes
            .runner
            .queued
            .checked_add(jobs.lanes.control.queued),
        running: jobs
            .lanes
            .runner
            .running
            .checked_add(jobs.lanes.control.running),
    };
    report.bosn.slots = Slots {
        task_used: Some(jobs.lanes.runner.running),
        task_limit: Some(jobs.capacity.runner_slots),
        ..Slots::default()
    };
}

fn read_jobs(
    state: &std::path::Path,
    started: Instant,
    deadline: Duration,
) -> Result<(Jobs, bosn_service::Client, async_engine::Runtime), Error> {
    let client = crate::daemon_client(state)?.without_recovery();
    let runtime = RuntimeBuilder::current_thread().enable_all().build()?;
    let remaining = deadline.saturating_sub(started.elapsed());
    let value = runtime
        .run(async_engine::timeout(remaining, client.jobs()))
        .map_err(|_| Error::Deadline)??;
    let jobs =
        serde_json::from_value(value).map_err(|_| Error::Protocol("malformed capacity jobs"))?;
    Ok((jobs, client, runtime))
}

fn failure(error: &Error) -> (Daemon, Reason) {
    match error {
        Error::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            (Daemon::Absent, Reason::NotFound)
        }
        Error::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            (Daemon::Unreachable, Reason::PermissionDenied)
        }
        Error::Deadline => (Daemon::Unreachable, Reason::Timeout),
        Error::Unauthorized => (Daemon::Unreachable, Reason::PermissionDenied),
        Error::ProtocolUnsupported { .. } => (Daemon::Unreachable, Reason::Unsupported),
        _ => (Daemon::Unreachable, Reason::ParseError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_bosn_jobs_do_not_overwrite_external_activity() {
        let jobs: Jobs = serde_json::from_str(r#"{"capacity":{"runner_slots":4},"lanes":{"runner":{"queued":0,"running":0},"control":{"queued":0,"running":0}},"jobs":[{"command":"secret-command"}]}"#).unwrap();
        let mut report = DaemonReport {
            bosn: Bosn::default(),
            coverage: Coverage::None,
            unavailable: Vec::new(),
        };
        apply_jobs(&mut report, jobs);
        let observation = Observation {
            bosn: report.bosn,
            external: bosn_service::capacity_observation::External {
                containers_running_outside_bosn: Some(2),
                by_label: Some(Vec::new()),
            },
            coverage: bosn_service::capacity_observation::Coverages {
                bosn: report.coverage,
                host: Coverage::Full,
                docker: Coverage::Full,
            },
            ..Observation::default()
        };
        assert_eq!(observation.bosn.jobs.running, Some(0));
        assert_eq!(
            observation.external.containers_running_outside_bosn,
            Some(2)
        );
        assert_eq!(observation.coverage.docker, Coverage::Full);
        let json = serde_json::to_string(&observation).unwrap();
        assert!(!json.contains("secret-command"));
        assert!(observation.advisory.contains("does not mean an idle host"));
    }
}
