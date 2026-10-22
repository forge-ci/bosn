//! Retention must preserve unfinished recovery and references, including legacy records.
use super::*;
use bosn_registry::cache_helper::{CacheHelperIntent, CacheHelperRole};

const OLD: f64 = 8.0 * 86400.0;

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
