use super::*;
use crate::cache_helper::{CacheHelperIntent, CacheHelperRecord, CacheHelperState};

const OWNER: &str = "11111111-2222-4333-8444-555555555555";
const NONCE: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

#[test]
fn history_and_checkpoint_maintenance_reclaim_unpinned_wal_allocation() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    let mut checkpointer = registry.take_storage_checkpointer().unwrap();
    for at in 1..=100 {
        let mut tx = registry.begin_immediate().unwrap();
        tx.append_event(
            f64::from(at),
            &format!("{}{}", cache_helper::PREFIX, NONCE),
            &serde_json::to_string(&helper(f64::from(at))).unwrap(),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let wal = path.with_file_name("registry.sqlite3-wal");
    let allocated = |path: &Path| fs::allocated_bytes(path, &std::fs::metadata(path).unwrap());
    let before = allocated(&wal);
    assert!(
        before > 64 * 1024,
        "fixture did not allocate meaningful WAL storage"
    );
    registry.maintain_ci_state(1000.0, 64).unwrap();
    checkpointer.run_pass().unwrap();
    assert!(
        allocated(&wal) < before,
        "maintenance left allocated WAL untouched"
    );
    assert_eq!(registry.cache_helper(NONCE).unwrap(), Some(helper(100.0)));
}

fn helper(at: f64) -> CacheHelperRecord {
    CacheHelperRecord {
        schema_version: 1,
        intent: CacheHelperIntent {
            registry_id: OWNER.into(),
            nonce: NONCE.into(),
            volume: "bosn-ci-cache-v1".into(),
            image: format!("docker.io/library/docker@sha256:{}", "a".repeat(64)),
            created_at: 1.0,
            role: None,
        },
        state: CacheHelperState::Created,
        container_id: Some("b".repeat(64)),
        updated_at: at,
    }
}

fn legacy(path: &Path, history: usize) {
    drop(fs::create_private_file(path).unwrap());
    let mut connection = Connection::open(path).unwrap();
    let mut tx = connection.begin_immediate().unwrap();
    for statement in crate::SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        tx.execute(statement, &[]).unwrap();
    }
    tx.execute(
        "INSERT INTO meta VALUES('schema_version','5'),('registry_id',?)",
        &[Value::Text(OWNER.into())],
    )
    .unwrap();
    for index in 1..=history {
        tx.execute(
            "INSERT INTO events(at,kind,detail) VALUES(?,?,?)",
            &[
                Value::Real(index as f64),
                Value::Text(format!("{}{}", cache_helper::PREFIX, NONCE)),
                Value::Text(serde_json::to_string(&helper(index as f64)).unwrap()),
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

#[test]
fn interrupted_backfill_is_gated_and_resumes_without_replacing_newest_authority() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    legacy(&path, 400);
    let mut registry = Registry::open_writer(&path).unwrap();
    assert_eq!(
        registry.meta("schema_version").unwrap().as_deref(),
        Some("6")
    );
    assert!(matches!(
        registry.begin_immediate(),
        Err(Error::MigrationInProgress)
    ));
    assert!(matches!(
        registry.cache_helper(NONCE),
        Err(Error::MigrationInProgress)
    ));
    assert!(!registry.migrate_ci_step().unwrap());
    drop(registry);
    let mut registry = Registry::open_writer(&path).unwrap();
    let mut pages = 1;
    while !registry.migrate_ci_step().unwrap() {
        pages += 1;
        assert!(pages < 10);
    }
    assert_eq!(registry.cache_helper(NONCE).unwrap(), Some(helper(400.0)));
    assert_eq!(
        registry
            .pending_cache_helpers(None, 64)
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(registry.events(0, 100).unwrap().items.len(), 3);
    assert!(registry.migrate_ci_step().unwrap());
    drop(registry);
    assert_eq!(
        Registry::open_read_only(&path)
            .unwrap()
            .cache_helper(NONCE)
            .unwrap(),
        Some(helper(400.0))
    );
}

#[test]
fn invalid_latest_snapshot_refuses_migration_without_advancing_cursor() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    legacy(&path, 200);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("UPDATE events SET detail='{}' WHERE id=200", &[])
        .unwrap();
    drop(connection);
    let mut registry = Registry::open_writer(&path).unwrap();
    let cursor = registry.meta("ci.migration.cursor").unwrap();
    assert!(registry.migrate_ci_step().is_err());
    assert_eq!(registry.meta("ci.migration.cursor").unwrap(), cursor);
    assert!(matches!(
        registry.begin_immediate(),
        Err(Error::MigrationInProgress)
    ));
}

#[test]
fn unrelated_large_events_are_preserved_without_blocking_bounded_backfill() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    legacy(&path, 200);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO events(at,kind,detail) VALUES(0,'unrelated',?)",
            &[Value::Text("x".repeat(9 * 1024 * 1024))],
        )
        .unwrap();
    drop(connection);
    let mut registry = Registry::open_writer(&path).unwrap();
    while !registry.migrate_ci_step().unwrap() {}
    assert_eq!(registry.cache_helper(NONCE).unwrap(), Some(helper(200.0)));
    let rows = registry
        .connection
        .query(
            "SELECT length(detail) FROM events WHERE kind='unrelated'",
            &[],
            QueryLimits::default(),
        )
        .unwrap();
    assert_eq!(integer(&rows[0], 0).unwrap(), 9 * 1024 * 1024);
}

#[test]
fn snapshot_churn_keeps_three_audit_rows_and_one_current_record() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    for index in 1..=500 {
        let mut tx = registry.begin_immediate().unwrap();
        let snapshot = helper(index as f64);
        tx.append_event(
            snapshot.updated_at,
            &format!("{}{}", cache_helper::PREFIX, NONCE),
            &serde_json::to_string(&snapshot).unwrap(),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(registry.events(0, 100).unwrap().items.len(), 3);
    assert_eq!(registry.cache_helper(NONCE).unwrap(), Some(helper(500.0)));
    let rows = registry
        .connection
        .query(
            "SELECT COUNT(*) FROM ci_current",
            &[],
            QueryLimits::default(),
        )
        .unwrap();
    assert_eq!(integer(&rows[0], 0).unwrap(), 1);
}

#[test]
fn oversized_managed_snapshot_is_rejected_before_audit_or_current_mutation() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let mut registry = Registry::create_writer(dir.path().join("registry.sqlite3"), OWNER).unwrap();
    let mut detail = serde_json::to_string(&helper(1.0)).unwrap();
    detail.push_str(&" ".repeat(cache_helper::RECORD_BYTES));
    let mut tx = registry.begin_immediate().unwrap();
    assert!(matches!(
        tx.append_event(1.0, &format!("{}{}", cache_helper::PREFIX, NONCE), &detail),
        Err(Error::BadRow("cache helper snapshot size"))
    ));
    tx.commit().unwrap();
    assert!(registry.cache_helper(NONCE).unwrap().is_none());
    assert!(registry.events(0, 100).unwrap().items.is_empty());
}

#[test]
fn imported_history_cannot_replace_newer_current_authority() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let mut registry = Registry::create_writer(dir.path().join("registry.sqlite3"), OWNER).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    for id in [100, 50] {
        tx.put_event_with_id(&Event {
            id,
            at: id as f64,
            kind: format!("{}{}", cache_helper::PREFIX, NONCE),
            detail: serde_json::to_string(&helper(id as f64)).unwrap(),
        })
        .unwrap();
    }
    tx.commit().unwrap();
    assert_eq!(registry.cache_helper(NONCE).unwrap(), Some(helper(100.0)));
}

#[test]
fn altered_queue_index_is_refused_before_writer_admission() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    drop(Registry::create_writer(&path, OWNER).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection.execute("DROP INDEX ci_pending", &[]).unwrap();
    connection
        .execute("CREATE INDEX ci_pending ON ci_current(id)", &[])
        .unwrap();
    drop(connection);
    assert!(matches!(
        Registry::open_writer(&path),
        Err(Error::InvalidSchema)
    ));
}

#[test]
fn trusted_maintenance_expires_removed_helpers_and_rejects_stale_intents() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let mut registry = Registry::create_writer(dir.path().join("registry.sqlite3"), OWNER).unwrap();
    let old = helper(1.0).intent;
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_cache_helper(&old).unwrap();
    tx.register_cache_helper(&old.nonce, &"b".repeat(64), 2.0)
        .unwrap();
    tx.finish_cache_helper(&old.nonce, &"b".repeat(64), 3.0)
        .unwrap();
    tx.commit().unwrap();
    let mut fresh = old.clone();
    fresh.nonce = "ffffffff-bbbb-4ccc-8ddd-eeeeeeeeeeee".into();
    fresh.created_at = 8.0 * 86400.0;
    assert_eq!(registry.maintain_ci_state(fresh.created_at, 64).unwrap(), 1);
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_cache_helper(&fresh).unwrap();
    tx.commit().unwrap();
    assert!(
        registry.cache_helper(&old.nonce).unwrap().is_none(),
        "expired terminal authority grew forever"
    );
    let mut tx = registry.begin_immediate().unwrap();
    assert!(
        tx.begin_cache_helper(&old).is_err(),
        "forgotten intent was replayed"
    );
}

#[test]
fn protected_capacity_refuses_new_helpers_without_blocking_existing_recovery() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let mut registry = Registry::create_writer(dir.path().join("registry.sqlite3"), OWNER).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    for index in 0..10_000 {
        let mut intent = helper(1.0).intent;
        intent.nonce = format!("{index:08x}-bbbb-4ccc-8ddd-eeeeeeeeeeee");
        tx.begin_cache_helper(&intent).unwrap();
    }
    assert!(
        tx.begin_cache_helper(&helper(1.0).intent).is_err(),
        "unbounded protected authority accepted new work"
    );
    tx.register_cache_helper("00000000-bbbb-4ccc-8ddd-eeeeeeeeeeee", &"b".repeat(64), 2.0)
        .unwrap();
    tx.commit().unwrap();
}

#[test]
fn future_intent_timestamp_cannot_expire_history_or_advance_replay_floor() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let mut registry = Registry::create_writer(dir.path().join("registry.sqlite3"), OWNER).unwrap();
    let old = helper(1.0).intent;
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_cache_helper(&old).unwrap();
    tx.finish_absent_cache_helper(&old.nonce, 632.0).unwrap();
    tx.commit().unwrap();
    let mut future = old.clone();
    future.nonce = "ffffffff-bbbb-4ccc-8ddd-eeeeeeeeeeee".into();
    future.created_at = 100.0 * 365.0 * 86400.0;
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_cache_helper(&future).unwrap();
    tx.commit().unwrap();
    assert!(
        registry.cache_helper(&old.nonce).unwrap().is_some(),
        "intent timestamp acted as maintenance authority"
    );
    assert!(registry.meta("ci.retention.floor").unwrap().is_none());
}
