//! Real task runs against an isolated workspace and daemon state.
use super::stack_task_docker::{StackWorkspace, stack_manifest};
use crate::support::setup_docker::*;

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_ephemeral_tasks_reattach_volumes_and_clean_crash_leftovers() {
    let manifest = stack_manifest(
        &test_unique_suffix(),
        &[(
            "probe",
            "if test -e /state/first; then echo reused; else touch /state/first; fi",
        )],
    )
    .replace("keep = true", "keep = false");
    let stack = StackWorkspace::new(&manifest);
    stack.run_task("probe");
    let name = format!(
        "probe-{}",
        bosn_service::runners::short_hash(stack.workspace.to_str().unwrap().as_bytes(), 16)
    );
    assert_absent(&stack, &name);
    assert!(stack.run_task("probe").contains("reused"));
    assert_absent(&stack, &name);

    // Reproduce the owned metadata of a stopped task from an older generation.
    let registry = Registry::open_read_only(stack.state.join("registry.sqlite3"))
        .expect("registry reader")
        .registry_id()
        .expect("registry id");
    let mut labels = bosn_core::ResourceLabels::new(
        &registry,
        bosn_core::ResourceKind::Container,
        "live",
        "crash",
        bosn_core::Scope::Stack,
        stack.workspace.to_str().unwrap(),
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string(),
        None,
    )
    .unwrap()
    .to_map()
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect::<std::collections::BTreeMap<_, _>>();
    labels.insert("com.zackees.bosn.setup-managed".into(), "v1".into());
    labels.insert("com.zackees.bosn.setup-container".into(), name.clone());
    labels.insert(bosn_core::LABEL_TASK.into(), "probe".into());
    let mut args = vec![
        "create".to_owned(),
        bosn_engine::LOW_CPU_PRIORITY_ARG.into(),
        "--name".into(),
        name.clone(),
    ];
    for (key, value) in labels {
        args.extend(["--label".into(), format!("{key}={value}")]);
    }
    args.extend([
        "--mount".into(),
        format!(
            "type=bind,src={},dst=/ws,readonly",
            stack.workspace.display()
        ),
    ]);
    args.push(PINNED_ALPINE.into());
    args.extend(["sh".into(), "-c".into(), "true".into()]);
    let created = docker_capture(&stack.engine, args);
    assert!(
        created.ok(),
        "create crash leftover: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(docker_capture(&stack.engine, ["start", &name]).ok());
    assert!(docker_capture(&stack.engine, ["wait", &name]).ok());
    assert!(stack.run_task("probe").contains("reused"));
    assert_absent(&stack, &name);
}

fn assert_absent(stack: &StackWorkspace, name: &str) {
    let output = docker_capture(
        &stack.engine,
        [
            "ps",
            "-a",
            "--filter",
            &format!("name=^/{name}$"),
            "--format",
            "{{.Names}}",
        ],
    );
    assert!(output.ok());
    assert!(
        output.stdout.is_empty(),
        "leftover: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_keep_true_preserves_the_running_stack_container() {
    let manifest = stack_manifest(&test_unique_suffix(), &[("probe", "echo kept")]).replace(
        "dockerfile = 'live.Dockerfile'",
        &format!("image = '{PINNED_ALPINE}'"),
    );
    let stack = StackWorkspace::new(&manifest);
    stack.run_task("probe");
    let first = stack.containers();
    stack.run_task("probe");
    assert_eq!(first, stack.containers());
    assert_eq!(first.len(), 1);
}
