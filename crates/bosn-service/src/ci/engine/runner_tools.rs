//! Small hosted-runner tools missing from the pinned slim Ubuntu image.
//! Kept in act's tool-cache volume, so the engine image stays unchanged and
//! completed generations survive across runs through the normal cache saver.

use super::{runner_tag, toolcache::TOOLCACHE_VOLUME};
use crate::ci::pins::RUNNER_PATH;

const TOOLS_ID: &str = "pwsh-7.6.6-gh-2.102.0";
const BOOTSTRAP: &str = include_str!("runner_tools.sh");

pub(super) fn path_env() -> String {
    format!("PATH=/opt/hostedtoolcache/bosn-runner-tools/{TOOLS_ID}/bin:{RUNNER_PATH}")
}

pub(super) fn prepare_script() -> String {
    let script = BOOTSTRAP
        .replace("@TOOLS_ID@", TOOLS_ID)
        .replace('\'', "'\\''");
    format!(
        "docker run --rm --platform linux/amd64 --volume {TOOLCACHE_VOLUME}:/opt/hostedtoolcache --entrypoint bash {} -ec '{script}'",
        runner_tag()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #459: the runner's PATH is Bosn's fixed tool directory plus the pinned runner image's
    /// PATH. Nothing from the daemon's own environment reaches it, so a tool resolves in a run
    /// exactly when the runner image (or Bosn's tool generation) provides it, never because
    /// the host happens to have it.
    #[test]
    fn the_runner_path_is_fixed_and_never_the_hosts() {
        let env = path_env();
        let path = env.strip_prefix("PATH=").expect("a PATH assignment");
        let (tools, rest) = path
            .split_once(':')
            .expect("the tool directory comes first");
        assert_eq!(
            tools,
            format!("/opt/hostedtoolcache/bosn-runner-tools/{TOOLS_ID}/bin")
        );
        assert_eq!(rest, RUNNER_PATH);
        assert!(
            rest.split(':').any(|dir| dir == "/usr/bin"),
            "tar lives in /usr/bin"
        );
        if let Some(host) = std::env::var_os("PATH").map(|p| p.to_string_lossy().into_owned())
            && host != RUNNER_PATH
        {
            assert!(
                !path.contains(&host),
                "the host PATH leaked into the runner PATH"
            );
        }
    }
}
