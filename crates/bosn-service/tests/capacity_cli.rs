//! Capacity is read-only even when its controller/daemon is gone (#797).
#![cfg(unix)]

use bosn_service::capacity_observation::{Coverage, Daemon, Observation, Reason};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
    time::{Duration, Instant},
};

fn docker(dir: &Path, body: &str) {
    let script = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$CAPACITY_ARGV\"\n{body}\n");
    let path = dir.join("docker");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bosn"))
        .arg("capacity")
        .args(args)
        .arg("--state-dir")
        .arg(dir.join("state"))
        .env("PATH", dir)
        .env("CAPACITY_ARGV", dir.join("argv"))
        .output()
        .unwrap()
}

fn observation(output: &Output) -> Observation {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn row(project: &str) -> String {
    serde_json::json!({"state":"running","labels":"","project":project,"service":""}).to_string()
}

#[test]
fn absent_daemon_and_external_containers_never_mean_an_idle_host() {
    let dir = tempfile::tempdir().unwrap();
    docker(
        dir.path(),
        &format!("printf '%s\\n' '{}' '{}'", row(""), row("")),
    );
    let output = run(dir.path(), &["--json"]);
    let report = observation(&output);
    assert_eq!(report.bosn.daemon, Daemon::Absent);
    assert_eq!(report.bosn.jobs.running, None);
    assert_eq!(report.external.containers_running_outside_bosn, Some(2));
    assert_eq!(report.coverage.docker, Coverage::Full);
    assert!(report.advisory.contains("does not mean an idle host"));
    assert!(
        !dir.path().join("state").exists(),
        "observation must not initialize daemon state"
    );
    assert_only_ps(dir.path());
}

fn assert_only_ps(dir: &Path) {
    let args = std::fs::read_to_string(dir.join("argv")).unwrap();
    let mut lines = args.lines();
    assert_eq!(lines.next(), Some("ps"));
    assert_eq!(lines.next(), Some("-a"));
    assert_eq!(lines.next(), Some("--format"));
    assert!(lines.next().unwrap().contains("com.docker.compose.project"));
    assert_eq!(lines.next(), None, "only one ps command is allowed");
    assert!(!args.contains(".Names"));
    assert!(!args.contains(".Image"));
    assert!(!args.contains(".Command"));
}

#[test]
fn compose_work_survives_its_controller_without_an_ownership_claim() {
    let dir = tempfile::tempdir().unwrap();
    docker(dir.path(), &format!("printf '%s\\n' '{}'", row("foo")));
    let report = observation(&run(dir.path(), &["--json"]));
    let labels = report.external.by_label.unwrap();
    assert_eq!(labels[0].key, "com.docker.compose.project");
    assert_eq!(labels[0].value, "foo");
    assert_eq!(labels[0].count, Some(1));
    assert_only_ps(dir.path());
}

#[test]
fn failed_or_malformed_docker_is_unknown_and_never_leaks_private_fields() {
    let dir = tempfile::tempdir().unwrap();
    for body in [
        "exit 1".to_owned(),
        format!("printf '%s\\n' '{}' 'malformed'", row("foo")),
        "printf '%s\\n' '{\"name\":\"secret-container-name\",\"command\":\"secret-command\"}'"
            .into(),
    ] {
        docker(dir.path(), &body);
        let output = run(dir.path(), &["--json"]);
        let report = observation(&output);
        assert_eq!(report.external.containers_running_outside_bosn, None);
        assert!(report.external.by_label.is_none());
        assert_eq!(report.coverage.docker, Coverage::None);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-"));
    }
}

#[test]
fn docker_default_deadline_and_no_docker_mode_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    docker(dir.path(), "while :; do :; done");
    let started = Instant::now();
    let report = observation(&run(dir.path(), &["--json"]));
    assert!(started.elapsed() < Duration::from_secs(7));
    assert!(
        report
            .unavailable
            .iter()
            .any(|u| u.field == "external" && u.reason == Reason::Timeout)
    );
    std::fs::write(dir.path().join("argv"), "").unwrap();
    let report = observation(&run(dir.path(), &["--json", "--no-docker"]));
    assert_eq!(report.coverage.docker, Coverage::None);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("argv")).unwrap(),
        ""
    );
    for value in ["0", "invalid"] {
        assert_eq!(
            run(dir.path(), &["--timeout-ms", value]).status.code(),
            Some(2)
        );
    }
}

#[test]
fn empty_docker_listing_is_known_zero_and_human_output_is_advisory() {
    let dir = tempfile::tempdir().unwrap();
    docker(dir.path(), "exit 0");
    let report = observation(&run(dir.path(), &["--json"]));
    assert_eq!(report.external.containers_running_outside_bosn, Some(0));
    assert_eq!(report.coverage.docker, Coverage::Full);
    let output = run(dir.path(), &[]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    for heading in ["Bosn:", "Host:", "Outside Bosn:", "Unavailable:"] {
        assert!(text.contains(heading));
    }
    assert!(text.ends_with(&format!("{}\n", report.advisory)));
}
