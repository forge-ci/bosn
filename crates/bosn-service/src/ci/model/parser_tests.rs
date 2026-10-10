//! Parser folding: every act line lands in one tree, once (#398, #404, #500).
//! Extracted from `model.rs` so both stay under the 1000-line gate.

use super::*;

const FIXTURE: &str =
    include_str!("../../../tests/fixtures/act/act-0.2.88-matrix-needs-failure.jsonl");
const LIST: &str = "Stage  Job ID  Job name  Workflow name  Workflow file  Events\n\
                        0      a       a         spike          ci.yml         push  \n\
                        1      b       b         spike          ci.yml         push  \n\
                        1      off     Off job   spike          ci.yml         push  \n";

fn parse(list: &str, lines: &str) -> (ActParser, Vec<LogRecord>) {
    let mut parser = ActParser::new(RunTree::declared(&parse_act_list(list)));
    let records = lines
        .lines()
        .enumerate()
        .filter_map(|(i, line)| parser.feed(i as u64 + 1, line))
        .collect();
    (parser, records)
}

#[test]
fn a_job_filter_declares_only_that_job() {
    let jobs = parse_act_list(LIST);
    assert_eq!(select_jobs(jobs.clone(), None).len(), 3);
    let only = select_jobs(jobs, Some("b"));
    assert_eq!(
        only.iter().map(|j| j.job_id.as_str()).collect::<Vec<_>>(),
        ["b"]
    );
}

/// #398: the end-of-output trap completes a partial last line; the
/// record keeps the line without the mark, and a bare mark is no record.
#[test]
fn a_partial_last_line_is_kept_and_the_bare_mark_dropped() {
    let raw = |msg: &str| {
        serde_json::json!({"job": "w/a", "jobID": "a", "msg": msg, "raw_output": true,
                "stage": "Main", "step": "one", "stepID": ["0"]})
        .to_string()
    };
    let mark = crate::ci::flush::MARK;
    let lines = [
            r#"{"job":"w/a","jobID":"a","msg":"⭐ Run Main one","stage":"Main","step":"one","stepID":["0"]}"#.to_string(),
            raw("ok\n"),
            raw(&format!("fatal: no newline{mark}\n")),
            raw(&format!("{mark}\n")),
        ]
        .join("\n");
    let (_, records) = parse(LIST, &lines);
    let texts: Vec<_> = records.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["⭐ Run Main one", "ok", "fatal: no newline"]);
    assert!(
        records
            .iter()
            .all(|r| r.section.as_deref() == Some("Main:0"))
    );
}

#[test]
fn raw_act_lines_keep_their_source_stream() {
    let raw = |message: &str, stream: Option<&str>| {
        let mut line = serde_json::json!({"job": "w/a", "jobID": "a", "msg": message,
                "raw_output": true, "stage": "Main", "step": "one", "stepID": ["0"]});
        if let Some(stream) = stream {
            line["raw_stream"] = stream.into();
        }
        line.to_string()
    };
    let lines = [
        raw("out\n", Some("stdout")),
        raw("err\n", Some("stderr")),
        raw("old\n", None),
    ]
    .join("\n");
    let (_, records) = parse(LIST, &lines);
    assert_eq!(
        records
            .iter()
            .map(|r| r.stream.as_str())
            .collect::<Vec<_>>(),
        ["stdout", "stderr", "stdout"]
    );
    assert!(
        records
            .iter()
            .all(|r| r.section.as_deref() == Some("Main:0"))
    );
}

#[test]
fn act_list_columns_tolerate_spaces_in_names() {
    let jobs = parse_act_list(LIST);
    assert_eq!(jobs.len(), 3);
    assert_eq!(jobs[2].name, "Off job");
    assert_eq!(jobs[1].stage, 1);
    assert!(parse_act_list("no header here").is_empty());
}

#[test]
fn fixture_folds_matrix_legs_needs_stages_and_failure() {
    let (mut parser, records) = parse(LIST, FIXTURE);
    parser.tree.settle_finished();
    let tree = &parser.tree;
    assert_eq!(tree.malformed_lines, 0);
    assert_eq!(
        tree.groups
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        ["0", "1"]
    );
    let legs: Vec<_> = tree.groups[0].jobs.iter().map(|j| j.key.as_str()).collect();
    assert_eq!(
        legs,
        ["spike/a (one)", "spike/a (two)"],
        "matrix legs share jobID a"
    );
    assert!(
        tree.groups[0]
            .jobs
            .iter()
            .all(|j| j.conclusion == Some(ItemConclusion::Success))
    );
    let (job, section) = tree.first_failure().expect("failure");
    assert_eq!(job.key, "spike/b");
    let section = section.expect("failing step");
    assert_eq!((section.id.as_str(), section.stage.as_str()), ("1", "Main"));
    assert_eq!(section.exit_code, Some(3));
    assert_eq!(tree.skipped_jobs(), ["off"], "declared but never run");
    // Every record that names a section lies inside that section's range.
    for record in records.iter().filter(|r| r.section.is_some()) {
        let job = tree
            .jobs()
            .find(|j| Some(&j.key) == record.job.as_ref())
            .unwrap();
        let s = job
            .sections
            .iter()
            .find(|s| Some(format!("{}:{}", s.stage, s.id)) == record.section)
            .unwrap();
        assert!(s.first_seq.unwrap() <= record.seq && record.seq <= s.last_seq.unwrap());
    }
}

#[test]
fn sections_never_overlap_within_a_job_property() {
    // Property over every prefix of the fixture and a shuffled-by-job
    // interleaving: ranges inside one job are disjoint.
    let lines: Vec<&str> = FIXTURE.lines().collect();
    for cut in 1..=lines.len() {
        let (parser, _) = parse(LIST, &lines[..cut].join("\n"));
        for job in parser.tree.jobs() {
            let mut ranges: Vec<_> = job
                .sections
                .iter()
                .filter_map(|s| Some((s.first_seq?, s.last_seq?)))
                .collect();
            ranges.sort();
            for pair in ranges.windows(2) {
                assert!(pair[0].1 < pair[1].0, "overlap in {}: {pair:?}", job.key);
            }
        }
    }
}

#[test]
fn a_reusable_workflow_callers_inner_jobs_stand_in_for_it() {
    // `build` calls a reusable workflow: act runs its inner jobs under
    // `<caller name>/<inner name>`, never under the caller's key.
    let list = "Stage  Job ID  Job name   Workflow name  Workflow file  Events\n\
                    0      static  Static     CI             ci.yml         push\n\
                    1      build   Build x64  CI             ci.yml         push\n";
    let lines = [
        r#"{"job":"CI/Static","jobID":"static","msg":"🏁","jobResult":"success"}"#,
        r#"{"job":"Build x64/Compile","jobID":"compile","msg":"⭐ Run Main c","stage":"Main","stepID":["0"]}"#,
        r#"{"job":"Build x64/Compile","jobID":"compile","msg":"done","stage":"Main","stepID":["0"],"stepResult":"success"}"#,
        r#"{"job":"Build x64/Compile","jobID":"compile","msg":"🏁","jobResult":"success"}"#,
        r#"{"job":"Build x64/Package","jobID":"package","msg":"🏁","jobResult":"failure"}"#,
    ];
    let (mut parser, _) = parse(list, &lines.join("\n"));
    parser.tree.settle_finished();
    assert!(
        parser.tree.skipped_jobs().is_empty(),
        "the caller ran: {:?}",
        parser.tree
    );
    let stage = |key: &str| {
        parser
            .tree
            .groups
            .iter()
            .find(|g| g.jobs.iter().any(|j| j.key == key))
            .map(|g| g.name.clone())
    };
    assert_eq!(
        stage("Build x64/Compile").as_deref(),
        Some("1"),
        "the caller's stage"
    );
    assert_eq!(stage("Build x64/Package").as_deref(), Some("1"));
    assert!(
        parser.tree.jobs().all(|j| j.key != "build"),
        "no placeholder left for the caller"
    );
    let compile = parser
        .tree
        .jobs()
        .find(|j| j.key == "Build x64/Compile")
        .unwrap();
    assert_eq!(compile.name, "Compile", "a called job keeps its own name");
    let package = parser
        .tree
        .jobs()
        .find(|j| j.key == "Build x64/Package")
        .unwrap();
    assert_eq!(package.conclusion, Some(ItemConclusion::Failure));
}

#[test]
fn unsupported_skip_unknown_fields_and_malformed_lines() {
    let list = "Stage  Job ID  Job name  Workflow name  Workflow file  Events\n\
                    0      lin     lin       w              ci.yml         push\n\
                    0      mac     mac       w              ci.yml         push\n";
    let lines = [
        r#"{"job":"w/mac","jobID":"mac","level":"info","msg":"🚧  Skipping unsupported platform -- Try running with `-P macos-latest=...`","future":{"x":1}}"#,
        r#"{"job":"w/lin","jobID":"lin","msg":"⭐ Run Main a","stage":"Main","step":"a","stepID":["0"]}"#,
        r#"{"job":"w/lin","jobID":"lin","msg":"noise","stage":"Main","step":"b","stepID":["1"]}"#,
        r#"{"job":"w/lin","jobID":"lin","msg":"  ✅  Success - Main a","stage":"Main","stepID":["0"],"stepResult":"success","executionTime":2000000}"#,
        r#"{"job":"w/lin","jobID":"lin","msg":"⭐ Run Main composite","stage":"Main","stepID":["2","0"]}"#,
        r#"{"job":"w/lin","jobID":"lin","msg":"🏁  Job succeeded","jobResult":"success"}"#,
        r#"{"truncated":"#,
        "",
    ];
    let (mut parser, records) = parse(list, &lines.join("\n"));
    parser.tree.settle_finished();
    assert_eq!(
        parser.tree.malformed_lines, 1,
        "empty lines are not malformed"
    );
    assert_eq!(parser.tree.unsupported_jobs(), ["w/mac"]);
    let lin = parser.tree.jobs().find(|j| j.key == "w/lin").unwrap();
    let by_id = |id: &str| lin.sections.iter().find(|s| s.id == id).unwrap();
    assert_eq!(by_id("0").duration_ms, Some(2));
    assert_eq!(by_id("1").conclusion, Some(ItemConclusion::Skipped));
    assert_eq!(by_id("2/0").conclusion, Some(ItemConclusion::Failure));
    assert_eq!(
        by_id("2/0").name,
        "2/0 › 0",
        "nested steps show their sub-path"
    );
    assert_eq!(records[2].section, None, "pre-start noise is not a section");
    assert_eq!(records[6].text, r#"{"truncated":"#);
}

/// #500: nested runner output that merely *contains* act's
/// unsupported-platform phrase is raw command output, not a lifecycle
/// notice. Folding it as `Unsupported` masked the outer failure:
/// `observe_result` never replaces `Unsupported`, so `first_failure`
/// searched a failure that was no longer there.
#[test]
fn raw_output_never_concludes_a_job_so_an_outer_failure_stays_visible() {
    use crate::ci::{
        lifecycle::{CleanupEnd, EngineReport, ExecutionEnd},
        report,
        workflow::Declared,
    };
    let list = "Stage  Job ID  Job name  Workflow name  Workflow file  Events\n\
                    0      lin     lin       w              ci.yml         push\n";
    let lines = [
        r#"{"job":"w/lin","jobID":"lin","msg":"⭐ Run Main tests","stage":"Main","step":"tests","stepID":["0"]}"#,
        // Ordinary nested test output (run 9b7f0c8b emitted 16 of these).
        r#"{"job":"w/lin","jobID":"lin","msg":"suite: Skipping unsupported platform cases\n","raw_output":true,"stage":"Main","step":"tests","stepID":["0"]}"#,
        // Lifecycle-looking text in raw output is output too.
        r#"{"job":"w/lin","jobID":"lin","msg":"⭐ Run Main done\n","raw_output":true,"stage":"Main","step":"tests","stepID":["0"]}"#,
        r#"{"job":"w/lin","jobID":"lin","msg":"exitcode '7': simulated\n","raw_output":true,"stage":"Main","step":"tests","stepID":["0"]}"#,
        r#"{"job":"w/lin","jobID":"lin","msg":"  ❌  Failure - Main tests","stage":"Main","step":"tests","stepID":["0"],"stepResult":"failure"}"#,
        r#"{"job":"w/lin","jobID":"lin","msg":"🏁  Job failed","jobResult":"failure"}"#,
    ];
    let (mut parser, _) = parse(list, &lines.join("\n"));
    parser.tree.settle_finished();
    assert_eq!(
        parser.tree.unsupported_jobs(),
        Vec::<String>::new(),
        "raw output is no lifecycle notice"
    );
    let (job, section) = parser
        .tree
        .first_failure()
        .expect("the outer failure stays visible");
    assert_eq!(job.key, "w/lin");
    assert_eq!(section.map(|s| s.id.as_str()), Some("0"));
    let section = &job.sections[0];
    assert_eq!(
        section.exit_code, None,
        "a raw line quoting `exitcode 'N'` is not act's exit notice"
    );
    // At run level act exited 1: Failure, never an unsupported verdict.
    let mut tree = std::mem::take(&mut parser.tree);
    let (conclusion, _) = report::conclude(
        &Ok(EngineReport {
            execution: ExecutionEnd::Exited(1),
            cleanup: CleanupEnd::Removed,
            engine_id: None,
            storage: None,
        }),
        &mut tree,
        &Declared::default(),
    );
    assert_eq!(conclusion, crate::ci::Conclusion::Failure);
}

#[test]
fn cancellation_marks_every_unfinished_item() {
    let lines: Vec<&str> = FIXTURE.lines().take(20).collect();
    let (mut parser, _) = parse(LIST, &lines.join("\n"));
    parser.tree.cancel_unfinished();
    for job in parser.tree.jobs() {
        assert_eq!(job.status, ItemStatus::Completed);
        assert!(job.conclusion.is_some());
        assert!(
            job.sections
                .iter()
                .all(|s| s.status == ItemStatus::Completed)
        );
    }
    assert!(
        parser
            .tree
            .jobs()
            .any(|j| j.conclusion == Some(ItemConclusion::Cancelled))
    );
}
