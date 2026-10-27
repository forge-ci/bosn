//! `bosn --version` reports the release version, not an internal crate's.
//!
//! The binary lives in the internal, unpublished `bosn-service` crate. Reading
//! `CARGO_PKG_VERSION` there answered that crate's own `0.1.0` for every
//! published wheel, so a user could not tell 0.1.25 from 0.1.0 -- and neither
//! could any check that gates on the reported version.

use std::process::Command;

const ROOT_MANIFEST: &str = include_str!("../../../Cargo.toml");

/// `[workspace.package].version` in the root manifest: the one place it is written.
fn release_version() -> String {
    let mut in_workspace_package = false;
    for line in ROOT_MANIFEST.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if !in_workspace_package {
            continue;
        }
        if let Some((name, value)) = line.split_once('=')
            && name.trim() == "version"
        {
            return value.trim().trim_matches('"').to_string();
        }
    }
    panic!("root Cargo.toml has no [workspace.package].version");
}

#[test]
fn version_reports_the_release_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_bosn"))
        .arg("--version")
        .output()
        .expect("bosn --version runs");
    assert!(
        output.status.success(),
        "--version exited {}",
        output.status
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("bosn {}", release_version())
    );
}
