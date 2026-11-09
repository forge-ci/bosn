//! The authoritative daemon executable (#509 phase 4).
//!
//! `ensure_daemon` starts the daemon from one fixed per-user path,
//! `<state>/daemon/bosn-daemon`, not from whichever client binary happened to
//! start it. A client installs its own binary there only when it is newer
//! than what is installed (or the same release from another build), always
//! by an atomic rename, so a pinned old client never downgrades it and never
//! starts an old daemon over newer state.

use std::path::{Path, PathBuf};

const STAMP: &str = "bosn-daemon.release";
const INSTALL_LOCK: &str = ".install.lock";

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "bosn-daemon.exe"
    } else {
        "bosn-daemon"
    }
}

/// The authoritative daemon executable for a state directory.
pub(crate) fn daemon_executable(state_dir: &Path) -> PathBuf {
    state_dir.join("daemon").join(executable_name())
}

/// `major.minor.patch` of a release, ignoring any pre-release or build suffix.
fn release_key(release: &str) -> Option<(u64, u64, u64)> {
    let core = release.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(str::parse::<u64>);
    let key = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    parts.next().is_none().then_some(key)
}

/// Whether release `ours` is strictly newer than `theirs`. An empty or
/// unparsable `theirs` (bosn 0.1.5 and older report no release) is older.
pub(crate) fn is_newer_release(ours: &str, theirs: &str) -> bool {
    match (release_key(ours), release_key(theirs)) {
        (Some(ours), Some(theirs)) => ours > theirs,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// What a stamp records: the release and the build it was copied from.
fn stamp_for(release: &str, source: &Path) -> String {
    bosn_service::BuildIdentity::from_executable(release, source)
        .map_or_else(|_| format!("{release}\n\n"), |build| build.stamp)
}

/// Whether a client of `release` built as `wanted` should replace `installed`.
fn should_replace(release: &str, installed: Option<&str>, wanted: &str) -> bool {
    let Some(installed) = installed else {
        return true;
    };
    let installed_release = installed.lines().next().unwrap_or_default();
    if is_newer_release(release, installed_release) {
        return true;
    }
    // The same release from another build (a developer rebuild) is no
    // downgrade; an older client never replaces a newer daemon.
    installed_release == release && installed != wanted
}

/// Put `source` at `target` by an atomic rename of a sibling temporary file.
/// A hard link is tried first so an install costs no copy.
fn replace_atomically(source: &Path, target: &Path) -> std::io::Result<()> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let temporary = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    if std::fs::hard_link(source, &temporary).is_err() {
        std::fs::copy(source, &temporary)?;
        let modified = std::fs::metadata(source)?.modified()?;
        std::fs::File::options()
            .write(true)
            .open(&temporary)?
            .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    }
    std::fs::rename(&temporary, target).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

/// Install this client's binary as the authoritative daemon executable when
/// it is newer, and return the executable to start. A failed install keeps
/// an existing executable (a running daemon may hold it on Windows); only
/// with none at all is the error returned.
pub(crate) fn install_daemon_executable(
    state_dir: &Path,
    release: &str,
) -> Result<PathBuf, String> {
    let source = std::env::current_exe()
        .map_err(|_| "cannot locate the bosn executable to start its daemon".to_owned())?;
    install_from(&source, &daemon_executable(state_dir), release)
}

/// The recorded install, unless the executable it describes is missing.
fn read_installed(stamp: &Path, target: &Path) -> Option<String> {
    std::fs::read_to_string(stamp)
        .ok()
        .filter(|_| target.exists())
}

fn install_from(source: &Path, target: &Path, release: &str) -> Result<PathBuf, String> {
    let stamp = target.with_file_name(STAMP);
    let wanted = stamp_for(release, source);
    // The usual case needs no lock: nothing to do.
    if !should_replace(release, read_installed(&stamp, target).as_deref(), &wanted) {
        return Ok(target.to_path_buf());
    }
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(target.parent().unwrap_or(Path::new(".")))?;
        // The binary and stamp renames are one step only under this lock, and
        // the decision must be remade inside it: another installer may have
        // finished a newer release since the check above (#768). Dropping the
        // file releases it, and so does a crashed holder's exit.
        let lock = std::fs::File::create(target.with_file_name(INSTALL_LOCK))?;
        lock.lock()?;
        if !should_replace(release, read_installed(&stamp, target).as_deref(), &wanted) {
            return Ok(());
        }
        replace_atomically(source, target)?;
        let pending = target.with_file_name(format!(".{STAMP}.{}.tmp", std::process::id()));
        std::fs::write(&pending, &wanted)?;
        std::fs::rename(&pending, &stamp)
    })();
    match result {
        Ok(()) => Ok(target.to_path_buf()),
        Err(_) if target.exists() => Ok(target.to_path_buf()),
        Err(error) => Err(format!(
            "cannot install the bosn daemon executable at {}: {error}",
            target.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_compare_numerically_and_never_downgrade() {
        assert!(is_newer_release("0.1.10", "0.1.9"));
        assert!(is_newer_release("0.2.0", "0.1.99"));
        assert!(is_newer_release("0.1.6", ""));
        assert!(!is_newer_release("0.1.6", "0.1.6"));
        assert!(!is_newer_release("0.1.5", "0.1.6"));
        assert!(!is_newer_release("garbage", "0.1.6"));
        assert!(!is_newer_release("1.0.0", "1.0.0-rc.1"));
    }

    #[test]
    fn only_a_newer_release_or_another_same_release_build_replaces() {
        let ours = "0.1.6\n10:1\n";
        assert!(should_replace("0.1.6", None, ours));
        assert!(should_replace("0.1.6", Some("0.1.5\n10:1\n"), ours));
        assert!(should_replace("0.1.6", Some("0.1.6\n10:2\n"), ours));
        assert!(!should_replace("0.1.6", Some(ours), ours));
        assert!(!should_replace("0.1.6", Some("0.1.7\n10:1\n"), ours));
    }

    #[test]
    fn replacement_is_a_rename_over_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let (source, target) = (dir.path().join("new"), dir.path().join("bosn-daemon"));
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&target, "old").unwrap();
        replace_atomically(&source, &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    // A real running image is required: an ordinary open file does not exercise
    // Windows' executable-image sharing restrictions.
    #[cfg(windows)]
    #[test]
    fn running_windows_executable_keeps_its_stamp_until_exit() {
        use std::io::{BufRead, Write};
        use std::process::{Command, Stdio};

        struct RunningImage(std::process::Child);
        impl Drop for RunningImage {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("client.exe");
        let command =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe");
        std::fs::copy(command, &source).unwrap();
        let release = "0.1.6";
        let target = daemon_executable(dir.path());
        install_from(&source, &target, release).unwrap();
        let original = std::fs::read(&target).unwrap();
        let stamp = target.with_file_name(STAMP);
        let original_stamp = std::fs::read_to_string(&stamp).unwrap();
        assert_eq!(original_stamp, stamp_for(release, &target));

        let mut running = RunningImage(
            Command::new(&target)
                .args(["/d", "/q", "/c", "echo ready&set /p bosn_wait="])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut ready = String::new();
        std::io::BufReader::new(running.0.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        assert!(running.0.try_wait().unwrap().is_none());

        // Do not mutate the original source: installation may have hard-linked it.
        let rebuilt = dir.path().join("rebuilt.exe");
        let mut different_build = original.clone();
        different_build.extend_from_slice(b"different build overlay");
        std::fs::write(&rebuilt, &different_build).unwrap();
        let wanted = stamp_for(release, &rebuilt);
        assert_ne!(wanted, original_stamp);
        assert_eq!(install_from(&rebuilt, &target, release).unwrap(), target);
        assert!(running.0.try_wait().unwrap().is_none());
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&stamp).unwrap(), original_stamp);
        assert_eq!(stamp_for(release, &target), original_stamp);
        // Only the executable, release stamp and persistent install lock remain.
        assert_eq!(
            std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
            3
        );

        running
            .0
            .stdin
            .take()
            .unwrap()
            .write_all(b"exit\r\n")
            .unwrap();
        assert!(running.0.wait().unwrap().success());
        assert_eq!(install_from(&rebuilt, &target, release).unwrap(), target);
        assert_eq!(std::fs::read(&target).unwrap(), different_build);
        assert_eq!(std::fs::read_to_string(&stamp).unwrap(), wanted);
        assert_eq!(stamp_for(release, &target), wanted);
        assert_eq!(
            std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
            3
        );
    }

    #[test]
    fn concurrent_installers_leave_the_newest_binary_and_its_stamp() {
        for _ in 0..50 {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("daemon").join("bosn-daemon");
            let releases = ["0.1.21", "0.1.23", "0.1.22", "0.1.20"];
            let barrier = std::sync::Barrier::new(releases.len());
            std::thread::scope(|scope| {
                for release in releases {
                    let source = dir.path().join(release);
                    std::fs::write(&source, release).unwrap();
                    let (target, barrier) = (&target, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        install_from(&source, target, release).unwrap();
                    });
                }
            });
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "0.1.23");
            let stamp = std::fs::read_to_string(target.with_file_name(STAMP)).unwrap();
            assert_eq!(stamp.lines().next(), Some("0.1.23"));
        }
    }
}

/// The identity recorded for the authoritative executable, if known.
pub(crate) fn installed_build(state: &Path) -> Option<bosn_service::BuildIdentity> {
    let target = daemon_executable(state);
    bosn_service::BuildIdentity::from_stamp(read_installed(&target.with_file_name(STAMP), &target)?)
}

/// An older client never requests a handoff, including for another build.
pub(crate) fn needs_handoff(
    release: &str,
    daemon: &bosn_service::DaemonIdentity,
    installed: Option<&bosn_service::BuildIdentity>,
) -> bool {
    is_newer_release(release, &daemon.release)
        || (release == daemon.release && stale_build(daemon, installed))
}

pub(crate) fn stale_build(
    daemon: &bosn_service::DaemonIdentity,
    installed: Option<&bosn_service::BuildIdentity>,
) -> bool {
    match (daemon.build.as_ref(), installed) {
        (Some(running), Some(installed)) => running != installed,
        _ => false,
    }
}

#[cfg(test)]
mod build_tests {
    use super::*;
    #[test]
    fn handoff_decisions_never_downgrade_or_restart_unknown_builds() {
        let build = |mtime| {
            bosn_service::BuildIdentity::from_stamp(format!("0.1.6\n10:{mtime}\n")).unwrap()
        };
        let installed = build(2);
        let mut daemon = bosn_service::DaemonIdentity {
            release: "0.1.5".into(),
            protocol: 2,
            protocol_min: 1,
            build: Some(build(1)),
        };
        assert!(needs_handoff("0.1.6", &daemon, Some(&installed)));
        daemon.release = "0.1.6".into();
        assert!(needs_handoff("0.1.6", &daemon, Some(&installed)));
        daemon.build = Some(installed.clone());
        assert!(!needs_handoff("0.1.6", &daemon, Some(&installed)));
        daemon.build = None;
        assert!(!needs_handoff("0.1.6", &daemon, Some(&installed)));
        daemon.build = Some(build(1));
        assert!(!needs_handoff("0.1.4", &daemon, Some(&installed)));
    }
}
