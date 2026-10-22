//! Exercise the ordinary runtime planner, not a hand-built native policy.
use super::*;
use crate::ci::lifecycle::tests::{FakeBackend, with_registry};

#[test]
fn ordinary_runtime_plan_selects_native_enrollment_before_admission() {
    with_registry(|registry, dir| async move {
        let backend = Arc::new(FakeBackend::default());
        let runtime = CiRuntime::start(&dir, registry, backend.clone(), 1);
        let record = crate::ci::tests::sample_record("50800000-0000-4000-8000-000000000001");
        let planned = runtime
            .plan(
                &record,
                async_engine::Deadline::after(Duration::from_secs(30)),
            )
            .await
            .unwrap();
        assert!(
            planned.shared.native_tools.is_some(),
            "ordinary execution still bypasses native enrollment and recovery"
        );
        assert!(planned.shared.native_tools.unwrap() <= i64::MAX as u64);
        planned.shared.validate_native_policy().unwrap();
        assert_eq!(backend.live(), 0, "planning must not create an engine");
        assert!(backend.enrollment_phases.lock().unwrap().is_empty());
    });
}

#[test]
fn memory_backed_native_execution_refuses_before_creating_or_fencing() {
    with_registry(|registry, dir| async move {
        std::fs::write(dir.join("config.toml"), "[engine]\nstorage = 'memory'\n").unwrap();
        let backend = Arc::new(FakeBackend::default());
        let runtime = CiRuntime::start(&dir, registry, backend.clone(), 1);
        let record = crate::ci::tests::sample_record("50800000-0000-4000-8000-000000000002");
        let error = match runtime
            .plan(
                &record,
                async_engine::Deadline::after(Duration::from_secs(30)),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("memory-backed execution cannot bypass durable native recovery"),
        };
        assert!(error.contains("disk-backed engine storage"), "{error}");
        assert_eq!(backend.live(), 0);
        assert!(backend.enrollment_phases.lock().unwrap().is_empty());
        assert!(backend.machine.claim.lock().unwrap().is_none());
    });
}

#[test]
fn ordinary_runtime_plan_requests_coordinated_archive_routing() {
    with_registry(|registry, dir| async move {
        let backend = Arc::new(FakeBackend::default());
        let runtime = CiRuntime::start(&dir, registry, backend.clone(), 1);
        let record = crate::ci::tests::sample_record("50800000-0000-4000-8000-000000000003");
        let planned = runtime
            .plan(
                &record,
                async_engine::Deadline::after(Duration::from_secs(30)),
            )
            .await
            .unwrap();
        assert!(
            matches!(
                planned.invocation.cache_route,
                crate::ci::cache_cohort::CacheRoute::Cohort { .. }
            ),
            "ordinary execution still bypasses coordinated archive migration and routing"
        );
        assert_eq!(backend.live(), 0, "planning must not mutate cache state");
        let crate::ci::cache_cohort::CacheRoute::Cohort { policy, .. } =
            planned.invocation.cache_route
        else {
            unreachable!()
        };
        assert_eq!(policy, crate::ci::cache_policy::CachePolicy::default());
        assert!(backend.archive_phases.lock().unwrap().is_empty());
    });
}

#[test]
fn configured_archive_policy_reaches_the_plan_without_enrollment_side_effects() {
    with_registry(|registry, dir| async move {
        let policy = "repository_max_bytes=100\naggregate_max_bytes=200\nmax_age_secs=3600\nunused_age_secs=1800\nmaintenance_interval_secs=60\n";
        std::fs::write(dir.join("config.toml"), format!("[cache]\n{policy}")).unwrap();
        let backend = Arc::new(FakeBackend::default());
        let runtime = CiRuntime::start(&dir, registry, backend.clone(), 1);
        let record = crate::ci::tests::sample_record("50800000-0000-4000-8000-000000000004");
        let planned = runtime
            .plan(
                &record,
                async_engine::Deadline::after(Duration::from_secs(30)),
            )
            .await
            .unwrap();
        let crate::ci::cache_cohort::CacheRoute::Cohort { policy: actual, .. } =
            planned.invocation.cache_route
        else {
            panic!("configured retention must use the coordinated route")
        };
        assert_eq!(actual, toml::from_str(policy).unwrap());
        assert_eq!(backend.live(), 0);
        assert!(backend.archive_phases.lock().unwrap().is_empty());
    });
}
