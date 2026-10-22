//! Seeds the pinned act archive into the shared cache when accounting finds
//! it missing, so the read-only footprint reader never depends on a prior CI
//! preparation. Only this short-lived helper has network and a writable mount;
//! it writes nothing but the digest-verified archive under its exclusive lock.

use super::super::{ActArtifact, act_install::fetch_act_script};
use super::{DockerActBackend, ENGINE_CACHE, engine_image, owned};

const SEED_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const SEED_LABEL: &str = "io.bosn.cache.seed=act";

pub(super) fn seed_args(volume: &str, act: ActArtifact) -> Vec<String> {
    let mount = format!("type=volume,source={volume},target={ENGINE_CACHE}");
    owned(&[
        "run",
        "--rm",
        bosn_engine::LOW_CPU_PRIORITY_ARG,
        "--label",
        SEED_LABEL,
        "--pull",
        "never",
        "--read-only",
        "--cap-drop",
        "ALL",
        "--memory",
        "128m",
        "--cpus",
        "1",
        "--pids-limit",
        "64",
        "--mount",
        &mount,
        "--entrypoint",
        "sh",
        &engine_image(),
        "-c",
        &fetch_act_script(act),
    ])
}

impl DockerActBackend {
    pub(super) async fn seed_act_archive(
        &self,
        volume: &str,
        act: ActArtifact,
    ) -> Result<(), String> {
        // The volume's ownership is proven before anything writes into it.
        self.verify_measured_volume(volume).await?;
        self.checked("act archive seed", seed_args(volume, act), SEED_DEADLINE)
            .await
            .map(drop)
    }
}
