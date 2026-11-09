use super::*;

#[test]
fn rollups_count_each_metric_independently_and_keep_nulls() {
    let mut history = History::default();
    for (at_ms, cpu, load) in [
        (60_000, Some(10.0), None),
        (70_000, None, Some(6.0)),
        (80_000, Some(30.0), None),
    ] {
        history.record(&Sample {
            at_ms,
            cpu_percent: cpu,
            load1: load,
            ..Sample::default()
        });
    }
    let bucket = history.tiers[1][1].as_ref().unwrap();
    assert_eq!(bucket.n, 3);
    assert_eq!(
        bucket.metrics[0],
        Metric {
            avg: Some(20.0),
            max: Some(30.0),
            n: 2
        }
    );
    assert_eq!(bucket.metrics[2].avg, Some(6.0));
    assert_eq!(bucket.metrics[3], Metric::default());
    let point = bucket.point();
    assert_eq!(point.avg.cpu_percent, Some(20.0));
    assert_eq!(point.max.cpu_percent, Some(30.0));
    assert_eq!(point.avg.mem_used_bytes, None);
}

#[test]
fn slots_wrap_and_stale_slots_become_gaps() {
    let mut history = History::default();
    history.record(&Sample {
        at_ms: 10_000,
        cpu_percent: Some(1.0),
        ..Sample::default()
    });
    let later = 3_610_000;
    assert!(
        history
            .points(Range::Hour, later)
            .iter()
            .all(|p| p.avg.cpu_percent.is_none())
    );
    history.record(&Sample {
        at_ms: later,
        cpu_percent: Some(9.0),
        ..Sample::default()
    });
    assert_eq!(history.tiers[0][1].as_ref().unwrap().at_ms, later);
    assert_eq!(
        history
            .points(Range::Hour, later)
            .last()
            .unwrap()
            .max
            .cpu_percent,
        Some(9.0)
    );
    history.load(0, Bucket::empty(10_000), later);
    assert_eq!(history.tiers[0][1].as_ref().unwrap().at_ms, later);
}

#[test]
fn raw_points_have_equal_average_and_maximum() {
    let mut history = History::default();
    history.record(&Sample {
        at_ms: 60_000,
        cpu_percent: Some(25.0),
        ..Sample::default()
    });
    let points = history.points(Range::Hour, 60_000);
    let point = points.last().unwrap();
    assert_eq!(point.avg.cpu_percent, point.max.cpu_percent);
}

#[test]
fn all_null_bucket_stays_null() {
    let mut history = History::default();
    history.record(&Sample {
        at_ms: 60_000,
        ..Sample::default()
    });
    let bucket = history.tiers[1][1].as_ref().unwrap();
    assert_eq!(bucket.n, 1);
    assert!(bucket.metrics.iter().all(|m| *m == Metric::default()));
}
