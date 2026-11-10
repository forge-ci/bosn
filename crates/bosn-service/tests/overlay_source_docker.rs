//! Copy-on-start proofs use a private workspace and daemon, never shared objects.
use super::stack_task_docker::StackWorkspace;
use crate::support::setup_docker::*;
use kernal_api::async_engine;
use serde::Deserialize;

struct OverlayWorkspace(StackWorkspace);
impl OverlayWorkspace {
    fn new(upper: &str, command: &str) -> Self {
        let volume = if upper == "tmpfs" {
            String::new()
        } else {
            "[stack.live.volumes.out]\nscope = 'stack'\n".into()
        };
        let manifest = format!(
            "[stack.live]\nimage = '{PINNED_ALPINE}'\nworkdir = '/src'\ndefault_target_volume = false\n\
            [stack.live.mounts.src]\nsource = '.'\ndestination = '/src'\nmode = 'overlay'\nupper = '{upper}'\n\
            {volume}[task.probe]\nstack = 'live'\ncmd = '''{command}'''\n"
        );
        Self(StackWorkspace::new(&manifest))
    }
    fn volumes(&self) -> Vec<String> {
        let output = docker_capture(
            &self.0.engine,
            [
                "volume",
                "ls",
                "--filter",
                &format!(
                    "label={}={}",
                    bosn_core::LABEL_WORKSPACE,
                    self.0.workspace.display()
                ),
                "--format",
                "{{.Name}}",
            ],
        );
        assert!(output.ok());
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}
impl Drop for OverlayWorkspace {
    fn drop(&mut self) {
        for volume in self.volumes() {
            let output = docker_capture(&self.0.engine, ["volume", "rm", &volume]);
            assert!(output.ok(), "test volume cleanup: {output:?}");
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspection {
    host_config: HostConfig,
    mounts: Vec<Mount>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HostConfig {
    cap_add: Option<Vec<String>>,
    devices: Option<Vec<Device>>,
    privileged: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Device {
    path_on_host: String,
}
#[derive(Deserialize)]
struct Mount {
    #[serde(rename = "Type")]
    kind: String,
}

fn inspect_running(stack: &StackWorkspace, child: &mut std::process::Child) -> Option<Inspection> {
    let runtime = RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let deadline = async_engine::Deadline::after(Duration::from_secs(30));
        loop {
            let output = docker_capture(
                &stack.engine,
                [
                    "ps",
                    "--filter",
                    &format!(
                        "label={}={}",
                        bosn_core::LABEL_WORKSPACE,
                        stack.workspace.display()
                    ),
                    "--filter",
                    &format!("label={}=probe", bosn_core::LABEL_TASK),
                    "--format",
                    "{{.ID}}",
                ],
            );
            let id = String::from_utf8(output.stdout).unwrap();
            if !id.trim().is_empty() {
                let output = docker_capture(&stack.engine, ["inspect", id.trim()]);
                let mut rows: Vec<Inspection> = serde_json::from_slice(&output.stdout).unwrap();
                return Some(rows.remove(0));
            }
            if child.try_wait().unwrap().is_some() {
                return None;
            }
            if deadline.remaining().is_zero() {
                child.kill().unwrap();
                return None;
            }
            async_engine::sleep(Duration::from_millis(100)).await;
        }
    })
}

#[test]
#[ignore = "requires local Docker and the pinned helper and Alpine images"]
fn live_docker_overlay_tmpfs_is_fresh_unprivileged_and_leaves_no_volumes() {
    let stack = OverlayWorkspace::new("tmpfs", "set -eu; test ! -e /src/x; touch /src/x; sleep 3");
    for _ in 0..2 {
        let mut child = stack
            .0
            .bosn_run("probe")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let row = inspect_running(&stack.0, &mut child);
        StackWorkspace::output_of("probe", child.wait_with_output().unwrap());
        let row = row.expect("task did not start");
        assert!(row.host_config.cap_add.unwrap_or_default().is_empty());
        assert!(!row.host_config.privileged);
        assert!(
            row.host_config
                .devices
                .unwrap_or_default()
                .iter()
                .all(|d| d.path_on_host != "/dev/fuse")
        );
        assert!(
            row.mounts.iter().all(|m| m.kind != "bind"),
            "task must never bind the host source"
        );
        assert!(!stack.0.workspace.join("x").exists());
        assert!(
            stack.volumes().is_empty(),
            "temporary upper must be removed"
        );
    }
}

#[test]
#[ignore = "requires local Docker and the pinned helper and Alpine images"]
fn live_docker_overlay_volume_refreshes_host_files_and_keeps_upper_only_files() {
    let stack = OverlayWorkspace::new(
        "out",
        "set -eu; if test -e /src/out; then test \"$(cat /src/a.txt)\" = new; test -e /src/deleted; else touch /src/out; fi; test -L /src/link; test -e /src/.git/marker; test -e /src/ignored",
    );
    let workspace = &stack.0.workspace;
    std::fs::create_dir(workspace.join(".git")).unwrap();
    std::fs::write(workspace.join(".git/marker"), "git data").unwrap();
    std::fs::write(workspace.join(".gitignore"), "ignored\n").unwrap();
    std::fs::write(workspace.join("ignored"), "ignored data").unwrap();
    std::fs::write(workspace.join("a.txt"), "old").unwrap();
    std::fs::write(workspace.join("deleted"), "keep").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.txt", workspace.join("link")).unwrap();
    stack.0.run_task("probe");
    std::fs::write(workspace.join("a.txt"), "new").unwrap();
    std::fs::rename(workspace.join("deleted"), stack.0.state.join("deleted")).unwrap();
    stack.0.run_task("probe");
    assert!(!workspace.join("out").exists());
}
