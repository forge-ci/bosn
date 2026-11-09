use super::super::{Sample, history::Range};
use super::*;

fn sample(at_ms: u64, cpu: f64) -> Sample {
    Sample {
        at_ms,
        cpu_percent: Some(cpu),
        ..Sample::default()
    }
}

#[test]
fn persistence_round_trip_and_open_bucket_continues() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::default();
    history.record(&sample(60_000, 10.0));
    let mut store = Store::new(dir.path());
    assert!(store.flush(&mut history, 70_000).unwrap());
    drop(store);
    let mut store = Store::new(dir.path());
    let mut restored = History::default();
    store.flush(&mut restored, 70_000).unwrap();
    assert_eq!(restored.tiers, history.tiers);
    restored.record(&sample(80_000, 30.0));
    store.flush(&mut restored, 80_000).unwrap();
    drop(store);
    let connection = Connection::open(dir.path().join("system-history.sqlite3")).unwrap();
    connection.integrity_check().unwrap();
    let loaded = load(&connection, 80_000).unwrap();
    let point = loaded.points(Range::Day, 80_000).pop().unwrap();
    assert_eq!(point.avg.cpu_percent, Some(20.0));
    assert_eq!(point.max.cpu_percent, Some(30.0));
}

#[test]
fn second_writer_stays_in_memory_then_merges_at_takeover() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = Store::new(dir.path());
    let mut second = Store::new(dir.path());
    let mut a = History::default();
    let mut b = History::default();
    a.record(&sample(60_000, 10.0));
    assert!(first.flush(&mut a, 70_000).unwrap());
    b.record(&sample(80_000, 30.0));
    assert!(!second.flush(&mut b, 80_000).unwrap());
    drop(first);
    assert!(second.flush(&mut b, 80_000).unwrap());
    assert_eq!(b.tiers[1][1].as_ref().unwrap().metrics[0].avg, Some(20.0));
    // A repeated flush must not merge persisted samples twice.
    assert!(second.flush(&mut b, 80_000).unwrap());
    assert_eq!(b.tiers[1][1].as_ref().unwrap().n, 2);
}

#[test]
fn unknown_schema_is_moved_aside_without_touching_registry() {
    for version in [0, 42] {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("system-history.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(&format!("PRAGMA user_version={version}"), &[])
            .unwrap();
        drop(connection);
        std::fs::write(dir.path().join("registry.sqlite3"), "untouched").unwrap();
        let mut store = Store::new(dir.path());
        let mut history = History::default();
        history.record(&sample(60_000, 10.0));
        assert!(store.flush(&mut history, 60_000).unwrap());
        assert!(dir.path().join("system-history.sqlite3.old").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("registry.sqlite3")).unwrap(),
            "untouched"
        );
        store.open().unwrap().integrity_check().unwrap();
    }
}

#[test]
fn ten_rotations_never_exceed_the_sum_of_capacities() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::new(dir.path());
    let mut history = History::default();
    for (tier, (step, capacity)) in TIERS.iter().copied().enumerate() {
        for tick in 0..capacity * 10 {
            let at = tick as u64 * step;
            let slot = tick % capacity;
            let mut bucket = Bucket::empty(at);
            bucket.n = 1;
            history.tiers[tier][slot] = Some(bucket);
            // Exercise INSERT OR REPLACE for every rotation in small transactions.
            if tick % capacity == capacity - 1 {
                store.flush(&mut history, at).unwrap();
            }
        }
    }
    drop(store);
    let connection = Connection::open(dir.path().join("system-history.sqlite3")).unwrap();
    let rows = connection
        .query("SELECT COUNT(*) FROM samples", &[], QueryLimits::default())
        .unwrap();
    assert_eq!(
        integer(rows[0].get(0)).unwrap() as usize,
        TIERS.iter().map(|t| t.1).sum::<usize>()
    );
    connection.integrity_check().unwrap();
}
