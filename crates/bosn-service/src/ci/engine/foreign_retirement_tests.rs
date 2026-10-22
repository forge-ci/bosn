//! Foreign takeover must not discard a still-protected native upper.
use super::*;
use crate::ci::machine::{EngineIdentity, Foreign, MachineEngine};
use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};

const OWNER: &str = "11111111-2222-4333-8444-555555555555";
const DOCKER: &str = r#"import json,pathlib,sys
root=pathlib.Path(sys.argv[1]);args=sys.argv[2:]
with (root/'commands').open('a') as output: output.write(json.dumps(args)+'\n')
if args[0]=='rm':
    (root/'engine').rename(root/'engine-removed')
elif args[:2]==['container','inspect']:
    if not (root/'engine').exists():
        print('Error: No such container',file=sys.stderr);sys.exit(1)
    document=json.loads((root/'container.json').read_text())
    document['State']['Running']=(root/'engine').read_text()=='running'
    print(json.dumps(document))
elif args[0]=='stop': (root/'engine').write_text('stopped')
elif args[0]=='exec': (root/'ready').write_text('ready')
elif args[0]=='ps': print('')
elif args[:2]==['volume','rm']:
    assert not (root/'engine').exists(),'attached source cannot be removed'
    (root/'source').rename(root/'source-removed')
elif args[:2]==['volume','inspect']:
    if not (root/'source').exists():
        print('Error: No such volume',file=sys.stderr);sys.exit(1)
    print((root/'volume.json').read_text())
else: raise AssertionError(args)
"#;

fn fixture() -> (TemporaryDirectory, DockerActBackend, Foreign) {
    let dir = TemporaryDirectory::new().unwrap();
    let mut intent = crate::ci::lifecycle::tests::intent("aaaaaaaa-bbbb-4ccc-8ddd-000000005084");
    intent.created_at = crate::ci::lifecycle::now_seconds().floor();
    intent.creation_profile.as_mut().unwrap().tool_generation =
        Some(bosn_registry::act::ActToolGenerationBinding {
            id: "a".repeat(64),
            max_payload_bytes: 1 << 30,
            overlay_recipe_sha256: tool_overlay_recipe_sha256(),
        });
    let profile = intent.creation_profile.as_ref().unwrap();
    let volume = intent.storage_volume_name().unwrap();
    let mut labels = crate::act_engine::creation_labels(&intent, OWNER).unwrap();
    labels.insert(
        "com.zackees.bosn.act.native-recovery-window".into(),
        "v1".into(),
    );
    let foreign = Foreign {
        id: "1".repeat(64),
        name: intent.engine_name(),
        running: true,
        identity: EngineIdentity::of(&intent),
        registry: OWNER.into(),
        storage_volume: Some(volume.clone()),
        memory_bytes: profile.memory_bytes,
        nano_cpus: profile.nano_cpus,
        pids: profile.pids,
        created: intent.created_at as u64,
    };
    let container = serde_json::json!({
        "Id": foreign.id, "Name": format!("/{}", foreign.name),
        "State": {"Running": true}, "Config": {"Labels": labels},
        "HostConfig": {}, "Mounts": [{"Type":"volume", "Name":volume,
            "Destination":crate::act_engine::STORAGE_TARGET}]
    });
    std::fs::write(dir.path().join("container.json"), container.to_string()).unwrap();
    std::fs::write(dir.path().join("volume.json"), "[]").unwrap();
    std::fs::write(dir.path().join("engine"), "running").unwrap();
    std::fs::write(dir.path().join("source"), "completed tool update").unwrap();
    let script = dir.path().join("docker.py");
    std::fs::write(&script, DOCKER).unwrap();
    let backend = DockerActBackend::new(DockerEngine::synthetic_for_test(
        "python3",
        [
            script.to_string_lossy().into_owned(),
            dir.path().to_string_lossy().into_owned(),
        ],
    ));
    (dir, backend, foreign)
}

#[test]
fn foreign_native_takeover_retains_the_protected_source() {
    let (dir, backend, foreign) = fixture();
    RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let result = backend.remove_engine(&foreign).await;
            assert!(
                result.is_err(),
                "missing recovery context must refuse takeover"
            );
            assert!(dir.path().join("engine").exists());
            assert!(
                dir.path().join("source").exists(),
                "foreign takeover deleted a protected native upper"
            );
        });
}

#[test]
fn native_readiness_requires_machine_visible_source_context() {
    let (dir, backend, foreign) = fixture();
    RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let result = backend.mark_ready(&foreign.id).await;
            assert!(
                result.is_err(),
                "native engine became ready without machine-visible source recovery context"
            );
            assert!(
                !dir.path().join("ready").exists(),
                "missing context must refuse before ready publication"
            );
        });
}
