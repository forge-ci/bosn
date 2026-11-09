//! `bosn ci` command-line contract without Docker: golden `plan` output in
//! text and JSON (checked against the published schema), the exit-code
//! contract for refusals (3), which never start a daemon, and the run verbs
//! against a real, drained daemon (nothing executes, so no Docker).

use std::{path::Path, process::Command};

use serde_json::Value;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(status.success());
}

fn repo(root: &Path) -> std::path::PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".github/workflows")).unwrap();
    std::fs::write(
        repo.join(".github/workflows/ci.yml"),
        "on: [push]\njobs: {}\n",
    )
    .unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/demo.git",
        ],
    );
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    repo
}

fn bosn(args: &[&str], cwd: &Path, state: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bosn"))
        .args(args)
        .arg("--state-dir")
        .arg(state)
        .current_dir(cwd)
        .env("BOSN_CI_ACTOR", "human")
        // No desktop: the CLI never launches the widget from a test.
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap()
}

fn schema() -> Value {
    serde_json::from_str(include_str!("../../../docs/ci.schema.json")).unwrap()
}

/// `value`'s top-level keys are the published definition's: none extra, none
/// required missing.
fn conforms(value: &Value, definition: &str) {
    let schema = schema();
    let def = &schema["$defs"][definition];
    let properties = def["properties"].as_object().expect(definition);
    for key in value.as_object().unwrap().keys() {
        assert!(properties.contains_key(key), "{key} is not in {definition}");
    }
    for required in def["required"].as_array().unwrap() {
        let required = required.as_str().unwrap();
        assert!(
            value.get(required).is_some(),
            "{definition}: {required} missing"
        );
    }
}

#[test]
fn plan_json_conforms_to_the_published_schema_and_text_is_golden() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo(root.path());
    let state = root.path().join("state");
    let out = bosn(
        &["ci", "plan", "--trigger", "pr", "--mode", "test", "--json"],
        &repo,
        &state,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan: Value = serde_json::from_slice(&out.stdout).unwrap();
    conforms(&plan, "Plan");
    assert_eq!(plan["event"], "pull_request");
    assert_eq!(plan["repository"], "example/demo");
    assert_eq!(
        plan["payload"]["pull_request"]["labels"][0]["name"],
        "ci-test"
    );
    assert!(!state.exists(), "plan never starts a daemon");

    let text = bosn(
        &["ci", "plan", "--trigger", "pr", "--mode", "test"],
        &repo,
        &state,
    );
    let sha = plan["sha"].as_str().unwrap();
    let golden = format!(
        "provider: github\nworkflow: .github/workflows/ci.yml\ntrigger: pr (pull_request)\nmode: test\nsha: {sha}\nactor: human\n"
    );
    assert_eq!(String::from_utf8_lossy(&text.stdout), golden);
}

#[test]
fn plan_takes_an_event_inputs_a_matrix_filter_and_env() {
    // zackees/clud's installer lane (#430).
    let root = tempfile::tempdir().unwrap();
    let repo = repo(root.path());
    let state = root.path().join("state");
    let lane: &[&str] = &[
        "ci",
        "plan",
        "--event",
        "workflow_call",
        "--input",
        "release_tag=2.8.25",
        "--input",
        "mode=candidate",
        "--matrix",
        "target:x86_64-unknown-linux-musl",
        "--env",
        "PYTEST_ADDOPTS=-s",
        "--json",
    ];
    let out = bosn(lane, &repo, &state);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan: Value = serde_json::from_slice(&out.stdout).unwrap();
    conforms(&plan, "Plan");
    assert_eq!(plan["trigger"], "workflow_call");
    assert_eq!(plan["event"], "workflow_call");
    assert_eq!(plan["payload"]["inputs"]["release_tag"], "2.8.25");
    assert_eq!(plan["payload"]["inputs"]["mode"], "candidate");
    assert_eq!(
        plan["params"]["matrix"]["target"],
        "x86_64-unknown-linux-musl"
    );
    assert_eq!(plan["params"]["env"]["PYTEST_ADDOPTS"], "-s");
    let text = bosn(&lane[..lane.len() - 1], &repo, &state);
    assert!(
        String::from_utf8_lossy(&text.stdout).contains(
            "params: inputs: mode=candidate, release_tag=2.8.25; \
             matrix: target:x86_64-unknown-linux-musl; env: PYTEST_ADDOPTS=-s\n"
        ),
        "{}",
        String::from_utf8_lossy(&text.stdout)
    );
    // Refused before any daemon starts: a secret through --env, inputs for
    // an event that takes none, and --event beside --trigger.
    let cases: [(&[&str], &str); 4] = [
        (&["ci", "run", "--env", "GITHUB_TOKEN=x"], "secrets"),
        (&["ci", "plan", "--input", "a=1"], "--event"),
        (
            &["ci", "plan", "--event", "workflow_call", "--trigger", "pr"],
            "--event or --trigger",
        ),
        (&["ci", "plan", "--event", "push"], "workflow_dispatch"),
    ];
    for (args, expected) in cases {
        let out = bosn(args, &repo, &state);
        assert_eq!(out.status.code(), Some(3), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
    assert!(!state.exists(), "plan never starts a daemon");
}

#[test]
fn refusals_exit_3_without_starting_a_daemon() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo(root.path());
    let state = root.path().join("state");
    std::fs::write(repo.join("dirty.txt"), "uncommitted").unwrap();
    let cases: [(&[&str], &str); 5] = [
        (
            &["ci", "run", "--trigger", "release", "--mode", "full"],
            "clean tree",
        ),
        (&["ci", "list", "--state", "bogus"], "unknown state"),
        (&["ci", "logs"], "exactly one RUN"),
        (&["ci", "nope"], "usage"),
        (&["ci", "plan", "--mode", "huge"], "unknown mode"),
    ];
    for (args, expected) in cases {
        let out = bosn(args, &repo, &state);
        assert_eq!(out.status.code(), Some(3), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
    std::fs::write(repo.join(".gitlab-ci.yml"), "stages: [test]\n").unwrap();
    let both = bosn(&["ci", "plan"], &repo, &state);
    assert_eq!(both.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&both.stderr).contains("pass --provider"));
    assert!(!state.exists(), "refusals never start a daemon");
}

/// Stops the daemon a test started, even when an assertion fails.
struct Daemon<'a> {
    cwd: &'a Path,
    state: &'a Path,
}
impl Drop for Daemon<'_> {
    fn drop(&mut self) {
        let _ = bosn(&["daemon", "stop"], self.cwd, self.state);
    }
}

fn json(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn run_verbs_keep_their_exit_codes_and_output_against_a_drained_daemon() {
    // A Unix socket path must stay short; long sandbox temp dirs are not.
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() > 40 {
        std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".cache")
    } else {
        base
    };
    let root = tempfile::Builder::new()
        .prefix("bcli")
        .tempdir_in(base)
        .unwrap();
    let repo = repo(root.path());
    let state = root.path().join("s");
    let _daemon = Daemon {
        cwd: &repo,
        state: &state,
    };

    let drained = bosn(&["ci", "runners", "drain", "--json"], &repo, &state);
    assert_eq!(drained.status.code(), Some(0));
    let drained = json(&drained);
    conforms(&drained, "RunnersReply");
    assert_eq!(drained["runners"]["drained"], true);

    let submitted = bosn(&["ci", "run", "--detach", "--json"], &repo, &state);
    assert_eq!(submitted.status.code(), Some(0), "queued is not a failure");
    let submitted = json(&submitted);
    conforms(&submitted, "SubmitReply");
    let run = submitted["run"].as_str().unwrap().to_string();

    let waited = bosn(
        &["ci", "wait", &run, "--deadline-ms", "300", "--json"],
        &repo,
        &state,
    );
    assert_eq!(
        waited.status.code(),
        Some(2),
        "not finished by the deadline"
    );
    let waited = json(&waited);
    conforms(&waited, "RunView");
    assert_eq!(waited["state"], "queued");

    let listed = bosn(&["ci", "list", "--json"], &repo, &state);
    assert_eq!(listed.status.code(), Some(0));
    let listed = json(&listed);
    conforms(&listed, "ListReply");
    assert_eq!(listed["runs"][0]["id"], run.as_str());
    let text = bosn(&["ci", "runners"], &repo, &state);
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        format!(
            "runners: 0 running, 1 queued, limit {} (drained)\n",
            drained["runners"]["limit"]
        )
    );

    let cancelled = bosn(&["ci", "cancel", &run], &repo, &state);
    assert_eq!(cancelled.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&cancelled.stdout),
        format!("run {run} cancelled (done)\n")
    );
    let again = bosn(&["ci", "cancel", &run, "--json"], &repo, &state);
    assert_eq!(again.status.code(), Some(1), "nothing left to cancel");
    let again = json(&again);
    conforms(&again, "CancelReply");
    assert_eq!(again["cancelled"], false);

    let done = bosn(
        &["ci", "wait", &run, "--deadline-ms", "5000"],
        &repo,
        &state,
    );
    assert_eq!(done.status.code(), Some(2), "a cancelled run exits 2");
    let shown = bosn(&["ci", "show", &run], &repo, &state);
    assert!(
        String::from_utf8_lossy(&shown.stdout)
            .starts_with(&format!("run {run} done cancelled  sha ")),
        "{}",
        String::from_utf8_lossy(&shown.stdout)
    );

    // Without --detach the command owns its run: when it stops waiting (here
    // at its deadline) it releases the run, which is cancelled.
    let owned = bosn(
        &["ci", "run", "--deadline-ms", "300", "--json"],
        &repo,
        &state,
    );
    assert_eq!(owned.status.code(), Some(2), "not finished by the deadline");
    let owned = json(&owned);
    conforms(&owned, "RunView");
    assert_eq!(owned["state"], "done", "released at the deadline: {owned}");
    let refused = bosn(&["ci", "run", "--detach", "--wait"], &repo, &state);
    assert_eq!(refused.status.code(), Some(3), "--detach does not wait");
}

#[test]
fn a_command_right_after_daemon_stop_reaches_a_fresh_daemon() {
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() > 40 {
        std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".cache")
    } else {
        base
    };
    let root = tempfile::Builder::new()
        .prefix("bstop")
        .tempdir_in(base)
        .unwrap();
    let repo = repo(root.path());
    let state = root.path().join("s");
    let _daemon = Daemon {
        cwd: &repo,
        state: &state,
    };
    assert!(bosn(&["ci", "runners"], &repo, &state).status.success());
    for round in 0..5 {
        let stopped = bosn(&["daemon", "stop"], &repo, &state);
        assert!(stopped.status.success(), "round {round}: stop failed");
        let next = bosn(&["ci", "runners"], &repo, &state);
        assert!(
            next.status.success(),
            "round {round}: {}",
            String::from_utf8_lossy(&next.stderr)
        );
    }
}

/// A checkout holding a fleet adapter V1 declaration and the workflow it
/// names, committed clean; returns its HEAD.
fn adapter_repo(root: &Path) -> String {
    let digest = format!("sha256:{}", "a".repeat(64));
    let adapter = serde_json::json!({
        "schema_version":1,"repository":{"owner":"example","name":"demo"},"default_branch":"main",
        "pins":{"interface_schema":1,"act_version":bosn_core::act::ACT_VERSION,"act_binary_digest":digest,"engine_manifest_digest":digest,"engine_config_digest":digest,"runner_manifest_digest":digest,"runner_config_digest":digest},
        "workflows":{"pull_request":[".github/workflows/ci.yml"],"push":[".github/workflows/ci.yml"],"release":[".github/workflows/ci.yml"]},
        "cells":[{"id":"lint","workflow":".github/workflows/ci.yml","job":"lint","runner":"ubuntu-latest","proof_scope":"local_linux"},{"id":"unit","workflow":".github/workflows/ci.yml","job":"unit","runner":"ubuntu-22.04","proof_scope":"local_linux"},{"id":"win","workflow":".github/workflows/ci.yml","job":"windows","runner":"windows-2022","proof_scope":"github_only"}],
        "tiers":{"minimal":["lint"],"test":["lint","unit"],"full":["lint","unit","win"]},
        "release_inputs":{"candidate_sha":"candidate","full_mode":"coverage","full_mode_value":"full","version":null},"permitted_secrets":[]
    });
    std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
    std::fs::write(
        root.join(".github/workflows/ci.yml"),
        "on: [push]\njobs: {}\n",
    )
    .unwrap();
    std::fs::write(root.join("adapter.json"), adapter.to_string()).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "adapter"]);
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8(head.stdout).unwrap().trim().to_owned()
}

fn adapter_plan(spelling: &[&str], options: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bosn"))
        .args(spelling)
        .args(options)
        .output()
        .unwrap()
}

/// `bosn ci plan --adapter` is the fleet adapter V1 plan (soldr#3345);
/// `bosn act plan --adapter` is its deprecated spelling: stdout is the same
/// JSON byte for byte, and the notice goes to stderr only.
#[test]
fn ci_plan_adapter_matches_the_deprecated_act_spelling_byte_for_byte() {
    let root = tempfile::tempdir().unwrap();
    let sha = adapter_repo(root.path());
    let workspace = root.path().to_str().unwrap();
    for (event, mode, extra) in [
        ("push", "minimal", &[][..]),
        ("release", "full", &[][..]),
        (
            "pull_request",
            "full",
            &[
                "--pr-number",
                "12",
                "--head-owner",
                "external",
                "--head-name",
                "fork",
                "--head-ref",
                "work",
                "--base-ref",
                "main",
                "--author-login",
                "contributor",
            ][..],
        ),
    ] {
        let mut options = vec![
            "--adapter",
            "adapter.json",
            "--workspace",
            workspace,
            "--event",
            event,
            "--mode",
            mode,
            "--sha",
            &sha,
            "--repo-owner",
            "example",
            "--repo-name",
            "demo",
            "--base-sha",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ];
        options.extend_from_slice(extra);
        let ci = adapter_plan(&["ci", "plan"], &options);
        assert!(
            ci.status.success(),
            "{}",
            String::from_utf8_lossy(&ci.stderr)
        );
        assert!(
            ci.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&ci.stderr)
        );
        let act = adapter_plan(&["act", "plan"], &options);
        assert!(
            act.status.success(),
            "{}",
            String::from_utf8_lossy(&act.stderr)
        );
        assert_eq!(ci.stdout, act.stdout, "{event}/{mode}: stdout differs");
        assert!(
            String::from_utf8_lossy(&act.stderr)
                .contains("`bosn act plan --adapter` is deprecated"),
            "{}",
            String::from_utf8_lossy(&act.stderr)
        );
        let receipt: Value = serde_json::from_slice(&ci.stdout).unwrap();
        assert_eq!(receipt["action"], "act_adapter_plan");
        assert_eq!(receipt["schema_version"], 1);
        assert_eq!(receipt["executable"], false);
        assert_eq!(receipt["plan"]["declaration_only"], true);
        let cells = if mode == "full" { 3 } else { 1 };
        assert_eq!(
            receipt["plan"]["required_cells"].as_array().unwrap().len(),
            cells,
            "{event}/{mode}"
        );
    }
}

/// The same refusals under both spellings; `bosn ci` maps them to its
/// refused exit code (3), `bosn act` keeps its own (2). Nothing on stdout.
#[test]
fn ci_plan_adapter_refusals_match_act_and_exit_3() {
    let root = tempfile::tempdir().unwrap();
    let sha = adapter_repo(root.path());
    let workspace = root.path().to_str().unwrap();
    let wrong_sha = "0".repeat(40);
    for options in [
        vec!["--adapter", "adapter.json", "--workspace", workspace],
        vec!["--adapter", "a", "--adapter", "b"],
        vec!["--adapter", "adapter.json", "--act-bin", "act"],
        vec![
            "--adapter",
            "adapter.json",
            "--workspace",
            workspace,
            "--event",
            "push",
            "--mode",
            "full",
            "--sha",
            &sha,
            "--repo-owner",
            "example",
            "--repo-name",
            "demo",
        ],
        vec![
            "--adapter",
            "adapter.json",
            "--workspace",
            workspace,
            "--event",
            "push",
            "--mode",
            "minimal",
            "--sha",
            &wrong_sha,
            "--repo-owner",
            "example",
            "--repo-name",
            "demo",
        ],
    ] {
        let ci = adapter_plan(&["ci", "plan"], &options);
        let act = adapter_plan(&["act", "plan"], &options);
        assert_eq!(ci.status.code(), Some(3), "{options:?}");
        assert_eq!(act.status.code(), Some(2), "{options:?}");
        assert!(ci.stdout.is_empty() && act.stdout.is_empty());
        let reason = String::from_utf8_lossy(&ci.stderr);
        let reason = reason.trim().strip_prefix("bosn ci: ").unwrap();
        let act_stderr = String::from_utf8_lossy(&act.stderr);
        assert!(
            act_stderr.contains(&format!("bosn act: {reason}")),
            "{options:?}: {act_stderr}"
        );
    }
}

#[test]
fn pr_title_reaches_plan_and_other_events_refuse_it() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo(root.path());
    let state = root.path().join("state");
    let out = bosn(
        &[
            "ci",
            "plan",
            "--trigger",
            "pr",
            "--pr-title",
            "[ci-windows] proof",
            "--json",
        ],
        &repo,
        &state,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        plan["payload"]["pull_request"]["title"],
        "[ci-windows] proof"
    );
    assert_eq!(plan["params"]["pr_title"], "[ci-windows] proof");
    let refused = bosn(
        &["ci", "plan", "--trigger", "push", "--pr-title", "title"],
        &repo,
        &state,
    );
    assert_eq!(refused.status.code(), Some(3));
    assert!(!state.exists());
}

#[test]
fn gate012_human_summary_is_golden_and_json_is_unchanged() {
    let root = tempfile::Builder::new().prefix("bnotes").tempdir().unwrap();
    let repo = repo(root.path());
    let state = root.path().join("s");
    let _daemon = Daemon {
        cwd: &repo,
        state: &state,
    };
    assert!(
        bosn(&["ci", "runners", "drain"], &repo, &state)
            .status
            .success()
    );
    let submitted = bosn(
        &["ci", "run", "--detach", "--verbose", "--json"],
        &repo,
        &state,
    );
    let submitted: bosn_service::ci::SubmitReply =
        serde_json::from_slice(&submitted.stdout).unwrap();
    let run = &submitted.run;
    let shown = bosn(&["ci", "show", run, "--json"], &repo, &state);
    let mut view: bosn_service::ci::RunView = serde_json::from_slice(&shown.stdout).unwrap();
    assert!(bosn(&["daemon", "stop"], &repo, &state).status.success());
    seed_gate012_tree(&mut view);
    let store = bosn_service::ci::store::Store::new(&state);
    store.save_run(&view.record);
    let before = bosn(&["ci", "show", run, "--json"], &repo, &state).stdout;
    let source = root.path().join("workflow.yml");
    std::fs::write(&source, "on: [push]\njobs:\n  remote-a:\n    runs-on: ubuntu-latest\n    strategy:\n      matrix:\n        leg: [a, b]\n    env:\n      CI_REMOTE_ONLY: needs GitHub\n    steps:\n      - run: echo local\n  remote-b:\n    runs-on: ubuntu-latest\n    env:\n      CI_REMOTE_ONLY: needs GitHub\n    steps:\n      - run: echo local\n  remote-c:\n    runs-on: ubuntu-latest\n    env:\n      CI_REMOTE_ONLY: needs GitHub\n    steps:\n      - run: echo local\n  pages:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/configure-pages@v5\n      - uses: actions/configure-pages@v5\n").unwrap();
    let mut localized = bosn_service::ci::checkout::Localized::default();
    bosn_service::ci::checkout::localize(
        &source,
        &root.path().join("overlay.yml"),
        "example/demo",
        &Default::default(),
        &mut localized,
    )
    .unwrap();
    assert_eq!(localized.remote_only.len(), 3);
    assert_eq!(localized.pages_stubbed.len(), 2);
    // Persist exactly the localization boundary data, including duplicate stub steps.
    let notes = bosn_service::ci::notes::Notes::from(&localized);
    store.save_notes(run, &notes).unwrap();
    let output = bosn(&["ci", "show", run], &repo, &state);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("local failure detail"));
    let lines: Vec<_> = text
        .lines()
        .filter(|line| line.contains("GATE-012"))
        .collect();
    assert_eq!(
        lines,
        [format!(
            "GATE-012: 3 remote-only, 1 stubbed locally (bosn ci show {run} --notes)"
        )]
    );
    for flag in ["--verbose", "--notes"] {
        let output = bosn(&["ci", "show", run, flag], &repo, &state);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            text.lines()
                .filter(|line| line.contains("GATE-012"))
                .count(),
            4
        );
        for job in ["remote-a", "remote-b", "remote-c", "pages"] {
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with(&format!("job {job}")))
                    .count(),
                1
            );
        }
        assert_eq!(
            before,
            bosn(&["ci", "show", run, flag, "--json"], &repo, &state).stdout
        );
    }
    let retried = bosn(
        &["ci", "retry", run, "--detach", "--verbose", "--json"],
        &repo,
        &state,
    );
    assert!(
        retried.status.success(),
        "{}",
        String::from_utf8_lossy(&retried.stderr)
    );
}

// A finished provider tree still carries the generated remote stub section names.
fn seed_gate012_tree(view: &mut bosn_service::ci::RunView) {
    use bosn_service::ci::model::{Group, ItemConclusion, ItemStatus, Job, Section};
    let jobs = [
        ("remote-a", "a"),
        ("remote-a", "b"),
        ("remote-b", ""),
        ("remote-c", ""),
        ("remote-a", "local"),
    ]
    .into_iter()
    .map(|(id, leg)| Job {
        key: format!("{id}/{leg}"),
        job_id: id.into(),
        name: id.into(),
        matrix: None,
        identity: None,
        output_evidence: None,
        status: ItemStatus::Completed,
        conclusion: Some(if leg == "local" {
            ItemConclusion::Failure
        } else {
            ItemConclusion::RemoteOnly
        }),
        reason: (leg != "local").then(|| "needs GitHub".into()),
        started_at: None,
        completed_at: None,
        duration_ms: None,
        sections: vec![Section {
            id: "1".into(),
            name: if leg == "local" {
                "local failure detail"
            } else {
                "Remote-only: runs on GitHub, not under act (GATE-012)"
            }
            .into(),
            stage: "Main".into(),
            status: ItemStatus::Completed,
            conclusion: Some(if leg == "local" {
                ItemConclusion::Failure
            } else {
                ItemConclusion::Success
            }),
            first_seq: None,
            last_seq: None,
            duration_ms: None,
            started_at: None,
            completed_at: None,
            exit_code: None,
        }],
    })
    .collect();
    view.record.tree.groups = vec![Group {
        name: "CI".into(),
        jobs,
    }];
    view.record
        .finish(bosn_service::ci::Conclusion::Failure, None);
}
