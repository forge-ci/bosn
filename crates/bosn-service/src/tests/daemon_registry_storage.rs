//! Slow checkpoint I/O must not occupy the authenticated registry request path.
use super::*;
use kernal_api::sqlite::{Connection, QueryLimits, Value};

#[test]
fn daemon_serves_ipc_during_checkpoint_and_joins_its_writer_lease_at_shutdown() {
    let temporary = fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let path = state.join("registry.sqlite3");
    let mut registry =
        Registry::create_writer(&path, "11111111-2222-4333-8444-555555555555").unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.append_event(1.0, "checkpoint.fixture", &"x".repeat(5 * 1024 * 1024))
        .unwrap();
    tx.commit().unwrap();
    let reader = Connection::open_read_only(&path).unwrap();
    reader.execute("BEGIN", &[]).unwrap();
    reader
        .query("SELECT COUNT(*) FROM events", &[], QueryLimits::default())
        .unwrap();
    drop(registry);
    assert!(
        std::fs::metadata(state.join("registry.sqlite3-wal"))
            .unwrap()
            .len()
            > 4 * 1024 * 1024
    );
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let (entered, waiting) = async_engine::oneshot_channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let mut service = Service::new(state.clone());
        service.registry_storage_gate = Some(crate::registry_storage::CheckpointGate {
            entered,
            release: blocked,
        });
        let mut server = async_engine::launch(service.serve());
        let client = wait_for_client(&state).await;
        async_engine::timeout(Duration::from_secs(3), waiting)
            .await
            .unwrap()
            .unwrap();
        let status = async_engine::timeout(Duration::from_secs(1), client.status())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.registry_id, "11111111-2222-4333-8444-555555555555");
        client.shutdown().await.unwrap();
        assert!(
            async_engine::timeout(Duration::from_millis(50), &mut server)
                .await
                .is_err(),
            "shutdown abandoned checkpoint work"
        );
        assert!(matches!(
            Registry::open_writer(&path),
            Err(bosn_registry::Error::WriterAlreadyHeld(_))
        ));
        drop(reader);
        release.send(()).unwrap();
        stopped(server).await;
        let registry = Registry::open_writer(&path).unwrap();
        let verify = Connection::open_read_only(&path).unwrap();
        let rows = verify
            .query(
                "SELECT COUNT(*) FROM events WHERE kind=? AND detail=?",
                &[
                    Value::Text("checkpoint.fixture".into()),
                    Value::Text("x".repeat(5 * 1024 * 1024)),
                ],
                QueryLimits {
                    max_rows: 1,
                    max_bytes: 128,
                },
            )
            .unwrap();
        assert_eq!(rows[0].get(0), Some(&Value::Integer(1)));
        assert_eq!(
            registry.storage_usage().unwrap().files[1].allocated_bytes,
            Some(0)
        );
    });
}
