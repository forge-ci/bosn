//! Recorded act output through the parser, persisted run and authenticated UI.

use super::*;
use crate::ci::{
    Conclusion, RunRecord, RunState,
    lifecycle::{CleanupEnd, EngineReport, ExecutionEnd},
    model::{ActParser, ItemConclusion, ItemStatus, RunTree, parse_act_list},
    reply::RunView,
    store::Store,
    workflow::{Declared, DeclaredStep},
};

// Reuse the existing single include bindings: no second fixture path (and no
// concat! outside src/, which the crates.io amalgamator cannot relocate).
use crate::ci::model::leg_tests::{LISTING, RECORDED};
use crate::ci::model::parser_tests::FIXTURE;

const NEEDS_LIST: &str = "Stage  Job ID  Job name  Workflow name  Workflow file  Events\n\
0      a       a         spike          ci.yml         push\n\
1      b       b         spike          ci.yml         push\n\
2      off     off       spike          ci.yml         push\n";

fn declared_steps() -> Declared {
    let steps = |names: &[&str]| {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| DeclaredStep {
                id: i.to_string(),
                name: (*name).into(),
            })
            .collect()
    };
    Declared {
        steps: [
            (
                "a".into(),
                steps(&["echo hello ${{ matrix.leg }}", "echo skipped"]),
            ),
            ("b".into(), steps(&["echo before", "exit 3"])),
            ("off".into(), steps(&["after failure"])),
        ]
        .into(),
        ..Declared::default()
    }
}

/// This is the parser_tests path, with each resulting LogRecord persisted and
/// each tree transition journalled before the next fixture line arrives.
fn record_fixture(
    dir: &std::path::Path,
    lines: &str,
    listing: &str,
    legs: bool,
) -> (RunRecord, Vec<String>) {
    let request = crate::ci::tests::request("fixture", 'a');
    let mut record = RunRecord::queued(
        "aaaaaaaa-bbbb-4ccc-8ddd-000000000001".into(),
        &request,
        "push",
        b"{}",
    );
    let store = Store::new(dir);
    std::fs::create_dir_all(store.run_dir(&record.id)).unwrap();
    crate::ci::status::append(&store, &record, 1).unwrap();
    record.state = RunState::Running;
    crate::ci::status::append(&store, &record, 2).unwrap();
    let mut parser = ActParser::new(RunTree::declared(&parse_act_list(listing)));
    let mut logs = store.log_writer(&record.id).unwrap();
    let mut kinds = vec!["status".into(), "status".into()];
    let mut status_seq = 2;
    for (index, line) in lines.lines().enumerate() {
        let before = parser.tree.clone();
        if let Some(log) = parser.feed(index as u64 + 1, line) {
            logs.append(&log).unwrap();
            logs.flush();
            record.log_records = log.seq;
            kinds.push(log.stream);
        }
        if before != parser.tree {
            record.tree = parser.tree.clone();
            status_seq += 1;
            crate::ci::status::append(&store, &record, status_seq).unwrap();
            kinds.push("status".into());
        }
    }
    let outcome = Ok(EngineReport {
        execution: ExecutionEnd::Exited(if legs { 0 } else { 1 }),
        cleanup: CleanupEnd::Removed,
        engine_id: None,
        storage: None,
    });
    let declared = if legs {
        Declared::default()
    } else {
        declared_steps()
    };
    let (conclusion, reason) = crate::ci::report::conclude(&outcome, &mut parser.tree, &declared);
    record.tree = parser.tree;
    record.finish(conclusion, reason);
    store.save_run(&record);
    crate::ci::status::append(&store, &record, status_seq + 1).unwrap();
    kinds.push("status".into());
    (record, kinds)
}

fn assert_needs(view: &RunView) {
    let tree = &view.record.tree;
    assert_eq!(view.record.conclusion, Some(Conclusion::Failure));
    assert_eq!(
        tree.groups
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        ["0", "1", "2"]
    );
    assert_eq!(
        tree.groups[0]
            .jobs
            .iter()
            .map(|j| j.key.as_str())
            .collect::<Vec<_>>(),
        ["spike/a (one)", "spike/a (two)"]
    );
    for (job, duration) in tree.groups[0].jobs.iter().zip([67, 74]) {
        assert_eq!(job.conclusion, Some(ItemConclusion::Success));
        assert_eq!(
            job.sections
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            [
                "Set up job",
                if duration == 67 {
                    "echo hello one"
                } else {
                    "echo hello two"
                },
                "echo skipped",
                "Complete job"
            ]
        );
        assert_eq!(job.sections[1].duration_ms, Some(duration));
        assert_eq!(job.sections[1].conclusion, Some(ItemConclusion::Success));
        assert_eq!(job.sections[2].conclusion, Some(ItemConclusion::Skipped));
    }
    let b = &tree.groups[1].jobs[0];
    assert_eq!(b.key, "spike/b");
    assert_eq!(b.conclusion, Some(ItemConclusion::Failure));
    assert_eq!(
        b.sections
            .iter()
            .map(|s| (s.name.as_str(), s.conclusion, s.duration_ms))
            .collect::<Vec<_>>(),
        [
            ("Set up job", Some(ItemConclusion::Success), Some(0)),
            ("echo before", Some(ItemConclusion::Success), Some(63)),
            ("exit 3", Some(ItemConclusion::Failure), Some(59)),
            ("Complete job", Some(ItemConclusion::Success), Some(0)),
        ]
    );
    assert_eq!(b.sections[2].exit_code, Some(3));
    assert_eq!(tree.groups[2].jobs[0].job_id, "off");
    assert_eq!(
        tree.groups[2].jobs[0].conclusion,
        Some(ItemConclusion::Skipped)
    );
}

fn assert_legs(view: &RunView) {
    let jobs: Vec<_> = view.record.tree.jobs().collect();
    assert_eq!(jobs.len(), 5);
    assert_eq!(view.record.conclusion, Some(Conclusion::Incomplete));
    for (name, conclusion, duration) in [
        ("legs/first", ItemConclusion::Success, 60),
        (
            "legs/Wheel (ubuntu-latest) (ubuntu-latest, 3.11)",
            ItemConclusion::Success,
            65,
        ),
        (
            "legs/Wheel (windows-latest) (windows-latest, 3.11)",
            ItemConclusion::Unsupported,
            76,
        ),
        ("legs/lint (ruff)", ItemConclusion::Success, 59),
        ("legs/lint (pyright)", ItemConclusion::Success, 60),
    ] {
        let job = jobs
            .iter()
            .find(|j| j.key == name)
            .unwrap_or_else(|| panic!("missing {name}: {jobs:#?}"));
        assert_eq!(job.conclusion, Some(conclusion));
        assert_eq!(job.sections.first().unwrap().name, "Set up job");
        assert_eq!(job.sections.last().unwrap().name, "Complete job");
        assert_eq!(job.sections[1].duration_ms, Some(duration));
    }
}

async fn assert_replay(port: u16, host: &str, cookie: &str, id: &str, kinds: Vec<String>) {
    let path = format!("/v1/runs/{id}/events?from_seq=0");
    let first = raw(port, get(port, &path, host, Some(cookie))).await;
    assert_eq!(first.status, 200);
    let frames = sse_ids_and_kinds(&first.body);
    assert_eq!(
        frames
            .iter()
            .map(|(_, kind)| kind.clone())
            .collect::<Vec<_>>(),
        kinds
    );
    assert!(frames.windows(2).all(|pair| pair[1].0 == pair[0].0 + 1));
    let middle = frames.len() / 2;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nCookie: {cookie}\r\nLast-Event-ID: {}\r\nConnection: close\r\n\r\n",
        frames[middle].0
    );
    let resumed = raw(port, request).await;
    assert_eq!(resumed.status, 200);
    assert_eq!(sse_ids_and_kinds(&resumed.body), frames[middle + 1..]);
    // Compare data as well as IDs/kinds: raw output and tree snapshots replay exactly.
    let suffix = first
        .body
        .find(&format!("id: {}\n", frames[middle + 1].0))
        .unwrap();
    assert_eq!(resumed.body, first.body[suffix..]);
    assert!(resumed.body.contains("\"state\":\"done\""));
}

#[test]
fn act_fixture_drives_the_full_dashboard() {
    for (lines, listing, legs) in [(FIXTURE, NEEDS_LIST, false), (RECORDED, LISTING, true)] {
        with_registry(|registry, dir| async move {
            let (record, kinds) = record_fixture(&dir, lines, listing, legs);
            let (ci, _server, port) = listener(&dir, registry).await;
            let cookie = sign_in(&ci, port).await;
            let host = format!("127.0.0.1:{port}");
            let page = format!("/ci/runs/{}", record.id);
            assert_eq!(raw(port, get(port, &page, &host, None)).await.status, 401);
            let page = raw(port, get(port, &page, &host, Some(&cookie))).await;
            assert_eq!(page.status, 200);
            assert!(page.body.contains("app.js"));
            let api = format!("/v1/runs/{}", record.id);
            assert_eq!(raw(port, get(port, &api, &host, None)).await.status, 401);
            let reply = raw(port, get(port, &api, &host, Some(&cookie))).await;
            assert_eq!(reply.status, 200, "{}", reply.body);
            let view: RunView = serde_json::from_str(&reply.body).unwrap();
            assert_eq!(view.record.tree.malformed_lines, 0);
            assert!(
                view.record
                    .tree
                    .jobs()
                    .all(|j| j.status == ItemStatus::Completed)
            );
            if legs {
                assert_legs(&view);
            } else {
                assert_needs(&view);
            }
            assert_replay(port, &host, &cookie, &record.id, kinds).await;
        });
    }
}
