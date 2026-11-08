//! The client recovery bridge uses the existing authoritative startup policy.
use bosn_service::Client;
use kernal_api::async_engine::RuntimeBuilder;
use std::process::Command;

#[test]
fn recovery_bridge_starts_once_and_respects_disabled_state() {
    let temporary = tempfile::tempdir().unwrap();
    let state = temporary.path().join("state");
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let ensure = || {
        Command::new(env!("CARGO_BIN_EXE_bosn"))
            .env("DOCKER_HOST", "tcp://127.0.0.1:1")
            .env("HOME", temporary.path())
            .args(["daemon", "ensure", "--state-dir"])
            .arg(&state)
            .output()
            .unwrap()
    };
    let first = ensure();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let client = Client::for_state(&state).unwrap();
    let registry = runtime.run(client.status()).unwrap().registry_id;
    let second = ensure();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(runtime.run(client.status()).unwrap().registry_id, registry);
    runtime
        .run(client.shutdown_and_wait(std::time::Duration::from_secs(5)))
        .unwrap();
    bosn_service::daemon_switch::disable(&state).unwrap();
    assert!(!ensure().status.success());
    assert!(runtime.run(client.ping()).is_err());
}
