//! Exercise the actual startup boundary, IPC readiness and durable restart.
use super::*;
use bosn_registry::cache_helper::{CacheHelperIntent, CacheHelperRecord, CacheHelperState};
use kernal_api::sqlite::{Connection, Value};

const OWNER: &str = "11111111-2222-4333-8444-555555555555";
const NONCE: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

fn legacy_registry(path: &Path) {
    drop(Registry::create_writer(path, OWNER).unwrap());
    let mut connection = Connection::open(path).unwrap();
    let mut tx = connection.begin_immediate().unwrap();
    // Remove precisely the v6 additions to reproduce the previous shipped layout.
    tx.execute("DROP TABLE ci_current", &[]).unwrap();
    tx.execute("DROP TABLE ci_snapshot_events", &[]).unwrap();
    tx.execute("UPDATE meta SET value='5' WHERE key='schema_version'", &[])
        .unwrap();
    for index in 1..=400 {
        let record = CacheHelperRecord {
            schema_version: 1,
            intent: CacheHelperIntent {
                registry_id: OWNER.into(),
                nonce: NONCE.into(),
                volume: "bosn-ci-cache-v1".into(),
                image: format!("docker.io/library/docker@sha256:{}", "a".repeat(64)),
                created_at: 1.0,
                role: None,
            },
            state: CacheHelperState::Removed,
            container_id: None,
            updated_at: index as f64,
        };
        tx.execute(
            "INSERT INTO events(at,kind,detail) VALUES(?,?,?)",
            &[
                Value::Real(index as f64),
                Value::Text(format!("ci.cache-helper.v1:{NONCE}")),
                Value::Text(serde_json::to_string(&record).unwrap()),
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

#[test]
fn daemon_upgrades_legacy_history_before_ipc_and_preserves_authority_on_restart() {
    let temporary = fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let db = state.join("registry.sqlite3");
    legacy_registry(&db);
    let runtime = RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        for _ in 0..2 {
            let server = async_engine::launch(Service::new(state.clone()).serve());
            let client = wait_for_client(&state).await;
            assert_eq!(
                client.status().await.unwrap().schema_version,
                bosn_registry::SCHEMA_VERSION
            );
            let reader = Registry::open_read_only(&db).unwrap();
            assert_eq!(reader.registry_id().unwrap(), OWNER);
            assert_eq!(
                reader.cache_helper(NONCE).unwrap().unwrap().updated_at,
                400.0
            );
            assert_eq!(reader.events(0, 100).unwrap().items.len(), 3);
            drop(reader);
            client.shutdown().await.unwrap();
            stopped(server).await;
        }
    });
}
