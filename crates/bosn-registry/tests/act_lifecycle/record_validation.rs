//! Legacy compatibility and eager rejection of invalid record versions.
use super::*;

#[test]
fn legacy_registered_snapshot_is_recoverable_but_cannot_execute() {
    let dir = TemporaryDirectory::new().unwrap();
    let mut registry = Registry::create_writer(dir.path().join("legacy"), OWNER).unwrap();
    {
        let mut tx = registry.begin_immediate().unwrap();
        tx.begin_act_engine(&intent()).unwrap();
        tx.register_act_engine(RUN, &observed(&intent()), 2.0)
            .unwrap();
        tx.commit().unwrap();
    }
    let mut old = serde_json::to_value(registry.act_engine(RUN).unwrap().unwrap()).unwrap();
    old["schema_version"] = serde_json::json!(1);
    old["intent"]
        .as_object_mut()
        .unwrap()
        .remove("creation_profile");
    old.as_object_mut().unwrap().remove("execution_claim");
    {
        let mut tx = registry.begin_immediate().unwrap();
        tx.append_event(
            2.0,
            &format!("act.engine.v1:{RUN}"),
            &serde_json::to_string(&old).unwrap(),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        registry.pending_act_engines(None, 16).unwrap().items.len(),
        1
    );
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .claim_act_execution(&intent(), &observed(&intent()), CLAIM, 3.0)
            .is_err()
    );
    {
        let mut tx = registry.begin_immediate().unwrap();
        tx.request_act_cleanup(RUN, ActRunOutcome::Interrupted, 3.0)
            .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        registry.act_engine(RUN).unwrap().unwrap().state,
        ActEngineState::CleanupRequired
    );
}

#[test]
fn legacy_v2_claim_cannot_resume_or_complete_but_owner_can_request_cleanup() {
    let dir = TemporaryDirectory::new().unwrap();
    let path = dir.path().join("legacy-v2.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_act_engine(&intent()).unwrap();
    tx.register_act_engine(RUN, &observed(&intent()), 2.0)
        .unwrap();
    tx.claim_act_execution(&intent(), &observed(&intent()), CLAIM, 3.0)
        .unwrap();
    tx.commit().unwrap();
    let mut old = serde_json::to_value(registry.act_engine(RUN).unwrap().unwrap()).unwrap();
    old["schema_version"] = serde_json::json!(2);
    old["intent"]
        .as_object_mut()
        .unwrap()
        .remove("creation_profile");
    let mut tx = registry.begin_immediate().unwrap();
    tx.append_event(
        3.0,
        &format!("act.engine.v1:{RUN}"),
        &serde_json::to_string(&old).unwrap(),
    )
    .unwrap();
    tx.commit().unwrap();
    drop(registry);
    let mut registry = Registry::open_writer(&path).unwrap();
    let old = registry.act_engine(RUN).unwrap().unwrap();
    assert_eq!(old.intent.creation_profile, None);
    let old_observed = observed(&old.intent);
    let mut unclaimed = serde_json::to_value(&old).unwrap();
    unclaimed["execution_claim"] = serde_json::json!(null);
    let mut tx = registry.begin_immediate().unwrap();
    tx.append_event(
        3.0,
        &format!("act.engine.v1:{RUN}"),
        &serde_json::to_string(&unclaimed).unwrap(),
    )
    .unwrap();
    tx.commit().unwrap();
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .claim_act_execution(&old.intent, &old_observed, CLAIM, 4.0)
            .is_err()
    );
    let mut tx = registry.begin_immediate().unwrap();
    tx.append_event(
        3.0,
        &format!("act.engine.v1:{RUN}"),
        &serde_json::to_string(&old).unwrap(),
    )
    .unwrap();
    tx.commit().unwrap();

    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .verify_act_execution(RUN, &old_observed, CLAIM)
            .is_err()
    );
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .record_act_execution(RUN, CLAIM, ActRunOutcome::Passed, 4.0)
            .is_err()
    );
    assert!(
        registry
            .begin_immediate()
            .unwrap()
            .request_act_cleanup(RUN, ActRunOutcome::Interrupted, 4.0)
            .is_err()
    );
    let mut tx = registry.begin_immediate().unwrap();
    tx.request_act_execution_cleanup(RUN, CLAIM, ActRunOutcome::Interrupted, 4.0)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        registry.act_engine(RUN).unwrap().unwrap().state,
        ActEngineState::CleanupRequired
    );
}

#[test]
fn record_versions_refuse_profile_presence_mismatches() {
    for (version, has_profile) in [(1, true), (2, true), (3, false), (4, true)] {
        let dir = TemporaryDirectory::new().unwrap();
        let mut registry = Registry::create_writer(dir.path().join("bad-version"), OWNER).unwrap();
        let mut tx = registry.begin_immediate().unwrap();
        tx.begin_act_engine(&intent()).unwrap();
        tx.commit().unwrap();
        let before = registry.act_engine(RUN).unwrap();
        let mut value = serde_json::to_value(before.as_ref().unwrap()).unwrap();
        value["schema_version"] = serde_json::json!(version);
        if !has_profile {
            value["intent"]
                .as_object_mut()
                .unwrap()
                .remove("creation_profile");
        }
        let mut tx = registry.begin_immediate().unwrap();
        assert!(
            tx.append_event(
                1.0,
                &format!("act.engine.v1:{RUN}"),
                &serde_json::to_string(&value).unwrap(),
            )
            .is_err(),
            "version{version},profile{has_profile}"
        );
        tx.commit().unwrap();
        assert_eq!(registry.act_engine(RUN).unwrap(), before);
    }
}
