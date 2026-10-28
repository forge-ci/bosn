use super::*;

#[test]
fn dashboard_retry_cannot_enter_after_handoff_closes_admission() {
    with_registry(|registry, dir| async move {
        let runtime = CiRuntime::start(&dir, registry, Arc::new(FakeBackend::default()), 1);
        let gate = runtime.admission();
        let held = gate.lock().await;
        let retry_runtime = runtime.clone();
        let (ready, wait_ready) = async_engine::oneshot_channel();
        let task = async_engine::launch(async move {
            let _ = ready.send(());
            retry_runtime
                .handle(CiRequest::Retry {
                    run: "unknown".into(),
                    job: None,
                })
                .await
        });
        wait_ready.await.unwrap();
        runtime
            .admission_closed()
            .store(true, std::sync::atomic::Ordering::Release);
        drop(held);
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error.code, "busy");
        assert!(error.message.contains("handoff"));
    });
}
