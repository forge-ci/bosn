//! act's tool cache, owned by act2's serve mode (zackees/act2#67).
//!
//! act mounts its `act-toolcache` volume at `/opt/hostedtoolcache` in every
//! job container. Serve seeds it once per engine from the machine-wide copy
//! in the cache volume (`{ENGINE_CACHE}/toolcache`), or mounts a frozen tool
//! generation's overlay, and after each run whose act exited on its own saves
//! every newly completed, unmodified install back, atomically. Bosn only
//! names the generation an engine was created with, and runs the runner's
//! stock tools once per engine after serve prepared the volume.

use super::{ENGINE_CACHE, ENGINE_WORK};

pub(super) const TOOLCACHE_VOLUME: &str = "act-toolcache";

/// The digest of act2 serve's tool-generation overlay recipe for this
/// engine's store and target (`serve.OverlayRecipe`). A generation is frozen
/// for it; serve refuses to mount a generation frozen for another recipe.
pub(super) const OVERLAY_RECIPE_SHA256: &str =
    "f6f345602dcd3a4651d99c771dc5ee918979717bc74511c2225adfcc87669818";

/// The `tool_generation` of serve's prepare request, if the engine has one.
pub(super) fn generation_request(
    generation: Option<&bosn_registry::act::ActToolGenerationBinding>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(generation) = generation else {
        return Ok(None);
    };
    generation.validate().map_err(|error| error.to_string())?;
    if generation.overlay_recipe_sha256 != OVERLAY_RECIPE_SHA256 {
        return Err("tool overlay recipe differs from frozen producer".into());
    }
    Ok(Some(serde_json::json!({
        "id": generation.id,
        "store": format!("{ENGINE_CACHE}/toolstore-v1"),
        "overlay_recipe_sha256": generation.overlay_recipe_sha256,
    })))
}

/// Run `tools` once per engine: runs that share an engine (#547) wait on the
/// first one's lock, then find its stamp.
pub(super) fn once_per_engine(tools: &str) -> String {
    let done = format!("{ENGINE_WORK}/.tools-ready");
    format!(
        "mkdir -p {ENGINE_WORK}; exec 9>>{ENGINE_WORK}/.tools.lock; flock 9; \
         [ ! -e {done} ] || exit 0; \
         ( {tools} ) && touch {done}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bosn_registry::act::ActToolGenerationBinding;

    #[test]
    fn a_generation_is_handed_to_serve_only_for_its_frozen_recipe() {
        assert_eq!(generation_request(None).unwrap(), None);
        let mut generation = ActToolGenerationBinding {
            id: "a".repeat(64),
            max_payload_bytes: 1 << 30,
            overlay_recipe_sha256: OVERLAY_RECIPE_SHA256.into(),
        };
        let request = generation_request(Some(&generation)).unwrap().unwrap();
        assert_eq!(request["store"], "/bosn/cache/toolstore-v1");
        assert_eq!(request["id"], "a".repeat(64));
        generation.overlay_recipe_sha256 = "f".repeat(64);
        assert!(generation_request(Some(&generation)).is_err());
    }

    #[test]
    fn runner_tools_run_once_per_engine() {
        let script = once_per_engine("true");
        assert!(script.contains("flock 9"), "{script}");
        assert!(script.ends_with("( true ) && touch /var/lib/docker/bosn-ci/.tools-ready"));
    }
}
