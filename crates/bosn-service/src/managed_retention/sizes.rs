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

/// Each volume's size in bytes, by name.
///
/// # Errors
/// The read failed or its document did not parse; the caller leaves sizes unmeasured.
pub(super) fn volume_sizes(
    engine: &DockerEngine,
    deadline: Duration,
) -> Result<BTreeMap<String, i128>, String> {
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
    Ok(report
        .volumes
        .into_iter()
        .filter_map(|volume| Some((volume.name, bosn_core::parse_docker_size(&volume.size)?)))
        .collect())
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
        let sizes = volume_sizes(&slow_df(1), Duration::from_secs(5)).unwrap();
        assert_eq!(sizes.get("bosn-v-owned"), Some(&2_000_000_000));
    }

    #[test]
    fn an_overrun_is_an_error_not_an_empty_map() {
        let error = volume_sizes(&slow_df(5), Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("docker system df -v"), "{error}");
    }
}
