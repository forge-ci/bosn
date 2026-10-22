//! Retention must preserve unfinished recovery and references, including legacy records.
use super::*;
use bosn_registry::cache_helper::{CacheHelperIntent, CacheHelperRole};

const OLD: f64 = 8.0 * 86400.0;

#[test]
fn full_ordinary_capacity_still_allows_more_than_eight_completed_recovery_attempts() {
    let (_dir, mut registry, intent, frozen) = recovery::stopped_recovery_fixture();
    let mut helper = CacheHelperIntent {
        registry_id: OWNER.into(),
        nonce: CLAIM.into(),
        volume: frozen.cache_volume.clone(),
        image: format!("docker.io/library/docker@{}", intent.engine_image_digest),
        created_at: 8.0,
        role: None,
    };
    let mut tx = registry.begin_immediate().unwrap();
    for index in 0..9999 {
        helper.nonce = format!("{index:08x}-1111-4111-8111-111111111111");
        tx.begin_cache_helper(&helper).unwrap();
    }
    tx.commit().unwrap();
    let mut first = None;
    for index in 0..12 {
        helper.nonce = format!("{index:08x}-2222-4222-8222-222222222222");
        helper.created_at = 10.0 + 3.0 * f64::from(index);
        helper.role = Some(CacheHelperRole::ToolRecoveryV1 {
            run_id: RUN.into(),
            intent: frozen.clone(),
            command_sha256: Some("d".repeat(64)),
            source_mountpoint: None,
        });
        first.get_or_insert_with(|| helper.clone());
        let mut tx = registry.begin_immediate().unwrap();
        tx.begin_cache_helper(&helper).unwrap();
        tx.register_cache_helper(&helper.nonce, &"b".repeat(64), helper.created_at + 1.0)
            .unwrap();
        tx.finish_cache_helper(&helper.nonce, &"b".repeat(64), helper.created_at + 2.0)
            .unwrap();
        tx.commit().unwrap();
    }
    let first = first.unwrap();
    assert!(registry.cache_helper(&first.nonce).unwrap().is_none());
    let mut tx = registry.begin_immediate().unwrap();
    assert!(
        tx.begin_cache_helper(&first).is_err(),
        "recycled nonce replayed"
    );
    tx.commit().unwrap();
    assert!(registry.act_engine(RUN).unwrap().is_some());
}

#[test]
fn recovery_reserve_never_recycles_uncertain_helpers_or_accepts_a_future_clock() {
    let (_dir, mut registry, intent, frozen) = recovery::stopped_recovery_fixture();
    let mut tx = registry.begin_immediate().unwrap();
    tx.claim_act_tool_source_disposal(RUN, 3604.0).unwrap();
    let mut helper = CacheHelperIntent {
        registry_id: OWNER.into(),
        nonce: CLAIM.into(),
        volume: frozen.cache_volume.clone(),
        image: format!("docker.io/library/docker@{}", intent.engine_image_digest),
        created_at: 1e12,
        role: Some(CacheHelperRole::ToolRecoveryDisposalV1 {
            run_id: RUN.into(),
            intent: frozen,
            command_sha256: "d".repeat(64),
        }),
    };
    assert!(tx.begin_cache_helper(&helper).is_err());
    for index in 0..8 {
        helper.nonce = format!("{index:08x}-2222-4222-8222-222222222222");
        helper.created_at = 3605.0 + f64::from(index);
        tx.begin_cache_helper(&helper).unwrap();
    }
    helper.nonce = "ffffffff-2222-4222-8222-222222222222".into();
    helper.created_at = 3620.0;
    assert!(tx.begin_cache_helper(&helper).is_err());
    tx.commit().unwrap();
    assert_eq!(
        registry
            .pending_cache_helpers(None, 64)
            .unwrap()
            .items
            .len(),
        8
    );
    assert!(registry.meta("ci.retention.floor").unwrap().is_none());
    assert!(registry.act_engine(RUN).unwrap().is_some());
}

fn removal(intent: &ActEngineIntent, frozen: &ActToolRecoveryIntent) -> ActEngineRemovalProof {
    ActEngineRemovalProof {
        name: intent.engine_name(),
        engine_id: Some(frozen.engine_id.clone()),
        storage_volume: intent.storage_volume_name(),
    }
}

#[test]
fn expired_abandonment_metadata_expires_only_after_exact_terminal_proof() {
    for disposal_marker in [false, true] {
        let (_dir, mut registry, intent, frozen) = recovery::stopped_recovery_fixture();
        assert_eq!(registry.maintain_ci_state(OLD, 64).unwrap(), 0);
        assert!(
            registry.act_engine(RUN).unwrap().is_some(),
            "unfinished source was discarded"
        );
        let mut tx = registry.begin_immediate().unwrap();
        if disposal_marker {
            tx.claim_act_tool_source_disposal(RUN, 3604.0).unwrap();
        }
        tx.finalize_act_cleanup(RUN, &removal(&intent, &frozen), 3604.0)
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            registry.maintain_ci_state(OLD, 64).unwrap(),
            1,
            "disposed legacy source was retained forever"
        );
        assert!(registry.act_engine(RUN).unwrap().is_none());
    }
}

#[test]
fn terminal_unreleased_reference_survives_expiry_until_release_acknowledgement() {
    let (_dir, mut registry, intent, frozen) = recovery::stopped_recovery_fixture();
    let mut tx = registry.begin_immediate().unwrap();
    tx.complete_act_tool_recovery(
        RUN,
        &ActToolRecoveryPublicationProof {
            intent: frozen.clone(),
            generation: "c".repeat(64),
        },
        8.0,
    )
    .unwrap();
    tx.finalize_act_cleanup(RUN, &removal(&intent, &frozen), 3604.0)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(registry.maintain_ci_state(OLD, 64).unwrap(), 0);
    assert_eq!(
        registry
            .pending_tool_recoveries(None, 1)
            .unwrap()
            .items
            .len(),
        1
    );
    let mut tx = registry.begin_immediate().unwrap();
    tx.release_act_tool_recovery(RUN, &frozen, OLD + 1.0)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(registry.maintain_ci_state(2.0 * OLD, 64).unwrap(), 1);
}

#[test]
fn pending_source_helper_preserves_terminal_parent_until_helper_absence() {
    let (_dir, mut registry, intent, frozen) = recovery::stopped_recovery_fixture();
    let context =
        ActToolSourceContext::from_record(&registry.act_engine(RUN).unwrap().unwrap()).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.claim_act_tool_source_disposal(RUN, 3604.0).unwrap();
    tx.finalize_act_cleanup(RUN, &removal(&intent, &frozen), 3604.0)
        .unwrap();
    let helper = CacheHelperIntent {
        registry_id: OWNER.into(),
        nonce: CLAIM.into(),
        volume: frozen.cache_volume.clone(),
        image: format!("docker.io/library/docker@{}", intent.engine_image_digest),
        created_at: 3605.0,
        role: Some(CacheHelperRole::ToolSourceFinalizationV1 {
            context: Box::new(context),
            context_id: "d".repeat(64),
            command_sha256: "e".repeat(64),
            source_mountpoint: None,
        }),
    };
    tx.begin_cache_helper(&helper).unwrap();
    let mut finished = helper.clone();
    finished.nonce = "ffffffff-bbbb-4ccc-8ddd-eeeeeeeeeeee".into();
    finished.created_at = 3606.0;
    finished.role = None;
    tx.begin_cache_helper(&finished).unwrap();
    tx.finish_absent_cache_helper(&finished.nonce, 5000.0)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(registry.maintain_ci_state(OLD, 1).unwrap(), 0);
    assert!(registry.act_engine(RUN).unwrap().is_some());
    assert_eq!(registry.maintain_ci_state(OLD, 1).unwrap(), 1);
    assert!(registry.cache_helper(&finished.nonce).unwrap().is_none());
    assert_eq!(registry.maintain_ci_state(OLD, 1).unwrap(), 0);
    let mut tx = registry.begin_immediate().unwrap();
    tx.finish_absent_cache_helper(&helper.nonce, OLD).unwrap();
    tx.commit().unwrap();
    assert_eq!(registry.maintain_ci_state(OLD + 1.0, 64).unwrap(), 1);
    assert!(
        registry.cache_helper(&helper.nonce).unwrap().is_some(),
        "recent helper history must remain"
    );
}
