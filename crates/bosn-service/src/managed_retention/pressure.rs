//! The disk-pressure probe for the maintenance pass (#658).
//!
//! The planner in `bosn-core` stays pure: this module measures the filesystem holding Docker's
//! data root and hands back a typed [`DiskSpace`], which the pass turns into a pressured policy.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use bosn_core::retention::{DiskSpace, RetentionPolicy};
use bosn_engine::DockerEngine;

/// Set once the "cannot measure" line has been printed, so a Docker Desktop VM or a remote engine
/// does not log it on every interval.
static UNMEASURED_LOGGED: AtomicBool = AtomicBool::new(false);

/// Free and total bytes under `engine`'s data root, or `None` when the root is unknown or not
/// visible from this process's mount namespace. `None` is "no pressure", never pressure.
#[must_use]
pub fn docker_root_disk(engine: &DockerEngine) -> Option<DiskSpace> {
    let root = crate::unmanaged::docker_root_dir(engine)?;
    measure(&root)
}

fn measure(root: &Path) -> Option<DiskSpace> {
    let free_bytes = kernal_api::resources_available_space(root).ok()?;
    let total_bytes = kernal_api::resources_total_space(root).ok()?;
    Some(DiskSpace {
        free_bytes,
        total_bytes,
    })
}

/// `policy` adjusted for the measured disk, logging the decision.
pub fn policy_for_disk(policy: RetentionPolicy, disk: Option<DiskSpace>) -> RetentionPolicy {
    match disk {
        None => {
            if !UNMEASURED_LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "bosn retention: cannot measure the disk under Docker's data root; \
                     assuming no disk pressure"
                );
            }
        }
        Some(space) => {
            if let Some(line) = pressure_line(space) {
                eprintln!("bosn retention: {line}");
            }
        }
    }
    policy.for_disk(disk)
}

/// The pressure log line, or `None` when the disk is not under pressure.
pub(super) fn pressure_line(space: DiskSpace) -> Option<String> {
    space.under_pressure().then(|| {
        format!(
            "disk pressure ({:.1} GiB free, {:.1}%); idle volume budget 0",
            space.free_gib(),
            space.free_percent(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pressure_line_names_free_space_and_the_zero_budget() {
        let gib = 1024 * 1024 * 1024;
        let full = DiskSpace {
            free_bytes: 83 * gib,
            total_bytes: 1660 * gib,
        };
        assert_eq!(
            pressure_line(full).as_deref(),
            Some("disk pressure (83.0 GiB free, 5.0%); idle volume budget 0")
        );
        let roomy = DiskSpace {
            free_bytes: 800 * gib,
            total_bytes: 1660 * gib,
        };
        assert_eq!(pressure_line(roomy), None);
    }

    #[test]
    fn an_unmeasured_disk_leaves_the_policy_alone() {
        let policy = RetentionPolicy::default();
        assert_eq!(policy_for_disk(policy, None), policy);
    }

    #[test]
    fn an_unreadable_root_is_not_measured() {
        assert_eq!(measure(Path::new("/nonexistent/bosn/docker-root")), None);
    }
}
