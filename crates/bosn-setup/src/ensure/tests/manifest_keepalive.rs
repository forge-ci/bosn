use super::*;

#[test]
fn manifest_keepalive_overrides_and_verifies_image_entrypoint() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path();
    std::fs::create_dir(workspace.join("src")).unwrap();
    let mut plan = plan(workspace);
    plan.app.command = Some(crate::MANIFEST_LINUX_IDLE_COMMAND.into());
    plan.manifest_keepalive = true;
    let image = prepared(&plan);
    let expected = derive_creation(&plan, workspace, &image).unwrap();
    let create = expected.create_command();
    let args = create.docker_args();
    assert!(args.windows(2).any(|pair| pair == ["--entrypoint", "sh"]));
    let mut base = fixture_image();
    base["Config"]["Entrypoint"] = serde_json::json!(["/image-entrypoint"]);
    let observed = observed(&plan, true);
    verify_actual_configuration(&observed, &expected, &base).unwrap();
    let mut wrong = observed.clone();
    wrong.configuration["Config"]["Entrypoint"] = serde_json::json!(["/image-entrypoint"]);
    assert!(verify_actual_configuration(&wrong, &expected, &base).is_err());
    let mut legacy = plan.clone();
    legacy.manifest_keepalive = false;
    let old_name = setup_container_name(&legacy, workspace, &image).unwrap();
    let new_name = setup_container_name(&plan, workspace, &image).unwrap();
    assert_ne!(
        old_name, new_name,
        "existing containers are never adopted into the new shape"
    );
}

#[test]
fn keepalive_flag_refuses_arbitrary_commands_and_guests() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("src")).unwrap();
    let mut plan = plan(workspace.path());
    plan.manifest_keepalive = true;
    assert!(derive_creation(&plan, workspace.path(), &prepared(&plan)).is_err());
    let mut guest = macos_guest_plan(workspace.path());
    guest.manifest_keepalive = true;
    guest.app.command = Some(crate::MANIFEST_LINUX_IDLE_COMMAND.into());
    assert!(derive_creation(&guest, workspace.path(), &prepared(&guest)).is_err());
}

#[path = "manifest_keepalive_live.rs"]
mod live;
