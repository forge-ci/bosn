//! Bring containers created before #643 down to Bosn's low CPU weight (#645).
//!
//! Long-lived containers (setup apps, the shared act engine) keep the weight they were
//! created with. At daemon start Bosn lowers every running container it owns in place:
//! `docker update` changes only the weight, never the container, so this is safe to
//! repeat. Moving a container into `bosn.slice` needs a recreation, which its own
//! lifecycle performs; this pass only closes the weight gap.
use bosn_engine::{DockerEngine, LOW_CPU_PRIORITY_ARG, RunOptions};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(30);
const OUTPUT: usize = 1024 * 1024;

/// The `docker ps` filters naming running containers this daemon may re-weight: those
/// labelled with this registry, and setup containers (machine-scoped, no registry).
#[must_use]
pub(crate) fn owned_filters(registry_id: &str) -> [String; 2] {
    [
        format!("label={}={registry_id}", bosn_core::LABEL_REGISTRY),
        format!("label={}", bosn_setup::SETUP_MANAGED_LABEL),
    ]
}

/// The `docker update` arguments for `ids`, or `None` when there is nothing to do.
#[must_use]
pub(crate) fn update_args(ids: &[String]) -> Option<Vec<String>> {
    if ids.is_empty() {
        return None;
    }
    let mut args = vec!["update".to_owned(), LOW_CPU_PRIORITY_ARG.to_owned()];
    args.extend(ids.iter().cloned());
    Some(args)
}

/// Lower every running owned container to the low weight. Best effort: a failure is
/// reported, never fatal, because priority must never be the reason the daemon stops.
pub(crate) fn reweight_running(registry_id: &str) {
    let docker = DockerEngine::docker();
    let options = || RunOptions::bounded(DEADLINE, OUTPUT);
    let mut ids = Vec::new();
    for filter in owned_filters(registry_id) {
        match docker
            .with_args(["ps", "-q", "--no-trunc", "--filter", filter.as_str()])
            .capture(options())
        {
            Ok(result) if result.ok() => ids.extend(
                String::from_utf8_lossy(&result.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned),
            ),
            Ok(result) => {
                eprintln!(
                    "bosn priority: listing owned containers exited {}",
                    result.exit_code
                );
                return;
            }
            Err(error) => {
                eprintln!("bosn priority: listing owned containers failed: {error}");
                return;
            }
        }
    }
    ids.sort();
    ids.dedup();
    let Some(args) = update_args(&ids) else {
        return;
    };
    match docker.with_args(args).capture(options()) {
        Ok(result) if result.ok() => eprintln!(
            "bosn priority: {} running container(s) at {LOW_CPU_PRIORITY_ARG}; placement {}",
            ids.len(),
            bosn_engine::placement().as_str()
        ),
        Ok(result) => eprintln!("bosn priority: docker update exited {}", result.exit_code),
        Err(error) => eprintln!("bosn priority: docker update failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_name_this_registry_and_setup_containers_only() {
        assert_eq!(
            owned_filters("r1"),
            [
                "label=com.zackees.bosn.registry=r1".to_owned(),
                "label=com.zackees.bosn.setup-managed".to_owned(),
            ]
        );
    }

    #[test]
    fn update_lowers_only_the_named_containers() {
        assert_eq!(update_args(&[]), None);
        let ids = ["a".to_owned(), "b".to_owned()];
        assert_eq!(
            update_args(&ids).unwrap(),
            ["update", LOW_CPU_PRIORITY_ARG, "a", "b"]
        );
    }
}
