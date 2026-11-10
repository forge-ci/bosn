//! Opt-in Docker proofs for independent warm worktree volumes (#327).
use crate::support::setup_docker::*;
use bosn_core::Scope;
use kernal_api::platform::fs;

struct Fixture {
    _root: tempfile::TempDir,
    clone: std::path::PathBuf,
    worktree: std::path::PathBuf,
    state: std::path::PathBuf,
    engine: DockerEngine,
    daemon: Option<DaemonChild>,
}

fn git(directory: &Path, args: &[&str]) {
    let result = Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
impl Fixture {
    fn new() -> Self {
        let engine = DockerEngine::docker();
        pinned_alpine_identity(&engine);
        let root = std::env::var_os("BOSN_LIVE_TEST_ROOT").map_or_else(
            || tempfile::tempdir().unwrap(),
            |p| tempfile::tempdir_in(p).unwrap(),
        );
        let origin = root.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        git(&origin, &["init"]);
        std::fs::write(origin.join("bosn.toml"), format!(
            "[stack.cook]\nimage = '{PINNED_ALPINE}'\ndefault = true\ndefault_target_volume = false\n\
             [stack.cook.volumes.state]\nscope = 'stack'\ndestination = '/state'\n\
             [task.write]\nstack = 'cook'\ncmd = 'printf warm > /state/marker'\n\
             [task.read]\nstack = 'cook'\ncmd = 'if test -f /state/marker; then cat /state/marker; else echo cold; fi'\n\
             [task.change]\nstack = 'cook'\ncmd = 'printf changed > /state/marker'\n"
        )).unwrap();
        git(&origin, &["add", "bosn.toml"]);
        git(
            &origin,
            &[
                "-c",
                "user.name=Bosn test",
                "-c",
                "user.email=bosn@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        );
        git(root.path(), &["clone", "origin", "clone"]);
        let clone = root.path().join("clone").canonicalize().unwrap();
        let worktree = root.path().join("worktree");
        git(
            &clone,
            &[
                "worktree",
                "add",
                "-b",
                "seed-test",
                worktree.to_str().unwrap(),
            ],
        );
        let state = root.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let stderr = std::fs::File::create(state.join("daemon.log")).unwrap();
        let mut daemon = DaemonChild {
            child: Command::new(env!("CARGO_BIN_EXE_bosn"))
                .args(["daemon", "serve", "--state-dir"])
                .arg(&state)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(stderr)
                .spawn()
                .unwrap(),
        };
        let runtime = RuntimeBuilder::current_thread()
            .enable_all()
            .build()
            .unwrap();
        wait_for_client(&runtime, &mut daemon, &state);
        Self {
            _root: root,
            clone,
            worktree: worktree.canonicalize().unwrap(),
            state,
            engine,
            daemon: Some(daemon),
        }
    }
    fn run(&self, workspace: &Path, task: &str) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_bosn"))
            .current_dir(workspace)
            .args(["run", "--task", task, "--state-dir"])
            .arg(&self.state)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn resources(&self) -> Vec<bosn_registry::Resource> {
        Registry::open_read_only(self.state.join("registry.sqlite3"))
            .unwrap()
            .resources(0, 256)
            .unwrap()
            .items
    }
    fn volume(&self, workspace: &Path) -> String {
        self.resources()
            .into_iter()
            .find(|r| {
                r.kind == ResourceKind::Volume
                    && r.scope == Scope::Stack
                    && r.workspace == workspace.to_str().unwrap()
            })
            .unwrap()
            .name
    }
    fn remove_containers(&self, workspace: &Path) {
        for resource in self.resources().into_iter().filter(|r| {
            r.kind == ResourceKind::Container && r.workspace == workspace.to_str().unwrap()
        }) {
            let result =
                docker_capture(&self.engine, ["container", "rm", "--force", &resource.name]);
            assert!(result.ok() || result.reports_missing());
        }
    }
    fn label(&self, volume: &str) -> String {
        let result = docker_capture(
            &self.engine,
            [
                "volume",
                "inspect",
                "--format",
                "{{index .Labels \"com.zackees.bosn.seeded-from\"}}",
                volume,
            ],
        );
        assert!(result.ok());
        String::from_utf8(result.stdout).unwrap().trim().into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.daemon.take());
        // This registry belongs only to the fixture. Never use global prune.
        let resources = self.resources();
        for r in resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Container)
        {
            docker_capture(&self.engine, ["container", "rm", "--force", &r.name]);
        }
        for r in resources.iter().filter(|r| r.kind == ResourceKind::Volume) {
            docker_capture(&self.engine, ["volume", "rm", &r.name]);
        }
    }
}

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_new_worktree_seeds_an_independent_stack_volume() {
    let f = Fixture::new();
    f.run(&f.clone, "write");
    let source = f.volume(&f.clone);
    f.remove_containers(&f.clone); // Even a stopped attachment is not an idle source.
    assert!(f.run(&f.worktree, "read").contains("warm"));
    let seeded = f.volume(&f.worktree);
    assert_ne!(source, seeded);
    assert_eq!(f.label(&seeded), source);
    f.run(&f.worktree, "change");
    assert!(f.run(&f.clone, "read").contains("warm"));
    f.remove_containers(&f.worktree);
    assert!(f.run(&f.worktree, "read").contains("changed")); // Existing volume is untouched.
}

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_exclusive_source_lease_creates_cold_without_waiting() {
    let f = Fixture::new();
    f.run(&f.clone, "write");
    let source = f.volume(&f.clone);
    f.remove_containers(&f.clone);
    let uid = kernal_api::platform::ipc::current_user_id().unwrap();
    let directory = std::path::PathBuf::from(format!("/tmp/bosn-{uid}/cache-locks"));
    std::fs::create_dir_all(&directory).unwrap();
    let lock = fs::try_lock_exclusive_owned(
        fs::open_lock_file(&directory.join(format!("{source}.lock"))).unwrap(),
    )
    .unwrap();
    // The call must finish while the exclusive source lease is still held.
    assert!(f.run(&f.worktree, "read").contains("cold"));
    assert!(f.label(&f.volume(&f.worktree)).is_empty());
    let log = std::fs::read_to_string(f.state.join("daemon.log")).unwrap();
    assert!(
        log.contains("exclusive lease") && log.contains("cold create"),
        "{log}"
    );
    drop(lock);
}
