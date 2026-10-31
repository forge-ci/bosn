//! `bosn jobs`: the daemon's runner accounting (#358).
//!
//! One line per queued, running or recently finished job: its lane and
//! runner slot, how long it has been idle, and for a running task the CPU
//! allocation, its setup container and the containers it created, which are
//! found by their `com.zackees.bosn.run` label.

use std::{
    ffi::OsString,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bosn_service::ci::{RunnerAction, RunnersReply, TaskLaneCapacity};
use bosn_service::docker_api::{DockerApi, LABEL_RUN};
use kernal_api::async_engine::RuntimeBuilder;
use serde_json::Value;
use std::fmt::Write as _;

pub const USAGE: &str = "bosn jobs [--state-dir STATE_DIR] [--json] [--all]";

pub fn run(mut arguments: impl Iterator<Item = OsString>) {
    let mut state_dir: Option<PathBuf> = None;
    let mut json = false;
    let mut all = false;
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--state-dir" if state_dir.is_none() => match arguments.next() {
                Some(value) if !value.is_empty() => state_dir = Some(PathBuf::from(value)),
                _ => fail(),
            },
            "--json" if !json => json = true,
            "--all" if !all => all = true,
            _ => fail(),
        }
    }
    let state_dir = state_dir.unwrap_or_else(bosn_service::mcp::default_state_dir);
    super::require_matching_daemon(&state_dir, "jobs", json);
    let reports = RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .ok()
        .and_then(|runtime| {
            let client = crate::daemon_client(&state_dir).ok()?;
            let jobs = runtime.run(client.jobs()).ok()?;
            let ci = runtime.run(client.ci_runners(RunnerAction::List)).ok();
            Some((jobs, ci))
        });
    let Some((mut view, ci)) = reports else {
        if json {
            println!(
                "{}",
                serde_json::json!({"action": "jobs", "daemon": "unavailable"})
            );
        } else {
            eprintln!(
                "bosn jobs: no daemon answered for {} (or it predates `bosn jobs`)",
                state_dir.display()
            );
        }
        std::process::exit(1);
    };
    attach_containers(&mut view, all);
    if json {
        if let Some(ci) = &ci {
            view["ci"] = serde_json::to_value(ci).unwrap_or(Value::Null);
        }
        emit(&format!("{view}\n"));
    } else {
        emit(&format_table(&view, ci.as_ref()));
    }
}

/// Write the whole report at once. A closed pipe (`bosn jobs | head`) is not
/// an error worth a panic.
fn emit(text: &str) {
    use std::io::Write;
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

fn fail() -> ! {
    eprintln!("usage: {USAGE}");
    std::process::exit(2);
}

/// How long `bosn jobs` waits for Docker before reporting without containers.
const ENRICH_DEADLINE: Duration = Duration::from_secs(3);

/// Add containers (by run label) to queued and running jobs, or to every job
/// with `--all`. One Docker label census serves every row (#796); if it is
/// slow or Docker is down, the report still goes out by the deadline and
/// `container_enrichment` says why the containers are missing.
fn attach_containers(view: &mut Value, all: bool) {
    attach_containers_with(view, all, ENRICH_DEADLINE, || {
        DockerApi::from_environment()
            .ok_or_else(|| std::io::Error::other("no Docker endpoint"))?
            .labelled_with_labels("containers", LABEL_RUN)
    });
}

type Census = Vec<(String, serde_json::Map<String, Value>)>;

fn attach_containers_with<F>(view: &mut Value, all: bool, deadline: Duration, census: F)
where
    F: FnOnce() -> std::io::Result<Census> + Send + 'static,
{
    let wanted = |job: &Value| all || matches!(job["state"].as_str(), Some("queued" | "running"));
    let any = view["jobs"].as_array().is_some_and(|jobs| {
        jobs.iter()
            .any(|job| wanted(job) && job.pointer("/run/run").is_some())
    });
    let status = if any {
        let (tx, rx) = std::sync::mpsc::channel();
        // Detached: on a missed deadline the census thread dies with the process.
        std::thread::spawn(move || {
            let _ = tx.send(census());
        });
        match rx.recv_timeout(deadline) {
            Ok(Ok(found)) => {
                fill_containers(view, &wanted, &found);
                "complete"
            }
            Ok(Err(_)) => "unavailable",
            Err(_) => "deadline_exceeded",
        }
    } else {
        "skipped"
    };
    view["container_enrichment"] = Value::from(status);
}

fn fill_containers(view: &mut Value, wanted: &dyn Fn(&Value) -> bool, found: &Census) {
    let mut by_run: std::collections::HashMap<&str, Vec<String>> = Default::default();
    for (id, labels) in found {
        if let Some(run) = labels.get(LABEL_RUN).and_then(Value::as_str) {
            by_run
                .entry(run)
                .or_default()
                .push(id[..id.len().min(12)].to_owned());
        }
    }
    let Some(jobs) = view.get_mut("jobs").and_then(Value::as_array_mut) else {
        return;
    };
    for job in jobs.iter_mut().filter(|job| wanted(job)) {
        let Some(run) = job.pointer("/run/run").and_then(Value::as_str) else {
            continue;
        };
        let ids = by_run.get(run).cloned().unwrap_or_default();
        job["run"]["containers"] = Value::from(ids);
    }
}

fn format_table(view: &Value, ci: Option<&RunnersReply>) -> String {
    let mut out = String::new();
    let capacity: Option<TaskLaneCapacity> = serde_json::from_value(view["capacity"].clone()).ok();
    let _ = writeln!(
        out,
        "{}",
        super::ci::format_task_capacity(capacity.as_ref()),
    );
    let _ = writeln!(
        out,
        "{}",
        super::ci::format_ci_capacity(
            ci.map(|reply| reply.runners.limit),
            ci.and_then(|reply| reply.engine_capacity.as_ref()),
        )
    );
    for lane in ["runner", "control"] {
        let _ = writeln!(
            out,
            "{lane:>7}: {} running, {} queued",
            view["lanes"][lane]["running"], view["lanes"][lane]["queued"]
        );
    }
    let jobs = view["jobs"].as_array().cloned().unwrap_or_default();
    if jobs.is_empty() {
        let _ = writeln!(out, "no jobs");
        return out;
    }
    let _ = writeln!(
        out,
        "{:>5}  {:<10}  {:<7}  {:>4}  {:>7}  {:>6}  {:<28}  WORKSPACE / CONTAINERS",
        "ID", "STATE", "LANE", "SLOT", "AGE", "IDLE", "KEY"
    );
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    for job in jobs {
        let age = job["started_ms"]
            .as_u64()
            .or(job["submitted_ms"].as_u64())
            .map(|start| {
                let end = job["finished_ms"].as_u64().unwrap_or(now_ms);
                duration(end.saturating_sub(start) / 1000)
            })
            .unwrap_or_default();
        let idle = job["idle_seconds"]
            .as_u64()
            .map(duration)
            .unwrap_or_default();
        let slot = job["slot"]
            .as_u64()
            .map_or_else(|| "-".to_owned(), |s| (s + 1).to_string());
        let _ = writeln!(
            out,
            "{:>5}  {:<10}  {:<7}  {:>4}  {:>7}  {:>6}  {:<28}  {}",
            job["id"].to_string(),
            job["state"].as_str().unwrap_or("?"),
            job["class"].as_str().unwrap_or("?"),
            slot,
            age,
            idle,
            truncate(job["key"].as_str().unwrap_or(""), 28),
            job["workspace"].as_str().unwrap_or(""),
        );
        if let Some(run) = job.get("run") {
            let containers = run["containers"]
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "{:>5}  task {} in {}; run {}; docker: {} requests, {} creates; live containers: [{}]; caches: {}",
                "",
                run["task"].as_str().unwrap_or(""),
                run["container"].as_str().unwrap_or("-"),
                run["run"].as_str().unwrap_or(""),
                run["docker_requests"],
                run["docker_creates"],
                containers,
                run["caches"]
                    .as_array()
                    .map(|c| c
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "))
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| "-".into()),
            );
        }
        if let Some(error) = job["error"].as_str() {
            let _ = writeln!(out, "{:>5}  error: {}", "", truncate(error, 160));
        }
    }
    out
}

fn duration(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m{:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        value.to_owned()
    } else {
        let mut text: String = value.chars().take(width.saturating_sub(1)).collect();
        text.push('~');
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn job(id: u64, state: &str, run: &str) -> Value {
        serde_json::json!({"id": id, "state": state, "run": {"run": run}})
    }

    fn container(id: &str, run: &str) -> (String, serde_json::Map<String, Value>) {
        let mut labels = serde_json::Map::new();
        labels.insert(LABEL_RUN.to_owned(), Value::from(run));
        (id.to_owned(), labels)
    }

    #[test]
    fn enrichment_takes_one_census_for_many_historical_rows() {
        let mut jobs: Vec<Value> = (0..500)
            .map(|i| job(i, "succeeded", &format!("old-{i}")))
            .collect();
        jobs.push(job(900, "running", "live"));
        let mut view = serde_json::json!({"jobs": jobs});
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        attach_containers_with(&mut view, false, Duration::from_secs(5), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(vec![
                container("aaaaaaaaaaaaaaaaaaaa", "live"),
                container("bbbbbbbbbbbbbbbbbbbb", "old-3"),
            ])
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(view["container_enrichment"], "complete");
        assert_eq!(view["jobs"][500]["run"]["containers"][0], "aaaaaaaaaaaa");
        // Finished jobs are not enriched unless asked.
        assert!(view["jobs"][3]["run"].get("containers").is_none());
    }

    #[test]
    fn enrichment_of_finished_jobs_is_available_on_demand() {
        let mut view = serde_json::json!({"jobs": [job(1, "failed", "old")]});
        attach_containers_with(&mut view, true, Duration::from_secs(5), || {
            Ok(vec![container("cccccccccccccccc", "old")])
        });
        assert_eq!(view["jobs"][0]["run"]["containers"][0], "cccccccccccc");
    }

    #[test]
    fn slow_docker_yields_a_partial_report_within_the_deadline() {
        let mut view = serde_json::json!({"jobs": [job(1, "running", "live")]});
        let started = std::time::Instant::now();
        attach_containers_with(&mut view, false, Duration::from_millis(100), || {
            std::thread::sleep(Duration::from_secs(10));
            Ok(Vec::new())
        });
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(view["container_enrichment"], "deadline_exceeded");
        assert_eq!(view["jobs"][0]["state"], "running");
        assert!(view["jobs"][0]["run"].get("containers").is_none());
    }

    #[test]
    fn unavailable_docker_is_reported_not_fatal() {
        let mut view = serde_json::json!({"jobs": [job(1, "queued", "q")]});
        attach_containers_with(&mut view, false, Duration::from_secs(5), || {
            Err(std::io::Error::other("no socket"))
        });
        assert_eq!(view["container_enrichment"], "unavailable");
    }

    #[test]
    fn no_active_jobs_skips_docker_entirely() {
        let mut view = serde_json::json!({"jobs": [job(1, "succeeded", "old")]});
        attach_containers_with(&mut view, false, Duration::from_secs(5), || {
            panic!("census must not run")
        });
        assert_eq!(view["container_enrichment"], "skipped");
    }

    #[test]
    fn jobs_capacity_names_task_and_ci_lanes_with_the_engine_ceiling() {
        let view = serde_json::json!({
            "capacity": {
                "runner_slots": 32,
                "cpus_per_slot": 4.0,
                "host_cpus": 16,
                "control_slots": 8,
                "stall_seconds": 1800,
                "docker_proxy": true
            },
            "lanes": {
                "runner": {"running": 0, "queued": 0},
                "control": {"running": 0, "queued": 0}
            },
            "jobs": []
        });
        let ci: RunnersReply = serde_json::from_value(serde_json::json!({
            "runners": {
                "limit": 2, "running": 0, "queued": 0, "drained": false,
                "engine": "act", "act_version": "act2.19", "widget": "absent"
            },
            "engine_capacity": {
                "cpus": 8, "memory_bytes": 17179869184_u64,
                "cpu_source": "default_ceiling", "running": true
            },
            "pruned_runs": null
        }))
        .unwrap();
        assert_eq!(
            format_table(&view, Some(&ci)),
            concat!(
                "task lane: 32 slots x 4 CPUs per slot (host 16 logical CPUs), control slots: 8, stall teardown: 1800s, docker proxy: on\n",
                "CI lane: admission limit 2; running engine 8 CPUs (default 8-CPU ceiling), 16 GiB memory\n",
                " runner: 0 running, 0 queued\n",
                "control: 0 running, 0 queued\n",
                "no jobs\n"
            )
        );
    }
}
