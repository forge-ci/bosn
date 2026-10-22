use super::*;

fn policy() -> CachePolicy {
    toml::from_str("repository_max_bytes=100\naggregate_max_bytes=200\nmax_age_secs=3600\nunused_age_secs=1800\nmaintenance_interval_secs=60\n").unwrap()
}

#[cfg(unix)]
#[test]
fn execution_policy_contention_obeys_deadline_and_cancellation() {
    use bosn_engine::DockerEngine;
    use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};
    let directory = TemporaryDirectory::new().unwrap();
    let script = directory.path().join("busy.py");
    std::fs::write(&script, "import sys\nsys.exit(75)\n").unwrap();
    let backend = DockerActBackend::new(DockerEngine::synthetic_for_test(
        "python3",
        [script.into_os_string()],
    ));
    RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let stop = async_engine::CancellationSource::new();
            let result = backend
                .guard_archive_execution(
                    "engine",
                    policy(),
                    async_engine::Deadline::after(Duration::from_millis(100)),
                    &stop.token(),
                )
                .await
                .unwrap();
            assert!(matches!(result, Some(super::super::ExecEnd::TimedOut)));
            let cancellation = stop.token();
            let cancel = async_engine::launch(async move {
                async_engine::sleep(Duration::from_millis(100)).await;
                stop.cancel();
            });
            let result = async_engine::timeout(
                Duration::from_secs(2),
                backend.guard_archive_execution(
                    "engine",
                    policy(),
                    async_engine::Deadline::after(Duration::from_secs(30)),
                    &cancellation,
                ),
            )
            .await
            .expect("policy contention must release execution on cancellation")
            .unwrap();
            assert!(matches!(result, Some(super::super::ExecEnd::Cancelled)));
            let _ = cancel.await;
        });
}

fn report(namespace: &Namespace) -> serde_json::Value {
    serde_json::json!({
        "schema_version":1, "source":namespace.legacy_path(),
        "root":crate::ci::cache_cohort::root(), "namespace":namespace.as_str(),
        "snapshot":format!("{ENGINE_CACHE}/actcache/.{}.archive-enrollment-{}",namespace.as_str(),"a".repeat(64)),
        "max_bytes":100,"source_fenced":true,"ready":false,
        "partial":false,"retryable":false,
    })
}

#[test]
fn report_requires_exact_identity_and_phase_before_admission() {
    let namespace = Namespace::parse("0123456789abcdef").unwrap();
    let request = ArchiveEnrollmentRequest {
        engine: "immutable-engine",
        namespace: &namespace,
        policy: policy(),
        phase: ArchiveEnrollmentPhase::Fence,
    };
    let value = report(&namespace);
    let parse = |value: &serde_json::Value, success| {
        request.parse(&serde_json::to_vec(value).unwrap(), success)
    };
    assert_eq!(
        parse(&value, true).unwrap(),
        ArchiveEnrollmentStatus::Fenced
    );
    for (field, invalid) in [
        ("schema_version", serde_json::json!(2)),
        ("source", serde_json::json!("/other/source")),
        ("namespace", serde_json::json!("fedcba9876543210")),
        ("root", serde_json::json!("/other/cohort")),
        ("snapshot", serde_json::json!("/other/snapshot")),
        ("max_bytes", serde_json::json!(101)),
        ("source_fenced", serde_json::json!(false)),
        ("partial", serde_json::json!(true)),
        ("retryable", serde_json::json!(true)),
        ("error", serde_json::json!("unexpected success error")),
    ] {
        let mut changed = value.clone();
        changed[field] = invalid;
        assert!(parse(&changed, true).is_err(), "{field}");
    }
    assert!(request.parse(&vec![b' '; 8193], true).is_err());
    for phase in [
        ArchiveEnrollmentPhase::Complete,
        ArchiveEnrollmentPhase::Require,
    ] {
        let next = ArchiveEnrollmentRequest { phase, ..request };
        assert!(
            next.parse(&serde_json::to_vec(&value).unwrap(), true)
                .is_err()
        );
        let mut ready = value.clone();
        ready["ready"] = serde_json::json!(true);
        assert_eq!(
            next.parse(&serde_json::to_vec(&ready).unwrap(), true)
                .unwrap(),
            ArchiveEnrollmentStatus::Ready
        );
    }
}

#[test]
fn application_refusal_is_distinct_from_retry_and_transport_failure() {
    let namespace = Namespace::parse("0123456789abcdef").unwrap();
    let request = ArchiveEnrollmentRequest {
        engine: "engine",
        namespace: &namespace,
        policy: policy(),
        phase: ArchiveEnrollmentPhase::Require,
    };
    let mut busy = report(&namespace);
    busy["partial"] = serde_json::json!(true);
    busy["retryable"] = serde_json::json!(true);
    busy["source_fenced"] = serde_json::json!(false);
    busy["snapshot"] = serde_json::json!("");
    busy["error"] = serde_json::json!("history busy");
    assert_eq!(
        request
            .parse(&serde_json::to_vec(&busy).unwrap(), false)
            .unwrap(),
        ArchiveEnrollmentStatus::Retry
    );
    busy["retryable"] = serde_json::json!(false);
    assert_eq!(
        request
            .parse(&serde_json::to_vec(&busy).unwrap(), false)
            .unwrap(),
        ArchiveEnrollmentStatus::Refused("history busy".into())
    );
    assert!(request.parse(b"not a report", false).is_err());
    assert!(
        request
            .parse(&serde_json::to_vec(&busy).unwrap(), true)
            .is_err()
    );
    busy["ready"] = serde_json::json!(true);
    assert!(
        request
            .parse(&serde_json::to_vec(&busy).unwrap(), false)
            .is_err()
    );
}

#[test]
fn existing_route_validation_can_share_the_legacy_lifetime_lease() {
    let namespace = Namespace::parse("0123456789abcdef").unwrap();
    for phase in [
        ArchiveEnrollmentPhase::Fence,
        ArchiveEnrollmentPhase::Complete,
        ArchiveEnrollmentPhase::Require,
    ] {
        let request = ArchiveEnrollmentRequest {
            engine: "engine",
            namespace: &namespace,
            policy: policy(),
            phase,
        };
        let args = request.args();
        assert_eq!(&args[..2], ["exec", "engine"]);
        assert!(args[4].contains(&format!("exec {}/bin/act", super::super::ENGINE_WORK)));
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "--from").count(),
            1
        );
        assert!(args.contains(&namespace.legacy_path()));
        if phase == ArchiveEnrollmentPhase::Require {
            assert!(args[4].contains("flock -s 8"));
            assert!(!args.contains(&"--apply".into()));
            assert!(args.contains(&"--require-ready".into()));
        } else {
            assert!(args[4].contains("flock -x -n 8"));
            assert!(args.contains(&"--apply".into()));
            assert_eq!(
                args.contains(&"--source-quiescent".into()),
                phase == ArchiveEnrollmentPhase::Complete
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn only_empty_wrapper_contention_is_a_transport_retry() {
    use bosn_engine::DockerEngine;
    use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};
    let directory = TemporaryDirectory::new().unwrap();
    let script = directory.path().join("docker.py");
    std::fs::write(&script,"import sys\nassert sys.argv[3:5] == ['exec','engine']\nif any('.bosn-cohort-policy-v1' in arg for arg in sys.argv[3:]): sys.exit(0)\nsys.stdout.write(sys.argv[1])\nsys.exit(int(sys.argv[2]))\n").unwrap();
    let namespace = Namespace::parse("0123456789abcdef").unwrap();
    for (output, code, retry) in [("", 75, true), ("malformed", 75, false), ("", 125, false)] {
        let backend = DockerActBackend::new(DockerEngine::synthetic_for_test(
            "python3",
            [
                script.clone().into_os_string(),
                output.into(),
                code.to_string().into(),
            ],
        ));
        let result = RuntimeBuilder::multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .run(backend.enroll_archives(ArchiveEnrollmentRequest {
                engine: "engine",
                namespace: &namespace,
                policy: policy(),
                phase: ArchiveEnrollmentPhase::Fence,
            }));
        if retry {
            assert_eq!(result.unwrap(), ArchiveEnrollmentStatus::Retry)
        } else {
            assert!(result.is_err())
        }
    }
}

#[cfg(unix)]
#[test]
fn ready_archive_receipt_cannot_bootstrap_missing_machine_policy() {
    use bosn_engine::DockerEngine;
    use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};
    let directory = TemporaryDirectory::new().unwrap();
    let script = directory.path().join("docker.py");
    std::fs::write(&script,"import sys\nargs=sys.argv[2:]\nif any('.bosn-cohort-policy-v1' in arg for arg in args):\n print('absent')\nelse:\n print(sys.argv[1])\n").unwrap();
    let namespace = Namespace::parse("0123456789abcdef").unwrap();
    let mut ready = report(&namespace);
    ready["ready"] = serde_json::json!(true);
    let backend = DockerActBackend::new(DockerEngine::synthetic_for_test(
        "python3",
        [script.into_os_string(), ready.to_string().into()],
    ));
    let result = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .run(backend.enroll_archives(ArchiveEnrollmentRequest {
            engine: "engine",
            namespace: &namespace,
            policy: policy(),
            phase: ArchiveEnrollmentPhase::Require,
        }));
    assert!(
        result.is_err(),
        "a ready receipt cannot replace missing immutable policy agreement"
    );
}

#[cfg(unix)]
#[test]
fn conflicting_machine_policy_refuses_before_source_fencing() {
    use bosn_engine::DockerEngine;
    use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};
    let directory = TemporaryDirectory::new().unwrap();
    let script = directory.path().join("docker.py");
    let contacted = directory.path().join("source-contacted");
    std::fs::write(&script,"import pathlib,sys\nargs=sys.argv[3:]\nif any('.bosn-cohort-policy-v1' in arg for arg in args):\n print('machine cache policy conflict',file=sys.stderr)\n sys.exit(78)\npathlib.Path(sys.argv[2]).write_text('contacted')\nprint(sys.argv[1])\n").unwrap();
    let namespace = Namespace::parse("0123456789abcdef").unwrap();
    let backend = DockerActBackend::new(DockerEngine::synthetic_for_test(
        "python3",
        [
            script.into_os_string(),
            report(&namespace).to_string().into(),
            contacted.clone().into_os_string(),
        ],
    ));
    let result = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .run(backend.enroll_archives(ArchiveEnrollmentRequest {
            engine: "engine",
            namespace: &namespace,
            policy: policy(),
            phase: ArchiveEnrollmentPhase::Fence,
        }));
    assert!(
        result.is_err(),
        "full machine-policy conflict must refuse cutover"
    );
    assert!(
        !contacted.exists(),
        "no source operation may precede agreement"
    );
}
