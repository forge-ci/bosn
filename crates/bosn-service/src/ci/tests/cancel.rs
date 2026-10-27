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
        // Observe cancellation during execution, not a preparation timeout.
        let run = submit_with_timeout(&runtime, 'a', 30).await.run;
        execution::wait_for_executions(&runtime, &backend, &[&run]).await;
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
