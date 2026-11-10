//! Planning the #666 manifest keys: the default build-output volume, volume
//! and mount modes, and `keep`.

use super::*;

struct Fixture {
    _temporary: kernal_api::platform::fs::TemporaryDirectory,
    workspace: PathBuf,
}

fn fixture() -> Fixture {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    Fixture {
        _temporary: temporary,
        workspace,
    }
}

fn plan(fixture: &Fixture, body: &str, task: Option<&str>) -> Result<ManifestRuntimePlan, String> {
    let image = format!("example.invalid/app@sha256:{}", "a".repeat(64));
    std::fs::write(
        fixture.workspace.join("bosn.toml"),
        format!("[stack.app]\nimage = '{image}'\n{body}"),
    )
    .unwrap();
    let runtime = RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap();
    match task {
        None => runtime.run(manifest_stack_setup_plan(&ManifestEnsureJobRequest {
            workspace: fixture.workspace.clone(),
            manifest: "bosn.toml".into(),
            stack: "app".into(),
            deadline: Duration::from_secs(1),
            output_limit: 64,
        })),
        Some(task) => runtime.run(manifest_stack_task_setup_plan(&ManifestAppTaskJobRequest {
            clean: Default::default(),
            wait: false,
            workspace: fixture.workspace.clone(),
            manifest: "bosn.toml".into(),
            stack: "app".into(),
            task_name: task.into(),
            args: Vec::new(),
            deadline: Duration::from_secs(1),
            output_limit: 64,
        })),
    }
}

const SOURCE: &str = "[stack.app.mounts.src]\nsource = '.'\ndestination = '/src'\n";

fn targets(plan: &ManifestRuntimePlan) -> Vec<&str> {
    plan.volumes.iter().map(|v| v.target.as_str()).collect()
}

#[test]
fn a_source_mount_gets_an_exclusive_stack_target_volume() {
    let fixture = fixture();
    let plan = plan(&fixture, SOURCE, None).unwrap();
    assert_eq!(targets(&plan), ["/src/target"]);
    let volume = &plan.volumes[0];
    assert_eq!(volume.scope, Scope::Stack);
    assert_eq!(
        plan.volume_modes[&volume.name],
        bosn_core::manifest::VolumeMode::Exclusive
    );
    assert_eq!(plan.plan.named_volumes[0].target, "/src/target");
}

#[test]
fn the_target_volume_is_absent_without_a_source_mount_or_when_opted_out() {
    let fixture = fixture();
    assert!(plan(&fixture, "", None).unwrap().volumes.is_empty());
    std::fs::create_dir(fixture.workspace.join("sub")).unwrap();
    let sub = "[stack.app.mounts.sub]\nsource = 'sub'\ndestination = '/sub'\n";
    assert!(plan(&fixture, sub, None).unwrap().volumes.is_empty());
    let opted_out = format!("default_target_volume = false\n{SOURCE}");
    assert!(plan(&fixture, &opted_out, None).unwrap().volumes.is_empty());
}

#[test]
fn a_declared_target_volume_suppresses_the_default() {
    let fixture = fixture();
    let declared =
        format!("{SOURCE}[stack.app.volumes.out]\nscope = 'stack'\ndestination = '/src/target'\n");
    let plan_ = plan(&fixture, &declared, None).unwrap();
    assert_eq!(targets(&plan_), ["/src/target"]);
    let named = format!("{SOURCE}[stack.app.volumes.target]\ndestination = '/cache/target'\n");
    assert_eq!(
        targets(&plan(&fixture, &named, None).unwrap()),
        ["/cache/target"]
    );
}

#[test]
fn a_read_only_source_gets_the_volume_only_over_an_existing_target_dir() {
    let fixture = fixture();
    let readonly = "[stack.app.mounts.src]\nsource = '.'\ndestination = '/src'\nmode = 'ro'\n";
    assert!(plan(&fixture, readonly, None).unwrap().volumes.is_empty());
    std::fs::create_dir(fixture.workspace.join("target")).unwrap();
    assert_eq!(
        targets(&plan(&fixture, readonly, None).unwrap()),
        ["/src/target"]
    );
}

#[test]
fn the_default_volume_and_a_volume_mode_roll_the_generation() {
    let fixture = fixture();
    let none = plan(&fixture, SOURCE, None).unwrap().generation;
    let opted_out = format!("default_target_volume = false\n{SOURCE}");
    assert_ne!(plan(&fixture, &opted_out, None).unwrap().generation, none);
    let cache =
        |mode: &str| format!("{SOURCE}[stack.app.volumes.cache]\nscope = 'machine'\n{mode}");
    let implicit = plan(&fixture, &cache(""), None).unwrap().generation;
    let explicit = plan(&fixture, &cache("mode = 'shared'\n"), None)
        .unwrap()
        .generation;
    let exclusive = plan(&fixture, &cache("mode = 'exclusive'\n"), None)
        .unwrap()
        .generation;
    assert_eq!(implicit, explicit, "a default mode keeps the generation");
    assert_ne!(implicit, exclusive);
}

#[test]
fn readonly_and_mode_ro_plan_the_same_bind() {
    let fixture = fixture();
    let alias = plan(&fixture, "default_target_volume = false\n[stack.app.mounts.src]\nsource = '.'\ndestination = '/src'\nreadonly = true\n", None).unwrap();
    let mode = plan(&fixture, "default_target_volume = false\n[stack.app.mounts.src]\nsource = '.'\ndestination = '/src'\nmode = 'ro'\n", None).unwrap();
    assert!(alias.plan.app.mounts[0].readonly);
    assert_eq!(alias.generation, mode.generation);
}

fn overlay(upper: &str) -> String {
    format!("default_target_volume = false\n{SOURCE}mode = 'overlay'\nupper = '{upper}'\n")
}

#[test]
fn overlay_tmpfs_binds_a_run_volume_without_a_workspace_bind() {
    let f = fixture();
    let p = plan(&f, &overlay("tmpfs"), None).unwrap();
    assert!(p.plan.app.mounts.is_empty());
    assert_eq!(p.plan.named_volumes[0].target, "/src");
    assert!(
        p.plan.named_volumes[0]
            .name
            .starts_with("bosn-overlay-run-")
    );
}

#[test]
fn overlay_named_upper_binds_the_declared_volume() {
    let f = fixture();
    let body = format!(
        "{}[stack.app.volumes.out]\nscope = 'stack'\n",
        overlay("out")
    );
    let p = plan(&f, &body, None).unwrap();
    assert!(p.plan.app.mounts.is_empty());
    let binding = p
        .plan
        .named_volumes
        .iter()
        .find(|v| v.target == "/src")
        .unwrap();
    assert_eq!(binding.name, p.volumes[0].name);
}

#[test]
fn overlay_rejects_undeclared_upper_and_kept_tasks() {
    let f = fixture();
    assert!(
        plan(&f, &overlay("missing"), None)
            .unwrap_err()
            .contains("not a volume")
    );
    let body = format!(
        "{}[task.shell]\ncmd = 'sh'\nkeep = true\n",
        overlay("tmpfs")
    );
    assert!(
        plan(&f, &body, Some("shell"))
            .unwrap_err()
            .contains("keep = true")
    );
}

#[test]
fn overlay_mode_and_upper_roll_the_generation() {
    let f = fixture();
    let volume = "[stack.app.volumes.out]\nscope = 'stack'\n";
    let ro = plan(
        &f,
        &format!("default_target_volume = false\n{SOURCE}mode = 'ro'\n{volume}"),
        None,
    )
    .unwrap();
    let tmp = plan(&f, &format!("{}{volume}", overlay("tmpfs")), None).unwrap();
    let named = plan(&f, &format!("{}{volume}", overlay("out")), None).unwrap();
    assert_ne!(ro.generation, tmp.generation);
    assert_ne!(tmp.generation, named.generation);
}

#[test]
fn keep_is_carried_from_the_selected_task() {
    let fixture = fixture();
    let tasks = "[task.shell]\ncmd = 'sh'\nkeep = true\n[task.build]\ncmd = 'make'\n";
    assert!(plan(&fixture, tasks, Some("shell")).unwrap().keep);
    assert!(!plan(&fixture, tasks, Some("build")).unwrap().keep);
    assert!(!plan(&fixture, tasks, None).unwrap().keep);
}

#[test]
fn overlay_workdir_is_applied_inside_the_task_without_a_host_bind() {
    let f = fixture();
    let body = format!(
        "workdir = '/src'\n{}[task.build]\ncmd = 'pwd'\n",
        overlay("tmpfs")
    );
    let p = plan(&f, &body, Some("build")).unwrap();
    assert!(p.plan.app.workdir.is_none());
    assert!(p.plan.app.mounts.is_empty());
    assert_eq!(p.plan.tasks["build"].command, "cd '/src' || exit; pwd");
}

#[test]
fn overlay_workdir_keeps_a_more_specific_bind_mapping() {
    let f = fixture();
    std::fs::create_dir(f.workspace.join("sub")).unwrap();
    let body = format!(
        "workdir = '/other'\n{}[stack.app.mounts.sub]\nsource = 'sub'\ndestination = '/other'\n[task.build]\ncmd = 'pwd'\n",
        overlay("tmpfs")
    );
    let p = plan(&f, &body, Some("build")).unwrap();
    assert_eq!(p.plan.app.workdir.as_deref(), Some("sub"));
    assert_eq!(p.plan.tasks["build"].command, "pwd");
}

#[test]
fn overlay_ensure_requires_an_ephemeral_task() {
    let f = fixture();
    let p = plan(&f, &overlay("tmpfs"), None).unwrap();
    assert!(
        crate::overlay_source::require_task(&p.overlay_sources)
            .unwrap_err()
            .contains("--task")
    );
}
