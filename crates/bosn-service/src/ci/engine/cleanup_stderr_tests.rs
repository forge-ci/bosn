//! A nonzero Docker client keeps its status even when a descendant holds stderr.
use super::*;
use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};

#[test]
fn checked_keeps_nonzero_status_and_abandoned_stderr_marker() {
    let dir = TemporaryDirectory::new().unwrap();
    let script = dir.path().join("docker.sh");
    std::fs::write(
        &script,
        "(sleep 2) >/dev/null &\nprintf 'retained cleanup error\\n' >&2\nexit 7\n",
    )
    .unwrap();
    let backend = DockerActBackend::new(DockerEngine::synthetic_for_test(
        "sh",
        vec![script.to_string_lossy().into_owned()],
    ));
    let error = RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(backend.checked("engine cleanup", Vec::new(), Duration::from_secs(5)))
        .unwrap_err();
    assert!(error.contains("exit status 7"), "{error}");
    assert!(error.contains("[stderr abandoned]"), "{error}");
    assert!(error.contains("retained cleanup error"), "{error}");
}
