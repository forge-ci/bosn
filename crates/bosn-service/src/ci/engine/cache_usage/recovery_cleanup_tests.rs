//! Restart cleanup exercises the real journal against a synthetic Docker boundary.
use super::*;
use crate::ci::lifecycle::tests::{OWNER, with_registry};
use crate::{
    RegistryActor,
    act_registry::{ActRegistryCommand, ActRegistryReply},
};
use bosn_registry::{
    Registry,
    act::*,
    cache_helper::{CacheHelperIntent, CacheHelperRole, CacheHelperState},
};
use kernal_api::platform::fs::TemporaryDirectory;

const RUN: &str = "11111111-2222-4333-8444-555555555508";
const CLAIM: &str = "11111111-2222-4333-8444-555555555509";
const ID: &str = "1111111111111111111111111111111111111111111111111111111111111111";

async fn apply(actor: &RegistryActor, command: ActRegistryCommand) {
    actor.act_registry(command).await.unwrap();
}
pub(in crate::ci::engine) async fn seed_source(
    actor: &RegistryActor,
    old: bool,
) -> ActEngineRecord {
    seed_source_with(actor, old, &"c".repeat(64), "bosn-ci-cache-test").await
}

pub(in crate::ci::engine) async fn seed_source_with(
    actor: &RegistryActor,
    old: bool,
    generation: &str,
    cache: &str,
) -> ActEngineRecord {
    seed_source_for_lifetime(actor, old, generation, cache, 3600).await
}

pub(in crate::ci::engine) async fn seed_source_for_lifetime(
    actor: &RegistryActor,
    old: bool,
    generation: &str,
    cache: &str,
    lifetime_secs: i64,
) -> ActEngineRecord {
    let mut engine = crate::ci::lifecycle::tests::intent(RUN);
    let profile = engine.creation_profile.as_mut().unwrap();
    profile.tmpfs_policy = ActEngineTmpfsPolicy::NamedDiskStorageRunTmpNoexecV2;
    profile.tool_generation = Some(ActToolGenerationBinding {
        id: generation.into(),
        max_payload_bytes: 64 << 20,
        overlay_recipe_sha256: crate::ci::engine::tool_overlay_recipe_sha256(),
    });
    profile.cache_volume.as_mut().unwrap().name = cache.into();
    let observed = ActEngineObservation {
        name: engine.engine_name(),
        engine_id: "b".repeat(64),
        image_digest: engine.engine_image_digest.clone(),
        labels: engine.required_labels(OWNER).unwrap(),
    };
    apply(actor, ActRegistryCommand::Begin(engine.clone())).await;
    apply(
        actor,
        ActRegistryCommand::Register {
            run: RUN.into(),
            observed: observed.clone(),
            at: 2.0,
        },
    )
    .await;
    apply(
        actor,
        ActRegistryCommand::Claim {
            intent: engine,
            observed,
            token: CLAIM.into(),
            at: 3.0,
        },
    )
    .await;
    let created = crate::ci::lifecycle::now_seconds().floor() as i64 - if old { 662 } else { 2 };
    apply(
        actor,
        ActRegistryCommand::ToolRecoveryBegin {
            run: RUN.into(),
            token: CLAIM.into(),
            created,
            expires: created + lifetime_secs,
        },
    )
    .await;
    let ActRegistryReply::Record(Some(record)) = actor
        .act_registry(ActRegistryCommand::Get { run: RUN.into() })
        .await
        .unwrap()
    else {
        panic!("engine missing");
    };
    let frozen = record.tool_recovery.clone().unwrap();
    apply(
        actor,
        ActRegistryCommand::ToolRecoveryReserved {
            run: RUN.into(),
            token: CLAIM.into(),
            intent: frozen.clone(),
            at: created as f64 + 0.1,
        },
    )
    .await;
    apply(
        actor,
        ActRegistryCommand::CleanupClaimed {
            run: RUN.into(),
            token: CLAIM.into(),
            outcome: ActRunOutcome::Interrupted,
            at: created as f64 + 0.2,
        },
    )
    .await;
    apply(
        actor,
        ActRegistryCommand::ToolRecoverySourceStopped {
            run: RUN.into(),
            proof: ActToolSourceStopProof {
                engine_name: record.intent.engine_name(),
                engine_id: frozen.engine_id.clone(),
                source_volume: frozen.source_volume.clone(),
            },
            at: created as f64 + 0.3,
        },
    )
    .await;
    let ActRegistryReply::Record(Some(record)) = actor
        .act_registry(ActRegistryCommand::Get { run: RUN.into() })
        .await
        .unwrap()
    else {
        panic!("source record missing");
    };
    *record
}

async fn seed(actor: &RegistryActor, old: bool, binding: bool) -> CacheHelperIntent {
    let record = seed_source(actor, old).await;
    let frozen = record.tool_recovery.unwrap();
    let created = frozen.created_at_seconds;
    let (mut helper, _) = super::recovery_identity_tests::fixture();
    helper.created_at = created as f64 + 0.4;
    helper.volume = frozen.cache_volume.clone();
    let command_sha256 =
        binding.then(|| "ae0ec6002965a1ecfc7799157a635844850854c81b71571d6443ee1e692daf34".into());
    helper.role = Some(CacheHelperRole::ToolRecoveryV1 {
        run_id: RUN.into(),
        intent: frozen,
        command_sha256,
    });
    apply(actor, ActRegistryCommand::HelperBegin(helper.clone())).await;
    helper
}

fn backend(helper: &CacheHelperIntent, mode: &str) -> (TemporaryDirectory, DockerActBackend) {
    let dir = TemporaryDirectory::new().unwrap();
    let (_, mut document) = super::recovery_identity_tests::fixture();
    let Some(CacheHelperRole::ToolRecoveryV1 { intent, .. }) = &helper.role else {
        panic!("role missing");
    };
    document["Mounts"][0]["Name"] = helper.volume.clone().into();
    document["Mounts"][1]["Name"] = intent.source_volume.clone().into();
    if mode == "foreign" {
        document["Mounts"][1]["Name"] = "foreign-source".into();
    }
    if mode == "command" {
        document["Config"]["Cmd"] = serde_json::json!(["sleep", "300"]);
    }
    // Reuse independently checked label construction; this test checks journal side effects.
    let identity = Identity::from_intent(helper).ok();
    if let Some(identity) = identity {
        let mut labels = identity.ownership.clone();
        labels.insert(identity.label_key().into(), helper.nonce.clone());
        document["Config"]["Labels"] = serde_json::to_value(labels).unwrap();
    }
    std::fs::write(
        dir.path().join("helper.json"),
        serde_json::to_string(&vec![document]).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.path().join("mode"), mode).unwrap();
    let script = dir.path().join("docker.py");
    std::fs::write(&script, r#"import json,pathlib,sys
root=pathlib.Path(sys.argv[1]); args=sys.argv[2:]; mode=(root/'mode').read_text()
with (root/'commands').open('a') as out: out.write(json.dumps(args)+'\n')
if args[:2]==['container','inspect']:
    if mode=='unreadable': print('permission denied',file=sys.stderr);sys.exit(1)
    if mode=='absent' or (root/'removed').exists(): print('Error: No such container: '+args[-1],file=sys.stderr);sys.exit(1)
    print((root/'helper.json').read_text())
elif args[0]=='rm':
    assert args==['rm','-f','-v','1'*64]
    (root/'removed').write_text('removed')
    if mode=='lost-reply': print('reply lost',file=sys.stderr);sys.exit(1)
else: sys.exit(2)
"#).unwrap();
    let backend = DockerActBackend::new(bosn_engine::DockerEngine::synthetic_for_test(
        "python3",
        [script.into_os_string(), dir.path().as_os_str().to_owned()],
    ));
    (dir, backend)
}

#[test]
fn recovery_cleanup_retains_foreign_or_unreadable_sources_and_reconciles_absence() {
    for mode in [
        "present",
        "present-known",
        "command-known",
        "lost-reply",
        "foreign",
        "command",
        "unreadable",
        "absent-old",
        "absent-known",
        "absent-recent",
        "active",
    ] {
        with_registry(|actor, state| async move {
            let old = mode == "absent-old";
            let binding = !matches!(mode, "absent-old" | "absent-known" | "absent-recent");
            let helper = seed(&actor, old, binding).await;
            let actual_mode = if mode.starts_with("absent-") {
                "absent"
            } else {
                mode.strip_suffix("-known").unwrap_or(mode)
            };
            let (dir, backend) = backend(&helper, actual_mode);
            if mode.ends_with("-known") {
                apply(
                    &actor,
                    ActRegistryCommand::HelperRegister {
                        nonce: helper.nonce.clone(),
                        id: ID.into(),
                        at: crate::ci::lifecycle::now_seconds(),
                    },
                )
                .await;
            }
            let active = (mode == "active")
                .then(|| super::super::journal::ActiveHelper::claim(&backend, &helper.nonce));
            let first = backend
                .retry_measurements(&actor, OWNER, None)
                .await
                .unwrap();
            if mode == "active" {
                assert!(first.removed.is_none() && first.deferred.is_none());
                drop(active);
            }
            if mode == "lost-reply" {
                assert!(first.deferred.is_some());
            }
            let removed = if matches!(mode, "lost-reply" | "active") {
                backend
                    .retry_measurements(&actor, OWNER, None)
                    .await
                    .unwrap()
                    .removed
                    .is_some()
            } else {
                first.removed.is_some()
            };
            let expected = matches!(
                mode,
                "present"
                    | "present-known"
                    | "lost-reply"
                    | "absent-old"
                    | "absent-known"
                    | "active"
            );
            assert_eq!(removed, expected, "{mode}");
            let reader = Registry::open_read_only(state.join("registry.sqlite3")).unwrap();
            let record = reader.cache_helper(&helper.nonce).unwrap().unwrap();
            assert_eq!(
                record.state == CacheHelperState::Removed,
                expected,
                "{mode}"
            );
            let commands = std::fs::read_to_string(dir.path().join("commands")).unwrap_or_default();
            let commands: Vec<Vec<String>> = commands
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(
                !commands
                    .iter()
                    .any(|args| args.first().is_some_and(|verb| verb == "volume")),
                "source/cache volume removal attempted"
            );
            if !expected {
                assert!(
                    !commands
                        .iter()
                        .any(|args| args.first().is_some_and(|verb| verb == "rm")),
                    "{mode}: foreign helper removed"
                );
            }
        });
    }
}
