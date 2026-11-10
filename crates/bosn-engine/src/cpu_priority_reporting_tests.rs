use super::*;
use std::path::Path;

#[test]
fn slice_cpu_max_parses_unlimited_and_quota() {
    assert_eq!(
        CpuMax::parse("max 100000"),
        Some(CpuMax {
            quota: None,
            period: 100000
        })
    );
    let limited = CpuMax::parse("200000 100000").unwrap();
    assert_eq!(limited.cpus(), Some(2.0));
    for bad in ["", "max", "1 0", "0 100000", "wat 100000", "1 2 extra"] {
        assert_eq!(CpuMax::parse(bad), None, "{bad}");
    }
}

#[test]
fn slice_cpu_weight_is_validated() {
    assert_eq!(parse_cpu_weight("10\n"), Some(10));
    for bad in ["0", "10001", "-1", "bad", "10 20"] {
        assert_eq!(parse_cpu_weight(bad), None);
    }
}

#[test]
fn slice_cpu_reporting_uses_fixture_cgroupfs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cgroupfs");
    let report = read_slice_cpu(Placement::Slice, &root);
    assert_eq!(report.placement, "bosn.slice");
    assert_eq!(report.cpu_max.unwrap().cpus(), Some(2.0));
    assert_eq!(report.cpu_weight, Some(10));
    let absent = read_slice_cpu(Placement::WeightOnly, &root);
    assert_eq!(absent.placement, "weight-only");
    assert_eq!(absent.cpu_max, None);
    assert_eq!(absent.cpu_weight, None);
    let missing = read_slice_cpu(Placement::Slice, &root.join("missing"));
    assert_eq!(missing.cpu_max, None);
    assert_eq!(missing.cpu_weight, None);
}

#[test]
fn slice_cpu_malformed_files_are_unavailable() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cgroupfs-malformed");
    let report = read_slice_cpu(Placement::Slice, &root);
    assert_eq!(report.cpu_max, None);
    assert_eq!(report.cpu_weight, None);
}
