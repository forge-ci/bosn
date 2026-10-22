//! Native engines must protect recovery before ordinary shared admission.
use super::*;
use crate::ci::{shared_engine::SharedEngine, spare::SparePlan};
use bosn_registry::act::ActToolGenerationBinding;

fn native_plan() -> SparePlan {
    let mut plan = shared_plan(&run_id(0x5080));
    plan.intent.created_at = now_seconds();
    let profile = crate::act_engine::creation_profile_with_tools(
        crate::ci::limits::size_engine(FAKE_HOST, Default::default()).unwrap(),
        Some(plan.cache.mount()),
        Some(ActToolGenerationBinding {
            id: "a".repeat(64),
            max_payload_bytes: 64 << 30,
            overlay_recipe_sha256: crate::ci::engine::tool_overlay_recipe_sha256(),
        }),
    )
    .unwrap();
    plan.intent.creation_profile = Some(profile);
    plan
}

#[test]
fn ordinary_template_selects_native_tools_before_first_admission() {
    with_registry(|registry, _dir| async move {
        let backend = Arc::new(FakeBackend::with(Faults::default()));
        let shared = SharedEngine::new(registry.clone(), backend.clone());
        let mut plan = shared_plan(&run_id(0x5083));
        plan.intent.created_at = now_seconds();
        plan.native_tools = Some(64 << 30);
        let lease = shared
            .lease(
                &run_id(0x5084),
                plan.clone(),
                &CancellationSource::new().token(),
                &mut |_: &str| {},
            )
            .await
            .unwrap();
        assert!(
            lease.tool_generation.is_some(),
            "ordinary native policy was admitted as a legacy engine"
        );
        assert_eq!(
            backend.live(),
            1,
            "preparation engine retired before admission"
        );
        let again = shared
            .lease(
                &run_id(0x5085),
                plan,
                &CancellationSource::new().token(),
                &mut |_| {},
            )
            .await
            .unwrap();
        assert_eq!(
            again.engine_id, lease.engine_id,
            "native policy reuses its frozen generation"
        );
        assert_eq!(
            *backend.engine_preparations.lock().unwrap(),
            2,
            "only preparation and actual engines were made"
        );
        shared.release(again).await;
        shared.release(lease).await;
        shared.close().await;
    });
}

#[test]
fn first_native_slot_requires_durable_recovery_reservation() {
    with_registry(|registry, _dir| async move {
        let backend = Arc::new(FakeBackend::with(Faults::default()));
        let shared = SharedEngine::new(registry.clone(), backend);
        let plan = native_plan();
        let run = plan.intent.run_id.clone();
        let lease = shared
            .lease(
                &run_id(0x5081),
                plan,
                &CancellationSource::new().token(),
                &mut |_: &str| {},
            )
            .await
            .unwrap();
        let record = match registry
            .act_registry(ActRegistryCommand::Get { run })
            .await
            .unwrap()
        {
            ActRegistryReply::Record(Some(record)) => record,
            reply => panic!("engine record missing: {reply:?}"),
        };
        assert!(
            record.tool_recovery.is_some() && record.tool_recovery_reserved_at.is_some(),
            "first native slot was admitted without durable lower protection"
        );
        shared.release(lease).await;
        shared.close().await;
    });
}

#[test]
fn uncertain_native_reservation_never_opens_a_slot() {
    with_registry(|registry, _dir| async move {
        let backend = Arc::new(FakeBackend::with(Faults {
            reserve: true,
            ..Default::default()
        }));
        let shared = SharedEngine::new(registry.clone(), backend.clone());
        let plan = native_plan();
        let run = plan.intent.run_id.clone();
        let error = shared
            .lease(
                &run_id(0x5082),
                plan,
                &CancellationSource::new().token(),
                &mut |_: &str| {},
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("synthetic native reservation failure"),
            "{error}"
        );
        assert!(
            backend
                .machine
                .tables
                .lock()
                .unwrap()
                .values()
                .all(|table| { !table.ready && table.held.is_empty() })
        );
        let record = match registry
            .act_registry(ActRegistryCommand::Get { run })
            .await
            .unwrap()
        {
            ActRegistryReply::Record(Some(record)) => record,
            reply => panic!("engine record missing: {reply:?}"),
        };
        assert!(record.tool_recovery.is_some());
        assert!(record.tool_recovery_reserved_at.is_none());
    });
}

#[test]
fn ordinary_native_policy_refuses_memory_before_source_mutation() {
    with_registry(|registry, _dir| async move {
        let backend = Arc::new(FakeBackend::with(Faults::default()));
        let shared = SharedEngine::new(registry.clone(), backend.clone());
        let mut plan = shared_plan(&run_id(0x5086));
        let limits = crate::ci::limits::size_engine(
            FAKE_HOST,
            crate::ci::limits::EngineConfig {
                storage: Some(crate::ci::limits::StorageBacking::Memory),
                ..Default::default()
            },
        )
        .unwrap();
        plan.intent.creation_profile = Some(
            crate::act_engine::creation_profile_with_cache(limits, Some(plan.cache.mount()))
                .unwrap(),
        );
        plan.native_tools = Some(64 << 30);
        let error = shared
            .lease(
                &run_id(0x5087),
                plan,
                &CancellationSource::new().token(),
                &mut |_| {},
            )
            .await
            .unwrap_err();
        assert!(error.contains("disk-backed"), "{error}");
        assert_eq!(backend.live(), 0);
        assert_eq!(*backend.engine_preparations.lock().unwrap(), 0);
        assert!(backend.machine.claim.lock().unwrap().is_none());
    });
}

#[test]
fn cancellation_during_either_enrollment_phase_never_opens_a_slot() {
    use crate::ci::engine::EnrollmentPhase;
    for phase in [EnrollmentPhase::Fence, EnrollmentPhase::Complete] {
        with_registry(|registry, _dir| async move {
            let backend = Arc::new(FakeBackend::with(Faults {
                enrollment_phase: Some(phase),
                enrollment_delay: Duration::from_secs(60),
                remove: true,
                ..Faults::default()
            }));
            let shared = SharedEngine::new(registry.clone(), backend.clone());
            let mut plan = shared_plan(&run_id(0x5088));
            plan.intent.created_at = now_seconds();
            plan.native_tools = Some(64 << 30);
            let source = CancellationSource::new();
            let token = source.token();
            let cancel = async {
                while !backend.enrollment_phases.lock().unwrap().contains(&phase) {
                    async_engine::sleep(Duration::from_millis(5)).await;
                }
                source.cancel();
            };
            let mut note = |_: &str| {};
            let run = run_id(0x5089);
            let lease = shared.lease(&run, plan, &token, &mut note);
            let (result, ()) =
                async_engine::timeout(Duration::from_secs(2), async_engine::join(lease, cancel))
                    .await
                    .expect("cancellation must interrupt enrollment promptly");
            assert!(result.unwrap_err().contains("native enrollment cancelled"));
            async_engine::sleep(Duration::from_millis(100)).await;
            assert_eq!(
                backend.live(),
                1,
                "failed removal must retain preparation engine"
            );
            assert!(
                backend.machine.claim.lock().unwrap().is_some(),
                "surviving producer stays fenced by claim"
            );
            assert!(
                backend
                    .machine
                    .tables
                    .lock()
                    .unwrap()
                    .values()
                    .all(|table| !table.ready && table.held.is_empty())
            );
            let mut next = shared_plan(&run_id(0x5090));
            next.native_tools = Some(64 << 30);
            let next_cancel = CancellationSource::new().token();
            assert!(
                async_engine::timeout(
                    Duration::from_millis(100),
                    shared.lease(&run_id(0x5091), next, &next_cancel, &mut |_: &str| {},)
                )
                .await
                .is_err(),
                "next request waits for durable cleanup instead of failing or admitting"
            );
            *backend.faults.lock().unwrap() = Faults::default();
            shared.tend_machine(Duration::ZERO).await;
            assert_eq!(
                backend.live(),
                0,
                "reaper must retry interrupted preparation cleanup"
            );
            assert!(
                backend.machine.claim.lock().unwrap().is_none(),
                "reaper must release claim after cleanup succeeds"
            );
        });
    }
}
