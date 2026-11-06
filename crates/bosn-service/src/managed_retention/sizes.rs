//! Volume sizes for managed retention (#549).
//!
//! `docker volume inspect` never reports a volume's size: Docker computes `UsageData` only for
//! `/system/df`. So a pass that observed owned volumes reads `docker system df -v` once. The read
//! walks every volume, so it is taken only when there is a volume to size, and a failed read
//! leaves sizes unmeasured rather than refusing the pass: sizes bound a budget and account for
//! reclaimed bytes, they never decide ownership.

use std::{collections::BTreeMap, time::Duration};

use bosn_core::SystemDfReport;
use bosn_engine::{CensusRead, DockerEngine, RunOptions};

/// `system df -v` sizes every volume on the host, so it is the slowest read of a pass: on the
/// development host (41 Bosn volumes, about 200 GB) it overran the 30 s per-read deadline, and
/// every volume was left unmeasured, so the owned-footprint ceiling saw volumes as 0 bytes (#508).
pub(super) const VOLUME_SIZE_DEADLINE: Duration = Duration::from_secs(300);
const VOLUME_SIZE_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

/// What one `docker system df -v` read measured.
#[derive(Debug, Default)]
pub(super) struct DfSizes {
    /// Each volume's size in bytes, by name.
    pub volumes: BTreeMap<String, i128>,
    /// Each image's (unique, shared) bytes, by id.
    images: BTreeMap<String, (i128, i128)>,
}

impl DfSizes {
    /// The disk `ids` hold without counting shared layers once per image (#771): every image's
    /// unique bytes plus the largest shared figure. Docker does not say which images share what,
    /// so several unrelated bases are undercounted; the free-disk check still covers those. `None`
    /// when any image is unmeasured.
    pub(super) fn image_bytes<'a>(&self, ids: impl Iterator<Item = &'a str>) -> Option<i128> {
        let mut unique = 0_i128;
        let mut shared = 0_i128;
        for id in ids {
            let (image_unique, image_shared) = self.images.get(id)?;
            unique = unique.saturating_add(*image_unique);
            shared = shared.max(*image_shared);
        }
        Some(unique.saturating_add(shared))
    }
}

/// The sizes `docker system df -v` reports.
///
/// # Errors
/// The read failed or its document did not parse; the caller leaves sizes unmeasured.
pub(super) fn df_sizes(engine: &DockerEngine, deadline: Duration) -> Result<DfSizes, String> {
    let options = RunOptions::bounded(deadline, VOLUME_SIZE_OUTPUT_LIMIT);
    let text = match engine.system_df_verbose(options) {
        Ok(CensusRead::Document(text)) => text,
        Ok(CensusRead::Unavailable { detail }) => {
            return Err(format!("docker system df -v: {detail}"));
        }
        Err(error) => return Err(format!("docker system df -v: {error}")),
    };
    let report = serde_json::from_str::<SystemDfReport>(&text)
        .map_err(|error| format!("docker system df -v: {error}"))?;
    Ok(DfSizes {
        volumes: report
            .volumes
            .into_iter()
            .filter_map(|volume| Some((volume.name, bosn_core::parse_docker_size(&volume.size)?)))
            .collect(),
        images: report
            .images
            .into_iter()
            .filter_map(|image| {
                let unique = bosn_core::parse_docker_size(&image.unique_size)?;
                let shared = bosn_core::parse_docker_size(&image.shared_size)?;
                Some((image.id, (unique, shared)))
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slow_df(seconds: u32) -> DockerEngine {
        let script = format!(
            "sleep {seconds}; printf '%s' '{{\"Volumes\":[{{\"Name\":\"bosn-v-owned\",\"Size\":\"2GB\"}}]}}'"
        );
        DockerEngine::synthetic_for_test("/bin/sh", ["-c".to_owned(), script, "fake-docker".into()])
    }

    #[test]
    fn the_size_read_outlives_the_per_read_deadline() {
        assert!(VOLUME_SIZE_DEADLINE >= super::super::RETENTION_READ_DEADLINE * 10);
        let sizes = df_sizes(&slow_df(1), Duration::from_secs(5)).unwrap();
        assert_eq!(sizes.volumes.get("bosn-v-owned"), Some(&2_000_000_000));
    }

    #[test]
    fn an_overrun_is_an_error_not_an_empty_map() {
        let error = df_sizes(&slow_df(5), Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("docker system df -v"), "{error}");
    }

    #[test]
    fn a_shared_base_counts_once_and_an_unmeasured_image_refuses_a_figure() {
        let sizes = DfSizes {
            volumes: BTreeMap::new(),
            images: BTreeMap::from([("a".to_owned(), (100, 9000)), ("b".to_owned(), (200, 9000))]),
        };
        assert_eq!(sizes.image_bytes(["a", "b"].into_iter()), Some(9300));
        assert_eq!(sizes.image_bytes(["a", "gone"].into_iter()), None);
    }
}
