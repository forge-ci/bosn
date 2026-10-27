use super::secure_random::{RandomBudget, diagnostic, shared_source};
use kernal_api::random::RandomError;

#[test]
fn secure_random_error_categories_are_bounded_and_distinct() {
    let cases = [
        (RandomError::TimedOut, "deadline"),
        (RandomError::Unavailable, "operating-system"),
        (RandomError::WorkerFailed, "worker failed"),
        (RandomError::RuntimeUnavailable, "runtime"),
        (RandomError::AtCapacity, "capacity"),
        (RandomError::Cancelled, "cancelled"),
        (RandomError::InvalidLimits, "invalid"),
        (RandomError::TooLarge, "byte limit"),
    ];
    let mut messages = std::collections::BTreeSet::new();
    for (error, expected) in cases {
        let message = diagnostic("run ID", RandomBudget::Short, error);
        assert!(message.contains(expected), "{message}");
        assert!(message.contains("run ID"));
        assert!(message.len() < 256);
        assert!(messages.insert(message));
    }
}

#[test]
fn secure_random_sources_are_reused_per_budget() {
    assert!(std::ptr::eq(
        shared_source(RandomBudget::Short),
        shared_source(RandomBudget::Short)
    ));
    assert!(std::ptr::eq(
        shared_source(RandomBudget::Long),
        shared_source(RandomBudget::Long)
    ));
}

#[test]
fn secure_random_run_id_failure_has_no_fallback() {
    let runtime = kernal_api::async_engine::RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap();
    for error in [
        RandomError::TimedOut,
        RandomError::Unavailable,
        RandomError::WorkerFailed,
        RandomError::RuntimeUnavailable,
    ] {
        let message = diagnostic("run ID", RandomBudget::Short, error);
        let actual = runtime
            .run(crate::ci::wire::uuid_from(async { Err(message.clone()) }))
            .unwrap_err();
        assert_eq!(actual.code, "internal");
        assert_eq!(actual.message, message);
    }
}

#[test]
fn secure_random_raw_log_failures_leave_no_artifacts() {
    let runtime = kernal_api::async_engine::RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap();
    for error in [
        RandomError::TimedOut,
        RandomError::Unavailable,
        RandomError::WorkerFailed,
        RandomError::AtCapacity,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let random =
            super::secure_random::map_result("secure identifier", RandomBudget::Short, async {
                Err(error)
            });
        let result = runtime.run(super::raw_run_log::create_job_log(
            dir.path(),
            9,
            "task",
            super::ci::wire::uuid_from(random),
        ));
        let message = match result {
            Ok(_) => panic!("random failure created a log"),
            Err(message) => message,
        };
        assert!(message.contains(&diagnostic("secure identifier", RandomBudget::Short, error)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn secure_random_timeout_and_cancellation_keep_queued_worker_capacity() {
    use kernal_api::async_engine;
    use std::{sync::mpsc, time::Duration};
    let runtime = async_engine::RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        for cancel in [false, true] {
            let (started_tx, started_rx) = mpsc::sync_channel(1);
            let (release_tx, release_rx) = mpsc::sync_channel(1);
            let blocked = async_engine::launch_blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let source =
                kernal_api::random::SecureRandom::new(1, Duration::from_millis(150)).unwrap();
            if cancel {
                let mut pending = Box::pin(source.bytes(16));
                assert!(futures_util::poll!(&mut pending).is_pending());
                drop(pending);
            } else {
                let result = super::ci::wire::uuid_from(super::secure_random::map_result(
                    "secure identifier",
                    RandomBudget::Short,
                    source.bytes(16),
                ))
                .await;
                let error = result.unwrap_err();
                assert!(error.message.contains("deadline"));
            }
            assert_eq!(source.bytes(16).await.unwrap_err(), RandomError::AtCapacity);
            release_tx.send(()).unwrap();
            blocked.await.unwrap();
            async_engine::timeout(Duration::from_secs(5), async {
                loop {
                    match source.bytes(16).await {
                        Ok(bytes) => {
                            assert_eq!(bytes.len(), 16);
                            break;
                        }
                        Err(RandomError::AtCapacity) => {
                            async_engine::sleep(Duration::from_millis(10)).await
                        }
                        Err(error) => panic!("unexpected random error: {error}"),
                    }
                }
            })
            .await
            .expect("cancelled queued worker did not release capacity");
        }
    });
}
