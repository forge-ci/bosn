//! Registering the daemon with the platform's user service manager.
//!
//! #147's second precondition was that something must be running to do the work, and on the
//! reference machine nothing was: `~/.local/state/bosn` did not exist and bosn had never run.
//! Autostart is opt-in, and this module is what makes opting in actually work.
//!
//! The original defect it replaces was precise: the Python implementation wrote the
//! LaunchAgent plist and returned, never registering it with `launchd`. Dropping a file into
//! `~/Library/LaunchAgents` does not make launchd run it, so a macOS user who explicitly
//! asked for autostart got no daemon until their next login. Writing the file is therefore
//! *not* the operation; registering it is. Every mutating step here goes through
//! [`CommandRunner`], so the argv a platform would receive is testable without requiring the
//! platform in CI.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Unit name shared by both platforms' generated entries.
pub const SERVICE_NAME: &str = "com.zackees.bosn";

/// Runs a bounded, trusted argv.
///
/// This is deliberately not a general shell: callers do not interpolate user input into a
/// command line, and the platform argv is fixed by this module.
pub trait CommandRunner {
    /// # Errors
    /// Reports a failure to launch, or a non-zero exit.
    fn run(&self, argv: &[OsString]) -> Result<(), String>;
}

/// Runs commands as child processes.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, argv: &[OsString]) -> Result<(), String> {
        let Some((program, rest)) = argv.split_first() else {
            return Err("empty command".to_owned());
        };
        let status = std::process::Command::new(program)
            .args(rest)
            .stdout(std::process::Stdio::null())
            .status()
            .map_err(|error| format!("{}: {error}", program.to_string_lossy()))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "{} exited with {}",
                program.to_string_lossy(),
                status.code().unwrap_or(-1)
            ))
        }
    }
}

/// The platform whose service manager will own the daemon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    /// A user `systemd` unit.
    LinuxSystemd,
    /// A per-user `launchd` agent.
    MacosLaunchd,
}

impl Platform {
    /// The platform this build is running on, if autostart is supported here.
    #[must_use]
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "linux") {
            Some(Self::LinuxSystemd)
        } else if cfg!(target_os = "macos") {
            Some(Self::MacosLaunchd)
        } else {
            None
        }
    }
}

/// What one enable, disable, or status call found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutostartStatus {
    pub platform: Platform,
    /// The generated entry file.
    pub path: PathBuf,
    /// Whether the entry file exists.
    pub written: bool,
    /// Whether the service manager has the entry registered.
    pub registered: bool,
}

/// The service-manager command that registers or unregisters an entry.
///
/// Kept separate from execution so the exact argv is asserted in tests. `enable` and
/// `disable` are deliberately symmetric: leaving an unloaded file behind, or an unlinked
/// registration behind, is the defect this module exists to fix.
#[must_use]
pub fn registration_argv(platform: Platform, enable: bool, path: &Path) -> Vec<OsString> {
    match platform {
        Platform::LinuxSystemd => {
            let action = if enable { "enable" } else { "disable" };
            argv(&["systemctl", "--user", action, "--now", SERVICE_NAME])
        }
        Platform::MacosLaunchd => {
            let action = if enable { "load" } else { "unload" };
            argv_os(&["launchctl", action, "-w"], path.as_os_str().to_owned())
        }
    }
}

/// The service-manager command that makes it re-read unit files, if the platform has one.
///
/// It must run *after* the file changes, otherwise the manager keeps serving the previous
/// state of the world.
#[must_use]
pub fn reload_argv(platform: Platform) -> Option<Vec<OsString>> {
    match platform {
        Platform::LinuxSystemd => Some(argv(&["systemctl", "--user", "daemon-reload"])),
        Platform::MacosLaunchd => None,
    }
}

fn enabled_argv(platform: Platform, uid: u32) -> Vec<OsString> {
    match platform {
        Platform::LinuxSystemd => {
            argv(&["systemctl", "--user", "is-enabled", "--quiet", SERVICE_NAME])
        }
        Platform::MacosLaunchd => {
            argv(&["launchctl", "print", &format!("gui/{uid}/{SERVICE_NAME}")])
        }
    }
}

fn start_argv(platform: Platform, uid: u32) -> Vec<OsString> {
    match platform {
        Platform::LinuxSystemd => {
            argv(&["systemctl", "--user", "start", "--no-block", SERVICE_NAME])
        }
        Platform::MacosLaunchd => argv(&[
            "launchctl",
            "kickstart",
            &format!("gui/{uid}/{SERVICE_NAME}"),
        ]),
    }
}

fn stop_argv(platform: Platform, uid: u32) -> Vec<OsString> {
    match platform {
        Platform::LinuxSystemd => argv(&["systemctl", "--user", "stop", SERVICE_NAME]),
        Platform::MacosLaunchd => argv(&[
            "launchctl",
            "kill",
            "SIGTERM",
            &format!("gui/{uid}/{SERVICE_NAME}"),
        ]),
    }
}

fn reset_failed_argv() -> Vec<OsString> {
    argv(&["systemctl", "--user", "reset-failed", SERVICE_NAME])
}

#[cfg(unix)]
fn user_id() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid reads the process's effective user id and has no inputs.
    unsafe { geteuid() }
}

#[cfg(not(unix))]
fn user_id() -> u32 {
    0
}

fn targets_state(platform: Platform, contents: &str, state_dir: &Path) -> bool {
    let state = state_dir.to_string_lossy();
    match platform {
        Platform::LinuxSystemd => contents.lines().any(|line| {
            line.starts_with("ExecStart=")
                && (line.ends_with(&format!(" --state-dir {}", systemd_arg(&state)))
                    || line.ends_with(&format!(" --state-dir {state}")))
        }),
        Platform::MacosLaunchd => {
            contents.contains(&format!("<string>{}</string>", xml_text(&state)))
                || contents.contains(&format!("<string>{state}</string>"))
        }
    }
}

/// Quote one systemd ExecStart argument, including specifier and environment
/// expansion escapes. Shared by daemon and widget user units.
pub fn systemd_arg(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '$' => result.push_str("$$"),
            '%' => result.push_str("%%"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            _ => result.push(character),
        }
    }
    result.push('"');
    result
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn write_unit(path: &Path, contents: &str) -> Result<(), String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = path.with_file_name(format!(".{name}.{}.{nonce}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Start an enabled Bosn user unit for this state directory. A unit for a
/// different state directory leaves on-demand startup in charge. An installed
/// unit whose enabled state cannot be verified refuses a competing fork. An
/// enabled unit is repaired to use the authoritative executable
/// before it starts; once selected, a start failure is reported instead of
/// racing it with an on-demand daemon.
pub fn start_if_enabled(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
    state_dir: &Path,
) -> Result<bool, String> {
    let path = unit_path(platform, home);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if !targets_state(platform, &contents, state_dir) {
        return Ok(false);
    }
    let uid = user_id();
    runner.run(&enabled_argv(platform, uid)).map_err(|error| {
        format!(
            "the Bosn user unit exists for {}; cannot prove it is enabled: {error}",
            state_dir.display()
        )
    })?;
    let expected = unit_contents(platform, state_dir);
    if contents != expected {
        write_unit(&path, &expected)?;
        if let Some(reload) = reload_argv(platform) {
            runner.run(&reload)?;
            // A pre-fix unit may already be failed with start-limit-hit.
            runner.run(&reset_failed_argv())?;
        } else {
            runner.run(&registration_argv(platform, false, &path))?;
            runner.run(&registration_argv(platform, true, &path))?;
        }
    }
    let start = start_argv(platform, uid);
    if let Err(error) = runner.run(&start) {
        if platform != Platform::LinuxSystemd {
            return Err(error);
        }
        // Recover an old unit that already reached systemd's start limit.
        runner.run(&reset_failed_argv())?;
        runner.run(&start)?;
    }
    Ok(true)
}

/// Stop, without unregistering, the user unit when it serves this state directory, so a
/// disabled daemon (#670) is not left running under its supervisor. Returns whether a unit
/// was asked to stop; a unit for another state directory is left alone.
///
/// # Errors
/// The unit file cannot be read, or the service manager refuses the stop.
pub fn stop_if_installed(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
    state_dir: &Path,
) -> Result<bool, String> {
    let path = unit_path(platform, home);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if !targets_state(platform, &contents, state_dir) {
        return Ok(false);
    }
    runner.run(&stop_argv(platform, user_id()))?;
    Ok(true)
}

/// The generated unit file for a platform.
#[must_use]
pub fn unit_contents(platform: Platform, state_dir: &Path) -> String {
    let binary = state_dir.join("daemon/bosn-daemon");
    let binary = binary.to_string_lossy();
    let state_dir = state_dir.to_string_lossy();
    match platform {
        Platform::LinuxSystemd => {
            let binary = systemd_arg(&binary);
            let state_dir = systemd_arg(&state_dir);
            format!(
                "[Unit]\n\
             Description=Bosn Docker lifecycle daemon\n\
             After=network.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={binary} daemon serve --state-dir {state_dir}\n\
             Restart=on-failure\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n"
            )
        }
        Platform::MacosLaunchd => {
            let binary = xml_text(&binary);
            let state_dir = xml_text(&state_dir);
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \x20 <key>Label</key><string>{SERVICE_NAME}</string>\n\
             \x20 <key>ProgramArguments</key>\n\
             \x20 <array>\n\
             \x20   <string>{binary}</string>\n\
             \x20   <string>daemon</string>\n\
             \x20   <string>serve</string>\n\
             \x20   <string>--state-dir</string>\n\
             \x20   <string>{state_dir}</string>\n\
             \x20 </array>\n\
             \x20 <key>RunAtLoad</key><true/>\n\
             </dict>\n\
             </plist>\n"
            )
        }
    }
}

/// Where the entry file lives for a user.
#[must_use]
pub fn unit_path(platform: Platform, home: &Path) -> PathBuf {
    match platform {
        Platform::LinuxSystemd => home
            .join(".config/systemd/user")
            .join(format!("{SERVICE_NAME}.service")),
        Platform::MacosLaunchd => home
            .join("Library/LaunchAgents")
            .join(format!("{SERVICE_NAME}.plist")),
    }
}

/// Write the entry and register it, in that order.
///
/// # Errors
/// Reports a filesystem failure or a service-manager refusal. A refusal after the file is
/// written leaves the file in place and says so, rather than pretending the pair succeeded.
pub fn enable(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
    state_dir: &Path,
) -> Result<AutostartStatus, String> {
    let path = unit_path(platform, home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    write_unit(&path, &unit_contents(platform, state_dir))?;
    // Reload first so the manager can see the file, then register it.
    if let Some(reload) = reload_argv(platform) {
        runner.run(&reload)?;
        // Re-enabling an old failed unit must clear its start limit before
        // `enable --now` asks it to start.
        runner.run(&reset_failed_argv())?;
    }
    runner.run(&registration_argv(platform, true, &path))?;
    Ok(AutostartStatus {
        platform,
        path,
        written: true,
        registered: true,
    })
}

/// Unregister the entry, then remove it.
///
/// # Errors
/// Reports a service-manager refusal. The file is only removed after the manager has let go
/// of it, so a failure leaves a loadable unit rather than an orphaned registration.
pub fn disable(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
) -> Result<AutostartStatus, String> {
    let path = unit_path(platform, home);
    if !path.exists() {
        return Ok(AutostartStatus {
            platform,
            path,
            written: false,
            registered: false,
        });
    }
    runner.run(&registration_argv(platform, false, &path))?;
    std::fs::remove_file(&path).map_err(|error| error.to_string())?;
    // Reload last, so the manager forgets the unit rather than holding an unlinked one.
    if let Some(reload) = reload_argv(platform) {
        runner.run(&reload)?;
    }
    Ok(AutostartStatus {
        platform,
        path,
        written: false,
        registered: false,
    })
}

/// Report what is on disk. This never mutates, and never consults the service manager.
#[must_use]
pub fn status(platform: Platform, home: &Path) -> AutostartStatus {
    let path = unit_path(platform, home);
    let written = path.is_file();
    AutostartStatus {
        platform,
        path,
        written,
        registered: written,
    }
}

fn argv(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

fn argv_os(prefix: &[&str], tail: OsString) -> Vec<OsString> {
    prefix
        .iter()
        .map(OsString::from)
        .chain(std::iter::once(tail))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<Vec<OsString>>>,
        fail_on: Option<usize>,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, argv: &[OsString]) -> Result<(), String> {
            let mut calls = self.calls.lock().expect("lock");
            calls.push(argv.to_vec());
            if self.fail_on == Some(calls.len() - 1) {
                return Err("refused".to_owned());
            }
            Ok(())
        }
    }

    fn home() -> kernal_api::platform::fs::TemporaryDirectory {
        kernal_api::platform::fs::TemporaryDirectory::new().expect("temporary directory")
    }

    #[test]
    fn linux_enable_writes_the_unit_and_registers_it() {
        let home = home();
        let runner = FakeRunner::default();
        let status = enable(
            &runner,
            Platform::LinuxSystemd,
            home.path(),
            Path::new("/home/u/.local/state/bosn"),
        )
        .expect("enable");
        assert!(status.written && status.registered);
        assert!(status.path.exists());
        assert_eq!(
            std::fs::read_dir(status.path.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        let contents = std::fs::read_to_string(&status.path).expect("read unit");
        assert!(contents.contains(
            "ExecStart=\"/home/u/.local/state/bosn/daemon/bosn-daemon\" daemon serve --state-dir"
        ));
        assert!(contents.contains("WantedBy=default.target"));
        // The defect this replaces: writing the file without ever invoking the manager.
        let calls = runner.calls.lock().expect("lock").clone();
        assert_eq!(calls.len(), 3, "reload, reset and enable are required");
        assert_eq!(calls[0], argv(&["systemctl", "--user", "daemon-reload"]));
        assert_eq!(calls[1], reset_failed_argv());
        assert_eq!(
            calls[2],
            argv(&["systemctl", "--user", "enable", "--now", SERVICE_NAME])
        );
    }

    #[test]
    fn linux_disable_unregisters_before_removing() {
        let home = home();
        let runner = FakeRunner::default();
        enable(
            &runner,
            Platform::LinuxSystemd,
            home.path(),
            Path::new("/state"),
        )
        .expect("enable");
        let status = disable(&runner, Platform::LinuxSystemd, home.path()).expect("disable");
        assert!(!status.written);
        assert!(!status.path.exists());
        let calls = runner.calls.lock().expect("lock").clone();
        assert_eq!(
            calls[3],
            argv(&["systemctl", "--user", "disable", "--now", SERVICE_NAME]),
            "unregister before removing the file"
        );
        assert_eq!(calls[4], argv(&["systemctl", "--user", "daemon-reload"]));
    }

    #[test]
    fn macos_enable_loads_the_agent_and_disable_unloads_it() {
        let home = home();
        let runner = FakeRunner::default();
        let status = enable(
            &runner,
            Platform::MacosLaunchd,
            home.path(),
            Path::new("/Users/u/.local/state/bosn"),
        )
        .expect("enable");
        assert!(
            status
                .path
                .ends_with("Library/LaunchAgents/com.zackees.bosn.plist")
        );
        let contents = std::fs::read_to_string(&status.path).expect("read plist");
        assert!(contents.contains("<key>RunAtLoad</key>"));
        assert!(
            contents.contains("<string>/Users/u/.local/state/bosn/daemon/bosn-daemon</string>")
        );
        let calls = runner.calls.lock().expect("lock").clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0][0], "launchctl");
        assert_eq!(calls[0][1], "load");
        assert_eq!(calls[0][2], "-w");
        disable(&runner, Platform::MacosLaunchd, home.path()).expect("disable");
        let calls = runner.calls.lock().expect("lock").clone();
        assert_eq!(calls[1][1], "unload");
        assert_eq!(calls[1][2], "-w");
    }

    #[test]
    fn a_refused_registration_does_not_claim_success() {
        let home = home();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            fail_on: Some(2),
        };
        let error = enable(
            &runner,
            Platform::LinuxSystemd,
            home.path(),
            Path::new("/state"),
        )
        .expect_err("enable must fail");
        assert!(error.contains("refused"));
    }

    #[test]
    fn disabling_something_that_was_never_enabled_is_a_no_op() {
        let home = home();
        let runner = FakeRunner::default();
        let status = disable(&runner, Platform::LinuxSystemd, home.path()).expect("disable");
        assert!(!status.written);
        assert!(runner.calls.lock().expect("lock").is_empty());
    }

    #[test]
    fn the_unit_paths_are_the_platform_conventional_ones() {
        let home = Path::new("/home/u");
        assert_eq!(
            unit_path(Platform::LinuxSystemd, home),
            Path::new("/home/u/.config/systemd/user/com.zackees.bosn.service")
        );
        assert_eq!(
            unit_path(Platform::MacosLaunchd, home),
            Path::new("/home/u/Library/LaunchAgents/com.zackees.bosn.plist")
        );
    }

    #[test]
    fn generated_entries_escape_path_arguments() {
        let state = Path::new("/home/a b/$cache%/a\\\"&<state>");
        let linux = unit_contents(Platform::LinuxSystemd, state);
        assert!(linux.contains(
            "ExecStart=\"/home/a b/$$cache%%/a\\\\\\\"&<state>/daemon/bosn-daemon\" daemon serve --state-dir \"/home/a b/$$cache%%/a\\\\\\\"&<state>\""
        ));
        assert!(targets_state(Platform::LinuxSystemd, &linux, state));

        let macos = unit_contents(Platform::MacosLaunchd, state);
        assert!(macos.contains("<string>/home/a b/$cache%/a\\&quot;&amp;&lt;state&gt;</string>"));
        assert!(targets_state(Platform::MacosLaunchd, &macos, state));
    }

    #[test]
    fn an_enabled_unit_is_started_and_an_old_binary_path_is_repaired() {
        let home = home();
        let state = Path::new("/home/u/.local/state/bosn");
        let path = unit_path(Platform::LinuxSystemd, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = unit_contents(Platform::LinuxSystemd, state).replace(
            "/home/u/.local/state/bosn/daemon/bosn-daemon",
            "/usr/bin/bosn",
        );
        std::fs::write(&path, old).unwrap();
        let runner = FakeRunner::default();
        assert!(start_if_enabled(&runner, Platform::LinuxSystemd, home.path(), state).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            unit_contents(Platform::LinuxSystemd, state)
        );
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0], enabled_argv(Platform::LinuxSystemd, 0));
        assert_eq!(calls[1], reload_argv(Platform::LinuxSystemd).unwrap());
        assert_eq!(calls[2], reset_failed_argv());
        assert_eq!(calls[3], start_argv(Platform::LinuxSystemd, 0));
    }

    #[test]
    fn another_state_directory_does_not_take_over_an_enabled_unit() {
        let home = home();
        let path = unit_path(Platform::LinuxSystemd, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            unit_contents(Platform::LinuxSystemd, Path::new("/state/a")),
        )
        .unwrap();
        let runner = FakeRunner::default();
        assert!(
            !start_if_enabled(
                &runner,
                Platform::LinuxSystemd,
                home.path(),
                Path::new("/state/b")
            )
            .unwrap()
        );
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unavailable_manager_does_not_fall_back_to_a_second_supervisor() {
        let home = home();
        let state = Path::new("/state/a");
        let path = unit_path(Platform::LinuxSystemd, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, unit_contents(Platform::LinuxSystemd, state)).unwrap();
        let runner = FakeRunner {
            fail_on: Some(0),
            ..FakeRunner::default()
        };
        let error = start_if_enabled(&runner, Platform::LinuxSystemd, home.path(), state)
            .expect_err("manager status must be proven");
        assert!(error.contains("cannot prove it is enabled"), "{error}");
        assert_eq!(runner.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_loaded_launch_agent_is_kickstarted() {
        let home = home();
        let state = Path::new("/Users/u/.local/state/bosn");
        let path = unit_path(Platform::MacosLaunchd, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, unit_contents(Platform::MacosLaunchd, state)).unwrap();
        let runner = FakeRunner::default();
        assert!(start_if_enabled(&runner, Platform::MacosLaunchd, home.path(), state).unwrap());
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], enabled_argv(Platform::MacosLaunchd, user_id()));
        assert_eq!(calls[1], start_argv(Platform::MacosLaunchd, user_id()));
    }
}
