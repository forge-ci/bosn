//! Enrollment runs inside the genuine claimed preparation engine.
use super::{DockerActBackend, ENGINE_WORK, RunOptions};
use bosn_registry::act::ActToolGenerationBinding;
use serde::Deserialize;
use std::time::Duration;

const SOURCE: &str = "/bosn/cache/toolcache";
const ROOT: &str = "/bosn/cache/toolstore-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnrollmentPhase {
    Fence,
    Complete,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Cold,
    Warm,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema_version: u32,
    source: String,
    root: String,
    #[serde(default)]
    generation: String,
    mode: Mode,
    enrolled: bool,
    source_fenced: bool,
    partial: bool,
    #[serde(default)]
    error: String,
}

fn receipt(
    text: &[u8],
    phase: EnrollmentPhase,
    max: u64,
) -> Result<Option<ActToolGenerationBinding>, String> {
    if text.len() > 4096 {
        return Err("enrollment receipt exceeds bound".into());
    }
    let report: Report =
        serde_json::from_slice(text).map_err(|e| format!("enrollment receipt: {e}"))?;
    let _mode = report.mode;
    if report.schema_version != 1
        || report.source != SOURCE
        || report.root != ROOT
        || !report.source_fenced
        || report.partial
        || !report.error.is_empty()
    {
        return Err("native enrollment did not prove its source fence".into());
    }
    match phase {
        EnrollmentPhase::Fence if !report.enrolled && report.generation.is_empty() => Ok(None),
        EnrollmentPhase::Complete if report.enrolled => {
            let binding = ActToolGenerationBinding {
                id: report.generation,
                max_payload_bytes: max,
                overlay_recipe_sha256: super::tool_overlay_recipe_sha256(),
            };
            binding.validate().map_err(|e| e.to_string())?;
            Ok(Some(binding))
        }
        _ => Err("native enrollment acknowledged the wrong phase".into()),
    }
}

impl DockerActBackend {
    pub(super) async fn enroll_tools(
        &self,
        engine: &str,
        phase: EnrollmentPhase,
        max: u64,
        timeout: Duration,
    ) -> Result<Option<ActToolGenerationBinding>, String> {
        if max == 0 || max > i64::MAX as u64 {
            return Err("invalid native payload bound".into());
        }
        let act = format!("{ENGINE_WORK}/bin/act");
        let mut args: Vec<String> = [
            "exec",
            engine,
            act.as_str(),
            "cache",
            "tool-enroll",
            "--from",
            SOURCE,
            "--cache-server-path",
            ROOT,
            "--max-bytes",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        args.extend([max.to_string(), "--apply".into()]);
        // Preserve act2's historical nested defaults unless an operator changes the budget.
        if timeout != Duration::from_secs(900) {
            args.extend(["--timeout-secs".into(), timeout.as_secs().to_string()]);
        }
        match phase {
            EnrollmentPhase::Fence => args.push("--fence-only".into()),
            EnrollmentPhase::Complete => {
                args.extend(["--require-fenced".into(), "--source-quiescent".into()])
            }
        }
        let result = self
            .run_bounded(args, RunOptions::bounded(timeout, 4096))
            .await?;
        if !result.ok() {
            return Err(format!(
                "native enrollment refused: {}",
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        receipt(&result.stdout, phase, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FENCE: &str = r#"{"schema_version":1,"source":"/bosn/cache/toolcache","root":"/bosn/cache/toolstore-v1","mode":"warm","enrolled":false,"source_fenced":true,"partial":false}"#;

    #[test]
    fn phases_require_exact_closed_receipts() {
        assert!(
            receipt(FENCE.as_bytes(), EnrollmentPhase::Fence, 1024)
                .unwrap()
                .is_none()
        );
        assert!(receipt(FENCE.as_bytes(), EnrollmentPhase::Complete, 1024).is_err());
        for bad in [
            FENCE.replace("true", "false"),
            FENCE.replace("toolstore-v1", "other-store"),
            FENCE.replace("warm", "unknown"),
            FENCE.replace("\"schema_version\":1", "\"schema_version\":2"),
            FENCE.replace("\"partial\":false", "\"partial\":true"),
            FENCE.replace("\"partial\":false", "\"partial\":false,\"surprise\":1"),
        ] {
            assert!(
                receipt(bad.as_bytes(), EnrollmentPhase::Fence, 1024).is_err(),
                "{bad}"
            );
        }
        let complete = FENCE.replace(
            "\"enrolled\":false",
            &format!("\"enrolled\":true,\"generation\":\"{}\"", "a".repeat(64)),
        );
        let binding = receipt(complete.as_bytes(), EnrollmentPhase::Complete, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(binding.max_payload_bytes, 1024);
        assert!(receipt(complete.as_bytes(), EnrollmentPhase::Fence, 1024).is_err());
        assert!(receipt(complete.as_bytes(), EnrollmentPhase::Complete, 0).is_err());
        assert!(
            receipt(
                complete.replace(&"a".repeat(64), "bad").as_bytes(),
                EnrollmentPhase::Complete,
                1024
            )
            .is_err()
        );
    }
}
