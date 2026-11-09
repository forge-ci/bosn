use super::*;

fn proc_file(name: &str) -> io::Result<String> {
    Ok(match name {
        "stat" => "cpu 0 0 0 100 0 0 0 0\n",
        "loadavg" => "0 0 0 1/1 1\n",
        "meminfo" => "MemTotal: 100 kB\nMemAvailable: 100 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
        "pressure/cpu" | "pressure/memory" | "pressure/io" => {
            "some avg10=0.00 avg60=0 avg300=0 total=0\nfull avg10=0.00 avg60=0 avg300=0 total=0\n"
        }
        _ => return Err(io::Error::from(io::ErrorKind::NotFound)),
    }
    .into())
}

#[test]
fn idle_proc_and_docker_inputs_are_zero_and_missing_quota_is_honest() {
    let dir = tempfile::tempdir().unwrap();
    let sample = HostSample::with_reader(dir.path(), proc_file);
    assert_eq!(sample.observation.host.load, [Some(0.0); 3]);
    assert_eq!(sample.observation.host.psi.cpu_some_avg10, Some(0.0));
    assert_eq!(sample.observation.host.swap.used_bytes, Some(0));
    assert_eq!(sample.observation.unavailable.len(), 1);
    assert_eq!(
        sample.observation.unavailable[0].reason,
        Reason::Unsupported
    );
    assert_eq!(
        host_probe::external::parse("")
            .unwrap()
            .containers_running_outside_bosn,
        Some(0)
    );
    assert_eq!(host_coverage(&sample.observation), Coverage::Partial);
}

#[test]
fn psi_permission_denied_is_not_zero_or_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let sample = HostSample::with_reader(dir.path(), |name| {
        if name == "pressure/memory" {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        } else {
            proc_file(name)
        }
    });
    assert_eq!(sample.observation.host.psi.memory_some_avg10, None);
    let reasons: Vec<_> = sample
        .observation
        .unavailable
        .iter()
        .filter(|u| u.field == "host.pressure/memory")
        .collect();
    assert_eq!(reasons.len(), 1);
    assert_eq!(reasons[0].reason, Reason::PermissionDenied);
}

#[test]
fn malformed_proc_is_reported_without_fabricating_measurements() {
    let dir = tempfile::tempdir().unwrap();
    let sample = HostSample::with_reader(dir.path(), |_| Ok("invalid".into()));
    assert_eq!(sample.observation.host.memory.total_bytes, None);
    assert_eq!(sample.observation.host.load, [None; 3]);
    assert!(
        sample
            .observation
            .unavailable
            .iter()
            .any(|u| u.field == "host.stat" && u.reason == Reason::ParseError)
    );
}

#[test]
fn schema_one_snapshot_retains_nulls_and_the_advisory() {
    let actual = serde_json::to_value(Observation::default()).unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("capacity_observation_schema.json")).unwrap();
    assert_eq!(actual, expected);
}
