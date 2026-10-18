use super::*;

#[test]
fn cancelling_is_visible_immediately_until_teardown_finishes() {
    with_registry(|registry, dir| async move {
        let backend = Arc::new(FakeBackend::with(Faults {
            hang: true,
            close_delay: Duration::from_millis(300),
            ..Faults::default()
        }));
        let runtime = CiRuntime::start(&dir, registry, backend.clone(), 1);
        let run = submit(&runtime, 'a').await.run;
        while *backend.executions.lock().unwrap() == 0 {
            async_engine::sleep(Duration::from_millis(5)).await;
        }
        let cancelled: CancelReply = call(&runtime, CiRequest::Cancel { run: run.clone() }).await;
        assert!(cancelled.cancelled);
        assert_eq!(cancelled.state, RunState::Running);
        let listed: ListReply = call(
            &runtime,
            CiRequest::List {
                workspace: None,
                state: None,
                limit: Some(10),
            },
        )
        .await;
        assert_eq!(listed.runs[0].record.display_state(), "cancelling");
        assert!(listed.runs[0].record.cancel_requested_at.is_some());
        let done = wait_done(&runtime, &run).await;
        assert_eq!(done.conclusion, Some(Conclusion::Cancelled));
        assert_eq!(done.display_state(), "done");
    });
}
