//! WebSocket ↔ bounded Docker pipes ↔ pinned TLS. No published port or host mount.
use std::{collections::BTreeMap, io, time::Duration};

use bosn_engine::{DockerEngine, RunOptions};
use futures_util::future::{Either, select};
use kernal_api::{
    ProcessOutputChunk, ProcessOutputEvent, ProcessSession, ProcessSessionOptions, async_engine,
    http_server::{WebSocket, WebSocketMessage as Message},
};
use serde::Deserialize;

use super::catalog::Target;

pub const IMAGE: &str =
    "python:3.13.11-slim@sha256:2b9c9803c6a287cafa0a8c917211dddd23dcd2016f049690ee5219f5d3f1636e";
const BRIDGE: &str = include_str!("bridge.py");
const SESSION_LABEL: &str = "com.zackees.bosn.console-session";
const IDLE: Duration = Duration::from_secs(600);
const LIFETIME: Duration = Duration::from_secs(43200);

fn run_args(target: &Target, registry: &str, name: &str, workspace: &str) -> Vec<String> {
    let generation = kernal_api::hash::sha256_bytes(BRIDGE.as_bytes()).to_hex();
    let mut args = vec![
        "run".into(),
        "--rm".into(),
        "--interactive".into(),
        "--name".into(),
        name.into(),
        bosn_engine::LOW_CPU_PRIORITY_ARG.into(),
        "--cpus=1".into(),
        "--memory=256m".into(),
        "--pids-limit=32".into(),
        "--read-only".into(),
        "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(),
        "--user=65534:65534".into(),
    ];
    for (key, value) in [
        (bosn_core::LABEL_REGISTRY, registry.to_owned()),
        (bosn_core::LABEL_KIND, "container".into()),
        (bosn_core::LABEL_STACK, "console".into()),
        (bosn_core::LABEL_GENERATION, format!("sha256:{generation}")),
        (bosn_core::LABEL_SCOPE, "stack".into()),
        (bosn_core::LABEL_WORKSPACE, workspace.into()),
        (
            bosn_core::LABEL_CREATED,
            format!("{:.0}", crate::ci::lifecycle::now_seconds()),
        ),
        (SESSION_LABEL, name.into()),
    ] {
        args.extend(["--label".into(), format!("{key}={value}")]);
    }
    args.extend([
        IMAGE.into(),
        "python".into(),
        "-B".into(),
        "-u".into(),
        "-c".into(),
        BRIDGE.into(),
        target.host.clone(),
        target.port.to_string(),
        target.certificate_sha256.clone(),
    ]);
    args
}

async fn error(socket: &mut WebSocket, code: &str) {
    let _ = async_engine::timeout(Duration::from_secs(5), async {
        let _ = socket
            .send(Message::Text(format!(r#"{{"console_error":"{code}"}}"#)))
            .await;
        let _ = socket.send(Message::Close).await;
    })
    .await;
}

/// The lease is held through cleanup, keeping idle handoff away from live sessions.
pub async fn serve(
    mut socket: WebSocket,
    target: Target,
    registry: String,
    workspace: String,
    _lease: super::SessionLease,
) {
    let random = match crate::secure_random::bytes(
        "console session",
        crate::secure_random::RandomBudget::Short,
        16,
    )
    .await
    {
        Ok(bytes) => bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        Err(_) => {
            error(&mut socket, "console_start_failed").await;
            return;
        }
    };
    let name = format!("bosn-console-{random}");
    let engine = DockerEngine::docker();
    let options = ProcessSessionOptions {
        max_queued_chunks: 16,
        max_chunk_bytes: 65536,
        ..Default::default()
    };
    let child = match engine
        .with_args(run_args(&target, &registry, &name, &workspace))
        .duplex_session(options)
        .await
    {
        Ok(child) => child,
        Err(_) => {
            error(&mut socket, "docker_unavailable").await;
            return;
        }
    };
    let result = async_engine::timeout(LIFETIME, pump(&mut socket, &child)).await;
    if !matches!(result, Ok(Ok(()))) {
        error(&mut socket, "console_connection_lost").await;
    }
    let _ = async_engine::timeout(Duration::from_secs(1), child.close_stdin()).await;
    cleanup(&engine, &name, &registry).await;
    let _ = child.kill().await;
}

enum Incoming {
    Viewer(io::Result<Option<Message>>),
    Container(Option<ProcessOutputEvent>),
}

async fn incoming(socket: &mut WebSocket, child: &ProcessSession) -> io::Result<Incoming> {
    let event = async_engine::timeout(
        IDLE,
        select(Box::pin(socket.receive()), Box::pin(child.next_output())),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "console idle"))?;
    Ok(match event {
        Either::Left((message, _)) => Incoming::Viewer(message),
        Either::Right((output, _)) => Incoming::Container(output),
    })
}

async fn pump(socket: &mut WebSocket, child: &ProcessSession) -> io::Result<()> {
    let mut completions = 0;
    let mut diagnostic = Vec::new();
    loop {
        let incoming = incoming(socket, child).await?;
        match incoming {
            Incoming::Viewer(message) => match message? {
                Some(Message::Binary(bytes)) => child.write_stdin(&bytes).await?,
                Some(Message::Ping(bytes)) => socket.send(Message::Pong(bytes)).await?,
                Some(Message::Pong(_)) => {}
                _ => return Ok(()),
            },
            Incoming::Container(output) => {
                match output {
                    Some(ProcessOutputEvent::Chunk(ProcessOutputChunk::Stdout(bytes))) => {
                        socket.send(Message::Binary(bytes)).await?
                    }
                    Some(ProcessOutputEvent::Chunk(ProcessOutputChunk::Stderr(bytes))) => {
                        diagnostic.extend(bytes);
                        if diagnostic.len() > 4096 {
                            diagnostic.drain(..diagnostic.len() - 4096);
                        }
                    }
                    Some(ProcessOutputEvent::Completion(_)) => completions += 1,
                    None => completions = 2,
                }
                if completions >= 2 {
                    let code = if diagnostic
                        .windows(b"certificate_changed".len())
                        .any(|v| v == b"certificate_changed")
                    {
                        "certificate_changed"
                    } else if diagnostic
                        .windows(b"console_unreachable".len())
                        .any(|v| v == b"console_unreachable")
                    {
                        "console_unreachable"
                    } else {
                        "console_closed"
                    };
                    error(socket, code).await;
                    return Ok(());
                }
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspect {
    id: String,
    config: ContainerConfig,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerConfig {
    labels: BTreeMap<String, String>,
}

async fn cleanup(engine: &DockerEngine, name: &str, registry: &str) {
    let options = RunOptions::bounded(Duration::from_secs(15), 65536);
    let Ok(reply) = engine
        .with_args(["container", "inspect", name])
        .capture_async(options)
        .await
    else {
        return;
    };
    if reply.exit_code != 0 {
        return;
    }
    let Ok(containers) = serde_json::from_slice::<Vec<Inspect>>(&reply.stdout) else {
        return;
    };
    let Some(container) = containers.first().filter(|_| containers.len() == 1) else {
        return;
    };
    if container
        .config
        .labels
        .get(SESSION_LABEL)
        .map(String::as_str)
        != Some(name)
        || !bosn_core::ownership_from_labels(&container.config.labels, registry)
    {
        return;
    }
    if !engine
        .with_args(["container", "rm", "--force", &container.id])
        .capture_async(options)
        .await
        .is_ok_and(|reply| reply.exit_code == 0)
    {
        daemon_log!("bosn console: container cleanup deferred");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_has_no_published_port_mount_privilege_or_password() {
        let target = Target {
            id: "rack".into(),
            name: "Rack".into(),
            host: "192.0.2.1".into(),
            port: 5901,
            certificate_sha256: "ab".repeat(32),
        };
        let args = run_args(&target, "registry", "session", "/state");
        assert!(args.iter().any(|a| a == bosn_engine::LOW_CPU_PRIORITY_ARG));
        assert!(args.iter().any(|a| a == "--read-only"));
        assert!(!args.iter().any(|a| a.starts_with("--publish")
            || a.starts_with("--volume")
            || a == "--privileged"));
        for label in bosn_core::REQUIRED_LABELS {
            assert!(args.iter().any(|arg| arg.starts_with(&format!("{label}="))));
        }
        assert_eq!(args.last(), Some(&target.certificate_sha256));
        let labels: BTreeMap<_, _> = args
            .windows(2)
            .filter(|pair| pair[0] == "--label")
            .map(|pair| pair[1].split_once('=').unwrap())
            .collect();
        assert!(
            bosn_core::ownership_from_labels(&labels, "registry"),
            "cleanup must recognize the labels created by the launcher"
        );
    }
}
