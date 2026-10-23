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

/// One case of [`disposal_receipts_and_teardown_gate_physical_source_removal`]'s table; each runs as its own test so nextest
/// runs them in parallel (#722: run in a loop they took up to 68 s).
fn disposal_receipts_and_teardown_gate_physical_source_removal(mode: &'static str) {
    with_registry(|actor, state| async move {
        let record = expired_disposal(&actor).await;
        let (dir, backend) = backend(&record, &state, mode);
        let result =
            crate::act_engine::remove_storage_volume(&actor, &backend.docker, &record, OWNER).await;
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

mod disposal_receipts_and_teardown_gate_physical_source_removal {
    #[test]
    fn ok() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("ok");
    }
    #[test]
    fn partial() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("partial");
    }
    #[test]
    fn bad_pin() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("bad-pin");
    }
    #[test]
    fn not_disposed() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("not-disposed");
    }
    #[test]
    fn wrong_root() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("wrong-root");
    }
    #[test]
    fn wrong_schema() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("wrong-schema");
    }
    #[test]
    fn unknown() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("unknown");
    }
    #[test]
    fn report_error() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("report-error");
    }
    #[test]
    fn trailing() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("trailing");
    }
    #[test]
    fn oversized() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("oversized");
    }
    #[test]
    fn failed_start() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("failed-start");
    }
    #[test]
    fn foreign_command() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("foreign-command");
    }
    #[test]
    fn cleanup_denied() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("cleanup-denied");
    }
    #[test]
    fn lost_remove() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("lost-remove");
    }
    #[test]
    fn helper_ambiguous() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("helper-ambiguous");
    }
    #[test]
    fn cache_changed() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("cache-changed");
    }
    #[test]
    fn engine_present() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("engine-present");
    }
    #[test]
    fn engine_ambiguous() {
        super::disposal_receipts_and_teardown_gate_physical_source_removal("engine-ambiguous");
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
