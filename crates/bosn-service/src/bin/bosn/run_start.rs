//! How a client waits for a daemon it just asked to start.

use std::time::{Duration, Instant};

use bosn_service::Client;
use kernal_api::async_engine;

use super::{DAEMON_START_WAIT, FAILED_CHILD_WAIT, POLL_INTERVAL};

pub(super) async fn wait_for_started_daemon(
    client: &Client,
    mut child: Option<&mut std::process::Child>,
) -> Option<bosn_service::DaemonIdentity> {
    let started = Instant::now();
    let mut budget = DAEMON_START_WAIT;
    loop {
        if let Ok(identity) = client.daemon_identity().await {
            return Some(identity);
        }
        if started.elapsed() >= budget {
            return client.daemon_identity().await.ok();
        }
        // A clean exit is how a losing starter yields to the winner, so only
        // a failure shortens the wait.
        if let Some(child) = child.as_mut()
            && matches!(child.try_wait(), Ok(Some(status)) if !status.success())
        {
            budget = FAILED_CHILD_WAIT;
        }
        async_engine::sleep(POLL_INTERVAL).await;
    }
}

/// Wait for a daemon a service manager owns, asking it again every `retry`. A
/// start that arrives while a stopped daemon's unit is still going inactive is a
/// no-op, and nothing else would ask again (#846). On an active unit it stays one.
pub(super) async fn wait_for_unit_daemon<F>(
    client: &Client,
    retry: Duration,
    start: F,
) -> Option<bosn_service::DaemonIdentity>
where
    F: Fn() + Send + Sync + 'static,
{
    let start = std::sync::Arc::new(start);
    let started = Instant::now();
    let mut asked = Instant::now();
    loop {
        if let Ok(identity) = client.daemon_identity().await {
            return Some(identity);
        }
        if started.elapsed() >= DAEMON_START_WAIT {
            return client.daemon_identity().await.ok();
        }
        if asked.elapsed() >= retry {
            asked = Instant::now();
            let start = std::sync::Arc::clone(&start);
            let _ = async_engine::launch_blocking(move || start()).await;
        }
        async_engine::sleep(POLL_INTERVAL).await;
    }
}
