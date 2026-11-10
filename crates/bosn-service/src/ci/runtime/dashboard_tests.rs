//! Test-only replay boundary: use the real observer, run slots and writer.

use super::super::status::StatusSnapshot;
use super::*;

#[derive(Debug)]
pub(crate) enum ExpectedEvent {
    Status(StatusSnapshot),
    Log(LogRecord),
}

/// Await each publication so fixture order is deterministic without sleeps.
async fn checkpoint(ci: &CiRuntime, id: &str, expected: &mut Vec<ExpectedEvent>) {
    let save = ci.lock().runs.get_mut(id).unwrap().snapshot();
    ci.writer.save(save).await.unwrap();
    let snapshot = StatusSnapshot::of(&ci.record(id).unwrap(), 0);
    let last = expected.iter().rev().find_map(|event| match event {
        ExpectedEvent::Status(status) => Some(status),
        ExpectedEvent::Log(_) => None,
    });
    if last.is_none_or(|last| !last.same_status(&snapshot)) {
        expected.push(ExpectedEvent::Status(snapshot));
    }
}

pub(crate) async fn replay(
    ci: &CiRuntime,
    lines: &str,
    listing: &str,
    declared: &workflow::Declared,
    exit_code: i32,
) -> (RunRecord, Vec<ExpectedEvent>) {
    let request = super::super::tests::request("fixture", 'a');
    let record = RunRecord::queued(
        "aaaaaaaa-bbbb-4ccc-8ddd-000000000001".into(),
        &request,
        "push",
        b"{}",
    );
    let id = record.id.clone();
    std::fs::create_dir_all(ci.store.run_dir(&id)).unwrap();
    ci.lock().insert(record, 0);
    let mut expected = Vec::new();
    checkpoint(ci, &id, &mut expected).await;
    ci.update(&id, |slot| slot.record.state = RunState::Running);
    checkpoint(ci, &id, &mut expected).await;
    let mut observer = RunObserver {
        runtime: ci.clone(),
        id: id.clone(),
        log: ci.store.log_writer(&id),
        parser: ActParser::default(),
        job: None,
        masker: SecretMasker::new(Vec::<String>::new()),
        proxy_logs: None,
        seq: 0,
        last_publish: Instant::now(),
    };
    observer.declared(listing);
    checkpoint(ci, &id, &mut expected).await;
    // An independent parser supplies the expected raw records. The observer's
    // real parser drives the runtime tree and its production status writer.
    let mut reference = ActParser::new(RunTree::declared(&parse_act_list(listing)));
    for line in lines.lines() {
        if let Some(log) = reference.feed(observer.seq + 1, line) {
            expected.push(ExpectedEvent::Log(log));
        }
        observer.line(EngineLine::Stdout(line.into()));
        observer.publish();
        checkpoint(ci, &id, &mut expected).await;
    }
    let outcome = Ok(EngineReport {
        execution: ExecutionEnd::Exited(exit_code),
        cleanup: CleanupEnd::Removed,
        engine_id: None,
        storage: None,
    });
    let mut tree = observer.parser.tree;
    let (conclusion, reason) = report::conclude(&outcome, &mut tree, declared);
    ci.complete(&id, |record| {
        record.tree = tree.clone();
        record.finish(conclusion, reason.clone());
    })
    .await;
    checkpoint(ci, &id, &mut expected).await;
    (ci.record(&id).unwrap(), expected)
}
