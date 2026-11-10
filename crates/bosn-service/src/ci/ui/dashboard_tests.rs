//! Recorded act output through the parser, persisted run and authenticated UI.

use super::*;
use crate::ci::{
    Conclusion,
    model::{ItemConclusion, ItemStatus},
    reply::RunView,
    runtime::dashboard_tests::{ExpectedEvent, replay},
    workflow::Declared,
};

#[path = "leg_dashboard_tests.rs"]
mod legs;
use legs::{LISTING, RECORDED, assert_legs};

const FIXTURE: &str =
    include_str!("../../../tests/fixtures/act/act-0.2.88-matrix-needs-failure.jsonl");

const NEEDS_LIST: &str = "Stage  Job ID  Job name  Workflow name  Workflow file  Events\n\
0      a       a         spike          ci.yml         push\n\
1      b       b         spike          ci.yml         push\n\
2      off     off       spike          ci.yml         push\n";

// The recorded workflow plus a declared dependent that act never starts after
// b fails. Its needs stage comes from the listing, as in a real act --list.
const NEEDS_WORKFLOW: &str = r#"name: spike
on: [push]
jobs:
  a:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        leg: [one, two]
    steps:
      - run: echo hello ${{ matrix.leg }}
      - if: false
        run: echo skipped
  b:
    needs: a
    runs-on: ubuntu-latest
    steps:
      - run: echo before
      - run: exit 3
  off:
    needs: b
    runs-on: ubuntu-latest
    steps:
      - run: echo after failure
"#;

fn declared_steps(dir: &std::path::Path) -> Declared {
    let source = dir.join("source");
    let workflow = ".github/workflows/ci.yml";
    let path = source.join(workflow);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, NEEDS_WORKFLOW).unwrap();
    let declared = crate::ci::workflow::declared(&source, workflow, "fixture");
    assert!(declared.errors.is_empty(), "{:?}", declared.errors);
    declared
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
            ("Set up job", Some(ItemConclusion::Success), None),
            ("echo before", Some(ItemConclusion::Success), Some(63)),
            ("exit 3", Some(ItemConclusion::Failure), Some(59)),
            ("Complete job", Some(ItemConclusion::Success), None),
        ]
    );
    assert_eq!(b.sections[2].exit_code, Some(3));
    assert_eq!(tree.groups[2].jobs[0].job_id, "off");
    assert_eq!(
        tree.groups[2].jobs[0].conclusion,
        Some(ItemConclusion::Skipped)
    );
}

async fn assert_replay(
    port: u16,
    host: &str,
    cookie: &str,
    id: &str,
    expected: Vec<ExpectedEvent>,
) {
    let path = format!("/v1/runs/{id}/events?from_seq=0");
    let first = raw(port, get(port, &path, host, Some(cookie))).await;
    assert_eq!(first.status, 200);
    let frames = sse_ids_and_kinds(&first.body);
    assert_eq!(
        frames
            .iter()
            .map(|(_, kind)| kind.clone())
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|event| match event {
                ExpectedEvent::Status(_) => "status".to_owned(),
                ExpectedEvent::Log(log) => log.stream.clone(),
            })
            .collect::<Vec<_>>()
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
    let payloads = |body: &str| {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let data = payloads(&first.body);
    assert_eq!(payloads(&resumed.body), data[middle + 1..]);
    assert_eq!(data.len(), expected.len());
    for (data, expected) in data.iter().zip(expected) {
        match expected {
            ExpectedEvent::Status(expected) => {
                let mut actual: crate::ci::status::StatusSnapshot =
                    serde_json::from_str(data).unwrap();
                actual.seq = 0;
                assert_eq!(actual, expected);
            }
            ExpectedEvent::Log(mut expected) => {
                let mut actual: crate::ci::model::LogRecord = serde_json::from_str(data).unwrap();
                actual.seq = 0;
                expected.seq = 0;
                assert_eq!(actual, expected);
            }
        }
    }
    assert!(resumed.body.contains("\"state\":\"done\""));
}

#[test]
fn act_fixture_drives_the_full_dashboard() {
    for (lines, listing, legs) in [(FIXTURE, NEEDS_LIST, false), (RECORDED, LISTING, true)] {
        with_registry(|registry, dir| async move {
            let (ci, _server, port) = listener(&dir, registry).await;
            let declared = if legs {
                Declared::default()
            } else {
                declared_steps(&dir)
            };
            let (record, expected) =
                replay(&ci, lines, listing, &declared, if legs { 0 } else { 1 }).await;
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
            assert_replay(port, &host, &cookie, &record.id, expected).await;
        });
    }
}
