//! The durable off switch for a state directory's daemon (#670).
//!
//! `bosn daemon disable` writes `<state>/daemon.disabled`; while it exists nothing starts a
//! daemon for that state directory: the CLI's on-demand start, the user unit's `daemon serve`
//! and the widget's autostart all refuse. A file, not an environment variable, because it
//! has to hold against every agent session on the host, not just the shell that set it.

use std::path::{Path, PathBuf};

/// What every refused start reports, with the remedy.
pub const DISABLED: &str = "bosn daemon is disabled (bosn daemon enable)";

const MARKER: &str = "daemon.disabled";

#[must_use]
pub fn marker(state_dir: &Path) -> PathBuf {
    state_dir.join(MARKER)
}

#[must_use]
pub fn is_disabled(state_dir: &Path) -> bool {
    marker(state_dir).exists()
}

/// Refuse with [`DISABLED`] while the marker exists.
///
/// # Errors
/// The disabled message, so a caller that would start the daemon stops instead.
pub fn refuse_if_disabled(state_dir: &Path) -> Result<(), String> {
    if is_disabled(state_dir) {
        Err(DISABLED.into())
    } else {
        Ok(())
    }
}

/// Write the marker. Disabling an already disabled directory is not an error.
///
/// # Errors
/// The state directory cannot be created or the marker cannot be written.
pub fn disable(state_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(
        marker(state_dir),
        b"bosn daemon disabled; `bosn daemon enable` removes this file\n",
    )
}

/// Remove the marker. Enabling a directory that was never disabled is not an error.
///
/// # Errors
/// The marker exists but cannot be removed.
pub fn enable(state_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(marker(state_dir)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_and_enable_round_trip_and_are_idempotent() {
        let state = tempfile::tempdir().unwrap();
        assert_eq!(refuse_if_disabled(state.path()), Ok(()));
        disable(state.path()).unwrap();
        disable(state.path()).unwrap();
        assert_eq!(refuse_if_disabled(state.path()), Err(DISABLED.into()));
        enable(state.path()).unwrap();
        enable(state.path()).unwrap();
        assert!(!is_disabled(state.path()));
    }
}
