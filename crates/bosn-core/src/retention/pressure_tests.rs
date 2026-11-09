//! #777: compare typed disk deficits with the existing planner's measured bytes.
use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn deficit_uses_the_larger_threshold_and_rounds_percentage_up() {
    assert_eq!(
        DiskSpace {
            free_bytes: PRESSURE_FREE_BYTES - 10,
            total_bytes: 500 * GIB
        }
        .deficit_bytes(),
        10
    );
    assert_eq!(
        DiskSpace {
            free_bytes: 140 * GIB,
            total_bytes: 1000 * GIB
        }
        .deficit_bytes(),
        i128::from(10 * GIB)
    );
    assert_eq!(
        DiskSpace {
            free_bytes: 150 * GIB,
            total_bytes: 1000 * GIB + 1
        }
        .deficit_bytes(),
        1
    );
    assert_eq!(
        DiskSpace {
            free_bytes: 150 * GIB,
            total_bytes: 1000 * GIB
        }
        .deficit_bytes(),
        0
    );
    assert_eq!(
        DiskSpace {
            free_bytes: 0,
            total_bytes: 0
        }
        .deficit_bytes(),
        0
    );
    assert!(
        DiskSpace {
            free_bytes: 0,
            total_bytes: u64::MAX
        }
        .deficit_bytes()
            > 0
    );
}

#[test]
fn advisory_requires_pressure_and_deficit_strictly_above_reclaimable() {
    let disk = Some(DiskSpace {
        free_bytes: PRESSURE_FREE_BYTES - 10,
        total_bytes: 500 * GIB,
    });
    let mut plan = RetentionPlan {
        bytes: 9,
        ..Default::default()
    };
    assert!(plan.compaction_advisory(disk));
    plan.bytes = 10;
    assert!(!plan.compaction_advisory(disk), "exactly enough");
    plan.bytes = 11;
    assert!(!plan.compaction_advisory(disk));
    assert!(!plan.compaction_advisory(None));
    plan.bytes = 0;
    assert!(!plan.compaction_advisory(Some(DiskSpace {
        free_bytes: 150 * GIB,
        total_bytes: 1000 * GIB
    })));
    assert!(
        plan.compaction_advisory(disk),
        "unmeasured bytes never count as reclaimed"
    );
}
