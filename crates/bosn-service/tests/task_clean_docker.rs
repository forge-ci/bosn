//! Exact plan reset, with isolated checkout/registry and unique machine family.
use super::stack_task_docker::{StackWorkspace, stack_manifest};
use crate::support::setup_docker::*;

struct OwnedStack(Option<StackWorkspace>);
impl OwnedStack {
    fn new() -> Self {
        let marker = test_unique_suffix();
        let manifest = stack_manifest(&marker, &[("probe", concat!(
            "if test -e /state/file; then echo stack-warm; else echo stack-cold; fi; ",
            "if test -e /machine/file; then echo machine-warm; else echo machine-cold; fi; ",
            "touch /state/file /machine/file"))])
            .replace("[stack.live]", &format!("[stack.live]\nfamily = '{marker}'"))
            .replace("[stack.live.env]", "machine = { scope = 'machine', destination = '/machine' }\n[stack.live.env]")
            .replace("keep = true", "keep = false");
        Self(Some(StackWorkspace::new(&manifest)))
    }
    fn stack(&self) -> &StackWorkspace {
        self.0.as_ref().unwrap()
    }
    fn stop_preparation(&self) {
        for id in self.stack().containers() {
            assert!(docker_capture(&self.stack().engine, ["stop", &id]).ok());
        }
    }
    fn clean(&self, flag: &str) -> String {
        let output = self.stack().bosn_run("probe").arg(flag).output().unwrap();
        StackWorkspace::output_of("probe", output)
    }
}
impl Drop for OwnedStack {
    fn drop(&mut self) {
        let stack = self.0.take().unwrap();
        let registry = Registry::open_read_only(stack.state.join("registry.sqlite3")).unwrap();
        let volumes: Vec<_> = registry
            .resources(0, 256)
            .unwrap()
            .items
            .into_iter()
            .filter(|r| r.kind == bosn_core::ResourceKind::Volume)
            .map(|r| r.name)
            .collect();
        let engine = stack.engine.clone();
        drop(registry);
        drop(stack);
        for name in volumes {
            docker_capture(&engine, ["volume", "rm", &name]);
        }
    }
}

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_clean_recreates_checkout_volumes_and_all_resets_machine_volumes() {
    let first = OwnedStack::new();
    let other = OwnedStack::new();
    assert!(first.stack().run_task("probe").contains("stack-cold"));
    other.stack().run_task("probe");
    let refused = first
        .stack()
        .bosn_run("probe")
        .arg("--clean")
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("container"));
    first.stop_preparation();
    let output = first.clean("--clean");
    assert!(
        output.contains("stack-cold") && output.contains("machine-warm"),
        "{output}"
    );
    let output = first.clean("--clean=all");
    assert!(
        output.contains("stack-cold") && output.contains("machine-cold"),
        "{output}"
    );
    let output = other.stack().run_task("probe");
    assert!(
        output.contains("stack-warm") && output.contains("machine-warm"),
        "{output}"
    );
}

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_clean_first_persists_ownership_for_repeated_clean_runs() {
    let stack = OwnedStack::new();
    let output = stack.clean("--clean");
    assert!(
        output.contains("stack-cold") && output.contains("machine-cold"),
        "{output}"
    );
    let output = stack.clean("--clean");
    assert!(
        output.contains("stack-cold") && output.contains("machine-warm"),
        "{output}"
    );
    let output = stack.clean("--clean=all");
    assert!(
        output.contains("stack-cold") && output.contains("machine-cold"),
        "{output}"
    );
}
