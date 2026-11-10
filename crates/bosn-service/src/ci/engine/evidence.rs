//! Bounded, read-only evidence tied to the original container, never a replacement.
use super::{
    ActEngineBackend, DockerActBackend, EngineObservation, EngineState, SAMPLE_DEADLINE, owned,
};
use serde::{Deserialize, Serialize};

const EVIDENCE_OUTPUT: usize = 128 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "availability", content = "value", rename_all = "snake_case")]
pub enum Capture<T> {
    Available(T),
    Unavailable(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Limits {
    memory: Option<i64>,
    nano_cpus: Option<i64>,
    pids_limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspect {
    id: String,
    state: EngineState,
    host_config: Option<Limits>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DockerEvent {
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "timeNano")]
    time_nano: u64,
    #[serde(rename = "Actor")]
    actor: EventActor,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EventActor {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Attributes")]
    attributes: EventAttributes,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EventAttributes {
    name: Option<String>,
    signal: Option<String>,
    #[serde(rename = "exitCode")]
    exit_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineEvidence {
    target: String,
    inspected_id: Option<String>,
    pub observation: EngineObservation,
    pub missing: bool,
    state: Option<EngineState>,
    limits: Option<Limits>,
    inspect_unavailable: Option<String>,
    cgroup_memory: Capture<String>,
    docker_events: Capture<Vec<DockerEvent>>,
}

impl EngineEvidence {
    pub fn observed(target: &str, observation: EngineObservation) -> Self {
        let state = match &observation {
            EngineObservation::Stopped(state) => Some(state.clone()),
            _ => None,
        };
        Self {
            target: target.into(),
            inspected_id: None,
            observation,
            missing: false,
            state,
            limits: None,
            inspect_unavailable: Some("full inspect not supplied by backend".into()),
            cgroup_memory: Capture::Unavailable("not sampled".into()),
            docker_events: Capture::Unavailable("not sampled".into()),
        }
    }

    pub fn summary(&self) -> String {
        if let Some(state) = &self.state {
            return format!(
                "{}: status={}, exit={}, OOMKilled={}, finished={}, Docker error={}",
                self.target,
                state.status,
                state.exit_code,
                state.oom_killed,
                state.finished_at,
                state.error.as_deref().unwrap_or("unavailable")
            );
        }
        format!(
            "{}: {}{}",
            self.target,
            if self.missing {
                "container absent; "
            } else {
                ""
            },
            self.inspect_unavailable
                .as_deref()
                .unwrap_or("state unavailable")
        )
        .chars()
        .take(1024)
        .collect()
    }

    pub fn json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|error| format!("evidence serialization unavailable: {error}"))
    }
}

fn parse(target: &str, json: &[u8]) -> Result<EngineEvidence, String> {
    let inspect: Inspect = serde_json::from_slice(json).map_err(|error| error.to_string())?;
    let observation = if inspect.state.running {
        EngineObservation::Running
    } else {
        EngineObservation::Stopped(inspect.state.clone())
    };
    let mut evidence = EngineEvidence::observed(target, observation);
    evidence.inspected_id = Some(inspect.id);
    evidence.state = Some(inspect.state);
    evidence.limits = inspect.host_config;
    evidence.inspect_unavailable = None;
    Ok(evidence)
}

async fn capture(backend: &DockerActBackend, args: Vec<String>) -> Capture<String> {
    match backend
        .run_bounded(
            args,
            super::RunOptions::bounded(SAMPLE_DEADLINE, EVIDENCE_OUTPUT),
        )
        .await
    {
        Ok(result) if result.ok() => {
            Capture::Available(String::from_utf8_lossy(&result.stdout).into_owned())
        }
        Ok(result) => Capture::Unavailable(format!(
            "exit {}: {}",
            result.exit_code,
            String::from_utf8_lossy(&result.stderr)
        )),
        Err(error) => Capture::Unavailable(error),
    }
}

pub(super) async fn collect(backend: &DockerActBackend, target: &str) -> EngineEvidence {
    let result = backend
        .run(
            owned(&["container", "inspect", "--format", "{{json .}}", target]),
            SAMPLE_DEADLINE,
        )
        .await;
    let mut evidence = match result {
        Ok(result) if result.ok() => parse(target, &result.stdout).unwrap_or_else(|error| {
            let mut evidence =
                EngineEvidence::observed(target, EngineObservation::Unknown(error.clone()));
            evidence.inspect_unavailable = Some(format!("invalid inspect: {error}"));
            evidence
        }),
        Ok(result) => {
            let message = format!(
                "inspect exit {}: {}",
                result.exit_code,
                String::from_utf8_lossy(&result.stderr)
            );
            let mut evidence =
                EngineEvidence::observed(target, EngineObservation::Unknown(message.clone()));
            evidence.missing = result.reports_missing();
            evidence.inspect_unavailable = Some(message);
            evidence
        }
        Err(error) => {
            let mut evidence =
                EngineEvidence::observed(target, EngineObservation::Unknown(error.clone()));
            evidence.inspect_unavailable = Some(error);
            evidence
        }
    };
    if matches!(evidence.observation, EngineObservation::Running) {
        evidence.cgroup_memory =
            capture(backend, owned(&["exec", target, "sh", "-c", MEMORY_EVENTS])).await;
    } else {
        evidence.cgroup_memory =
            Capture::Unavailable("engine not observed running; cgroup read unavailable".into());
    }
    evidence.docker_events = events(backend, target).await;
    evidence
}

async fn events(backend: &DockerActBackend, target: &str) -> Capture<Vec<DockerEvent>> {
    let until = super::super::lifecycle::now_seconds().ceil() as u64;
    let mut args = owned(&[
        "events",
        "--since",
        &until.saturating_sub(600).to_string(),
        "--until",
        &until.to_string(),
        "--filter",
        &format!("container={target}"),
        "--format",
        "{{json .}}",
    ]);
    for event in ["kill", "die", "oom", "stop", "destroy"] {
        args.extend(owned(&["--filter", &format!("event={event}")]));
    }
    match capture(backend, args).await {
        Capture::Available(json) => match json
            .lines()
            .map(serde_json::from_str::<DockerEvent>)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(events) => Capture::Available(events),
            Err(error) => Capture::Unavailable(format!("invalid Docker event: {error}")),
        },
        Capture::Unavailable(error) => Capture::Unavailable(error),
    }
}

// The namespace may hide host ancestors. Record that explicitly, never infer their limits.
const MEMORY_EVENTS: &str = r#"
base=$(awk -F: '$1 == "0" {print $3}' /proc/1/cgroup)
if [ "$base" = / ]; then printf 'host ancestors: unavailable (engine cgroup namespace)\n'; fi
for depth in 0 1 2 3 4 5 6 7; do
  for leaf in memory.events memory.max memory.high cpu.max pids.max; do
    file="/sys/fs/cgroup${base%/}/$leaf"
    printf '\n%s\n' "$file"
    cat "$file" 2>&1 || printf 'unavailable\n'
  done
  [ "$base" = / ] && break
  base=$(dirname "$base")
done
count=0
for file in /sys/fs/cgroup/run-*/memory.events; do
  [ "$count" -ge 32 ] && break
  printf '\n%s\n' "$file"
  cat "$file" 2>&1 || printf 'unavailable\n'
  count=$((count+1))
done
"#;

#[derive(Clone, Copy, Debug)]
pub enum RemovalReason {
    Idle,
    Stopped,
    Replacement,
    Takeover,
    Preparation,
    Spare,
    Legacy,
    Recovery,
}

impl RemovalReason {
    fn actor(self) -> &'static str {
        match self {
            Self::Idle | Self::Stopped => "shared-engine retirement",
            Self::Replacement => "shared-engine admission",
            Self::Takeover => "dead-daemon recovery",
            Self::Recovery => "registry cleanup recovery",
            Self::Preparation => "preparation cleanup",
            Self::Spare => "spare retirement",
            Self::Legacy => "legacy drain",
        }
    }
    fn reason(self) -> &'static str {
        match self {
            Self::Idle => "idle or retirement requested",
            Self::Stopped => "positively stopped",
            Self::Replacement => "configuration or pins changed",
            Self::Takeover => "maker daemon dead",
            Self::Recovery => "durable cleanup_required record",
            Self::Preparation => "interrupted preparation",
            Self::Spare => "spare retired",
            Self::Legacy => "legacy engine proven idle twice",
        }
    }
}

pub async fn log_removal(
    backend: &dyn ActEngineBackend,
    target: &str,
    name: &str,
    reason: RemovalReason,
) {
    let evidence = backend.engine_evidence(target).await;
    daemon_log!(
        "bosn ci: engine {name} removed by {} pid {} ({}): {}",
        reason.actor(),
        std::process::id(),
        reason.reason(),
        evidence.json()
    );
}

#[cfg(test)]
mod tests;
