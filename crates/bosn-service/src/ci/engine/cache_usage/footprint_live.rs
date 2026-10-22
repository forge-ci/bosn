//! Production read-only helper versus the independent du oracle, on an explicitly
//! provisioned disposable Docker daemon. Never run against the host daemon.
use super::*;
use crate::ci::lifecycle::tests::{OWNER, native_live::Fixture, with_registry};

#[test]
#[ignore = "requires a private Docker daemon, owned fixture volume and verified pinned archive"]
fn actual_readonly_footprint_helper_matches_reference_and_cleans_up() {
    let fixture: Fixture = serde_json::from_slice(
        &std::fs::read(std::env::var("BOSN_NATIVE_ADMISSION_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(fixture.act_version, super::super::ACT_VERSION);
    with_registry(|registry, _| async move {
        let backend = DockerActBackend::new(bosn_engine::DockerEngine::docker());
        let docker_id = backend
            .checked(
                "private fixture Docker identity",
                owned(&["info", "--format", "{{.ID}}"]),
                CONTROL_DEADLINE,
            )
            .await
            .unwrap();
        assert_eq!(docker_id, fixture.docker_id);
        let sample = backend
            .checked(
                "independent reference sample",
                owned(&[
                    "run",
                    "--rm",
                    bosn_engine::LOW_CPU_PRIORITY_ARG,
                    "--label",
                    "io.bosn.fixture=footprint-reference",
                    "--network",
                    "none",
                    "--read-only",
                    "--cap-drop",
                    "ALL",
                    "--memory",
                    "128m",
                    "--cpus",
                    "1",
                    "--pids-limit",
                    "64",
                    "--tmpfs",
                    "/var/lib/docker:ro,noexec,size=1m",
                    "--mount",
                    "type=volume,source=bosn-ci-cache-v1,target=/cache,readonly",
                    "--entrypoint",
                    "sh",
                    &engine_image(),
                    "-c",
                    SCRIPT,
                ]),
                CONTROL_DEADLINE,
            )
            .await
            .unwrap();
        let reference = parse(super::super::CACHE_VOLUME, &sample);
        assert!(!reference.partial, "{reference:?}");
        let measured = backend
            .measure_cache_tracked(super::super::CACHE_VOLUME, Some((&registry, OWNER)))
            .await
            .unwrap();
        assert!(!measured.partial, "{measured:?}");
        assert_eq!(measured.bytes, reference.bytes);
        assert_eq!(measured.allocated_bytes, reference.allocated_bytes);
        assert!(
            measured.bytes.unwrap() > measured.allocated_bytes.unwrap(),
            "fixture must distinguish sparse apparent bytes"
        );
        for expected in &reference.components {
            let current = measured
                .components
                .iter()
                .find(|row| row.class == expected.class && row.store_path == expected.store_path)
                .unwrap();
            assert_eq!(current.bytes, expected.bytes, "{expected:?}");
            assert_eq!(
                current.allocated_bytes, expected.allocated_bytes,
                "{expected:?}"
            );
        }
        assert_eq!(
            measured
                .components
                .iter()
                .filter(|row| row.namespace.is_some())
                .count(),
            2
        );
        let helpers = backend
            .checked(
                "accounting helper absence",
                owned(&["ps", "-aq", "--filter", "label=io.bosn.cache.measurement"]),
                CONTROL_DEADLINE,
            )
            .await
            .unwrap();
        assert!(helpers.is_empty(), "{helpers}");
        std::fs::write(
            fixture.report,
            serde_json::to_vec_pretty(&serde_json::json!({
                "passed":true,"reference":reference,"measured":measured,"helpers_absent":true
            }))
            .unwrap(),
        )
        .unwrap();
    });
}
