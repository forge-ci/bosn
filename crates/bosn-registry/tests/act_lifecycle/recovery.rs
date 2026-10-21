use super::*;

#[test]
fn tool_recovery_intent_survives_restart_and_refuses_changed_claim_or_lifetime() {
    let (dir, mut registry, i) = recovery_fixture();
    let path = dir.path().join("registry.sqlite3");
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .begin_act_tool_recovery(RUN, "wrong", 4, 3604)
            .is_err()
    );
    {
        let mut tx = registry.begin_immediate().unwrap();
        tx.begin_act_tool_recovery(RUN, CLAIM, 4, 3604).unwrap();
        // No commit: uncertain external reservations must never rely on this.
    }
    assert!(
        registry
            .act_engine(RUN)
            .unwrap()
            .unwrap()
            .tool_recovery
            .is_none()
    );
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .begin_act_tool_recovery(RUN, CLAIM, 4, 86405)
            .is_err()
    );
    {
        let mut tx = registry.begin_immediate().unwrap();
        tx.begin_act_tool_recovery(RUN, CLAIM, 4, 3604).unwrap();
        tx.commit().unwrap();
    }
    drop(registry);
    let mut registry = Registry::open_writer(&path).unwrap();
    let record = registry.act_engine(RUN).unwrap().unwrap();
    let frozen = record.tool_recovery.unwrap();
    assert_eq!(frozen.source_volume, i.storage_volume_name().unwrap());
    assert_eq!(frozen.engine_id, observed(&i).engine_id);
    assert_eq!(frozen.generation, "a".repeat(64));
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_act_tool_recovery(RUN, CLAIM, 4, 3604).unwrap();
    tx.commit().unwrap();
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .begin_act_tool_recovery(RUN, CLAIM, 4, 3605)
            .is_err()
    );
    let mut tx = registry.begin_immediate().unwrap();
    tx.acknowledge_act_tool_recovery(RUN, CLAIM, &frozen, 5.0)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        registry
            .act_engine(RUN)
            .unwrap()
            .unwrap()
            .tool_recovery_reserved_at,
        Some(5.0)
    );
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .acknowledge_act_tool_recovery(RUN, CLAIM, &frozen, 3604.0)
            .is_err()
    );
    assert_eq!(
        registry.act_engine(RUN).unwrap().unwrap().tool_recovery,
        Some(frozen)
    );
}

fn recovery_fixture() -> (TemporaryDirectory, Registry, ActEngineIntent) {
    let dir = TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    let mut i = intent();
    let profile = i.creation_profile.as_mut().unwrap();
    profile.tmpfs_policy = ActEngineTmpfsPolicy::NamedDiskStorageRunTmpNoexecV2;
    profile.cache_volume = Some(ActEngineCacheVolume {
        name: "bosn-ci-cache-v1".into(),
        target: "/bosn/cache".into(),
    });
    profile.cache_coordination = Some(ActCacheCoordination::SharedLegacyLeaseV1);
    profile.tool_generation = Some(ActToolGenerationBinding {
        id: "a".repeat(64),
        max_payload_bytes: 100,
        overlay_recipe_sha256: "b".repeat(64),
    });
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_act_engine(&i).unwrap();
    tx.register_act_engine(RUN, &observed(&i), 2.0).unwrap();
    tx.claim_act_execution(&i, &observed(&i), CLAIM, 4.25)
        .unwrap();
    tx.commit().unwrap();
    (dir, registry, i)
}

#[test]
fn tool_recovery_source_stop_requires_cleanup_and_exact_identity() {
    let (_dir, mut registry, i) = recovery_fixture();
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_act_tool_recovery(RUN, CLAIM, 4, 3604).unwrap();
    tx.commit().unwrap();
    let frozen = registry
        .act_engine(RUN)
        .unwrap()
        .unwrap()
        .tool_recovery
        .unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.acknowledge_act_tool_recovery(RUN, CLAIM, &frozen, 5.0)
        .unwrap();
    tx.commit().unwrap();
    let proof = ActToolSourceStopProof {
        engine_name: i.engine_name(),
        engine_id: frozen.engine_id.clone(),
        source_volume: frozen.source_volume.clone(),
    };
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .record_act_tool_source_stopped(RUN, &proof, 6.0)
            .is_err()
    );
    let mut tx = registry.begin_immediate().unwrap();
    tx.request_act_execution_cleanup(RUN, CLAIM, ActRunOutcome::Failed, 5.5)
        .unwrap();
    tx.commit().unwrap();
    let mut wrong = proof.clone();
    wrong.engine_id = "f".repeat(64);
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .record_act_tool_source_stopped(RUN, &wrong, 6.0)
            .is_err()
    );
    let mut tx = registry.begin_immediate().unwrap();
    tx.record_act_tool_source_stopped(RUN, &proof, 6.0).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        registry
            .act_engine(RUN)
            .unwrap()
            .unwrap()
            .tool_recovery_source_stopped_at,
        Some(6.0)
    );
}

#[test]
fn live_recovery_reference_prevents_terminal_cleanup() {
    let (_dir, mut registry, i, frozen) = stopped_recovery_fixture();
    let removal = ActEngineRemovalProof {
        name: i.engine_name(),
        engine_id: Some(frozen.engine_id),
        storage_volume: i.storage_volume_name(),
    };
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .finalize_act_cleanup(RUN, &removal, 8.0)
            .is_err(),
        "a live unreleased recovery must not be finalized by another cleanup caller"
    );
    let mut tx = registry.begin_immediate().unwrap();
    tx.finalize_act_cleanup(RUN, &removal, 3604.0).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        registry.act_engine(RUN).unwrap().unwrap().state,
        ActEngineState::Terminal
    );
}

fn stopped_recovery_fixture() -> (
    TemporaryDirectory,
    Registry,
    ActEngineIntent,
    ActToolRecoveryIntent,
) {
    let (dir, mut registry, i) = recovery_fixture();
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_act_tool_recovery(RUN, CLAIM, 4, 3604).unwrap();
    tx.commit().unwrap();
    let frozen = registry
        .act_engine(RUN)
        .unwrap()
        .unwrap()
        .tool_recovery
        .unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.acknowledge_act_tool_recovery(RUN, CLAIM, &frozen, 5.0)
        .unwrap();
    tx.request_act_execution_cleanup(RUN, CLAIM, ActRunOutcome::Interrupted, 6.0)
        .unwrap();
    tx.record_act_tool_source_stopped(
        RUN,
        &ActToolSourceStopProof {
            engine_name: i.engine_name(),
            engine_id: frozen.engine_id.clone(),
            source_volume: frozen.source_volume.clone(),
        },
        7.0,
    )
    .unwrap();
    tx.commit().unwrap();
    (dir, registry, i, frozen)
}

#[path = "recovery_completion.rs"]
mod completion;
