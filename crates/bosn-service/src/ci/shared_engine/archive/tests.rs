use super::*;
use crate::ci::{
    cache_cohort::Namespace,
    lifecycle::tests::{FakeBackend, run_id, shared_plan, with_registry},
};
use kernal_api::async_engine::CancellationSource;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

fn route() -> CacheRoute {
    CacheRoute::Cohort {
        namespace: Namespace::parse("0123456789abcdef").unwrap(),
        policy: toml::from_str("repository_max_bytes=100\naggregate_max_bytes=200\nmax_age_secs=3600\nunused_age_secs=1800\nmaintenance_interval_secs=60\n").unwrap(),
    }
}

async fn leased(shared: &SharedEngine) -> Lease {
    shared
        .lease(
            &run_id(508),
            shared_plan(&run_id(509)),
            &CancellationSource::new().token(),
            &mut |_| {},
        )
        .await
        .unwrap()
}

#[test]
fn ready_route_does_not_attempt_an_exclusive_cutover() {
    with_registry(|registry, _| async move {
        let backend = Arc::new(FakeBackend::default());
        backend
            .archive_replies
            .lock()
            .unwrap()
            .get_or_insert_default()
            .push_back((Phase::Require, Ok(Status::Ready)));
        let shared = SharedEngine::new(registry, backend.clone());
        let lease = leased(&shared).await;
        shared
            .prepare_archives(
                &lease,
                &route(),
                &CancellationSource::new().token(),
                &mut |_| {},
            )
            .await
            .unwrap();
        assert_eq!(*backend.archive_phases.lock().unwrap(), [Phase::Require]);
        shared.release(lease).await;
        shared.close().await;
    });
}

#[test]
fn completion_waits_until_fenced_old_writers_are_gone() {
    with_registry(|registry, _| async move {
        let backend = Arc::new(FakeBackend::default());
        backend
            .archive_replies
            .lock()
            .unwrap()
            .get_or_insert_default()
            .extend([
                (Phase::Require, Ok(Status::Refused("not enrolled".into()))),
                (Phase::Fence, Ok(Status::Fenced)),
                (Phase::Complete, Ok(Status::Ready)),
            ]);
        let shared = SharedEngine::new(registry, backend.clone());
        let lease = leased(&shared).await;
        let old = backend.insert("legacy-archive-writer", "sha256:old", BTreeMap::new());
        backend
            .machine
            .busy
            .lock()
            .unwrap()
            .insert(old.engine_id.clone());
        {
            let route = route();
            let cancellation = CancellationSource::new().token();
            let mut note = |_: &str| {};
            let preparing = shared.prepare_archives(&lease, &route, &cancellation, &mut note);
            // The same future continues after observation; no resubmission hides
            // whether Complete crossed the still-busy source boundary.
            let preparing = std::pin::pin!(preparing);
            let mut preparing = preparing;
            assert!(
                async_engine::timeout(Duration::from_millis(100), preparing.as_mut())
                    .await
                    .is_err()
            );
            assert_eq!(
                *backend.archive_phases.lock().unwrap(),
                [Phase::Require, Phase::Fence]
            );
            backend.machine.busy.lock().unwrap().remove(&old.engine_id);
            async_engine::timeout(Duration::from_secs(8), preparing)
                .await
                .unwrap()
                .unwrap();
        }
        assert!(backend.machine.removed.lock().unwrap().contains(&old.name));
        assert!(!backend.engines.lock().unwrap().contains_key(&old.name));
        assert_eq!(
            *backend.archive_phases.lock().unwrap(),
            [Phase::Require, Phase::Fence, Phase::Complete]
        );
        shared.release(lease).await;
        shared.close().await;
    });
}

#[test]
fn unknown_transport_failure_never_falls_back_to_fencing() {
    with_registry(|registry, _| async move {
        let backend = Arc::new(FakeBackend::default());
        backend
            .archive_replies
            .lock()
            .unwrap()
            .get_or_insert_default()
            .push_back((Phase::Require, Err("lost Docker reply".into())));
        let shared = SharedEngine::new(registry, backend.clone());
        let lease = leased(&shared).await;
        let error = shared
            .prepare_archives(
                &lease,
                &route(),
                &CancellationSource::new().token(),
                &mut |_| {},
            )
            .await
            .unwrap_err();
        assert!(error.contains("lost Docker reply"));
        assert_eq!(*backend.archive_phases.lock().unwrap(), [Phase::Require]);
        shared.release(lease).await;
        shared.close().await;
    });
}

#[test]
fn contention_wait_is_async_and_cancellable() {
    with_registry(|registry, _| async move {
        let backend = Arc::new(FakeBackend::default());
        backend
            .archive_replies
            .lock()
            .unwrap()
            .get_or_insert_default()
            .push_back((Phase::Require, Ok(Status::Retry)));
        let shared = SharedEngine::new(registry, backend.clone());
        let lease = leased(&shared).await;
        let stop = CancellationSource::new();
        let cancellation = stop.token();
        // Cancel during the contention wait, once the first Require is seen:
        // a fixed 25 ms delay could cancel before it was sent on a loaded runner.
        let observed = backend.clone();
        let cancel = async_engine::launch(async move {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            while observed.archive_phases.lock().unwrap().is_empty()
                && std::time::Instant::now() < deadline
            {
                async_engine::sleep(Duration::from_millis(5)).await;
            }
            stop.cancel();
        });
        let error = shared
            .prepare_archives(&lease, &route(), &cancellation, &mut |_| {})
            .await
            .unwrap_err();
        assert!(error.contains("cancelled"));
        let _ = cancel.await;
        assert_eq!(*backend.archive_phases.lock().unwrap(), [Phase::Require]);
        shared.release(lease).await;
        shared.close().await;
    });
}

/// #735: contention that never clears ends the run with an actionable error instead of
/// holding its admission slot forever. RED before the bound: this test never returned.
#[test]
fn endless_contention_ends_the_run_with_an_actionable_error() {
    with_registry(|registry, _| async move {
        let backend = Arc::new(FakeBackend::default());
        backend
            .archive_replies
            .lock()
            .unwrap()
            .get_or_insert_default()
            .extend(std::iter::repeat_with(|| (Phase::Require, Ok(Status::Retry))).take(64));
        let shared = SharedEngine::new(registry, backend.clone());
        let lease = leased(&shared).await;
        let mut notes = Vec::new();
        let started = std::time::Instant::now();
        let error = shared
            .prepare_archives(
                &lease,
                &route(),
                &CancellationSource::new().token(),
                &mut |note| notes.push(note.to_owned()),
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("still contended") && error.contains("retry the run"),
            "{error}"
        );
        assert!(started.elapsed() >= ENROLLMENT_WAIT);
        assert_eq!(notes, ["waiting for archive enrollment coordination"]);
        shared.release(lease).await;
        shared.close().await;
    });
}
