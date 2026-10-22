use super::*;

const OWNER: &str = "11111111-2222-4333-8444-555555555555";

#[test]
fn pending_wal_and_database_growth_share_the_projected_admission_budget() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    let usage = registry.storage_usage().unwrap();
    let physical = usage.allocated_bytes.unwrap();
    let database = registry.storage_pragma("PRAGMA page_count").unwrap()
        * registry.storage_pragma("PRAGMA page_size").unwrap();
    let sidecars = usage
        .files
        .iter()
        .filter(|file| file.kind != RegistryFileKind::Database)
        .map(|file| file.bytes.unwrap())
        .sum::<u64>();
    let projected = database + sidecars;
    assert!(
        projected > physical,
        "fixture must have database pages pending in WAL"
    );
    registry
        .set_ci_metadata_limit_bytes((physical + projected) / 2)
        .unwrap();
    let intent = cache_helper::CacheHelperIntent {
        registry_id: OWNER.into(),
        nonce: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into(),
        volume: "bosn-ci-cache-v1".into(),
        image: format!("docker.io/library/docker@sha256:{}", "a".repeat(64)),
        created_at: 1.0,
        role: None,
    };
    let mut tx = registry.begin_immediate().unwrap();
    assert!(
        tx.begin_cache_helper(&intent).is_err(),
        "pending WAL/database allocation escaped the budget"
    );
    tx.commit().unwrap();
    assert!(registry.cache_helper(&intent.nonce).unwrap().is_none());
}

#[test]
fn application_writes_do_not_inherit_automatic_wal_checkpoint_work() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let registry = Registry::create_writer(&path, OWNER).unwrap();
    assert_eq!(
        registry
            .storage_pragma("PRAGMA wal_autocheckpoint")
            .unwrap(),
        0
    );
    drop(registry);
    let registry = Registry::open_writer(&path).unwrap();
    assert_eq!(
        registry
            .storage_pragma("PRAGMA wal_autocheckpoint")
            .unwrap(),
        0
    );
}

#[test]
fn checkpoint_worker_reclaims_large_wal_and_keeps_the_writer_fence_until_close() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    let mut checkpointer = registry.take_storage_checkpointer().unwrap();
    assert!(registry.take_storage_checkpointer().is_none());
    let mut tx = registry.begin_immediate().unwrap();
    tx.append_event(1.0, "storage.test", &"x".repeat(5 * 1024 * 1024))
        .unwrap();
    tx.commit().unwrap();
    assert!(registry.reclaim_storage().unwrap().checkpoint_deferred);
    let report = checkpointer.run_pass().unwrap();
    assert!(!report.blocked);
    assert_eq!(report.usage.files[1].allocated_bytes, Some(0));
    assert_eq!(
        registry.storage_pragma("PRAGMA busy_timeout").unwrap(),
        5000
    );
    drop(registry);
    assert!(matches!(
        Registry::open_writer(&path),
        Err(Error::WriterAlreadyHeld(_))
    ));
    drop(checkpointer);
    let registry = Registry::open_writer(&path).unwrap();
    let rows = registry
        .connection
        .query(
            "SELECT COUNT(*) FROM events WHERE kind=? AND detail=?",
            &[
                Value::Text("storage.test".into()),
                Value::Text("x".repeat(5 * 1024 * 1024)),
            ],
            QueryLimits {
                max_rows: 1,
                max_bytes: 128,
            },
        )
        .unwrap();
    assert_eq!(integer(&rows[0], 0).unwrap(), 1);
}

#[test]
fn allocated_metadata_pressure_refuses_new_authority_but_preserves_existing_progress() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    let mut intent = cache_helper::CacheHelperIntent {
        registry_id: OWNER.into(),
        nonce: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into(),
        volume: "bosn-ci-cache-v1".into(),
        image: format!("docker.io/library/docker@sha256:{}", "a".repeat(64)),
        created_at: 1.0,
        role: None,
    };
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_cache_helper(&intent).unwrap();
    tx.commit().unwrap();
    registry
        .set_ci_metadata_limit_bytes(registry.storage_usage().unwrap().allocated_bytes.unwrap())
        .unwrap();
    let original = intent.nonce.clone();
    intent.nonce = "ffffffff-bbbb-4ccc-8ddd-eeeeeeeeeeee".into();
    let mut tx = registry.begin_immediate().unwrap();
    assert!(
        tx.begin_cache_helper(&intent).is_err(),
        "physical pressure admitted new authority"
    );
    tx.register_cache_helper(&original, &"b".repeat(64), 2.0)
        .unwrap();
    tx.commit().unwrap();
    assert!(registry.cache_helper(&intent.nonce).unwrap().is_none());
    assert!(
        registry
            .cache_helper(&original)
            .unwrap()
            .unwrap()
            .container_id
            .is_some()
    );
}

fn event(registry: &mut Registry, at: f64) {
    let mut tx = registry.begin_immediate().unwrap();
    tx.append_event(at, "storage.test", &"x".repeat(4096))
        .unwrap();
    tx.commit().unwrap();
}

#[test]
fn a_pinned_reader_defers_truncation_without_waiting_or_losing_its_snapshot() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    event(&mut registry, 1.0);
    let reader = Connection::open_read_only(&path).unwrap();
    reader.execute("BEGIN", &[]).unwrap();
    assert_eq!(
        reader
            .query("SELECT COUNT(*) FROM events", &[], QueryLimits::default())
            .unwrap()[0]
            .get(0),
        Some(&Value::Integer(1))
    );
    event(&mut registry, 2.0);
    let began = std::time::Instant::now();
    let held = registry.reclaim_storage().unwrap();
    assert!(held.checkpoint_blocked);
    assert!(began.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(
        reader
            .query("SELECT COUNT(*) FROM events", &[], QueryLimits::default())
            .unwrap()[0]
            .get(0),
        Some(&Value::Integer(1))
    );
    assert_eq!(
        registry.storage_pragma("PRAGMA busy_timeout").unwrap(),
        5000
    );
    drop(reader);
    let released = registry.reclaim_storage().unwrap();
    assert!(!released.checkpoint_blocked);
    assert_eq!(released.usage.files[1].allocated_bytes, Some(0));
    assert_eq!(registry.events(0, 10).unwrap().items.len(), 2);
}

#[test]
fn fresh_registries_incrementally_reclaim_free_pages() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    assert_eq!(registry.storage_pragma("PRAGMA auto_vacuum").unwrap(), 2);
    for at in 1..=100 {
        event(&mut registry, f64::from(at));
    }
    registry.reclaim_storage().unwrap();
    let before = registry.storage_usage().unwrap().allocated_bytes.unwrap();
    let mut tx = registry.connection.begin_immediate().unwrap();
    tx.execute("DELETE FROM events WHERE kind='storage.test'", &[])
        .unwrap();
    tx.commit().unwrap();
    let freed = registry.reclaim_storage().unwrap();
    assert!(freed.incremental_vacuum);
    assert!(freed.free_pages > 0, "fixture must exceed one 64-page pass");
    assert!(freed.usage.allocated_bytes.unwrap() < before);
    assert!(registry.events(0, 10).unwrap().items.is_empty());
}

#[test]
fn legacy_high_water_allocation_is_reported_without_rebuilding_or_losing_unrelated_data() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    drop(fs::create_private_file(&path).unwrap());
    let mut connection = Connection::open(&path).unwrap();
    let mut tx = connection.begin_immediate().unwrap();
    for sql in SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|sql| !sql.is_empty())
    {
        tx.execute(sql, &[]).unwrap();
    }
    tx.execute(
        "INSERT INTO meta VALUES('schema_version','5'),('registry_id',?)",
        &[Value::Text(OWNER.into())],
    )
    .unwrap();
    for _ in 0..128 {
        tx.execute(
            "INSERT INTO events(at,kind,detail) VALUES(1,'fixture.payload',?)",
            &[Value::Text("x".repeat(4096))],
        )
        .unwrap();
    }
    tx.execute(
        "INSERT INTO events(at,kind,detail) VALUES(1,'unrelated.keep','keep')",
        &[],
    )
    .unwrap();
    tx.commit().unwrap();
    drop(connection);
    let mut registry = Registry::open_writer(&path).unwrap();
    while !registry.migrate_ci_step().unwrap() {}
    let before = registry.reclaim_storage().unwrap();
    assert!(!before.incremental_vacuum);
    let mut tx = registry.connection.begin_immediate().unwrap();
    tx.execute("DELETE FROM events WHERE kind='fixture.payload'", &[])
        .unwrap();
    tx.commit().unwrap();
    let after = registry.reclaim_storage().unwrap();
    assert!(!after.incremental_vacuum);
    assert!(after.free_pages > 0);
    assert_eq!(after.usage.files[0].bytes, before.usage.files[0].bytes);
    assert_eq!(
        after.usage.files[0].allocated_bytes,
        before.usage.files[0].allocated_bytes
    );
    let events = registry.events(0, 10).unwrap();
    assert_eq!(events.items.len(), 1);
    assert_eq!(events.items[0].detail, "keep");
}

#[cfg(unix)]
#[test]
fn sparse_files_and_hard_links_are_counted_by_allocation_and_unique_inode() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let file = fs::create_private_file(&path).unwrap();
    file.set_len(16 * 1024 * 1024).unwrap();
    std::fs::hard_link(&path, dir.path().join("registry.sqlite3-wal")).unwrap();
    let report = registry_storage_usage(&path);
    assert!(!report.partial);
    assert_eq!(report.bytes, Some(16 * 1024 * 1024));
    assert_eq!(
        report.allocated_bytes,
        Some(fs::allocated_bytes(&path, &file.metadata().unwrap()))
    );
    assert!(report.allocated_bytes.unwrap() < report.bytes.unwrap());
}

#[cfg(unix)]
#[test]
fn symlinked_sidecars_are_unknown_and_never_count_external_storage_as_owned() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    drop(fs::create_private_file(&path).unwrap());
    let outside = dir.path().join("unrelated");
    drop(fs::create_private_file(&outside).unwrap());
    std::os::unix::fs::symlink(&outside, dir.path().join("registry.sqlite3-wal")).unwrap();
    let report = registry_storage_usage(&path);
    assert!(report.partial);
    assert_eq!(report.allocated_bytes, None);
    assert_eq!(report.files[1].allocated_bytes, None);
    assert!(outside.exists());
}

#[test]
fn absent_database_is_unknown_and_a_replaced_writer_path_is_refused() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    assert!(registry_storage_usage(&path).partial);
    #[cfg(unix)]
    {
        let registry = Registry::create_writer(&path, OWNER).unwrap();
        std::fs::rename(&path, dir.path().join("original.sqlite3")).unwrap();
        drop(fs::create_private_file(&path).unwrap());
        assert!(matches!(
            registry.storage_usage(),
            Err(Error::ReplacedPath(_))
        ));
    }
}

#[test]
fn large_wal_is_deferred_instead_of_copied_on_the_registry_actor() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.append_event(1.0, "storage.test", &"x".repeat(5 * 1024 * 1024))
        .unwrap();
    tx.commit().unwrap();
    let before = registry.storage_usage().unwrap();
    assert!(before.files[1].bytes.unwrap() > 4 * 1024 * 1024);
    let report = registry.reclaim_storage().unwrap();
    assert!(report.checkpoint_deferred);
    assert_eq!(
        report.usage.files[1].allocated_bytes,
        before.files[1].allocated_bytes
    );
}

#[test]
fn uncommitted_new_pages_cannot_bypass_the_metadata_budget() {
    let dir = fs::TemporaryDirectory::new().unwrap();
    let path = dir.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    registry.reclaim_storage().unwrap();
    let limit = registry.storage_usage().unwrap().allocated_bytes.unwrap() + 256 * 1024;
    let initial_database_bytes = registry.storage_pragma("PRAGMA page_count").unwrap()
        * registry.storage_pragma("PRAGMA page_size").unwrap();
    registry.set_ci_metadata_limit_bytes(limit).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    let mut refused = false;
    for index in 0..1000 {
        let intent = cache_helper::CacheHelperIntent {
            registry_id: OWNER.into(),
            nonce: format!("{index:08x}-1111-4111-8111-111111111111"),
            volume: "bosn-ci-cache-v1".into(),
            image: format!("docker.io/library/docker@sha256:{}", "a".repeat(64)),
            created_at: 1.0,
            role: None,
        };
        match tx.begin_cache_helper(&intent) {
            Ok(()) => {}
            Err(Error::CiMetadataStoragePressure {
                allocated_bytes,
                database_bytes,
                projected_bytes,
                limit_bytes,
            }) => {
                assert!(
                    allocated_bytes < limit_bytes,
                    "fixture must exercise unflushed growth"
                );
                assert!(database_bytes > initial_database_bytes);
                assert!(projected_bytes >= limit_bytes);
                refused = true;
                break;
            }
            Err(error) => panic!("unexpected refusal: {error}"),
        }
    }
    assert!(refused, "one transaction bypassed its metadata budget");
    tx.commit().unwrap();
}
