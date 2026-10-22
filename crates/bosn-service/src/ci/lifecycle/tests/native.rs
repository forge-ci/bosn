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
