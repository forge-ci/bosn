//! Frozen context canonical records and Docker profile validation.
use super::*;
use crate::ci::{
    engine::cache_usage::helper::recovery_cleanup_tests::seed_source,
    lifecycle::tests::with_registry,
};

pub(in crate::ci::engine) fn document(context: &SourceContext) -> serde_json::Value {
    serde_json::json!({
        "Id":"e".repeat(64), "Name":format!("/{}",context.name()),
        "State":{"Running":false,"Status":"created"},
        "Config":{"Image":format!("docker.io/library/docker@{}",context.intent.engine_image_digest),
            "Labels":context.labels().unwrap(),"Entrypoint":["/bin/true"],"Cmd":null},
        "HostConfig":{"ReadonlyRootfs":true,"Privileged":false,"NetworkMode":"none","CapDrop":["ALL"],
            "Tmpfs":{"/var/lib/docker":"ro,noexec,size=1m"},"AutoRemove":false,"CpuShares":256,
            "Init":false,"RestartPolicy":{"Name":"no","MaximumRetryCount":0}},"Mounts":[]
    })
}

#[test]
fn source_context_is_canonical_and_contains_no_data_mounts() {
    with_registry(|actor, _| async move {
        let record = seed_source(&actor, false).await;
        let context = SourceContext::from_record(&record).unwrap();
        assert_eq!(
            context
                .labels()
                .unwrap()
                .get(bosn_core::LABEL_RETENTION)
                .map(String::as_str),
            Some("pinned"),
            "generic age/pressure GC must retain source authority"
        );
        let row = document(&context);
        let (id, parsed) = parse(&row.to_string()).unwrap();
        assert_eq!(id, "e".repeat(64));
        assert_eq!(context, parsed);
        let args = context.create_args().unwrap();
        assert!(
            !args
                .iter()
                .any(|a| a == "--mount" || a == "--privileged" || a == "--rm")
        );
        assert!(
            args.iter()
                .any(|a| a == "/var/lib/docker:ro,noexec,size=1m")
        );
        for (field, value) in [
            ("ReadonlyRootfs", serde_json::json!(false)),
            ("Privileged", serde_json::json!(true)),
            ("NetworkMode", serde_json::json!("host")),
            ("CapDrop", serde_json::json!([])),
            ("Tmpfs", serde_json::json!({})),
            ("AutoRemove", serde_json::json!(true)),
            ("CpuShares", serde_json::json!(1024)),
            ("Init", serde_json::json!(true)),
            (
                "RestartPolicy",
                serde_json::json!({"Name":"always","MaximumRetryCount":0}),
            ),
        ] {
            let mut altered = row.clone();
            altered["HostConfig"][field] = value;
            assert!(parse(&altered.to_string()).is_err(), "{field}");
        }
    });
}

#[test]
fn source_context_rejects_running_foreign_aliased_and_extended_metadata() {
    with_registry(|actor, _| async move {
        let record = seed_source(&actor, false).await;
        let context = SourceContext::from_record(&record).unwrap();
        let row = document(&context);
        let mut cases = Vec::new();
        let mut changed = row.clone();
        changed["State"]["Running"] = true.into();
        cases.push(changed);
        let mut changed = row.clone();
        changed["State"]["Status"] = "exited".into();
        cases.push(changed);
        let mut changed = row.clone();
        changed["Name"] = "/foreign".into();
        cases.push(changed);
        let mut changed = row.clone();
        changed["Config"]["Entrypoint"] = serde_json::json!(["/bin/sh"]);
        cases.push(changed);
        let mut changed = row.clone();
        changed["Config"]["Cmd"] = serde_json::json!(["-c", "sleep 1"]);
        cases.push(changed);
        let mut changed = row.clone();
        changed["Mounts"] = serde_json::json!([{"Type":"volume","Name":"source"}]);
        cases.push(changed);
        let mut changed = row.clone();
        changed["Config"]["Labels"][ENGINE] = "a".repeat(64).into();
        cases.push(changed);
        let mut changed = row.clone();
        let mut data: serde_json::Value =
            serde_json::from_str(changed["Config"]["Labels"][RECORD].as_str().unwrap()).unwrap();
        data["intent"]["unrecognized"] = true.into();
        changed["Config"]["Labels"][RECORD] = data.to_string().into();
        cases.push(changed);
        for changed in cases {
            assert!(parse(&changed.to_string()).is_err(), "{changed}");
        }
        assert!(parse(&" ".repeat(128 * 1024 + 1)).is_err());
        assert!(parse(&format!("{row}\n{row}")).is_err());
    });
}

#[test]
#[ignore = "requires explicit Docker metadata fixture and cleanup report"]
fn actual_docker_source_context_survives_retry_without_any_data_mount() {
    let report = std::env::var("BOSN_SOURCE_CONTEXT_LIVE_REPORT").unwrap();
    with_registry(|actor, _| async move {
        let cache = format!("bosn693-context-{}", crate::ci::new_uuid().await.unwrap());
        let record=crate::ci::engine::cache_usage::helper::recovery_cleanup_tests::seed_source_for_lifetime(
            &actor,false,&"c".repeat(64),&cache,3600).await;
        let context = SourceContext::from_record(&record).unwrap();
        std::fs::write(
            &report,
            serde_json::json!({"name":context.name(),"context":context,"passed":false}).to_string(),
        )
        .unwrap();
        let backend = super::super::DockerActBackend::default();
        backend.ensure_native_source_context(&record).await.unwrap();
        let observed = backend
            .native_source_context(&context.recovery.engine_id)
            .await
            .unwrap();
        assert_eq!(observed, context);
        backend.ensure_native_source_context(&record).await.unwrap();
        assert_eq!(
            backend
                .native_source_context(&context.recovery.engine_id)
                .await
                .unwrap(),
            context
        );
        std::fs::write(
            &report,
            serde_json::json!({"name":context.name(),"context":context,"passed":true}).to_string(),
        )
        .unwrap();
    });
}

#[test]
fn native_readiness_checks_exact_context_and_full_protection_window() {
    use crate::ci::machine::MachineEngine;
    use kernal_api::platform::fs::TemporaryDirectory;
    for mode in [
        "ok",
        "short-window",
        "missing",
        "ambiguous",
        "foreign-engine",
        "running",
    ] {
        with_registry(|actor, _| async move {
            let mut record = seed_source(&actor, false).await;
            let recovery = record.tool_recovery.as_mut().unwrap();
            record.intent.created_at = recovery.created_at_seconds as f64;
            if mode != "short-window" {
                recovery.expires_at_seconds = recovery.created_at_seconds + 86400;
            }
            let context = SourceContext::from_record(&record).unwrap();
            let dir = TemporaryDirectory::new().unwrap();
            let profile = record.intent.creation_profile.as_ref().unwrap();
            let mut engine_id = context.recovery.engine_id.clone();
            if mode == "foreign-engine" {
                engine_id = "d".repeat(64);
            }
            let engine = serde_json::json!({"Id":engine_id,"Name":format!("/{}",record.intent.engine_name()),
                "State":{"Running":true},"Config":{"Labels":crate::act_engine::creation_labels(&record.intent,&record.registry_id).unwrap()},
                "HostConfig":{"Memory":profile.memory_bytes,"NanoCpus":profile.nano_cpus,"PidsLimit":profile.pids},
                "Mounts":[{"Type":"volume","Name":context.recovery.source_volume,"Destination":crate::act_engine::STORAGE_TARGET}]});
            let mut row = document(&context);
            if mode == "running" {
                row["State"]["Running"] = true.into();
            }
            std::fs::write(dir.path().join("engine.json"), engine.to_string()).unwrap();
            std::fs::write(dir.path().join("context.json"), row.to_string()).unwrap();
            std::fs::write(dir.path().join("mode"), mode).unwrap();
            let script = dir.path().join("docker.py");
            std::fs::write(&script, READY_DOCKER).unwrap();
            let backend =
                super::super::DockerActBackend::new(bosn_engine::DockerEngine::synthetic_for_test(
                    "python3",
                    [script.into_os_string(), dir.path().as_os_str().to_owned()],
                ));
            let result = backend.mark_ready(&engine_id).await;
            assert_eq!(result.is_ok(), mode == "ok", "{mode}: {result:?}");
            assert_eq!(dir.path().join("ready").exists(), mode == "ok", "{mode}");
        });
    }
}

const READY_DOCKER: &str = r#"import json,pathlib,sys
root=pathlib.Path(sys.argv[1]);args=sys.argv[2:];mode=(root/'mode').read_text()
if args[0]=='ps':
    if mode=='missing': print('')
    elif mode=='ambiguous': print('e'*64+'\n'+'e'*64)
    else: print('e'*64)
elif args[:2]==['container','inspect']:
    print((root/('context.json' if args[-1]=='e'*64 else 'engine.json')).read_text())
elif args[0]=='exec': (root/'ready').write_text('ready')
else: raise AssertionError(args)
"#;

#[test]
fn foreign_native_retirement_preserves_verified_context_and_source() {
    use crate::ci::machine::{EngineIdentity, Foreign, MachineEngine};
    use kernal_api::platform::fs::TemporaryDirectory;
    with_registry(|actor, _| async move {
        let record = seed_source(&actor, false).await;
        let context = SourceContext::from_record(&record).unwrap();
        let profile = record.intent.creation_profile.as_ref().unwrap();
        let foreign = Foreign {
            id: context.recovery.engine_id.clone(),
            name: record.intent.engine_name(),
            running: false,
            identity: EngineIdentity::of(&record.intent),
            registry: record.registry_id.clone(),
            storage_volume: Some(context.recovery.source_volume.clone()),
            memory_bytes: profile.memory_bytes,
            nano_cpus: profile.nano_cpus,
            pids: profile.pids,
            created: record.intent.created_at as u64,
        };
        let dir = TemporaryDirectory::new().unwrap();
        std::fs::write(
            dir.path().join("context.json"),
            document(&context).to_string(),
        )
        .unwrap();
        std::fs::write(dir.path().join("engine"), "stopped").unwrap();
        std::fs::write(dir.path().join("source"), "protected payload").unwrap();
        let script = dir.path().join("docker.py");
        std::fs::write(&script, RETIRE_DOCKER).unwrap();
        let backend =
            super::super::DockerActBackend::new(bosn_engine::DockerEngine::synthetic_for_test(
                "python3",
                [script.into_os_string(), dir.path().as_os_str().to_owned()],
            ));
        backend.remove_engine(&foreign).await.unwrap();
        assert!(!dir.path().join("engine").exists());
        assert!(dir.path().join("source").exists());
        assert!(dir.path().join("context.json").exists());
    });
}

const RETIRE_DOCKER: &str = r#"import pathlib,sys
root=pathlib.Path(sys.argv[1]);args=sys.argv[2:]
if args[0]=='ps': print('e'*64)
elif args[:2]==['container','inspect']:
    if args[-1]=='e'*64: print((root/'context.json').read_text())
    else:
        assert not (root/'engine').exists()
        print('Error: No such container',file=sys.stderr);sys.exit(1)
elif args[0]=='rm': (root/'engine').rename(root/'removed-engine')
else: raise AssertionError(args)
"#;

#[test]
fn source_finalization_journal_uses_frozen_foreign_authority_without_engine_record() {
    with_registry(|actor, state| async move {
        use bosn_registry::{
            Registry,
            cache_helper::{CacheHelperIntent, CacheHelperRole},
        };
        let source = seed_source(&actor, false).await;
        let context = ActToolSourceContext::from_record(&source).unwrap();
        let owner = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let path = state.join("replacement.sqlite3");
        let mut registry = Registry::create_writer(&path, owner).unwrap();
        assert!(
            registry
                .act_engine(&source.intent.run_id)
                .unwrap()
                .is_none()
        );
        let mut intent = CacheHelperIntent {
            registry_id: owner.into(),
            nonce: "12345678-2222-4333-8444-555555555555".into(),
            volume: context.recovery.cache_volume.clone(),
            image: format!(
                "docker.io/library/docker@{}",
                context.intent.engine_image_digest
            ),
            created_at: crate::ci::lifecycle::now_seconds(),
            role: Some(CacheHelperRole::ToolRecoveryV1 {
                run_id: context.intent.run_id.clone(),
                intent: context.recovery.clone(),
                command_sha256: Some("d".repeat(64)),
                source_mountpoint: None,
            }),
        };
        {
            let mut tx = registry.begin_immediate().unwrap();
            assert!(
                tx.begin_cache_helper(&intent).is_err(),
                "old local-record helper cannot recover a foreign source"
            );
        }
        intent.role = Some(CacheHelperRole::ToolSourceFinalizationV1 {
            context: Box::new(context.clone()),
            context_id: "e".repeat(64),
            command_sha256: "d".repeat(64),
            source_mountpoint: None,
        });
        assert_invalid_finalization_authority(&intent);
        let mut tx = registry.begin_immediate().unwrap();
        tx.begin_cache_helper(&intent).unwrap();
        tx.commit().unwrap();
        let mut duplicate = intent.clone();
        duplicate.nonce = "87654321-2222-4333-8444-555555555555".into();
        {
            let mut tx = registry.begin_immediate().unwrap();
            assert!(
                tx.begin_cache_helper(&duplicate).is_err(),
                "a pending source helper must remain exclusive in this journal"
            );
        }
        drop(registry);
        let reopened = Registry::open_read_only(&path).unwrap();
        assert_eq!(
            reopened
                .cache_helper(&intent.nonce)
                .unwrap()
                .unwrap()
                .intent,
            intent
        );
        assert!(
            reopened
                .act_engine(&source.intent.run_id)
                .unwrap()
                .is_none()
        );
    });
}

fn assert_invalid_finalization_authority(intent: &bosn_registry::cache_helper::CacheHelperIntent) {
    use bosn_registry::cache_helper::CacheHelperRole;
    for field in [
        "owner",
        "generation",
        "context-id",
        "image",
        "cache",
        "mount",
    ] {
        let mut changed = intent.clone();
        let Some(CacheHelperRole::ToolSourceFinalizationV1 {
            context,
            context_id,
            source_mountpoint,
            ..
        }) = &mut changed.role
        else {
            unreachable!()
        };
        match field {
            "owner" => context.recovery.owner = "f".repeat(64),
            "generation" => context.recovery.generation = "f".repeat(64),
            "context-id" => *context_id = "bad".into(),
            "image" => {
                changed.image = format!("docker.io/library/docker@sha256:{}", "f".repeat(64))
            }
            "cache" => changed.volume = "foreign-cache".into(),
            "mount" => *source_mountpoint = Some("/tmp/../foreign".into()),
            _ => unreachable!(),
        }
        assert!(changed.validate().is_err(), "{field}");
    }
}
