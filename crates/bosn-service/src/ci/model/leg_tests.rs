//! #404: matrix legs share act's `jobID`, arrive interleaved, and act numbers
//! them (`<name>-<n>`) in an order that is no identity. A leg's identity,
//! stage, place and conclusion must come from its own records alone.

use super::*;

#[test]
fn qualified_jobs_with_identical_display_names_remain_separate() {
    let lines = [
        r#"{"job":"same display","jobID":"test","jobIdentity":[{"jobID":"maintenance","matrix":{"os":"linux"}},{"jobID":"test","matrix":null}],"jobResult":"success"}"#,
        r#"{"job":"same display","jobID":"test","jobIdentity":[{"jobID":"validation","matrix":{"os":"linux"}},{"jobID":"test","matrix":null}],"jobResult":"success"}"#,
        r#"{"job":"same display","jobID":"test","jobIdentity":[{"jobID":"maintenance","matrix":{"os":"darwin"}},{"jobID":"test","matrix":null}],"jobResult":"success"}"#,
    ];
    let tree = fold("", &lines);
    assert_eq!(tree.jobs().count(), 3);
    assert!(
        tree.jobs()
            .all(|job| job.conclusion == Some(ItemConclusion::Success))
    );
}

#[test]
fn qualified_remote_jobs_and_unresolved_execution_paths_are_reported_correctly() {
    use crate::ci::{
        lifecycle::{CleanupEnd, EngineReport, ExecutionEnd},
        report,
        workflow::Declared,
    };
    let declared = Declared {
        steps: [
            ("maintenance/test".into(), Vec::new()),
            ("validation/test".into(), Vec::new()),
        ]
        .into(),
        remote_only: [("maintenance/test".into(), "GitHub maintenance".into())].into(),
        needs_qualified_identity: true,
        ..Declared::default()
    };
    let outcome = Ok(EngineReport {
        execution: ExecutionEnd::Exited(0),
        cleanup: CleanupEnd::Removed,
        engine_id: None,
        storage: None,
    });
    let remote = r#"{"job":"same display","jobID":"test","jobIdentity":[{"jobID":"maintenance","matrix":null},{"jobID":"test","matrix":null}],"jobResult":"success"}"#;
    let local = r#"{"job":"same display","jobID":"test","jobIdentity":[{"jobID":"validation","matrix":null},{"jobID":"test","matrix":null}],"jobResult":"success"}"#;
    let mut tree = fold("", &[remote, local]);
    assert_eq!(
        report::conclude(&outcome, &mut tree, &declared).0,
        crate::ci::Conclusion::Success
    );
    assert_eq!(
        tree.jobs()
            .filter(|job| job.conclusion == Some(ItemConclusion::RemoteOnly))
            .count(),
        1
    );
    assert_eq!(
        tree.jobs()
            .filter(|job| job.conclusion == Some(ItemConclusion::Success))
            .count(),
        1
    );
    for unresolved in [
        r#"{"job":"missing-parent-matrix","jobID":"test","jobIdentity":[{"jobID":"validation"},{"jobID":"test","matrix":null}],"jobResult":"success"}"#,
        r#"{"job":"legacy","jobID":"test","jobResult":"success"}"#,
        r#"{"job":"unknown","jobID":"test","jobIdentity":[{"jobID":"unknown","matrix":null},{"jobID":"test","matrix":null}],"jobResult":"success"}"#,
        r#"{"job":"wrong-leaf","jobID":"test","jobIdentity":[{"jobID":"validation","matrix":null},{"jobID":"other","matrix":null}],"jobResult":"success"}"#,
        r#"{"job":"wrong-matrix","jobID":"test","matrix":{"os":"linux"},"jobIdentity":[{"jobID":"validation","matrix":null},{"jobID":"test","matrix":{"os":"darwin"}}],"jobResult":"success"}"#,
    ] {
        let mut tree = fold("", &[local, unresolved]);
        assert_eq!(
            report::conclude(&outcome, &mut tree, &declared).0,
            crate::ci::Conclusion::Incomplete
        );
    }
}

#[test]
fn direct_job_without_qualified_identity_is_covered_beside_reusable_jobs() {
    use crate::ci::workflow::Declared;

    let declared = Declared {
        steps: [
            ("rust".into(), Vec::new()),
            ("reusable/test".into(), Vec::new()),
        ]
        .into(),
        needs_qualified_identity: true,
        ..Declared::default()
    };
    let direct = fold(
        "",
        &[r#"{"job":"rust","jobID":"rust","jobResult":"success"}"#],
    );
    assert!(!direct.qualified_coverage_missing(&declared));

    let nested = fold(
        "",
        &[r#"{"job":"test","jobID":"test","jobResult":"success"}"#],
    );
    assert!(nested.qualified_coverage_missing(&declared));
}

#[test]
fn skipped_qualified_remote_job_stays_skipped() {
    let mut tree = fold(
        "",
        &[
            r#"{"job":"remote","jobID":"test","jobIdentity":[{"jobID":"maintenance","matrix":null},{"jobID":"test","matrix":null}],"jobResult":"skipped"}"#,
        ],
    );
    tree.mark_remote_only(&[("maintenance/test".into(), "GitHub maintenance".into())].into());
    assert_eq!(
        tree.jobs().next().unwrap().conclusion,
        Some(ItemConclusion::Skipped)
    );
}

/// Recorded with act 0.2.88 from bosn's copy of the workflow beside it (the
/// `wheel` job gated by [`crate::ci::matrix_runner`]).
const RECORDED: &str =
    include_str!("../../../tests/fixtures/act/act-0.2.88-matrix-legs-runner.jsonl");
const LISTING: &str = "Stage  Job ID  Job name                  Workflow name  Workflow file  Events\n\
                       0      first   first                     legs           legs.yml       push  \n\
                       1      lint    lint                      legs           legs.yml       push  \n\
                       1      wheel   Wheel (${{ matrix.os }})  legs           legs.yml       push  \n";

fn fold(listing: &str, lines: &[&str]) -> RunTree {
    let mut parser = ActParser::new(RunTree::declared(&parse_act_list(listing)));
    for (i, line) in lines.iter().enumerate() {
        parser.feed(i as u64 + 1, line);
    }
    parser.tree.settle_finished();
    parser.tree
}

/// The tree without its seq ranges, which follow arrival order by design.
fn shape(tree: &RunTree) -> Value {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.remove("first_seq");
                map.remove("last_seq");
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(tree).unwrap();
    strip(&mut value);
    value
}

/// The recorded lines regrouped leg by leg (each leg's own order kept), in
/// the given order of first appearance.
fn leg_by_leg(lines: &[&'static str], reverse: bool) -> Vec<&'static str> {
    let mut legs: Vec<(String, Vec<&str>)> = Vec::new();
    for line in lines {
        let job: Value = serde_json::from_str(line).unwrap();
        let job = job["job"].as_str().unwrap().trim().to_string();
        match legs.iter_mut().find(|(key, _)| *key == job) {
            Some((_, own)) => own.push(line),
            None => legs.push((job, vec![line])),
        }
    }
    if reverse {
        legs.reverse();
    }
    legs.into_iter().flat_map(|(_, own)| own).collect()
}

#[test]
fn recorded_legs_fold_to_one_tree_in_any_arrival_order() {
    let recorded: Vec<&str> = RECORDED.lines().collect();
    let tree = fold(LISTING, &recorded);
    let mut shuffled: Vec<&str> = LISTING.lines().collect();
    shuffled[2..].reverse();
    assert_eq!(
        shape(&fold(&shuffled.join("\n"), &recorded)),
        shape(&tree),
        "act -l lists a stage's jobs in map order; the tree does not follow it"
    );
    for reverse in [false, true] {
        assert_eq!(
            shape(&fold(LISTING, &leg_by_leg(&recorded, reverse))),
            shape(&tree),
            "legs one after another (reversed: {reverse})"
        );
    }
    let groups: Vec<(&str, Vec<&str>)> = tree
        .groups
        .iter()
        .map(|g| {
            (
                g.name.as_str(),
                g.jobs.iter().map(|j| j.key.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        groups,
        [
            ("0", vec!["legs/first"]),
            (
                "1",
                vec![
                    "legs/lint (pyright)",
                    "legs/lint (ruff)",
                    "legs/Wheel (ubuntu-latest) (3.11)",
                    "legs/Wheel (windows-latest) (3.11)",
                ]
            ),
        ],
        "every leg in its job's stage, named by its matrix values, in a fixed order"
    );
    let conclusion = |key: &str| tree.jobs().find(|j| j.key == key).unwrap().conclusion;
    assert_eq!(
        conclusion("legs/Wheel (windows-latest) (3.11)"),
        Some(ItemConclusion::Unsupported),
        "the leg whose own runner act cannot provide"
    );
    assert_eq!(
        conclusion("legs/Wheel (ubuntu-latest) (3.11)"),
        Some(ItemConclusion::Success)
    );
    assert_eq!(
        tree.unsupported_jobs(),
        ["legs/Wheel (windows-latest) (3.11)"]
    );
}

/// #461: declarations gathered from the local reusable-workflow graph reach
/// the report with their qualified identities, two levels deep. The nested
/// maintenance stub is `remote_only` with its reason — not silent success.
#[test]
fn doubly_nested_remote_only_jobs_are_reported_with_their_reasons() {
    use crate::ci::{
        lifecycle::{CleanupEnd, EngineReport, ExecutionEnd},
        report,
        workflow,
    };
    let root = tempfile::tempdir().unwrap();
    let workflows = root.path().join(".github/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(
        workflows.join("entry.yml"),
        "jobs:\n  maintenance:\n    uses: ./.github/workflows/mid.yml\n  lint:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo lint\n",
    )
    .unwrap();
    std::fs::write(
        workflows.join("mid.yml"),
        "jobs:\n  inner:\n    uses: ./.github/workflows/leaf.yml\n",
    )
    .unwrap();
    std::fs::write(
        workflows.join("leaf.yml"),
        "jobs:\n  budget:\n    runs-on: ubuntu-latest\n    env:\n      CI_REMOTE_ONLY: reads the cache budget from the GitHub API\n    steps:\n      - run: echo remote\n  check:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo local\n",
    )
    .unwrap();
    let declared = workflow::declared(root.path(), ".github/workflows/entry.yml", "zackees/bosn");
    assert!(declared.errors.is_empty(), "{:?}", declared.errors);
    assert_eq!(
        declared
            .remote_only
            .get("maintenance/inner/budget")
            .map(String::as_str),
        Some("reads the cache budget from the GitHub API"),
        "the doubly nested stub is declared under its qualified identity"
    );
    assert!(
        !declared.remote_only.contains_key("budget"),
        "an identically named job elsewhere must not inherit the reason"
    );
    // The run itself: lint passes, the nested maintenance stub "runs" its
    // localized success, and the nested local check passes.
    let lines = [
        r#"{"job":"w/lint","jobID":"lint","msg":"🏁","jobResult":"success"}"#,
        r#"{"job":"precheck/inner/budget","jobID":"budget","jobIdentity":[{"jobID":"maintenance","matrix":null},{"jobID":"inner","matrix":null},{"jobID":"budget","matrix":null}],"msg":"🏁","jobResult":"success"}"#,
        r#"{"job":"precheck/inner/check","jobID":"check","jobIdentity":[{"jobID":"maintenance","matrix":null},{"jobID":"inner","matrix":null},{"jobID":"check","matrix":null}],"msg":"🏁","jobResult":"success"}"#,
    ];
    let mut tree = fold("", &lines);
    let outcome = Ok(EngineReport {
        execution: ExecutionEnd::Exited(0),
        cleanup: CleanupEnd::Removed,
        engine_id: None,
        storage: None,
    });
    let (conclusion, _) = report::conclude(&outcome, &mut tree, &declared);
    assert_eq!(conclusion, crate::ci::Conclusion::Success);
    let mut record = crate::ci::tests::sample_record("run-461");
    record.tree = tree;
    let report = report::report(&record, None, |_, _| Vec::new());
    assert_eq!(report.jobs.remote_only.len(), 1, "{:?}", report.jobs.remote_only);
    let stub = &report.jobs.remote_only[0];
    assert_eq!(
        stub.reason, "reads the cache budget from the GitHub API",
        "qualified stub reported: {}",
        stub.job
    );
    assert_eq!(report.jobs.succeeded, 2, "lint plus the nested local check");
    assert!(
        report.coverage_complete,
        "a declared remote-only stub is not a coverage gap"
    );
}

/// #525 / #477: a fully green replay of a workflow that mixes direct jobs
/// with local reusable calls concludes Success with exit_code 0 — verified at
/// the receipt (`RunView`), where the gate reads it, not only at the
/// coverage predicate (#478 fixed the predicate; this pins the receipt).
#[test]
fn a_fully_green_direct_and_reusable_run_receives_a_zero_exit_receipt() {
    use crate::ci::{
        lifecycle::{CleanupEnd, EngineReport, ExecutionEnd},
        report,
        reply::RunView,
        workflow,
    };
    let root = tempfile::tempdir().unwrap();
    let workflows = root.path().join(".github/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(
        workflows.join("entry.yml"),
        "jobs:\n  rust:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n  perf:\n    runs-on: ubuntu-latest\n    steps:\n      - run: check-perf\n  build:\n    uses: ./.github/workflows/build.yml\n",
    )
    .unwrap();
    std::fs::write(
        workflows.join("build.yml"),
        "jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo test\n",
    )
    .unwrap();
    let declared = workflow::declared(root.path(), ".github/workflows/entry.yml", "zackees/bosn");
    assert!(declared.errors.is_empty(), "{:?}", declared.errors);
    assert!(declared.needs_qualified_identity);
    let lines = [
        r#"{"job":"w/rust","jobID":"rust","msg":"🏁","jobResult":"success"}"#,
        r#"{"job":"w/perf","jobID":"perf","msg":"🏁","jobResult":"success"}"#,
        r#"{"job":"build/test","jobID":"test","jobIdentity":[{"jobID":"build","matrix":null},{"jobID":"test","matrix":null}],"msg":"🏁","jobResult":"success"}"#,
    ];
    let mut tree = fold("", &lines);
    let outcome = Ok(EngineReport {
        execution: ExecutionEnd::Exited(0),
        cleanup: CleanupEnd::Removed,
        engine_id: None,
        storage: None,
    });
    let (conclusion, reason) = report::conclude(&outcome, &mut tree, &declared);
    assert_eq!(
        (conclusion, reason.as_deref()),
        (crate::ci::Conclusion::Success, None),
        "4/4 green must not conclude Incomplete"
    );
    let mut record = crate::ci::tests::sample_record("run-525");
    record.tree = tree;
    record.finish(conclusion, reason);
    let view = RunView::of(record, false);
    assert_eq!(view.exit_code, Some(0), "the gate reads exit_code");
    assert_eq!(view.jobs.total, 3);
    assert_eq!(view.jobs.completed, 3);
    assert_eq!(view.jobs.failed, 0);
    // The receipt itself must distinguish "all green" from "did not run".
    let receipt = serde_json::to_value(&view).unwrap();
    assert_eq!(receipt["conclusion"], "success");
    assert!(
        receipt.get("reason").is_some(),
        "the receipt carries conclusion and reason, not only counts"
    );
}

/// xorshift64*: deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone, Copy, Debug)]
enum Outcome {
    Success,
    Failure,
    /// act's own `Skipping unsupported platform` (a job act did not gate).
    ActUnsupported,
    /// bosn's gate printed the leg's runner ([`crate::ci::matrix_runner`]).
    Gated,
}

impl Outcome {
    fn conclusion(self) -> ItemConclusion {
        match self {
            Self::Success => ItemConclusion::Success,
            Self::Failure => ItemConclusion::Failure,
            Self::ActUnsupported | Self::Gated => ItemConclusion::Unsupported,
        }
    }
}

/// One leg of job `m`: its matrix value, whether act's name shows it, its
/// outcome.
#[derive(Clone, Copy, Debug)]
struct Leg {
    value: u64,
    named: bool,
    outcome: Outcome,
}

/// The leg's lines in act's order, under act's number `n` for it.
fn leg_lines(leg: Leg, n: usize) -> Vec<String> {
    let name = if leg.named {
        format!("M (os{})", leg.value)
    } else {
        "m".to_string()
    };
    let head = format!(
        r#""job":"w/{name}-{n}","jobID":"m","matrix":{{"os":"os{}","py":"3"}}"#,
        leg.value
    );
    let step = r#""stage":"Main","step":"s","stepID":["0"]"#;
    let mut lines = vec![format!(
        r#"{{{head},"msg":"⭐ Run Set up job","step":"Set up job","stepid":["--setup-job"]}}"#
    )];
    match leg.outcome {
        Outcome::ActUnsupported => {
            lines = vec![format!(
                r#"{{{head},"msg":"🚧  Skipping unsupported platform -- Try running with `-P os{}=...`"}}"#,
                leg.value
            )];
            return lines;
        }
        Outcome::Gated => {
            let mark = crate::ci::matrix_runner::MARK;
            lines.push(format!(r#"{{{head},{step},"msg":"⭐ Run Main gate"}}"#));
            lines.push(format!(
                r#"{{{head},{step},"msg":{},"raw_output":true}}"#,
                serde_json::to_string(&format!("{mark} os{}\n", leg.value)).unwrap()
            ));
            lines.push(format!(
                r#"{{{head},{step},"msg":"ok","stepResult":"success"}}"#
            ));
        }
        Outcome::Success | Outcome::Failure => {
            let result = if matches!(leg.outcome, Outcome::Success) {
                "success"
            } else {
                "failure"
            };
            lines.push(format!(r#"{{{head},{step},"msg":"⭐ Run Main s"}}"#));
            lines.push(format!(
                r#"{{{head},{step},"msg":"out\n","raw_output":true}}"#
            ));
            lines.push(format!(
                r#"{{{head},{step},"msg":"done","stepResult":"{result}"}}"#
            ));
        }
    }
    let result = if matches!(leg.outcome, Outcome::Failure) {
        "failure"
    } else {
        "success"
    };
    lines.push(format!(r#"{{{head},"msg":"🏁","jobResult":"{result}"}}"#));
    lines
}

const PROPERTY_LISTING: &str = "Stage  Job ID  Job name  Workflow name  Workflow file  Events\n\
                                0      first   first     w              ci.yml         push\n\
                                1      m       m         w              ci.yml         push\n";
const FIRST: [&str; 2] = [
    r#"{"job":"w/first","jobID":"first","matrix":{},"msg":"⭐ Run Set up job","step":"Set up job","stepid":["--setup-job"]}"#,
    r#"{"job":"w/first","jobID":"first","matrix":{},"msg":"🏁","jobResult":"success"}"#,
];

/// One run: act numbers the legs in a random order (as Go's map iteration
/// does) and their lines interleave at random after `first` finished.
fn run(rng: &mut Rng, legs: &[Leg]) -> RunTree {
    let mut numbers: Vec<usize> = (1..=legs.len()).collect();
    for i in (1..numbers.len()).rev() {
        numbers.swap(i, rng.below(i as u64 + 1) as usize);
    }
    let mut pending: Vec<Vec<String>> = legs
        .iter()
        .zip(&numbers)
        .map(|(leg, &n)| {
            let mut lines = leg_lines(*leg, n);
            lines.reverse();
            lines
        })
        .collect();
    let mut lines: Vec<String> = FIRST.iter().map(|l| l.to_string()).collect();
    loop {
        let live: Vec<usize> = (0..pending.len())
            .filter(|&i| !pending[i].is_empty())
            .collect();
        if live.is_empty() {
            break;
        }
        let pick = live[rng.below(live.len() as u64) as usize];
        lines.extend(pending[pick].pop());
    }
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    fold(PROPERTY_LISTING, &lines)
}

/// Every leg's conclusion and stage depend only on its own records: the
/// same legs give the same tree whatever act numbers them and however their
/// records interleave, and a leg folded with its siblings concludes as it
/// does alone.
#[test]
fn a_legs_conclusion_and_stage_depend_only_on_its_own_records() {
    for seed in 1..=300u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let named = rng.below(2) == 0;
        let legs: Vec<Leg> = (0..2 + rng.below(3))
            .map(|value| Leg {
                value,
                named,
                outcome: match rng.below(4) {
                    0 => Outcome::Success,
                    1 => Outcome::Failure,
                    2 => Outcome::ActUnsupported,
                    _ => Outcome::Gated,
                },
            })
            .collect();
        let tree = run(&mut rng, &legs);
        assert_eq!(
            shape(&run(&mut rng, &legs)),
            shape(&tree),
            "seed {seed}: another numbering and interleaving, another tree"
        );
        let stage1: Vec<&Job> = tree
            .groups
            .iter()
            .filter(|g| g.name == "1")
            .flat_map(|g| &g.jobs)
            .collect();
        assert_eq!(
            stage1.len(),
            legs.len(),
            "seed {seed}: every leg in stage 1"
        );
        for leg in &legs {
            let alone = run(&mut rng, std::slice::from_ref(leg));
            let own = |tree: &RunTree| {
                tree.groups
                    .iter()
                    .flat_map(|g| g.jobs.iter().map(move |j| (g.name.clone(), j)))
                    .find(|(_, j)| {
                        j.matrix.as_ref().and_then(|m| m["os"].as_str())
                            == Some(&format!("os{}", leg.value))
                    })
                    .map(|(group, j)| (group, j.key.clone(), j.conclusion))
                    .unwrap_or_else(|| panic!("seed {seed}: {leg:?} is missing"))
            };
            let (group, key, conclusion) = own(&tree);
            assert_eq!(
                conclusion,
                Some(leg.outcome.conclusion()),
                "seed {seed}: {key}"
            );
            assert_eq!(group, "1", "seed {seed}: {key}");
            let (_, alone_key, alone_conclusion) = own(&alone);
            assert_eq!(
                (alone_key, alone_conclusion),
                (key, conclusion),
                "seed {seed}"
            );
        }
    }
}
