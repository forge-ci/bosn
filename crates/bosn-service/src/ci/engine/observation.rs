//! Typed Docker state: uncertainty must never authorize engine removal.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EngineState {
    pub running: bool,
    pub exit_code: i32,
    #[serde(rename = "OOMKilled")]
    pub oom_killed: bool,
    pub finished_at: String,
    pub status: String,
}

#[derive(Clone, Debug)]
pub enum EngineObservation {
    Running,
    Stopped(EngineState),
    Unknown(String),
}

pub(super) fn parse(json: &str) -> EngineObservation {
    match serde_json::from_str::<EngineState>(json) {
        Ok(state) if state.running => EngineObservation::Running,
        Ok(state) => EngineObservation::Stopped(state),
        Err(error) => EngineObservation::Unknown(format!("invalid engine state: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_state_is_typed_and_incomplete_reads_are_unknown() {
        let state = r#"{"Running":false,"ExitCode":137,"OOMKilled":true,"FinishedAt":"2026-11-09T00:00:00Z","Status":"exited"}"#;
        let EngineObservation::Stopped(state) = parse(state) else {
            panic!("expected stopped state");
        };
        assert_eq!(state.exit_code, 137);
        assert!(state.oom_killed);
        assert!(matches!(parse("{}"), EngineObservation::Unknown(_)));
        assert!(matches!(parse("true"), EngineObservation::Unknown(_)));
    }
}
