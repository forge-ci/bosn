//! How a client waits for a daemon it just started.

use std::time::Instant;

use bosn_service::Client;
use kernal_api::async_engine::{self, RuntimeBuilder};

use super::{DAEMON_START_WAIT, FAILED_CHILD_WAIT, POLL_INTERVAL, wait_for_started_daemon};

#[cfg(unix)]
#[test]
fn a_losing_starter_waits_for_the_winners_socket() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let mut loser = std::process::Command::new("true").spawn().unwrap();
        assert!(loser.wait().unwrap().success());
        let delayed_state = state.clone();
        let winner = async_engine::launch(async move {
            async_engine::sleep(POLL_INTERVAL * 2).await;
            bosn_service::Service::new(delayed_state).serve().await
        });
        let client = Client::for_state(&state).unwrap();
        let identity = wait_for_started_daemon(&client, Some(&mut loser)).await;
        assert!(
            identity.is_some(),
            "the losing starter gave up before the winner was ready"
        );
        client.shutdown().await.unwrap();
        winner.await.unwrap().unwrap();
    });
}

#[cfg(unix)]
#[test]
fn a_child_that_fails_shortens_the_wait() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let client = Client::for_state(temporary.path().join("state")).unwrap();
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let mut failed = std::process::Command::new("false").spawn().unwrap();
    let started = Instant::now();
    let identity = runtime.run(wait_for_started_daemon(&client, Some(&mut failed)));
    assert!(identity.is_none());
    assert!(started.elapsed() >= FAILED_CHILD_WAIT);
    assert!(started.elapsed() < DAEMON_START_WAIT / 2);
}
