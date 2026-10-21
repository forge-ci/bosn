//! #508: exercise expiry through retirement, not just the retention predicate.
use super::*;
use bosn_registry::{
    Registry,
    act::{ActEngineObservation, ActEngineState, ActRunOutcome, ActToolGenerationBinding},
};
use kernal_api::{
    async_engine::{self, RuntimeBuilder},
    platform::fs::TemporaryDirectory,
};
use std::time::Duration;

const CLAIM: &str = "12345678-1234-4234-8234-123456789abc";
const DOCKER: &str = r#"import json, pathlib, sys
mode, name, state, document, log = sys.argv[1:6]
state, log = pathlib.Path(state), pathlib.Path(log)
args = sys.argv[6:]
with log.open('a') as output: output.write(json.dumps(args) + '\n')
if args[:2] == ['container', 'ls']:
    if mode == 'engine-list-failed': sys.exit(1)
elif args[:2] == ['volume', 'ls']:
    if mode == 'volume-list-failed': sys.exit(1)
    if state.exists(): print(name)
elif args[:2] == ['volume', 'inspect']:
    if mode == 'inspect-failed' or not state.exists(): sys.exit(1)
    print(pathlib.Path(document).read_text())
elif args[:2] == ['volume', 'rm']:
    assert args == ['volume', 'rm', name]
    state.rename(state.with_name('removed'))
    if mode.endswith('lost-remove-reply'): sys.exit(1)
    print(name)
else: sys.exit(2)
"#;

fn fixture(path: &std::path::Path, live: bool, acknowledged: bool) -> (Registry, ActEngineRecord) {
    use crate::act_engine::tests::{OWNER, disk_intent};
    let mut intent = disk_intent();
    intent.engine_image_digest = super::super::ENGINE_MANIFEST.into();
    let profile = intent.creation_profile.as_mut().unwrap();
    profile.cache_volume = Some(bosn_registry::act::ActEngineCacheVolume {
        name: "bosn-ci-cache-v1".into(),
        target: "/bosn/cache".into(),
    });
    profile.cache_coordination =
        Some(bosn_registry::act::ActCacheCoordination::SharedLegacyLeaseV1);
    profile.tool_generation = Some(ActToolGenerationBinding {
        id: "a".repeat(64),
        max_payload_bytes: 100,
        overlay_recipe_sha256: "b".repeat(64),
    });
    let observed = ActEngineObservation {
        name: intent.engine_name(),
        engine_id: "1".repeat(64),
        image_digest: intent.engine_image_digest.clone(),
        labels: intent.required_labels(OWNER).unwrap(),
    };
    let created = if live {
        crate::act_runtime::now() as i64 - 2
    } else {
        4
    };
    let mut registry = Registry::create_writer(path, OWNER).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_act_engine(&intent).unwrap();
    tx.register_act_engine(&intent.run_id, &observed, 2.0)
        .unwrap();
    tx.claim_act_execution(&intent, &observed, CLAIM, 3.0)
        .unwrap();
    tx.begin_act_tool_recovery(&intent.run_id, CLAIM, created, created + 3600)
        .unwrap();
    tx.commit().unwrap();
    let frozen = registry
        .act_engine(&intent.run_id)
        .unwrap()
        .unwrap()
        .tool_recovery
        .unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    if acknowledged {
        tx.acknowledge_act_tool_recovery(&intent.run_id, CLAIM, &frozen, created as f64 + 0.1)
            .unwrap();
    }
    tx.request_act_execution_cleanup(
        &intent.run_id,
        CLAIM,
        ActRunOutcome::Interrupted,
        created as f64 + 0.2,
    )
    .unwrap();
    tx.commit().unwrap();
    let record = registry.act_engine(&intent.run_id).unwrap().unwrap();
    (registry, record)
}

#[test]
fn expired_recovery_retires_through_the_actual_cleanup_path() {
    for mode in [
        "expired",
        "unacknowledged",
        "absent",
        "lost-remove-reply",
        "released",
        "released-lost-remove-reply",
    ] {
        check_retirement(mode);
    }
}

#[test]
fn expiry_never_bypasses_ownership_liveness_or_failed_observation() {
    for mode in [
        "live",
        "live-unacknowledged",
        "foreign",
        "inspect-failed",
        "engine-list-failed",
        "volume-list-failed",
        "published",
    ] {
        check_retirement(mode);
    }
}

struct DockerFixture {
    engine: DockerEngine,
    state: std::path::PathBuf,
    log: std::path::PathBuf,
}

fn docker_fixture(dir: &std::path::Path, mode: &str, record: &ActEngineRecord) -> DockerFixture {
    use crate::act_engine::tests::OWNER;
    let name = record.intent.storage_volume_name().unwrap();
    let document = dir.join("inspect.json");
    let mut labels = super::super::storage_volume::labels(&record.intent, OWNER).unwrap();
    if mode == "foreign" {
        labels.insert(bosn_core::LABEL_REGISTRY.into(), "another-registry".into());
    }
    std::fs::write(
        &document,
        serde_json::to_vec(&serde_json::json!([{
            "Name": name, "Driver": "local", "Scope": "local",
            "Options": null, "Labels": labels,
        }]))
        .unwrap(),
    )
    .unwrap();
    let state = dir.join("present");
    if mode != "absent" {
        std::fs::write(&state, "present").unwrap();
    }
    let log = dir.join("commands");
    let script = dir.join("docker.py");
    std::fs::write(&script, DOCKER).unwrap();
    let engine = DockerEngine::synthetic_for_test(
        "python3",
        [
            script.to_string_lossy().into_owned(),
            mode.into(),
            name,
            state.to_string_lossy().into_owned(),
            document.to_string_lossy().into_owned(),
            log.to_string_lossy().into_owned(),
        ],
    );
    DockerFixture { engine, state, log }
}

fn check_retirement(mode: &str) {
    use crate::act_engine::tests::OWNER;
    let dir = TemporaryDirectory::new().unwrap();
    let database = dir.path().join("registry.sqlite3");
    let live = mode.starts_with("live") || mode.starts_with("released") || mode == "published";
    let acknowledged = !mode.ends_with("unacknowledged");
    let (mut registry, mut record) = fixture(&database, live, acknowledged);
    if mode.starts_with("released") || mode == "published" {
        record_publication(&mut registry, &record, mode.starts_with("released"));
        record = registry.act_engine(&record.intent.run_id).unwrap().unwrap();
    }
    let DockerFixture { engine, state, log } = docker_fixture(dir.path(), mode, &record);
    RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let (sender, receiver) = async_engine::channel(16);
            let actor = RegistryActor { sender };
            let task = async_engine::launch(crate::registry_actor(registry, receiver, None));
            let cancellation = async_engine::CancellationSource::new();
            let token = cancellation.token();
            let manifests = super::super::bundled_engine_manifests().unwrap();
            let retire = || {
                crate::act_runtime::retire_engine(
                    &actor,
                    &engine,
                    OWNER,
                    &manifests,
                    &record,
                    Duration::from_secs(30),
                    &token,
                )
            };
            let result = retire().await;
            let should_retire =
                matches!(mode, "expired" | "unacknowledged" | "absent" | "released");
            assert_eq!(result.is_ok(), should_retire, "{mode}: {result:?}");
            if mode.ends_with("lost-remove-reply") {
                assert!(!state.exists(), "Docker removed it before losing the reply");
                retire().await.unwrap();
            }
            let ActRegistryReply::Record(Some(current)) = actor
                .act_registry(ActRegistryCommand::Get {
                    run: record.intent.run_id.clone(),
                })
                .await
                .unwrap()
            else {
                panic!("record missing");
            };
            assert_eq!(
                current.state == ActEngineState::Terminal,
                should_retire || mode.ends_with("lost-remove-reply"),
                "{mode}"
            );
            if current.state != ActEngineState::Terminal {
                assert!(state.exists(), "{mode}: retained source vanished");
                assert!(
                    !std::fs::read_to_string(&log).unwrap().contains("\"rm\""),
                    "{mode}: removal must not be attempted"
                );
            }
            actor.stop().await;
            task.await.unwrap();
        });
    let reopened = Registry::open_writer(&database).unwrap();
    assert_eq!(
        reopened
            .act_engine(&record.intent.run_id)
            .unwrap()
            .unwrap()
            .state
            == ActEngineState::Terminal,
        matches!(
            mode,
            "expired"
                | "unacknowledged"
                | "absent"
                | "lost-remove-reply"
                | "released"
                | "released-lost-remove-reply"
        ),
        "{mode}: cleanup receipt must survive a restart"
    );
}

fn record_publication(registry: &mut Registry, record: &ActEngineRecord, release: bool) {
    let intent = record.tool_recovery.as_ref().unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.record_act_tool_source_stopped(
        &record.intent.run_id,
        &ActToolSourceStopProof {
            engine_name: record.intent.engine_name(),
            engine_id: intent.engine_id.clone(),
            source_volume: intent.source_volume.clone(),
        },
        record.updated_at + 0.1,
    )
    .unwrap();
    tx.complete_act_tool_recovery(
        &record.intent.run_id,
        &bosn_registry::act::ActToolRecoveryPublicationProof {
            intent: intent.clone(),
            generation: "c".repeat(64),
        },
        record.updated_at + 0.2,
    )
    .unwrap();
    if release {
        tx.release_act_tool_recovery(&record.intent.run_id, intent, record.updated_at + 0.3)
            .unwrap();
    }
    tx.commit().unwrap();
}
