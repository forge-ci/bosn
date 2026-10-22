//! The enrollment executable must be the private digest-verified installation.
use super::{DockerActBackend, EnrollmentPhase};
use bosn_engine::DockerEngine;
use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};

#[test]
fn enrollment_executes_the_verified_private_act_binary() {
    let dir = TemporaryDirectory::new().unwrap();
    std::fs::write(dir.path().join("docker.sh"), r#"
set -eu
[ "$1" = exec ]
[ "$2" = immutable-engine-id ]
[ "$3" = /var/lib/docker/bosn-ci/bin/act ] || { echo 'unverified PATH binary refused' >&2; exit 42; }
[ "$4" = cache ]
[ "$5" = tool-enroll ]
printf '%s\n' '{"schema_version":1,"source":"/bosn/cache/toolcache","root":"/bosn/cache/toolstore-v1","mode":"warm","enrolled":false,"source_fenced":true,"partial":false}'
"#).unwrap();
    let backend = DockerActBackend::new(DockerEngine::synthetic_for_test(
        "sh",
        vec![dir.path().join("docker.sh").to_string_lossy().into_owned()],
    ));
    let result = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .run(backend.enroll_tools("immutable-engine-id", EnrollmentPhase::Fence, 1024));
    assert!(result.unwrap().is_none());
}

#[test]
#[ignore = "requires a private Docker fixture with the verified released act installed"]
fn actual_docker_enrollment_transport_preserves_warm_selection() {
    let engine = std::env::var("BOSN_ENROLLMENT_ENGINE_ID").unwrap();
    assert!(engine.len() == 64 && engine.bytes().all(|b| b.is_ascii_hexdigit()));
    let report = std::env::var("BOSN_ENROLLMENT_RESULT").unwrap();
    let backend = DockerActBackend::default();
    RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let max = 64 << 20;
            assert!(
                backend
                    .enroll_tools(&engine, EnrollmentPhase::Fence, max)
                    .await
                    .unwrap()
                    .is_none()
            );
            // The harness owns this private source and starts no legacy writer.
            let binding = backend
                .enroll_tools(&engine, EnrollmentPhase::Complete, max)
                .await
                .unwrap()
                .unwrap();
            assert!(
                backend
                    .enroll_tools(&engine, EnrollmentPhase::Fence, max)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                backend
                    .enroll_tools(&engine, EnrollmentPhase::Complete, max)
                    .await
                    .unwrap(),
                Some(binding.clone())
            );
            std::fs::write(report, serde_json::to_vec(&binding).unwrap()).unwrap();
        });
}
