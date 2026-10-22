//! Native data and enrollment metadata belong to the existing toolcache class.
use super::*;
use std::{collections::HashSet, os::unix::fs::MetadataExt, path::Path};

fn measured(root: &Path) -> CacheUsage {
    let script = SCRIPT.replace("root=/cache", &format!("root='{}'", root.display()));
    let output = kernal_api::run_bounded_command(
        kernal_api::SpawnSpec::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(kernal_api::StreamMode::Null)
            .stdout(kernal_api::StreamMode::Piped)
            .stderr(kernal_api::StreamMode::Piped),
        std::time::Duration::from_secs(10),
        1 << 16,
    )
    .unwrap();
    assert_eq!(output.exit.raw_code(), 0);
    parse("test", &String::from_utf8(output.stdout).unwrap())
}

fn unique_usage(path: &Path, seen: &mut HashSet<(u64, u64)>) -> (u64, u64, u64) {
    let metadata = std::fs::symlink_metadata(path).unwrap();
    if !seen.insert((metadata.dev(), metadata.ino())) {
        return (0, 0, 0);
    }
    let mut usage = (
        metadata.len(),
        metadata.blocks() * 512,
        if metadata.is_dir() { metadata.len() } else { 0 },
    );
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            let child = unique_usage(&entry.unwrap().path(), seen);
            usage.0 += child.0;
            usage.1 += child.1;
            usage.2 += child.2;
        }
    }
    usage
}

#[test]
fn native_tool_accounting_includes_enrollment_and_deduplicates_root_links() {
    let root = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let foreign = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    std::fs::write(foreign.path().join("unrelated"), vec![8; 1 << 20]).unwrap();
    let legacy = root.path().join("toolcache");
    let native = root.path().join("toolstore-v1");
    let history = root.path().join(".toolstore-v1.enrollment-v1.bolt");
    let snapshot = root
        .path()
        .join(format!(".toolcache.native-enrollment-{}", "a".repeat(64)));
    std::fs::write(&legacy, "legacy-refusal").unwrap();
    std::fs::write(&history, vec![1; 4096]).unwrap();
    std::fs::create_dir_all(native.join("tree")).unwrap();
    std::fs::create_dir(&snapshot).unwrap();
    std::fs::write(native.join("object"), vec![2; 1 << 20]).unwrap();
    std::fs::hard_link(native.join("object"), native.join("tree/tool")).unwrap();
    std::fs::hard_link(native.join("object"), snapshot.join("payload")).unwrap();
    std::os::unix::fs::symlink(foreign.path(), snapshot.join("external")).unwrap();
    let mut seen = HashSet::new();
    let expected = [&legacy, &native, &history, &snapshot]
        .into_iter()
        .map(|path| unique_usage(path, &mut seen))
        .fold((0, 0, 0), |sum, entry| {
            (sum.0 + entry.0, sum.1 + entry.1, sum.2 + entry.2)
        });
    let report = measured(root.path());
    assert!(!report.partial, "{report:?}");
    let tools = report
        .components
        .iter()
        .find(|component| component.class == CacheClass::Toolcache)
        .unwrap();
    // The probed GNU du 9.4 excludes directory lengths; the probed BusyBox and
    // GNU du 9.1 include them. Both retain exact file/symlink lengths and count
    // each inode once. Allocated bytes have the same meaning in both tools.
    assert!(
        tools.bytes == Some(expected.0) || tools.bytes == Some(expected.0 - expected.2),
        "unexpected apparent bytes: {tools:?}; inode totals: {expected:?}"
    );
    assert_eq!(tools.allocated_bytes, Some(expected.1));
    assert_eq!(
        std::fs::read(foreign.path().join("unrelated")).unwrap(),
        vec![8; 1 << 20]
    );
}

#[test]
fn excess_enrollment_snapshots_report_unknown_component_usage() {
    let root = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    for index in 0..33 {
        std::fs::create_dir(
            root.path()
                .join(format!(".toolcache.native-enrollment-{index:064x}")),
        )
        .unwrap();
    }
    let report = measured(root.path());
    assert!(report.partial);
    assert!(report.bytes.is_some());
    assert!(report.allocated_bytes.is_some());
    let tools = report
        .components
        .iter()
        .find(|component| component.class == CacheClass::Toolcache)
        .unwrap();
    assert_eq!(tools.bytes, None);
    assert_eq!(tools.allocated_bytes, None);
}
