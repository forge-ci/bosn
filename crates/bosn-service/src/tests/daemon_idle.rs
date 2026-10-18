//! Idle exit: a daemon nobody uses exits, keeps its state, and hands off cleanly.

use super::*;

#[test]
fn an_idle_daemon_exits_and_the_next_daemon_reopens_its_registry() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let idle = Some(Duration::from_millis(300));
        let first = async_engine::launch(Service::new(state.clone()).with_idle_exit(idle).serve());
        let client = wait_for_client(&state).await;
        let registry = client.status().await.unwrap().registry_id;
        drop(client);
        // Nothing connected and nothing busy: it exits by itself.
        stopped(first).await;
        // The writer fence is released and the registry is intact for the next daemon.
        let second = async_engine::launch(Service::new(state.clone()).serve());
        let client = wait_for_client(&state).await;
        assert_eq!(client.status().await.unwrap().registry_id, registry);
        client.shutdown().await.unwrap();
        stopped(second).await;
    });
}

#[test]
fn a_daemon_without_idle_exit_keeps_running() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let server = async_engine::launch(Service::new(state.clone()).serve());
        let client = wait_for_client(&state).await;
        async_engine::sleep(Duration::from_millis(600)).await;
        client.ping().await.unwrap();
        client.shutdown().await.unwrap();
        stopped(server).await;
    });
}

#[test]
fn an_unfinished_job_prevents_idle_exit() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let server = async_engine::launch(
            Service::new(state.clone())
                .with_idle_exit(Some(Duration::from_millis(200)))
                .serve(),
        );
        let client = wait_for_client(&state).await;
        let job = client
            .submit_job("workspace", "stack", "sha256:abc")
            .await
            .unwrap();
        async_engine::sleep(Duration::from_millis(600)).await;
        assert_eq!(client.job_status(job).await.unwrap().state, "Running");
        client.shutdown().await.unwrap();
        stopped(server).await;
    });
}
