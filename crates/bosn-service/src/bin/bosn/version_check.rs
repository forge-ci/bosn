//! The one pre-flight every daemon-backed command runs before it sends work (#324).
//!
//! A daemon on another wire protocol can misread a request and answer with a
//! reset connection or a refusal, which the command would report as a bare
//! "request failed". Ask for the daemon's identity first -- the one request
//! every release answers -- and refuse with both protocols and the remedy
//! instead. Compatibility is by protocol window (#509); comparing releases is
//! the legacy path for a protocol-0 daemon only.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Connect to the daemon for `state_dir`, the one way every daemon-backed command does.
///
/// It warns, once per process, when the daemon answering runs another release than
/// this client, and it never fails a command over it: a daemon that does not answer
/// prints nothing (the command then starts one, or fails, in its own way), and only
/// [`require_matching_daemon`] refuses, for a daemon on another wire protocol.
/// Commands that need no daemon (`--version`, `compose plan`, ...) never call it.
pub(crate) fn daemon_client(state_dir: impl AsRef<Path>) -> Result<Client, bosn_service::Error> {
    let state_dir = state_dir.as_ref();
    let client = Client::for_state(state_dir)?;
    if let Ok(runtime) = RuntimeBuilder::current_thread().enable_all().build()
        && let Ok(identity) = runtime.run(client.daemon_identity())
    {
        warn_release_skew(state_dir, &identity);
    }
    Ok(client)
}

/// Print the daemon/client mismatch warning on stderr, at most once per process: for a
/// compatible daemon of another release, and for one on another wire protocol. It only
/// warns; refusing an incompatible daemon is [`require_matching_daemon`]'s job, for the
/// commands that send it work.
pub(crate) fn warn_release_skew(state_dir: &Path, daemon: &bosn_service::DaemonIdentity) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let client = env!("CARGO_PKG_VERSION");
    let warning = bosn_service::daemon_release_skew(state_dir, client, daemon).or_else(|| {
        bosn_service::daemon_version_mismatch(state_dir, client, daemon)
            .map(|mismatch| format!("warning: bosn daemon/client mismatch: {mismatch}"))
    });
    if let Some(warning) = warning
        && !WARNED.swap(true, Ordering::Relaxed)
    {
        eprintln!("bosn: {warning}");
    }
}

/// The mismatch message when the daemon for `state_dir` speaks another wire
/// protocol (or, predating the handshake, is another release); `None` when it
/// is compatible or does not answer (the command then fails, or starts a
/// daemon, in its own way). A compatible daemon from another release gets the
/// daemon/client mismatch warning (see [`warn_release_skew`]).
pub(crate) fn daemon_mismatch(
    runtime: &kernal_api::async_engine::Runtime,
    client: &Client,
    state_dir: &Path,
) -> Option<String> {
    let identity = runtime.run(client.daemon_identity()).ok()?;
    let client_version = env!("CARGO_PKG_VERSION");
    warn_release_skew(state_dir, &identity);
    bosn_service::daemon_version_mismatch(state_dir, client_version, &identity)
}

/// Exit with the mismatch, as JSON on stdout or text on stderr, before
/// `command` sends anything to a daemon it cannot talk to.
/// `command` is the CLI spelling (`setup ensure`); its JSON `action` is the
/// snake-case form every command's JSON already uses (`setup_ensure`).
pub(crate) fn require_matching_daemon(
    state_dir: impl AsRef<Path>,
    command: &str,
    json_output: bool,
) {
    let state_dir = state_dir.as_ref();
    let Ok(client) = Client::for_state(state_dir) else {
        return;
    };
    let Ok(runtime) = RuntimeBuilder::current_thread().enable_all().build() else {
        return;
    };
    let Some(mismatch) = daemon_mismatch(&runtime, &client, state_dir) else {
        return;
    };
    if json_output {
        println!(
            "{}",
            json!({"action": command.replace([' ', '-'], "_"), "error": "version_mismatch", "message": mismatch})
        );
    } else {
        eprintln!("bosn {command}: {mismatch}");
    }
    std::process::exit(1);
}
