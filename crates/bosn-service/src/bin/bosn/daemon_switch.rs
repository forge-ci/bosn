//! `bosn daemon disable` and `bosn daemon enable` (#670).

use super::*;

pub(crate) fn run(verb: &str, mut arguments: impl Iterator<Item = std::ffi::OsString>) {
    let mut state_dir = None;
    let mut json_output = false;
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--state-dir" => set_once_parsed(&mut state_dir, arguments.next(), parse_state_dir),
            "--json" if !json_output => {
                json_output = true;
                Ok(())
            }
            _ => Err(()),
        }
        .unwrap_or_else(|_| usage());
    }
    let state_dir = state_dir.unwrap_or_else(bosn_service::mcp::default_state_dir);
    let result = if verb == "disable" {
        disable(&state_dir)
    } else {
        bosn_service::daemon_switch::enable(&state_dir).map_err(|error| error.to_string())
    };
    if let Err(detail) = result {
        bosn_service::daemon_log!("bosn daemon {verb}: {detail}");
        std::process::exit(1);
    }
    if json_output {
        println!(
            "{}",
            json!({"action": format!("daemon_{verb}"), "disabled": verb == "disable"})
        );
    } else {
        println!("daemon {verb}d for {}", state_dir.display());
    }
}

/// Marker first, so nothing restarts the daemon between the stops below; then the unit,
/// so its supervisor lets go; then any daemon still answering.
fn disable(state_dir: &Path) -> Result<(), String> {
    bosn_service::daemon_switch::disable(state_dir).map_err(|error| error.to_string())?;
    if let (Some(platform), Some(home)) = (
        bosn_service::autostart::Platform::current(),
        std::env::var_os("HOME").map(PathBuf::from),
    ) {
        bosn_service::autostart::stop_if_installed(
            &bosn_service::autostart::SystemRunner,
            platform,
            &home,
            state_dir,
        )?;
    }
    let Ok(client) = crate::daemon_client(state_dir) else {
        return Ok(());
    };
    let runtime = RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    if runtime.run(client.daemon_identity()).is_ok() {
        runtime
            .run(client.shutdown_and_wait(std::time::Duration::from_secs(30)))
            .map_err(|error| format!("cannot stop the running daemon: {error}"))?;
    }
    Ok(())
}
