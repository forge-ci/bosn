//! #508: publication must carry exact frozen identity and a bounded live receipt.
use super::*;

#[test]
fn reference_release_requires_exact_durable_absence_even_on_retry() {
    let pin = RecoveryPin {
        schema_version: 1,
        owner: "a".repeat(64),
        generation: "b".repeat(64),
        created_at: "2026-10-21T00:00:00Z".into(),
        expires_at: "2026-10-21T01:00:00Z".into(),
    };
    let report = serde_json::json!({"schema_version":1,"root":"/bosn/cache/toolstore-v1",
        "pin":pin,"removed":true,"absent":true,"partial":false});
    let check = |row: &serde_json::Value| {
        require_release(&row.to_string(), &pin, "/bosn/cache/toolstore-v1")
    };
    assert!(check(&report).is_ok());
    let mut retry = report.clone();
    retry["removed"] = false.into();
    assert!(
        check(&retry).is_ok(),
        "durable absence on an identical retry is sufficient"
    );
    for (field, value) in [
        ("absent", serde_json::json!(false)),
        ("partial", serde_json::json!(true)),
        ("schema_version", serde_json::json!(2)),
        ("root", serde_json::json!("/foreign")),
        ("pin", serde_json::json!({})),
        ("error", serde_json::json!("directory sync failed")),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut changed = report.clone();
        changed[field] = value;
        assert!(check(&changed).is_err(), "{field}: {changed}");
    }
    assert!(require_release(&" ".repeat(4097), &pin, "root").is_err());
    assert!(require_release(&format!("{report}\n{report}"), &pin, "root").is_err());
}

#[test]
fn publication_receipts_refuse_partial_foreign_and_out_of_lifetime_results() {
    let pin = RecoveryPin {
        schema_version: 1,
        owner: "a".repeat(64),
        generation: "b".repeat(64),
        created_at: "2026-10-21T00:00:00Z".into(),
        expires_at: "2026-10-21T01:00:00Z".into(),
    };
    let now = 1792542600.0; // 2026-10-21T00:30:00Z
    let row = serde_json::json!({"schema_version":1,"root":"/bosn/cache/toolstore-v1",
        "pin":pin,"generation":"c".repeat(64),"completed_at":"2026-10-21T00:15:00Z",
        "completed":true,"partial":false});
    let check = |value: &serde_json::Value| {
        require_publication(
            &value.to_string(),
            &pin,
            "/bosn/cache/toolstore-v1",
            now - 1000.0,
            now,
        )
    };
    assert!(check(&row).is_ok());
    for (field, value) in [
        ("schema_version", serde_json::json!(2)),
        ("root", serde_json::json!("/foreign")),
        ("pin", serde_json::json!({})),
        ("generation", serde_json::json!("C".repeat(64))),
        ("generation", serde_json::json!("bad")),
        ("completed", serde_json::json!(false)),
        ("partial", serde_json::json!(true)),
        ("error", serde_json::json!("failed")),
        ("pending_stage", serde_json::json!("stage")),
        ("completed_at", serde_json::json!("2026-10-21T00:00:01Z")),
        ("completed_at", serde_json::json!("2026-10-21T00:31:00Z")),
        ("completed_at", serde_json::json!("2026-10-21T01:00:00Z")),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut changed = row.clone();
        changed[field] = value;
        assert!(check(&changed).is_err(), "{field}: {changed}");
    }
    assert!(require_publication(&" ".repeat(4097), &pin, "root", now, now).is_err());
    assert!(require_publication(&format!("{row}\n{row}"), &pin, "root", now, now).is_err());
}
