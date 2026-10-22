//! Producer receipt and helper teardown must precede physical source deletion.
use super::*;
use crate::ci::engine::cache_usage::helper::recovery_cleanup_tests::seed_source_for_lifetime;

async fn expired_disposal(actor: &RegistryActor) -> ActEngineRecord {
    let record =
        seed_source_for_lifetime(actor, true, &"c".repeat(64), "bosn-ci-cache-v1", 600).await;
    let reply = actor
        .act_registry(ActRegistryCommand::ToolRecoverySourceDisposal {
            run: record.intent.run_id.clone(),
            at: crate::ci::lifecycle::now_seconds(),
        })
        .await
        .unwrap();
    assert!(matches!(reply, ActRegistryReply::Committed));
    let ActRegistryReply::Record(Some(record)) = actor
        .act_registry(ActRegistryCommand::Get {
            run: record.intent.run_id,
        })
        .await
        .unwrap()
    else {
        panic!("missing record")
    };
    *record
}

#[test]
fn disposal_receipts_and_teardown_gate_physical_source_removal() {
    for mode in [
        "ok",
        "partial",
        "bad-pin",
        "not-disposed",
        "wrong-root",
        "wrong-schema",
        "unknown",
        "report-error",
        "trailing",
        "oversized",
        "failed-start",
        "foreign-command",
        "cleanup-denied",
        "lost-remove",
        "helper-ambiguous",
        "cache-changed",
        "engine-present",
        "engine-ambiguous",
    ] {
        with_registry(|actor, state| async move {
            let record = expired_disposal(&actor).await;
            let (dir, backend) = backend(&record, &state, mode);
            let result =
                crate::act_engine::remove_storage_volume(&actor, &backend.docker, &record, OWNER)
                    .await;
            assert_eq!(result.is_ok(), mode == "ok", "{mode}: {result:?}");
            assert_eq!(
                dir.path().join("source.json").exists(),
                mode != "ok",
                "{mode}"
            );
            let calls = commands(dir.path());
            for args in calls.iter().filter(|a| a[0] == "create") {
                assert!(!args.iter().any(|a| a == "--privileged"));
                let mounts: Vec<_> = args.windows(2).filter(|a| a[0] == "--mount").collect();
                assert_eq!(mounts.len(), 1);
                assert!(mounts[0][1].contains(&format!(
                    "source={},",
                    record.tool_recovery.as_ref().unwrap().cache_volume
                )));
                assert!(!args.last().unwrap().contains("--source-root"));
            }
            assert_eq!(
                calls
                    .iter()
                    .any(|a| a.first().is_some_and(|x| x == "volume")
                        && a.get(1).is_some_and(|x| x == "rm")),
                mode == "ok"
            );
        });
    }
}

#[test]
fn lost_physical_removal_reply_repeats_the_producer_fence_before_retry() {
    with_registry(|actor, state| async move {
        let record = expired_disposal(&actor).await;
        let (dir, backend) = backend(&record, &state, "lost-volume-remove");
        assert!(
            crate::act_engine::remove_storage_volume(&actor, &backend.docker, &record, OWNER)
                .await
                .is_err()
        );
        assert!(!dir.path().join("source.json").exists());
        crate::act_engine::remove_storage_volume(&actor, &backend.docker, &record, OWNER)
            .await
            .unwrap();
        assert_eq!(
            commands(dir.path())
                .iter()
                .filter(|args| args.first().is_some_and(|x| x == "start"))
                .count(),
            2
        );
    });
}

#[test]
fn unclaimed_source_disposal_refuses_before_contacting_docker() {
    with_registry(|actor, state| async move {
        let record = seed_source(&actor, false).await;
        let (dir, backend) = backend(&record, &state, "ok");
        assert!(
            backend
                .fence_tool_source_with_helper(&actor, &record.intent.run_id, OWNER)
                .await
                .is_err()
        );
        assert!(!dir.path().join("commands").exists());
    });
}
