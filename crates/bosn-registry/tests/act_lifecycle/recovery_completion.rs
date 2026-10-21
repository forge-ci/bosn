use super::*;

#[test]
fn publication_and_reference_release_are_durable_ordered_and_idempotent() {
    let (dir, mut registry, intent, frozen) = stopped_recovery_fixture();
    let proof = ActToolRecoveryPublicationProof {
        intent: frozen.clone(),
        generation: "c".repeat(64),
    };
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .release_act_tool_recovery(RUN, &frozen, 8.0)
            .is_err()
    );
    {
        let mut tx = registry.begin_immediate().unwrap();
        tx.complete_act_tool_recovery(RUN, &proof, 8.0).unwrap();
        // A lost/rolled-back transaction cannot authorize reference release.
    }
    assert!(
        registry
            .act_engine(RUN)
            .unwrap()
            .unwrap()
            .tool_recovery_completion
            .is_none()
    );
    let mut tx = registry.begin_immediate().unwrap();
    tx.complete_act_tool_recovery(RUN, &proof, 8.0).unwrap();
    tx.commit().unwrap();
    let completed = registry.act_engine(RUN).unwrap().unwrap();
    assert!(completed.retains_tool_recovery_source(9.0));
    assert!(completed.retains_tool_recovery_source(f64::NAN));
    drop(registry);
    let mut registry = Registry::open_writer(dir.path().join("registry.sqlite3")).unwrap();
    assert_eq!(registry.act_engine(RUN).unwrap().unwrap(), completed);
    let mut tx = registry.begin_immediate().unwrap();
    tx.complete_act_tool_recovery(RUN, &proof, 9.0).unwrap();
    tx.commit().unwrap();
    assert_eq!(registry.act_engine(RUN).unwrap().unwrap(), completed);
    {
        let mut tx = registry.begin_immediate().unwrap();
        tx.release_act_tool_recovery(RUN, &frozen, 9.0).unwrap();
    }
    assert_eq!(registry.act_engine(RUN).unwrap().unwrap(), completed);
    let mut tx = registry.begin_immediate().unwrap();
    tx.release_act_tool_recovery(RUN, &frozen, 10.0).unwrap();
    tx.commit().unwrap();
    let released = registry.act_engine(RUN).unwrap().unwrap();
    assert!(!released.retains_tool_recovery_source(11.0));
    let mut tx = registry.begin_immediate().unwrap();
    tx.release_act_tool_recovery(RUN, &frozen, 11.0).unwrap();
    tx.complete_act_tool_recovery(RUN, &proof, 4000.0).unwrap();
    tx.commit().unwrap();
    assert_eq!(registry.act_engine(RUN).unwrap().unwrap(), released);
    let removal = ActEngineRemovalProof {
        name: intent.engine_name(),
        engine_id: Some(frozen.engine_id),
        storage_volume: intent.storage_volume_name(),
    };
    let mut tx = registry.begin_immediate().unwrap();
    tx.finalize_act_cleanup(RUN, &removal, 11.0).unwrap();
    tx.commit().unwrap();
    drop(registry);
    let registry = Registry::open_writer(dir.path().join("registry.sqlite3")).unwrap();
    assert_eq!(
        registry.act_engine(RUN).unwrap().unwrap().state,
        ActEngineState::Terminal
    );
}

#[test]
fn publication_refuses_changed_identity_time_and_generation() {
    let (_dir, mut registry, _intent, frozen) = stopped_recovery_fixture();
    let proof = ActToolRecoveryPublicationProof {
        intent: frozen.clone(),
        generation: "c".repeat(64),
    };
    for at in [6.0, 3604.0, f64::NAN, f64::INFINITY] {
        assert!(
            registry
                .begin_immediate()
                .unwrap()
                .complete_act_tool_recovery(RUN, &proof, at)
                .is_err()
        );
    }
    for change in [
        "owner",
        "generation",
        "source",
        "lifetime",
        "published-generation",
    ] {
        let mut changed = proof.clone();
        match change {
            "owner" => changed.intent.owner = "f".repeat(64),
            "generation" => changed.intent.generation = "f".repeat(64),
            "source" => changed.intent.source_volume = "foreign".into(),
            "lifetime" => changed.intent.expires_at_seconds += 1,
            _ => changed.generation = "invalid".into(),
        }
        assert!(
            registry
                .begin_immediate()
                .unwrap()
                .complete_act_tool_recovery(RUN, &changed, 8.0)
                .is_err(),
            "{change}"
        );
    }
    let mut tx = registry.begin_immediate().unwrap();
    tx.complete_act_tool_recovery(RUN, &proof, 8.0).unwrap();
    tx.commit().unwrap();
    let mut changed = proof;
    changed.generation = "d".repeat(64);
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .complete_act_tool_recovery(RUN, &changed, 9.0)
            .is_err()
    );
    let mut foreign = frozen.clone();
    foreign.owner = "f".repeat(64);
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .release_act_tool_recovery(RUN, &foreign, 9.0)
            .is_err()
    );
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .release_act_tool_recovery(RUN, &frozen, 7.0)
            .is_err()
    );
    assert!(
        registry
            .act_engine(RUN)
            .unwrap()
            .unwrap()
            .retains_tool_recovery_source(9.0)
    );
}

#[test]
fn incomplete_or_invalid_completion_snapshots_refuse_to_reopen() {
    for change in [
        "missing-stop",
        "invalid-generation",
        "invalid-time",
        "early-release",
        "late-release",
        "expired-publication",
    ] {
        let (_dir, mut registry, _intent, frozen) = stopped_recovery_fixture();
        let proof = ActToolRecoveryPublicationProof {
            intent: frozen,
            generation: "c".repeat(64),
        };
        let mut tx = registry.begin_immediate().unwrap();
        tx.complete_act_tool_recovery(RUN, &proof, 8.0).unwrap();
        tx.commit().unwrap();
        let mut snapshot =
            serde_json::to_value(registry.act_engine(RUN).unwrap().unwrap()).unwrap();
        match change {
            "missing-stop" => snapshot["tool_recovery_source_stopped_at"] = serde_json::Value::Null,
            "invalid-generation" => {
                snapshot["tool_recovery_completion"]["generation"] = "bad".into()
            }
            "invalid-time" => snapshot["tool_recovery_completion"]["completed_at"] = (-1).into(),
            "early-release" => {
                snapshot["tool_recovery_completion"]["reference_released_at"] = 7.into()
            }
            "late-release" => {
                snapshot["tool_recovery_completion"]["reference_released_at"] = 10.into()
            }
            _ => snapshot["tool_recovery_completion"]["completed_at"] = 3604.into(),
        }
        let mut tx = registry.begin_immediate().unwrap();
        tx.append_event(9.0, &format!("act.engine.v1:{RUN}"), &snapshot.to_string())
            .unwrap();
        tx.commit().unwrap();
        assert!(registry.act_engine(RUN).is_err(), "{change}");
    }
}
