//! One retry only when refusal is definite or a lost reply is read-only.
use super::*;

pub(super) fn should_retry(
    request: &Request,
    result: &Result<Reply, Error>,
    received: bool,
) -> bool {
    if matches!(result, Err(Error::DaemonExiting)) {
        return true;
    }
    let lost = match result {
        Err(Error::Protocol("eof")) => true,
        Err(Error::Io(error)) => error.kind() == std::io::ErrorKind::ConnectionReset,
        _ => false,
    };
    !received && lost && read_only(request)
}

fn read_only(request: &Request) -> bool {
    match request.operation {
        // Ping, registry status, job status/logs/list and registry read pages.
        1 | 2 | 5 | 7 | 11 | 12 | 13 | 37 => true,
        36 => matches!(
            serde_json::from_str::<ci::CiRequest>(&request.ci_request),
            Ok(ci::CiRequest::List { .. }
                | ci::CiRequest::Show { .. }
                | ci::CiRequest::Logs { .. }
                | ci::CiRequest::Report { .. }
                | ci::CiRequest::Doctor)
        ),
        _ => false,
    }
}

pub(super) async fn restart(state: &Path) -> Result<(), Error> {
    // The fixed executable exposes the existing CLI startup path, including
    // supervisor selection. Never launch a competing daemon from this library.
    let state = state.to_path_buf();
    async_engine::launch_blocking(move || {
        let status = std::process::Command::new(state.join("daemon/bosn-daemon"))
            .args(["daemon", "ensure", "--state-dir"])
            .arg(&state)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Protocol("daemon restart failed"))
        }
    })
    .await
    .map_err(|_| Error::ActorClosed)?
}
