//! #670: a disabled state directory starts no daemon, by any route.

use super::start_daemon;
use bosn_service::autostart::{CommandRunner, Platform, unit_contents, unit_path};
use std::ffi::OsString;
use std::sync::Mutex;

#[derive(Default)]
struct FakeManager(Mutex<Vec<Vec<OsString>>>);
impl CommandRunner for FakeManager {
    fn run(&self, argv: &[OsString]) -> Result<(), String> {
        self.0.lock().unwrap().push(argv.to_vec());
        Ok(())
    }
}

#[test]
fn a_disabled_state_dir_refuses_both_the_unit_and_the_on_demand_start() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    let unit = unit_path(Platform::LinuxSystemd, home.path());
    std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
    std::fs::write(&unit, unit_contents(Platform::LinuxSystemd, &state)).unwrap();
    bosn_service::daemon_switch::disable(&state).unwrap();
    let manager = FakeManager::default();
    let error = start_daemon(
        &state.join("daemon/bosn-daemon"),
        &state,
        Some(Platform::LinuxSystemd),
        Some(home.path()),
        &manager,
        |_, _| panic!("a disabled daemon must not be forked"),
    )
    .unwrap_err();
    assert_eq!(error, bosn_service::daemon_switch::DISABLED);
    assert!(
        manager.0.lock().unwrap().is_empty(),
        "the unit was not asked to start"
    );

    let error = start_daemon(
        &state.join("daemon/bosn-daemon"),
        &state,
        None,
        None,
        &manager,
        |_, _| panic!("a disabled daemon must not be forked"),
    )
    .unwrap_err();
    assert_eq!(error, bosn_service::daemon_switch::DISABLED);

    bosn_service::daemon_switch::enable(&state).unwrap();
    let started = start_daemon(
        &state.join("daemon/bosn-daemon"),
        &state,
        Some(Platform::LinuxSystemd),
        Some(home.path()),
        &manager,
        |_, _| panic!("the enabled unit owns the start"),
    )
    .unwrap();
    assert!(started.is_none());
    assert!(
        !manager.0.lock().unwrap().is_empty(),
        "enable restores the normal path"
    );
}

#[test]
fn stopping_a_disabled_daemon_stops_only_this_state_dirs_unit() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state");
    let manager = FakeManager::default();
    let stop = |state: &std::path::Path| {
        bosn_service::autostart::stop_if_installed(
            &manager,
            Platform::LinuxSystemd,
            home.path(),
            state,
        )
    };
    assert_eq!(stop(&state), Ok(false), "no unit");
    let unit = unit_path(Platform::LinuxSystemd, home.path());
    std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
    std::fs::write(&unit, unit_contents(Platform::LinuxSystemd, &state)).unwrap();
    assert_eq!(stop(&home.path().join("other")), Ok(false));
    assert!(manager.0.lock().unwrap().is_empty());
    assert_eq!(stop(&state), Ok(true));
    assert_eq!(
        manager.0.lock().unwrap().as_slice(),
        [[
            "systemctl",
            "--user",
            "stop",
            bosn_service::autostart::SERVICE_NAME
        ]
        .map(OsString::from)
        .to_vec()]
    );
}
