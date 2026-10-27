//! Exercise reservation timeouts at the real Docker transport boundary.
use super::*;
use crate::{
    act_registry::{ActRegistryCommand, ActRegistryReply},
    ci::lifecycle::tests::{OWNER, intent, with_registry},
};
use bosn_registry::act::{ActEngineObservation, ActEngineTmpfsPolicy, ActToolGenerationBinding};
use kernal_api::platform::fs::TemporaryDirectory;

const RUN: &str = "11111111-2222-4333-8444-555555555508";
const CLAIM: &str = "11111111-2222-4333-8444-555555555509";

fn slow_backend(
    frozen: &bosn_registry::act::ActToolRecoveryIntent,
) -> (TemporaryDirectory, DockerActBackend) {
    let pin = RecoveryPin::from_intent(frozen).unwrap();
    let dir = TemporaryDirectory::new().unwrap();
    let receipt = serde_json::json!({"schema_version":1,"root":"/bosn/cache/toolstore-v1","pin":pin,"published":true,"reused":false,"partial":false});
    std::fs::write(dir.path().join("receipt.json"), receipt.to_string()).unwrap();
    let script = format!(
        "set -eu\ncase \" $* \" in *\" --timeout-secs \"*) ;; *) exit 42 ;; esac\nprintf '%s\\n' \"$*\" >> '{}'\nsleep 1.5\ncat '{}'\n",
        dir.path().join("commands.log").display(),
        dir.path().join("receipt.json").display()
    );
    std::fs::write(dir.path().join("docker.sh"), script).unwrap();
    let backend = DockerActBackend::new(bosn_engine::DockerEngine::synthetic_for_test(
        "sh",
        vec![dir.path().join("docker.sh").to_string_lossy().into_owned()],
    ));
    (dir, backend)
}

#[test]
fn ci_stage_timeouts_bound_real_reservation_transport_and_keep_intent() {
    with_registry(|registry, _dir| async move {
        let mut intent = intent(RUN);
        let profile = intent.creation_profile.as_mut().unwrap();
        profile.tmpfs_policy = ActEngineTmpfsPolicy::NamedDiskStorageRunTmpNoexecV2;
        profile.tool_generation = Some(ActToolGenerationBinding {
            id: "c".repeat(64),
            max_payload_bytes: 64 << 20,
            overlay_recipe_sha256: crate::ci::engine::tool_overlay_recipe_sha256(),
        });
        let observed = ActEngineObservation {
            name: intent.engine_name(),
            engine_id: "b".repeat(64),
            image_digest: intent.engine_image_digest.clone(),
            labels: intent.required_labels(OWNER).unwrap(),
        };
        registry
            .act_registry(ActRegistryCommand::Begin(intent.clone()))
            .await
            .unwrap();
        registry
            .act_registry(ActRegistryCommand::Register {
                run: RUN.into(),
                observed: observed.clone(),
                at: 2.0,
            })
            .await
            .unwrap();
        registry
            .act_registry(ActRegistryCommand::Claim {
                intent,
                observed,
                token: CLAIM.into(),
                at: 3.0,
            })
            .await
            .unwrap();
        let created = crate::ci::lifecycle::now_seconds().floor() as i64 - 2;
        registry
            .act_registry(ActRegistryCommand::ToolRecoveryBegin {
                run: RUN.into(),
                token: CLAIM.into(),
                created,
                expires: created + 3600,
            })
            .await
            .unwrap();
        let ActRegistryReply::Record(Some(record)) = registry
            .act_registry(ActRegistryCommand::Get { run: RUN.into() })
            .await
            .unwrap()
        else {
            panic!("pending record missing")
        };
        let frozen = record.tool_recovery.clone().unwrap();
        let (dir, backend) = slow_backend(&frozen);
        assert!(
            backend
                .reserve_tool_recovery(&record, Duration::from_secs(1))
                .await
                .is_err()
        );
        let ActRegistryReply::Record(Some(pending)) = registry
            .act_registry(ActRegistryCommand::Get { run: RUN.into() })
            .await
            .unwrap()
        else {
            panic!("pending record lost")
        };
        assert_eq!(pending.tool_recovery.as_ref(), Some(&frozen));
        assert_eq!(pending.tool_recovery_reserved_at, None);
        backend
            .reserve_tool_recovery(&pending, Duration::from_secs(3))
            .await
            .unwrap();
        let ActRegistryReply::Record(Some(still_pending)) = registry
            .act_registry(ActRegistryCommand::Get { run: RUN.into() })
            .await
            .unwrap()
        else {
            panic!("pending record lost")
        };
        assert_eq!(still_pending.tool_recovery.as_ref(), Some(&frozen));
        assert_eq!(still_pending.tool_recovery_reserved_at, None);
        let commands = std::fs::read_to_string(dir.path().join("commands.log")).unwrap();
        let commands: Vec<_> = commands.lines().collect();
        assert_eq!(commands.len(), 2);
        let has_budget = |command: &str, expected: &str| {
            let words: Vec<_> = command.split_whitespace().collect();
            words
                .windows(2)
                .any(|pair| pair == ["--timeout-secs", expected])
        };
        assert!(has_budget(commands[0], "1"));
        assert!(has_budget(commands[1], "3"));
    });
}
