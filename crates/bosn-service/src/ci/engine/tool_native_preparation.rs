//! Provision the recoverable source before reserving it or admitting any work.
use super::{DockerActBackend, ENGINE_CACHE, PULL_DEADLINE, RunOptions, run_scope};
use bosn_registry::act::ActEngineRecord;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Preparation<'a> {
    tool_generation: Generation<'a>,
}

#[derive(Serialize)]
struct Generation<'a> {
    id: &'a str,
    store: String,
    overlay_recipe_sha256: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    tool_cache: String,
}

fn require_prepared(text: &str, generation: &str) -> Result<(), String> {
    if text.len() > 4096 {
        return Err("native source preparation receipt exceeds bound".into());
    }
    let receipt: Receipt = serde_json::from_str(text)
        .map_err(|e| format!("native source preparation receipt: {e}"))?;
    if receipt.tool_cache != format!("generation {generation}") {
        return Err("native source preparation differs from frozen generation".into());
    }
    Ok(())
}

impl DockerActBackend {
    pub(super) async fn prepare_native_source(
        &self,
        record: &ActEngineRecord,
    ) -> Result<(), String> {
        let generation = record
            .intent
            .creation_profile
            .as_ref()
            .and_then(|profile| profile.tool_generation.as_ref())
            .ok_or("native source preparation has no frozen generation")?;
        let engine = record
            .engine_id
            .as_deref()
            .ok_or("native source engine is unobserved")?;
        self.prepare_native_generation(engine, generation).await
    }

    async fn prepare_native_generation(
        &self,
        engine: &str,
        generation: &bosn_registry::act::ActToolGenerationBinding,
    ) -> Result<(), String> {
        generation.validate().map_err(|e| e.to_string())?;
        if generation.overlay_recipe_sha256 != super::tool_overlay_recipe_sha256() {
            return Err("native source preparation recipe differs".into());
        }
        let request = serde_json::to_string(&Preparation {
            tool_generation: Generation {
                id: &generation.id,
                store: format!("{ENGINE_CACHE}/toolstore-v1"),
                overlay_recipe_sha256: &generation.overlay_recipe_sha256,
            },
        })
        .map_err(|e| e.to_string())?;
        // Act2 validates PID1's original reader and prepares only its own source.
        // Omit the image: no runner image, stock tools or workflow is started.
        let output = self
            .run_bounded(
                Self::exec(engine, &run_scope::prepare_script(&request)),
                RunOptions::bounded(PULL_DEADLINE, 4096),
            )
            .await?;
        if !output.ok() {
            return Err(format!(
                "native source preparation failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(512)
                    .collect::<String>()
            ));
        }
        let receipt = std::str::from_utf8(&output.stdout)
            .map_err(|_| "native source preparation receipt is not UTF-8")?;
        require_prepared(receipt, &generation.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn preparation_bounds_raw_receipt_including_whitespace() {
        use bosn_engine::DockerEngine;
        use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};
        for (before, after, refused) in [(0, 0, false), (5000, 0, true), (0, 5000, true)] {
            let dir = TemporaryDirectory::new().unwrap();
            let generation = bosn_registry::act::ActToolGenerationBinding {
                id: "a".repeat(64),
                max_payload_bytes: 1024,
                overlay_recipe_sha256: super::super::tool_overlay_recipe_sha256(),
            };
            let receipt = format!(r#"{{"tool_cache":"generation {}"}}"#, generation.id);
            let script = dir.path().join("docker.sh");
            std::fs::write(
                &script,
                format!("printf '%{before}s' ''; printf '%s' '{receipt}'; printf '%{after}s' ''"),
            )
            .unwrap();
            let backend = DockerActBackend::new(DockerEngine::synthetic_for_test(
                "sh",
                [script.into_os_string()],
            ));
            let result = RuntimeBuilder::multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
                .run(backend.prepare_native_generation("engine-id", &generation));
            assert_eq!(
                result.is_err(),
                refused,
                "padding before={before} after={after}: {result:?}"
            );
        }
    }

    #[test]
    fn only_exact_generation_preparation_is_acknowledged() {
        let id = "a".repeat(64);
        let valid = format!(r#"{{"tool_cache":"generation {id}"}}"#);
        assert!(require_prepared(&valid, &id).is_ok());
        for invalid in [
            "{}",
            r#"{"tool_cache":"legacy"}"#,
            r#"{"tool_cache":"generation b"}"#,
        ] {
            assert!(require_prepared(invalid, &id).is_err());
        }
        assert!(require_prepared(&valid.replace('}', ",\"image\":null}"), &id).is_err());
    }
}
