use super::*;

#[test]
fn recorded_inspect_preserves_original_state_and_constraints() {
    let evidence = parse("recorded-original", include_bytes!("inspect.json")).unwrap();
    assert!(
        evidence
            .inspected_id
            .as_ref()
            .is_some_and(|id| id.len() == 64)
    );
    let state = evidence.state.unwrap();
    assert!(state.running);
    assert!(state.error.is_some());
    let limits = evidence.limits.unwrap();
    assert!(limits.memory.is_some());
    assert!(limits.nano_cpus.is_some());
}

#[test]
fn incomplete_inspect_is_not_a_stopped_engine() {
    assert!(
        parse(
            "original",
            br#"{"Id":"original","State":{"Running":false}}"#
        )
        .is_err()
    );
}

#[test]
fn terminal_events_preserve_signal_exit_and_time_without_command_environment() {
    let json = r#"{"Action":"die","timeNano":1794269000000000000,"Actor":{"ID":"original","Attributes":{"exitCode":"137","name":"bosn-act-original"}}}"#;
    let event: DockerEvent = serde_json::from_str(json).unwrap();
    assert_eq!(event.action, "die");
    assert_eq!(event.actor.attributes.exit_code.as_deref(), Some("137"));
    assert_eq!(event.actor.id, "original");
}
