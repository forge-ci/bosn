fn parse(volume: &str, text: &str) -> Result<crate::ci::CacheUsage, String> {
    super::footprint::parse(volume, text, 0)
}
use serde_json::{Value, json};

fn receipt() -> Value {
    let usage = json!({"filesystem_device":1,"path_entries":5,"unique_inodes":4,
        "apparent_bytes":1024,"allocated_bytes":512,"unique_file_bytes":32,
        "referenced_file_bytes":64});
    json!({"schema_version":1,"root":"/cache","observed_at":"2026-10-22T00:00:00Z",
        "total":usage,"components":[
            {"class":"tools","usage":usage},{"class":"images","usage":usage},
            {"class":"actions","usage":usage},{"class":"toolcache","usage":usage},
            {"class":"actcache","usage":usage},{"class":"other","usage":usage}],
        "namespaces":[{"namespace":"0123456789abcdef",
            "store_path":"actcache/cohort-v1/0123456789abcdef","usage":usage}],
        "omitted_namespaces":0,"partial":false})
}

#[test]
fn bounded_footprint_preserves_inclusive_total_and_store_identity() {
    let mut value = receipt();
    let report = parse("owned", &value.to_string()).unwrap();
    assert_eq!(report.volume, "owned");
    assert_eq!(report.bytes, Some(1024));
    assert_eq!(report.allocated_bytes, Some(512));
    assert_eq!(report.components.len(), 6);
    assert_eq!(
        report.components[5].store_path.as_deref(),
        Some("actcache/cohort-v1/0123456789abcdef")
    );
    assert!(!report.partial);
    value["omitted_namespaces"] = json!(2);
    let omitted = parse("owned", &value.to_string()).unwrap();
    assert!(omitted.partial);
    assert_eq!(
        omitted.allocated_bytes,
        Some(512),
        "detail truncation must not truncate the measured root"
    );
    assert!(omitted.errors[0].contains("2 namespace details omitted"));
}

#[test]
fn malformed_footprint_never_exposes_known_totals() {
    for scenario in [
        "root",
        "schema",
        "timestamp",
        "duplicate-class",
        "missing-class",
        "unknown-field",
        "namespace-escape",
        "namespace-mismatch",
        "duplicate-namespace",
        "negative-bytes",
        "entry-limit",
        "missing-bytes",
        "partial-with-known-bytes",
        "unexpected-error",
        "oversized",
    ] {
        let mut value = receipt();
        match scenario {
            "root" => value["root"] = json!("/other"),
            "schema" => value["schema_version"] = json!(2),
            "timestamp" => value["observed_at"] = json!("invalid"),
            "duplicate-class" => value["components"][1]["class"] = json!("tools"),
            "missing-class" => {
                value["components"].as_array_mut().unwrap().pop();
            }
            "unknown-field" => value["extra"] = json!(true),
            "namespace-escape" => value["namespaces"][0]["store_path"] = json!("../foreign"),
            "namespace-mismatch" => value["namespaces"][0]["namespace"] = json!("a".repeat(16)),
            "duplicate-namespace" => {
                let row = value["namespaces"][0].clone();
                value["namespaces"].as_array_mut().unwrap().push(row);
            }
            "negative-bytes" => value["total"]["allocated_bytes"] = json!(-1),
            "entry-limit" => value["total"]["path_entries"] = json!(1_000_001),
            "missing-bytes" => value["total"]["allocated_bytes"] = Value::Null,
            "partial-with-known-bytes" => {
                value["partial"] = json!(true);
                value["error"] = json!("incomplete");
            }
            "unexpected-error" => value["error"] = json!("failure"),
            "oversized" => value["error"] = json!("x".repeat(256 << 10)),
            _ => unreachable!(),
        }
        assert!(parse("owned", &value.to_string()).is_err(), "{scenario}");
    }
}

#[test]
fn incomplete_footprint_keeps_every_byte_total_unknown() {
    let mut value = receipt();
    value["partial"] = json!(true);
    value["error"] = json!("cache inventory entry bound exceeded");
    value["namespaces"] = Value::Null;
    for field in [
        "apparent_bytes",
        "allocated_bytes",
        "unique_file_bytes",
        "referenced_file_bytes",
    ] {
        value["total"][field] = Value::Null;
        for row in value["components"].as_array_mut().unwrap() {
            row["usage"][field] = Value::Null;
        }
    }
    let report = super::footprint::parse("owned", &value.to_string(), 1).unwrap();
    assert!(report.partial);
    assert_eq!(report.bytes, None);
    assert_eq!(report.allocated_bytes, None);
    assert!(
        report
            .components
            .iter()
            .all(|row| row.bytes.is_none() && row.allocated_bytes.is_none())
    );
    assert_eq!(report.errors, ["cache inventory entry bound exceeded"]);
}

#[test]
fn footprint_completeness_requires_matching_process_exit() {
    let complete = receipt().to_string();
    assert!(super::footprint::parse("owned", &complete, 1).is_err());
    assert!(super::footprint::parse("owned", &complete, 75).is_err());
    let mut partial = receipt();
    partial["partial"] = json!(true);
    partial["error"] = json!("incomplete");
    assert!(super::footprint::parse("owned", &partial.to_string(), 0).is_err());
    assert!(
        super::footprint::parse("owned", &partial.to_string(), 1).is_err(),
        "partial process cannot publish known bytes"
    );
}

#[test]
fn maximum_footprint_details_stay_within_the_tool_reply_limit() {
    let mut value = receipt();
    let row = value["namespaces"][0].clone();
    let mut rows = Vec::new();
    for index in 0..256 {
        let mut current = row.clone();
        current["namespace"] = json!(format!("{index:016x}"));
        current["store_path"] = json!(format!("actcache/cohort-v1/{index:016x}"));
        current["usage"]["apparent_bytes"] = json!(i64::MAX);
        current["usage"]["allocated_bytes"] = json!(i64::MAX - 511);
        current["usage"]["unique_file_bytes"] = json!(i64::MAX);
        current["usage"]["referenced_file_bytes"] = json!(i64::MAX);
        rows.push(current);
    }
    value["namespaces"] = Value::Array(rows);
    let report = parse(&"v".repeat(255), &value.to_string()).unwrap();
    assert_eq!(report.components.len(), 261);
    assert!(serde_json::to_vec(&report).unwrap().len() < 60 * 1024);
}
