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

#[test]
fn idle_decision_refuses_backlog_without_submitting_work() {
    backlog_during_decision(false);
}

#[test]
fn busy_decision_serves_backlog_normally() {
    backlog_during_decision(true);
}

fn backlog_during_decision(busy: bool) {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let armed = CancellationSource::new();
        let entered = CancellationSource::new();
        let release = CancellationSource::new();
        let mut service =
            Service::new(state.clone()).with_idle_exit(Some(Duration::from_millis(50)));
        let unfinished = Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
        service.idle_gate = Some(service::IdleGate {
            armed: armed.clone(),
            entered: entered.clone(),
            release: release.clone(),
            unfinished: unfinished.clone(),
        });
        let server = async_engine::launch(service.serve());
        let client = wait_for_client(&state).await.without_recovery();
        let registry = client.status().await.unwrap().registry_id;
        if busy {
            client
                .submit_job("workspace", "stack", "existing")
                .await
                .unwrap();
        }
        armed.cancel();
        async_engine::timeout(Duration::from_secs(5), async {
            while !entered.is_cancelled() {
                async_engine::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        let mut streams = Vec::new();
        for id in 1..=4 {
            let mut stream = AsyncStream::connect(&endpoint(&state).unwrap())
                .await
                .unwrap();
            let mut payload = Vec::new();
            // A run request must never reach the job actor during the drain.
            Request {
                workspace: "workspace".into(),
                stack: "stack".into(),
                digest: format!("drained-{id}"),
                ..Request::operation(4)
            }
            .encode(&mut payload)
            .unwrap();
            write_frame(
                &mut stream,
                DaemonFrame::request(PAYLOAD_PROTOCOL, payload).with_request_id(id),
            )
            .await
            .unwrap();
            streams.push((id, stream));
        }
        release.cancel();
        for (id, mut stream) in streams {
            let frame = read_frame(&mut stream).await.unwrap();
            assert_eq!(frame.request_id(), id);
            let reply = ReplyWire::decode(frame.payload()).unwrap();
            assert_eq!(reply.code, if busy { 40 } else { handoff::CODE_EXITING });
        }
        if busy {
            client.shutdown().await.unwrap();
            stopped(server).await;
        } else {
            stopped(server).await;
            assert_eq!(unfinished.load(std::sync::atomic::Ordering::SeqCst), 0);
            let second = async_engine::launch(Service::new(state.clone()).serve());
            let client = wait_for_client(&state).await;
            assert_eq!(client.status().await.unwrap().registry_id, registry);
            #[derive(serde::Deserialize)]
            struct JobsView {
                jobs: Vec<serde_json::Value>,
            }
            let view: JobsView = serde_json::from_value(client.jobs().await.unwrap()).unwrap();
            assert!(view.jobs.is_empty());
            assert_eq!(client.status().await.unwrap().resources, 0);
            client.shutdown().await.unwrap();
            stopped(second).await;
        }
    });
}
