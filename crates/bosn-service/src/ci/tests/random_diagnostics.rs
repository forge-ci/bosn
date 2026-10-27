use super::*;
use kernal_api::random::RandomError;

#[test]
fn secure_random_concurrent_failed_submissions_do_not_admit_or_replace_runs() {
    with_registry(|registry, dir| async move {
        let backend = Arc::new(FakeBackend::with(Faults {
            hang: true,
            ..Faults::default()
        }));
        let runtime = CiRuntime::start(&dir, registry, backend.clone(), 1);
        let prior = submit_with_timeout(&runtime, 'a', 30).await;
        let prior_id = prior.run.clone();
        execution::wait_for_executions(&runtime, &backend, &[&prior_id]).await;
        assert_eq!(runtime.record(&prior_id).unwrap().state, RunState::Running);
        let mut submissions = Vec::new();
        let errors = [
            RandomError::TimedOut,
            RandomError::Unavailable,
            RandomError::WorkerFailed,
            RandomError::AtCapacity,
            RandomError::Cancelled,
        ];
        for error in errors {
            let staging = new_uuid().await.unwrap();
            staged(&runtime, &staging);
            let request = request(&staging, 'b');
            let bytes = crate::secure_random::map_result(
                "secure identifier",
                crate::secure_random::RandomBudget::Short,
                async move { Err(error) },
            );
            submissions.push(runtime.submit_with_id(
                request,
                None,
                crate::ci::wire::uuid_from(bytes),
            ));
        }
        let results = futures_util::future::join_all(submissions).await;
        for (result, category) in results.into_iter().zip(errors) {
            let error = result.expect_err("random failure must not admit a run");
            let expected = crate::secure_random::diagnostic(
                "secure identifier",
                crate::secure_random::RandomBudget::Short,
                category,
            );
            assert_eq!(error.code, "internal");
            assert_eq!(error.message, expected);
            let wire = crate::dispatch::ci_error_wire(error.code, error.message);
            let reply: crate::ci::ErrorReply = serde_json::from_str(&wire.ci_reply).unwrap();
            assert_eq!(reply.code, "internal");
            assert_eq!(reply.message, expected);
        }
        let listing = runtime.listing();
        assert_eq!(listing.runs.len(), 1);
        assert_eq!(runtime.record(&prior_id).unwrap().state, RunState::Running);
        assert_eq!(*backend.executions.lock().unwrap(), 1);
    });
}

#[test]
fn secure_random_cancelled_submission_cannot_admit_late_id() {
    with_registry(|registry, dir| async move {
        let runtime = CiRuntime::start(&dir, registry, Arc::new(FakeBackend::default()), 1);
        let prior = submit_with_timeout(&runtime, 'a', 5).await;
        let staging = new_uuid().await.unwrap();
        staged(&runtime, &staging);
        let request = request(&staging, 'b');
        let (sender, receiver) = async_engine::oneshot_channel::<String>();
        let result = async_engine::timeout(
            Duration::from_millis(100),
            runtime.submit_with_id(request, None, async move {
                receiver
                    .await
                    .map_err(|_| CiError::new("internal", "random worker stopped"))
            }),
        )
        .await;
        assert!(result.is_err());
        assert!(
            sender
                .send("00000000-0000-4000-8000-000000000001".into())
                .is_err()
        );
        assert_eq!(runtime.listing().runs.len(), 1);
        assert!(runtime.record(&prior.run).is_ok());
    });
}
