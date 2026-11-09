//! Runs owned by client processes (2026-10-16): a run ends when its last
//! owner releases it or exits; a detached submission pins it.

use super::*;
use crate::ClientOwner;

async fn submit_owned(
    runtime: &CiRuntime,
    sha_byte: char,
    owner: Option<ClientOwner>,
) -> SubmitReply {
    let staging = new_uuid().await.unwrap();
    staged(runtime, &staging);
    let request = CiRequest::Submit {
        request: Box::new(request(&staging, sha_byte)),
    };
    serde_json::from_value(runtime.handle_owned(request, owner).await.unwrap()).unwrap()
}

async fn release(runtime: &CiRuntime, run: &str, owner: ClientOwner) -> CancelReply {
    let request = CiRequest::Release { run: run.into() };
    serde_json::from_value(runtime.handle_owned(request, Some(owner)).await.unwrap()).unwrap()
}

/// A drained runtime: submissions stay queued, so no engine is involved.
async fn drained(runtime: &CiRuntime) {
    let action = RunnerAction::Drain;
    let _: RunnersReply = call(runtime, CiRequest::Runners { action }).await;
}

fn child() -> (std::process::Child, ClientOwner) {
    let child = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .unwrap();
    let owner = ClientOwner::of_pid(child.id()).unwrap();
    (child, owner)
}

#[test]
fn a_run_ends_when_its_last_owner_releases_it() {
    with_registry(|registry, dir| async move {
        let runtime = CiRuntime::start(&dir, registry, Arc::new(FakeBackend::default()), 1);
        drained(&runtime).await;
        let me = ClientOwner::current().unwrap();
        let (mut other, other_owner) = child();
        let run = submit_owned(&runtime, 'a', Some(me)).await.run;
        let joined = submit_owned(&runtime, 'a', Some(other_owner)).await;
        assert!(joined.coalesced);
        let first = release(&runtime, &run, me).await;
        assert!(!first.cancelled, "the joined owner still holds the run");
        assert_eq!(runtime.record(&run).unwrap().state, RunState::Queued);
        assert!(release(&runtime, &run, other_owner).await.cancelled);
        let record = runtime.record(&run).unwrap();
        assert_eq!(record.conclusion, Some(Conclusion::Cancelled));
        assert!(record.reason.unwrap().contains("exited"));
        other.kill().unwrap();
        other.wait().unwrap();
    });
}

#[test]
fn the_daemon_cancels_a_run_whose_owners_died_but_not_a_pinned_one() {
    with_registry(|registry, dir| async move {
        let runtime = CiRuntime::start(&dir, registry, Arc::new(FakeBackend::default()), 1);
        drained(&runtime).await;
        let (mut doomed, owner) = child();
        let owned = submit_owned(&runtime, 'b', Some(owner)).await.run;
        let pinned = submit_owned(&runtime, 'c', Some(owner)).await.run;
        assert!(submit_owned(&runtime, 'c', None).await.coalesced);
        let detached = submit_owned(&runtime, 'd', None).await.run;
        doomed.kill().unwrap(); // SIGKILL: nothing is released
        let record = wait_done_with_budget(&runtime, &owned, Duration::from_secs(3)).await;
        assert_eq!(record.conclusion, Some(Conclusion::Cancelled));
        let pid = format!("pid {}", owner.pid());
        assert!(
            record.reason.unwrap().contains(&pid),
            "the reason names the client"
        );
        async_engine::sleep(Duration::from_secs(1)).await;
        for run in [&pinned, &detached] {
            assert_eq!(
                runtime.record(run).unwrap().state,
                RunState::Queued,
                "{run}"
            );
        }
        doomed.wait().unwrap();
    });
}

#[test]
fn list_and_record_report_owned_and_detached_runs() {
    with_registry(|registry, dir| async move {
        let runtime = CiRuntime::start(&dir, registry, Arc::new(FakeBackend::default()), 1);
        drained(&runtime).await;
        let me = ClientOwner::current().unwrap();
        let owned = submit_owned(&runtime, 'a', Some(me)).await.run;
        let detached = submit_owned(&runtime, 'b', None).await.run;
        for (id, expected) in [(&owned, vec![me.pid()]), (&detached, vec![])] {
            assert_eq!(runtime.record(id).unwrap().owner_pids, expected);
            let listed = runtime.listing();
            let view = listed.runs.iter().find(|v| &v.record.id == id).unwrap();
            assert_eq!(view.record.owner_pids, expected);
        }
    });
}

#[test]
fn a_reused_owner_pid_cancels_the_ci_run() {
    with_registry(|registry, dir| async move {
        let runtime = CiRuntime::start(&dir, registry, Arc::new(FakeBackend::default()), 1);
        drained(&runtime).await;
        let me = ClientOwner::current().unwrap();
        let (pid, mut created) = ClientOwner::to_wire(Some(me));
        created[0] ^= 1;
        let previous = ClientOwner::from_wire(pid, &created).unwrap().unwrap();
        let run = submit_owned(&runtime, 'a', Some(previous)).await.run;
        let record = wait_done_with_budget(&runtime, &run, Duration::from_secs(3)).await;
        assert_eq!(record.conclusion, Some(Conclusion::Cancelled));
        assert!(record.reason.unwrap().contains("exited"));
        assert!(record.owner_pids.is_empty());
    });
}
