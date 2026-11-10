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
    extra_containers: Vec<String>,
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
             [stack.cook.volumes.state]\nscope = 'stack'\nseed = true\ndestination = '/state'\n\
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
        own_registry(&state);
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
        if let Err(error) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wait_for_client(&runtime, &mut daemon, &state)
        })) {
            eprintln!(
                "{}",
                std::fs::read_to_string(state.join("daemon.log")).unwrap()
            );
            std::panic::resume_unwind(error);
        }
        Self {
            _root: root,
            clone,
            worktree: worktree.canonicalize().unwrap(),
            state,
            engine,
            daemon: Some(daemon),
            extra_containers: Vec::new(),
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
        for id in &self.extra_containers {
            docker_capture(&self.engine, ["container", "rm", "--force", id]);
        }
        // This registry belongs only to the fixture. Never use global prune.
        let resources = self.resources();
        let registry = Registry::open_read_only(self.state.join("registry.sqlite3")).unwrap();
        let mut volumes: std::collections::BTreeSet<String> = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Volume)
            .map(|r| r.name.clone())
            .collect();
        volumes.extend(
            registry
                .volume_creation_intents(0, 256)
                .unwrap()
                .items
                .into_iter()
                .map(|i| i.name),
        );
        let owner = registry.registry_id().unwrap();
        for seed in registry.volume_seeds().unwrap() {
            volumes.extend([seed.source, seed.destination]);
            if let Some(helper) = registry.cache_helper(&seed.nonce).unwrap() {
                let proof = docker_capture(
                    &self.engine,
                    [
                        "container",
                        "inspect",
                        "--format",
                        "{{.Id}} {{index .Config.Labels \"com.zackees.bosn.registry\"}}",
                        &helper.intent.name(),
                    ],
                );
                let proof = String::from_utf8_lossy(&proof.stdout);
                let fields: Vec<_> = proof.split_whitespace().collect();
                if fields.len() == 2 && fields[1] == owner {
                    docker_capture(&self.engine, ["container", "rm", "--force", fields[0]]);
                }
            }
        }
        for r in resources
            .iter()
            .filter(|r| r.kind == ResourceKind::Container)
        {
            docker_capture(&self.engine, ["container", "rm", "--force", &r.name]);
        }
        for volume in volumes {
            docker_capture(&self.engine, ["volume", "rm", &volume]);
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
    let registry = Registry::open_read_only(f.state.join("registry.sqlite3")).unwrap();
    let seed = registry
        .volume_seeds()
        .unwrap()
        .into_iter()
        .find(|s| s.destination == seeded)
        .unwrap();
    assert_eq!(seed.state, bosn_registry::volume_seed::SeedState::Complete);
    assert_eq!(
        registry.cache_helper(&seed.nonce).unwrap().unwrap().state,
        bosn_registry::cache_helper::CacheHelperState::Removed
    );
    assert!(registry.leases(0, 256).unwrap().items.is_empty());
    drop(registry);
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

impl Fixture {
    fn restart(&mut self) {
        drop(self.daemon.take());
        let mut daemon = DaemonChild::start(&self.state);
        let runtime = RuntimeBuilder::current_thread()
            .enable_all()
            .build()
            .unwrap();
        wait_for_client(&runtime, &mut daemon, &self.state);
        self.daemon = Some(daemon);
    }
    fn attempt(&self, deadline: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_bosn"))
            .current_dir(&self.worktree)
            .args(["run", "--stack", "cook", "--state-dir"])
            .arg(&self.state)
            .args(["--deadline-ms", deadline])
            .output()
            .unwrap()
    }
    fn writer(&mut self) -> Registry {
        drop(self.daemon.take());
        Registry::open_writer(self.state.join("registry.sqlite3")).unwrap()
    }
    fn inject_partial(&mut self) -> bosn_registry::cache_helper::CacheHelperIntent {
        use bosn_registry::{
            cache_helper::{CacheHelperIntent, CacheHelperRole},
            volume_seed::{SeedState, VolumeSeed},
        };
        self.run(&self.clone, "write");
        self.run(&self.worktree, "change"); // Source is attached, so this is an ordinary cold volume.
        let source = self.volume(&self.clone);
        let destination = self.volume(&self.worktree);
        self.remove_containers(&self.clone);
        self.remove_containers(&self.worktree);
        let resource = self
            .resources()
            .into_iter()
            .find(|r| r.name == destination)
            .unwrap();
        let mut registry = self.writer();
        let owner = registry.registry_id().unwrap();
        let intent = CacheHelperIntent {
            registry_id: owner.clone(),
            nonce: owner,
            volume: destination.clone(),
            image: bosn_service::ci::engine::engine_image(),
            // Inject a clock past the existing bounded late-create window; no wall-clock sleep.
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
                - 631.0,
            role: Some(CacheHelperRole::VolumeSeedV1 {
                source: source.clone(),
                destination: destination.clone(),
            }),
        };
        let labels = std::collections::BTreeMap::from([
            ("com.zackees.bosn.setup-managed".into(), "v1".into()),
            (
                "com.zackees.bosn.setup-content-sha256".into(),
                resource.generation.trim_start_matches("sha256:").into(),
            ),
            (
                "com.zackees.bosn.setup-container".into(),
                destination.clone(),
            ),
        ]);
        let mut tx = registry.begin_immediate().unwrap();
        tx.put_volume_creation_intent(&bosn_registry::VolumeCreationIntent {
            name: destination.clone(),
            labels: labels.clone(),
            stack: resource.stack,
            generation: resource.generation,
            scope: resource.scope,
            workspace: resource.workspace,
        })
        .unwrap();
        tx.begin_volume_seed(
            &VolumeSeed {
                source: source.clone(),
                destination: destination.clone(),
                nonce: intent.nonce.clone(),
                state: SeedState::Copying,
            },
            &intent,
        )
        .unwrap();
        tx.commit().unwrap();
        drop(registry);
        assert!(docker_capture(&self.engine, ["volume", "rm", &destination]).ok());
        let mut args = vec!["volume".into(), "create".into()];
        for (k, v) in labels {
            args.extend(["--label".into(), format!("{k}={v}")]);
        }
        args.extend([
            "--label".into(),
            format!("com.zackees.bosn.seeded-from={source}"),
            "--label".into(),
            format!("{}={}", bosn_core::LABEL_SEED_REGISTRY, intent.registry_id),
            "--label".into(),
            format!("com.zackees.bosn.seed-nonce={}", intent.nonce),
            destination.clone(),
        ]);
        assert!(docker_capture(&self.engine, args).ok());
        // Explicitly inject partial bytes; they must never be mistaken for Complete.
        self.volume_command(&destination, "printf partial > /state/marker");
        intent
    }
    fn volume_command(&self, volume: &str, command: &str) {
        let result = docker_capture(
            &self.engine,
            [
                "run",
                "--rm",
                bosn_engine::LOW_CPU_PRIORITY_ARG,
                "--label",
                "com.zackees.bosn.test=volume-seed-327",
                "--mount",
                &format!("type=volume,source={volume},target=/state"),
                PINNED_ALPINE,
                "sh",
                "-ec",
                command,
            ],
        );
        assert!(result.ok(), "{}", String::from_utf8_lossy(&result.stderr));
    }
    fn create_seed_helper(
        &mut self,
        intent: &bosn_registry::cache_helper::CacheHelperIntent,
        register: bool,
    ) -> String {
        let bosn_registry::cache_helper::CacheHelperRole::VolumeSeedV1 {
            source,
            destination,
        } = intent.role.as_ref().unwrap()
        else {
            panic!("seed role")
        };
        let labels = bosn_core::ResourceLabels::new(
            &intent.registry_id,
            ResourceKind::Container,
            "volume-seed",
            &intent.nonce,
            Scope::Spec,
            "machine",
            &intent.created_at.to_string(),
            Some(bosn_core::Retention::Warm),
        )
        .unwrap();
        let mut args = vec![
            "create".into(),
            "--rm".into(),
            bosn_engine::LOW_CPU_PRIORITY_ARG.into(),
            "--name".into(),
            intent.name(),
            "--label".into(),
            format!("io.bosn.volume.seed={}", intent.nonce),
            "--pull=never".into(),
            "--network=none".into(),
            "--read-only".into(),
            "--tmpfs".into(),
            format!(
                "/var/lib/docker:{}",
                bosn_registry::cache_helper::VOLUME_SEED_TMPFS
            ),
            "--init=false".into(),
            "--restart=no".into(),
            "--memory=512m".into(),
            "--cpus=2".into(),
            "--pids-limit=128".into(),
            "--mount".into(),
            format!("type=volume,source={source},target=/source,readonly,volume-nocopy"),
            "--mount".into(),
            format!("type=volume,source={destination},target=/destination,volume-nocopy"),
            "--entrypoint".into(),
            "/bin/sh".into(),
            intent.image.clone(),
            "-ec".into(),
            bosn_registry::cache_helper::VOLUME_SEED_COMMAND.into(),
        ];
        for (k, v) in labels.to_map() {
            args.splice(1..1, ["--label".into(), format!("{k}={v}")]);
        }
        let result = docker_capture(&self.engine, args);
        assert!(result.ok(), "{}", String::from_utf8_lossy(&result.stderr));
        let id = String::from_utf8(result.stdout).unwrap().trim().to_owned();
        self.extra_containers.push(id.clone());
        if register {
            let mut registry = self.writer();
            let mut tx = registry.begin_immediate().unwrap();
            tx.register_cache_helper(&intent.nonce, &id, intent.created_at + 1.0)
                .unwrap();
            tx.commit().unwrap();
        }
        id
    }
    fn assert_recovered_cold(&self, intent: &bosn_registry::cache_helper::CacheHelperIntent) {
        assert!(self.run(&self.worktree, "read").contains("cold"));
        let registry = Registry::open_read_only(self.state.join("registry.sqlite3")).unwrap();
        assert!(registry.volume_seeds().unwrap().is_empty());
        assert_eq!(
            registry.cache_helper(&intent.nonce).unwrap().unwrap().state,
            bosn_registry::cache_helper::CacheHelperState::Removed
        );
        assert!(registry.leases(0, 256).unwrap().items.is_empty());
        assert!(self.label(&intent.volume).is_empty());
    }
}
#[test]
#[ignore = "requires Docker and the pinned Alpine and Docker helper images"]
fn live_docker_partial_copy_with_killed_registered_helper_recovers_cold() {
    let mut f = Fixture::new();
    let intent = f.inject_partial();
    let id = f.create_seed_helper(&intent, true);
    // Inject the persisted state of a helper killed after it wrote partial bytes.
    assert!(docker_capture(&f.engine, ["container", "rm", "--force", &id]).ok());
    f.restart();
    f.assert_recovered_cold(&intent);
}
#[test]
#[ignore = "requires Docker and the pinned Alpine and Docker helper images"]
fn live_docker_crash_after_create_before_registration_recovers_by_nonce() {
    let mut f = Fixture::new();
    let intent = f.inject_partial();
    let id = f.create_seed_helper(&intent, false);
    f.restart();
    f.assert_recovered_cold(&intent);
    assert!(docker_capture(&f.engine, ["container", "inspect", &id]).reports_missing());
}
#[test]
#[ignore = "requires Docker and the pinned Alpine and Docker helper images"]
fn live_docker_absent_late_create_recovers_after_injected_window() {
    let mut f = Fixture::new();
    let intent = f.inject_partial();
    f.restart();
    f.assert_recovered_cold(&intent);
}
#[test]
#[ignore = "requires Docker and the pinned Alpine and Docker helper images"]
fn live_docker_external_attachment_keeps_copying_destination_untouched() {
    let mut f = Fixture::new();
    let intent = f.inject_partial();
    let result = docker_capture(
        &f.engine,
        [
            "create",
            bosn_engine::LOW_CPU_PRIORITY_ARG,
            "--label",
            "com.zackees.bosn.test=volume-seed-327",
            "--mount",
            &format!("type=volume,source={},target=/state", intent.volume),
            PINNED_ALPINE,
            "sh",
            "-c",
            "sleep 3600",
        ],
    );
    assert!(result.ok());
    let id = String::from_utf8(result.stdout).unwrap().trim().to_owned();
    f.extra_containers.push(id.clone());
    assert!(docker_capture(&f.engine, ["start", &id]).ok());
    f.restart();
    let result = f.attempt("30000");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("SeedDestinationUncertain"));
    let contents = docker_capture(&f.engine, ["exec", &id, "cat", "/state/marker"]);
    assert_eq!(contents.stdout, b"partial");
    assert_eq!(
        Registry::open_read_only(f.state.join("registry.sqlite3"))
            .unwrap()
            .volume_seeds()
            .unwrap()[0]
            .state,
        bosn_registry::volume_seed::SeedState::Copying
    );
    assert!(docker_capture(&f.engine, ["container", "rm", "--force", &id]).ok());
    f.assert_recovered_cold(&intent);
}
#[test]
#[ignore = "requires Docker and the pinned Alpine and Docker helper images"]
fn live_docker_budget_below_ensure_floor_creates_no_helper() {
    let f = Fixture::new();
    f.run(&f.clone, "write");
    f.remove_containers(&f.clone);
    let result = f.attempt("30000");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let registry = Registry::open_read_only(f.state.join("registry.sqlite3")).unwrap();
    assert!(registry.volume_seeds().unwrap().is_empty());
    assert!(
        registry
            .events(0, 1000)
            .unwrap()
            .items
            .iter()
            .filter(|e| e.kind.starts_with("ci.cache-helper.v1:"))
            .all(|e| {
                let helper: bosn_registry::cache_helper::CacheHelperRecord =
                    serde_json::from_str(&e.detail).unwrap();
                !matches!(
                    helper.intent.role,
                    Some(bosn_registry::cache_helper::CacheHelperRole::VolumeSeedV1 { .. })
                )
            })
    );
    assert!(f.label(&f.volume(&f.worktree)).is_empty());
}

impl Fixture {
    fn wait_for_paused_seed(&self) -> (bosn_registry::volume_seed::VolumeSeed, String) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let registry = Registry::open_read_only(self.state.join("registry.sqlite3")).unwrap();
            if let Some(seed) = registry
                .volume_seeds()
                .unwrap()
                .into_iter()
                .find(|s| s.state == bosn_registry::volume_seed::SeedState::Copying)
                && let Some(id) = registry
                    .cache_helper(&seed.nonce)
                    .unwrap()
                    .and_then(|h| h.container_id)
                && docker_capture(&self.engine, ["pause", &id]).ok()
            {
                return (seed, id);
            }
            assert!(
                Instant::now() < deadline,
                "copy helper was not paused in time"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
#[test]
#[ignore = "requires Docker and pinned helper images; writes a temporary 1 GiB source file"]
fn live_docker_cancel_mid_copy_removes_helper_and_resets_before_deadline() {
    let f = Fixture::new();
    f.run(&f.clone, "write");
    let source = f.volume(&f.clone);
    f.remove_containers(&f.clone);
    f.volume_command(
        &source,
        "dd if=/dev/zero of=/state/large bs=1048576 count=1024",
    );
    let started = Instant::now();
    let mut client = ClientGuard(
        Command::new(env!("CARGO_BIN_EXE_bosn"))
            .current_dir(&f.worktree)
            .args(["run", "--task", "read", "--state-dir"])
            .arg(&f.state)
            .args(["--deadline-ms", "120000"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (seed, id) = f.wait_for_paused_seed();
    client.0.kill().unwrap();
    client.0.wait().unwrap();
    let deadline = started + Duration::from_secs(120);
    loop {
        let registry = Registry::open_read_only(f.state.join("registry.sqlite3")).unwrap();
        if registry.volume_seeds().unwrap().is_empty() {
            assert_eq!(
                registry.cache_helper(&seed.nonce).unwrap().unwrap().state,
                bosn_registry::cache_helper::CacheHelperState::Removed
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "seed cancellation consumed ensure deadline"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(docker_capture(&f.engine, ["container", "inspect", &id]).reports_missing());
    assert!(f.label(&seed.destination).is_empty());
    f.volume_command(&seed.destination, "test -z \"$(ls -A /state)\"");
}

struct ClientGuard(std::process::Child);
impl Drop for ClientGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
#[ignore = "requires Docker and the pinned helper images"]
fn live_docker_crash_after_reset_removal_before_record_delete_replays() {
    let mut f = Fixture::new();
    let intent = f.inject_partial();
    let mut registry = f.writer();
    let mut tx = registry.begin_immediate().unwrap();
    tx.finish_absent_cache_helper(
        &intent.nonce,
        intent.created_at + bosn_registry::cache_helper::LATE_CREATE_WINDOW_SECS,
    )
    .unwrap();
    tx.commit().unwrap();
    drop(registry);
    assert!(docker_capture(&f.engine, ["volume", "rm", &intent.volume]).ok());
    // Crash here: Copying remains, but absence is a replayable reset result.
    f.restart();
    f.assert_recovered_cold(&intent);
}
#[test]
#[ignore = "requires Docker and the pinned helper images"]
fn live_docker_other_registry_cannot_reset_completed_seed_data() {
    let f = Fixture::new();
    f.run(&f.clone, "write");
    f.remove_containers(&f.clone);
    assert!(f.run(&f.worktree, "read").contains("warm"));
    f.run(&f.worktree, "change");
    f.remove_containers(&f.worktree);
    let other_state = f._root.path().join("other-state");
    own_registry(&other_state);
    let mut other = DaemonChild::start(&other_state);
    let runtime = RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap();
    wait_for_client(&runtime, &mut other, &other_state);
    let result = Command::new(env!("CARGO_BIN_EXE_bosn"))
        .current_dir(&f.worktree)
        .args(["run", "--stack", "cook", "--state-dir"])
        .arg(&other_state)
        .args(["--deadline-ms", "30000"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("SeedDestinationUncertain"));
    assert!(f.run(&f.worktree, "read").contains("changed"));
    drop(other);
}
#[test]
#[ignore = "requires BOSN_V6_TEST_BINARY pointing to a saved schema-6 daemon"]
fn live_docker_schema6_writer_refuses_schema7_seed_registry() {
    let mut f = Fixture::new();
    drop(f.daemon.take());
    let binary =
        std::env::var_os("BOSN_V6_TEST_BINARY").expect("provide a real schema-6 daemon binary");
    let result = Command::new(binary)
        .args(["daemon", "serve", "--state-dir"])
        .arg(&f.state)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains('7') && (error.contains("schema") || error.contains("UnsupportedSchema")),
        "{error}"
    );
    assert_eq!(
        Registry::open_read_only(f.state.join("registry.sqlite3"))
            .unwrap()
            .meta("schema_version")
            .unwrap(),
        Some("7".into())
    );
}
