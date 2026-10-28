use super::*;
use bosn_registry::{Registry, act::*};
use kernal_api::{async_engine, platform::fs::TemporaryDirectory};

fn seed_pending_recovery(path: &std::path::Path) {
    let owner = "11111111-2222-4333-8444-555555555555";
    let run = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let claim = "12345678-1234-4234-8234-123456789abc";
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let intent = ActEngineIntent {
        run_id: run.into(),
        workspace: "/private/source".into(),
        candidate_sha: "a".repeat(40),
        payload_sha256: "b".repeat(64),
        snapshot_sha256: "c".repeat(64),
        act_version: "0.2.88".into(),
        act_image_digest: format!("sha256:{}", "d".repeat(64)),
        engine_image_digest: format!("sha256:{}", "e".repeat(64)),
        runner_image_digest: format!("sha256:{}", "f".repeat(64)),
        created_at: 1.0,
        spare: false,
        creation_profile: Some(ActEngineCreationProfile {
            memory_bytes: 28 << 30,
            storage_bytes: 20 << 30,
            nano_cpus: 2_000_000_000,
            pids: 1024,
            run_tmpfs_bytes: 16 << 20,
            tmp_tmpfs_bytes: 64 << 20,
            tmpfs_policy: ActEngineTmpfsPolicy::NamedDiskStorageRunTmpNoexecV2,
            init_command_sha256: "a".repeat(64),
            cache_volume: Some(ActEngineCacheVolume {
                name: "bosn-ci-cache-v1".into(),
                target: "/bosn/cache".into(),
            }),
            cache_coordination: Some(ActCacheCoordination::SharedLegacyLeaseV1),
            tool_generation: Some(ActToolGenerationBinding {
                id: "a".repeat(64),
                max_payload_bytes: 100,
                overlay_recipe_sha256: "b".repeat(64),
            }),
            docker_socket: None,
        }),
    };
    let observation = ActEngineObservation {
        name: intent.engine_name(),
        engine_id: "1".repeat(64),
        image_digest: intent.engine_image_digest.clone(),
        labels: intent.required_labels(owner).unwrap(),
    };
    let mut registry = Registry::create_writer(path, owner).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_act_engine(&intent).unwrap();
    tx.register_act_engine(run, &observation, 2.0).unwrap();
    tx.claim_act_execution(&intent, &observation, claim, 4.25)
        .unwrap();
    tx.begin_act_tool_recovery(run, claim, now, now + 3600)
        .unwrap();
    tx.commit().unwrap();
    let frozen = registry
        .act_engine(run)
        .unwrap()
        .unwrap()
        .tool_recovery
        .unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.acknowledge_act_tool_recovery(run, claim, &frozen, (now + 1) as f64)
        .unwrap();
    tx.request_act_execution_cleanup(run, claim, ActRunOutcome::Interrupted, (now + 2) as f64)
        .unwrap();
    tx.record_act_tool_source_stopped(
        run,
        &ActToolSourceStopProof {
            engine_name: intent.engine_name(),
            engine_id: frozen.engine_id.clone(),
            source_volume: frozen.source_volume.clone(),
        },
        (now + 3) as f64,
    )
    .unwrap();
    tx.commit().unwrap();
    drop(registry);
}

#[test]
fn pending_tool_recovery_prevents_idle_handoff_and_survives_reopen() {
    let dir = TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let run = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    seed_pending_recovery(&path);
    let registry = Registry::open_writer(&path).unwrap();
    let runtime = async_engine::RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let (sender, receiver) = async_engine::channel(16);
        let actor = RegistryActor { sender };
        let task = async_engine::launch(crate::registry_actor(registry, receiver, None));
        let reason = actor
            .recovery_handoff(CancellationSource::new(), Arc::default())
            .await
            .unwrap()
            .expect("pending recovery must block handoff");
        assert!(reason.contains(run));
        drop(actor);
        task.await.unwrap();
    });
    assert!(
        Registry::open_writer(&path)
            .unwrap()
            .act_engine(run)
            .unwrap()
            .unwrap()
            .tool_recovery
            .is_some()
    );
}

#[test]
fn idle_handoff_seals_recovery_admission_before_stopping() {
    let dir = TemporaryDirectory::new().unwrap();
    let registry = Registry::create_writer(
        dir.path().join("registry.sqlite3"),
        "11111111-2222-4333-8444-555555555555",
    )
    .unwrap();
    let runtime = async_engine::RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let (sender, receiver) = async_engine::channel(16);
        let actor = RegistryActor { sender };
        let task = async_engine::launch(crate::registry_actor(registry, receiver, None));
        let stop = CancellationSource::new();
        assert_eq!(
            actor
                .recovery_handoff(stop.clone(), Arc::default())
                .await
                .unwrap(),
            None
        );
        assert!(stop.token().is_cancelled());
        let later = actor
            .act_registry(crate::act_registry::ActRegistryCommand::SealStartup)
            .await;
        assert!(matches!(
            later,
            Err(Error::Protocol("daemon handoff in progress"))
        ));
        drop(actor);
        task.await.unwrap();
    });
}

#[test]
fn new_client_does_not_stop_an_older_daemon_without_recovery_fence() {
    let dir = TemporaryDirectory::new().unwrap();
    let state = dir.path().join("state");
    ipc::ensure_owner_private_directory(&state).unwrap();
    let runtime = async_engine::RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let listener = AsyncListener::bind_owner_only(&endpoint(&state).unwrap()).unwrap();
        let fake_old = async_engine::launch(async move {
            let mut stream = listener.accept().await.unwrap();
            let request = read_frame(&mut stream).await.unwrap();
            let payload = Request::decode(request.payload()).unwrap();
            assert_eq!(payload.operation, OP_STOP_IF_IDLE);
            assert_ne!(payload.operation, OP_STOP_IF_IDLE_LEGACY);
            let mut bytes = Vec::new();
            ReplyWire {
                code: 2,
                ..Default::default()
            }
            .encode(&mut bytes)
            .unwrap();
            write_frame(&mut stream, DaemonFrame::response_to(&request, bytes))
                .await
                .unwrap();
        });
        let client = Client::for_state(&state).unwrap();
        assert_eq!(
            client.stop_if_idle().await.unwrap(),
            IdleHandoff::Unsupported
        );
        fake_old.await.unwrap();
    });
}

#[test]
fn pending_recovery_blocks_both_wire_handoffs_and_idle_exit() {
    use crate::ci::lifecycle::tests::{FakeBackend, Faults};
    let dir = TemporaryDirectory::new().unwrap();
    let state = dir.path().join("state");
    ipc::ensure_owner_private_directory(&state).unwrap();
    seed_pending_recovery(&state.join("registry.sqlite3"));
    let runtime = async_engine::RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let backend = Arc::new(FakeBackend::with(Faults {
            remove: true,
            ..Faults::default()
        }));
        let server = async_engine::launch(
            Service::new(state.clone())
                .with_act_backend(backend)
                .with_idle_exit(Some(Duration::from_millis(150)))
                .serve(),
        );
        let client = Client::for_state(&state).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while client.ping().await.is_err() {
            assert!(std::time::Instant::now() < deadline);
            async_engine::sleep(Duration::from_millis(20)).await;
        }
        assert!(matches!(
            client
                .call(Request::operation(OP_STOP_IF_IDLE_LEGACY))
                .await
                .unwrap(),
            Reply::Busy(_)
        ));
        assert!(matches!(
            client.stop_if_idle().await.unwrap(),
            IdleHandoff::Busy(_)
        ));
        async_engine::sleep(Duration::from_millis(500)).await;
        client.ping().await.unwrap();
        client.shutdown().await.unwrap();
        async_engine::timeout(Duration::from_secs(30), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    });
    let reader = Registry::open_read_only(state.join("registry.sqlite3")).unwrap();
    assert!(
        reader
            .act_engine("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee")
            .unwrap()
            .unwrap()
            .tool_recovery
            .is_some()
    );
}
