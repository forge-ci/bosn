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
            workspace: fixture.workspace.clone(),
            manifest: "bosn.toml".into(),
            stack: "app".into(),
            task_name: task.into(),
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

#[test]
fn an_overlay_mount_is_refused_until_implemented() {
    let fixture = fixture();
    let overlay = "[stack.app.mounts.src]\nsource = '.'\ndestination = '/src'\nmode = 'overlay'\n";
    let error = plan(&fixture, overlay, None).unwrap_err();
    assert_eq!(error, "overlay mounts are not implemented yet (#666)");
}

#[test]
fn keep_is_carried_from_the_selected_task() {
    let fixture = fixture();
    let tasks = "[task.shell]\ncmd = 'sh'\nkeep = true\n[task.build]\ncmd = 'make'\n";
    assert!(plan(&fixture, tasks, Some("shell")).unwrap().keep);
    assert!(!plan(&fixture, tasks, Some("build")).unwrap().keep);
    assert!(!plan(&fixture, tasks, None).unwrap().keep);
}
