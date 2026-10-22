//! Producer-owned finalization of exact retained sources after registry loss.
use super::{PublicationReport, RecoveryPin, validate_publication_report};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinalizationReport {
    schema_version: u32,
    root: String,
    pin: RecoveryPin,
    publication: Option<Completion>,
    disposed: bool,
    reference_absent: bool,
    partial: bool,
    #[serde(default)]
    error: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    pin: RecoveryPin,
    generation: String,
    completed_at: String,
}

fn timestamp(text: &str) -> Result<f64, String> {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .map(|at| at.unix_timestamp_nanos() as f64 / 1e9)
        .map_err(|e| format!("source finalization timestamp: {e}"))
}

fn require_finalization(
    text: &str,
    pin: &RecoveryPin,
    root: &str,
    observed: f64,
) -> Result<(), String> {
    if text.len() > 4096 {
        return Err("source finalization receipt exceeds bound".into());
    }
    let report: FinalizationReport =
        serde_json::from_str(text).map_err(|e| format!("source finalization receipt: {e}"))?;
    if report.schema_version != 1
        || report.root != root
        || report.pin != *pin
        || !report.disposed
        || !report.reference_absent
        || report.partial
        || !report.error.is_empty()
        || !observed.is_finite()
    {
        return Err("source finalization incomplete or identity differs".into());
    }
    if let Some(done) = report.publication {
        validate_publication_report(
            PublicationReport {
                schema_version: 1,
                root: root.into(),
                pin: done.pin,
                generation: done.generation,
                completed_at: done.completed_at,
                completed: true,
                partial: false,
                error: String::new(),
                pending_stage: String::new(),
            },
            pin,
            root,
            timestamp(&pin.created_at)?,
            observed,
        )?;
    } else if observed < timestamp(&pin.expires_at)? {
        return Err("unexpired source finalization lacks durable publication".into());
    }
    Ok(())
}

#[path = "source_discovery.rs"]
mod discovery;
#[path = "source_finalization_runtime.rs"]
mod runtime;
#[path = "source_finalization_worker.rs"]
mod worker;

#[cfg(all(test, unix))]
#[path = "source_finalization_tests.rs"]
mod transport_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalized_source_requires_both_producer_fence_and_reference_absence() {
        let pin = RecoveryPin {
            schema_version: 1,
            owner: "a".repeat(64),
            generation: "b".repeat(64),
            created_at: "2026-10-21T00:00:00Z".into(),
            expires_at: "2026-10-22T00:00:00Z".into(),
        };
        let report = serde_json::json!({"schema_version":1,"root":"/bosn/cache/toolstore-v1",
            "pin":pin,"disposed":true,"reference_absent":true,"partial":false});
        let check = |row: &serde_json::Value| {
            require_finalization(
                &row.to_string(),
                &pin,
                "/bosn/cache/toolstore-v1",
                1792629000.0,
            )
        };
        assert!(
            check(&report).is_ok(),
            "actual act2 finalization receipt must be understood"
        );
        for (field, value) in [
            ("reference_absent", serde_json::json!(false)),
            ("disposed", serde_json::json!(false)),
            ("partial", serde_json::json!(true)),
            ("schema_version", serde_json::json!(2)),
            ("root", serde_json::json!("/foreign")),
            ("error", serde_json::json!("sync failed")),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut altered = report.clone();
            altered[field] = value;
            assert!(check(&altered).is_err(), "{field}");
        }
    }
}
