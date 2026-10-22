//! The Bosn-labelled Docker footprint and its machine ceiling (#508).
//!
//! Free disk alone is not owned allocation: a roomy disk can still hold an unbounded pile of
//! Bosn containers, volumes and images. The pass sums what it observed by kind; past
//! [`DEFAULT_OWNED_CEILING`] it uses the same pressured policy a full disk does, so every gate
//! still applies and only the budget and the stopped-container age shorten.

use super::RetentionPolicy;
use crate::{ObservedArtifact, ResourceKind};

/// Bytes of Bosn-labelled containers, volumes and images held before the pass treats the
/// machine as under pressure.
pub const DEFAULT_OWNED_CEILING: i128 = 256 * 1024 * 1024 * 1024;

/// Measured bytes per kind; objects Docker reported no size for are counted, not guessed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OwnedFootprint {
    pub containers: i128,
    pub volumes: i128,
    pub images: i128,
    pub unmeasured: u64,
}

impl OwnedFootprint {
    #[must_use]
    pub fn of(artifacts: &[ObservedArtifact]) -> Self {
        let mut footprint = Self::default();
        for artifact in artifacts {
            let Some(bytes) = artifact.bytes else {
                footprint.unmeasured += 1;
                continue;
            };
            let slot = match artifact.kind {
                ResourceKind::Container => &mut footprint.containers,
                ResourceKind::Volume => &mut footprint.volumes,
                ResourceKind::Image => &mut footprint.images,
                ResourceKind::Builder | ResourceKind::Network => continue,
            };
            *slot = slot.saturating_add(bytes.max(0));
        }
        footprint
    }

    #[must_use]
    pub fn total(self) -> i128 {
        self.containers
            .saturating_add(self.volumes)
            .saturating_add(self.images)
    }

    #[must_use]
    pub fn over(self, ceiling: i128) -> bool {
        self.total() > ceiling
    }
}

impl RetentionPolicy {
    /// [`Self::under_disk_pressure`] when the measured footprint exceeds `ceiling`, else `self`.
    #[must_use]
    pub fn for_footprint(self, footprint: OwnedFootprint, ceiling: i128) -> Self {
        if footprint.over(ceiling) {
            self.under_disk_pressure()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(kind: ResourceKind, bytes: Option<i128>) -> ObservedArtifact {
        ObservedArtifact {
            id: "x".into(),
            kind,
            labels: Default::default(),
            signals: Default::default(),
            bytes,
            age_seconds: None,
        }
    }

    #[test]
    fn the_footprint_sums_by_kind_and_counts_unmeasured_objects() {
        let footprint = OwnedFootprint::of(&[
            artifact(ResourceKind::Container, Some(5)),
            artifact(ResourceKind::Volume, Some(7)),
            artifact(ResourceKind::Volume, None),
            artifact(ResourceKind::Image, Some(11)),
        ]);
        assert_eq!(
            footprint,
            OwnedFootprint {
                containers: 5,
                volumes: 7,
                images: 11,
                unmeasured: 1
            }
        );
        assert_eq!(footprint.total(), 23);
    }

    #[test]
    fn only_a_footprint_past_the_ceiling_pressures_the_policy() {
        let policy = RetentionPolicy::default();
        let under = OwnedFootprint {
            volumes: 10,
            ..OwnedFootprint::default()
        };
        assert_eq!(policy.for_footprint(under, 10), policy);
        let over = OwnedFootprint {
            volumes: 11,
            ..OwnedFootprint::default()
        };
        assert_eq!(policy.for_footprint(over, 10), policy.under_disk_pressure());
    }
}
