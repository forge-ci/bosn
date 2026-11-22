//! Bounded startup readiness, including verified act bootstrap before dockerd.

use super::{CONTROL_DEADLINE, DockerActBackend, PULL_DEADLINE, async_engine, owned};
use std::time::Duration;

#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct StartupState {
    running: bool,
    exit_code: i64,
    error: String,
}

fn require_running(document: &[u8]) -> Result<(), String> {
    let state: StartupState = serde_json::from_slice(document)
        .map_err(|error| format!("engine startup state: {error}"))?;
    if !state.running {
        return Err(format!(
            "engine exited before readiness: code {}, error {:?}",
            state.exit_code, state.error
        ));
    }
    Ok(())
}

/// Lines of the exited engine's own output kept in the error. The container is
/// removed during cleanup, so this is the only copy of why bootstrap failed.
const EXIT_LOG_LINES: usize = 20;

/// Append the exited engine's last output to the readiness error. Docker's
/// `.State.Error` is empty for an ordinary non-zero exit, so without this the
/// cause (e.g. act2's `context deadline exceeded`) was lost with the container.
fn name_exit_cause(reason: String, stdout: &[u8], stderr: &[u8]) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for stream in [stdout, stderr] {
        lines.extend(
            std::str::from_utf8(stream)
                .unwrap_or("")
                .lines()
                .filter(|line| !line.trim().is_empty()),
        );
    }
    if lines.is_empty() {
        return format!("{reason}; the engine left no output");
    }
    let tail = &lines[lines.len().saturating_sub(EXIT_LOG_LINES)..];
    format!(
        "{reason}; engine output (last {} lines):\n{}",
        tail.len(),
        tail.join("\n")
    )
}

impl DockerActBackend {
    async fn with_engine_log(&self, engine: &str, reason: String) -> String {
        let tail = EXIT_LOG_LINES.to_string();
        let args = ["logs", "--tail", tail.as_str(), engine]
            .into_iter()
            .map(String::from)
            .collect();
        match self.run(args, CONTROL_DEADLINE).await {
            Ok(logs) => name_exit_cause(reason, &logs.stdout, &logs.stderr),
            Err(error) => format!("{reason}; engine output unavailable: {error}"),
        }
    }

    pub(super) async fn wait_ready(&self, engine: &str) -> Result<(), String> {
        // Previously installation had its own pull budget after readiness.
        // Bootstrap now does it before dockerd, retaining that total allowance.
        let budget = PULL_DEADLINE + Duration::from_secs(60);
        let deadline = async_engine::Deadline::after(budget);
        loop {
            if deadline.is_elapsed() {
                return Err(format!(
                    "nested engine did not become ready within {}s",
                    budget.as_secs()
                ));
            }
            let probe = Self::exec(engine, "docker info >/dev/null 2>&1");
            if self
                .run(probe, deadline.remaining().min(CONTROL_DEADLINE))
                .await?
                .ok()
            {
                return Ok(());
            }
            let state = self
                .run(
                    owned(&[
                        "container",
                        "inspect",
                        "--format",
                        "{{json .State}}",
                        engine,
                    ]),
                    deadline.remaining().min(CONTROL_DEADLINE),
                )
                .await?;
            if !state.ok() {
                return Err(format!(
                    "engine startup inspection failed: {}",
                    String::from_utf8_lossy(&state.stderr).trim()
                ));
            }
            if let Err(reason) = require_running(&state.stdout) {
                return Err(self.with_engine_log(engine, reason).await);
            }
            async_engine::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exited_or_unprovable_engine_does_not_wait_for_download_budget() {
        assert!(
            require_running(br#"{"Running":true,"ExitCode":0,"Error":"","Status":"running"}"#)
                .is_ok()
        );
        for document in [
            &br#"{"Running":false,"ExitCode":1,"Error":"bootstrap failed"}"#[..],
            &br#"{"Running":false,"ExitCode":0,"Error":""}"#[..],
            &br#"{"Running":true}"#[..],
            &br#"{"Running":"true","ExitCode":0,"Error":""}"#[..],
            &br#"null"#[..],
        ] {
            assert!(require_running(document).is_err());
        }
    }

    #[test]
    fn exited_engine_error_carries_its_own_last_output() {
        let reason = "engine exited before readiness: code 1, error \"\"".to_string();
        let named = name_exit_cause(
            reason.clone(),
            b"act version 0.2.89-act2.36\n",
            b"Error: context deadline exceeded\n",
        );
        assert!(named.starts_with(&reason));
        assert!(named.contains("act version 0.2.89-act2.36"));
        assert!(named.ends_with("Error: context deadline exceeded"));

        let noisy: Vec<u8> = (0..50)
            .map(|i| format!("line {i}\n"))
            .collect::<String>()
            .into_bytes();
        let named = name_exit_cause(reason.clone(), &noisy, b"");
        assert!(named.contains("last 20 lines"));
        assert!(!named.contains("line 29\n") && named.ends_with("line 49"));

        assert!(name_exit_cause(reason, b"", b"\n").ends_with("the engine left no output"));
    }
}
