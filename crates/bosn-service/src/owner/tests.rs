use super::*;

fn owner(pid: u32, created: u64) -> ClientOwner {
    ClientOwner {
        pid,
        created: [created, 0],
    }
}

#[test]
fn the_current_process_has_an_owner_identity() {
    let me = ClientOwner::current().expect("this host identifies its own process");
    assert_eq!(me.pid(), std::process::id());
    assert_eq!(me.liveness(), Liveness::Alive);
    assert_eq!(
        ClientOwner::current(),
        Some(me),
        "the creation key is stable"
    );
}

#[test]
fn a_reused_pid_is_not_the_owner() {
    let me = ClientOwner::current().unwrap();
    let previous = ClientOwner {
        created: [me.created[0] ^ 1, me.created[1]],
        ..me
    };
    assert_eq!(previous.liveness(), Liveness::Gone);
}

#[test]
fn the_wire_form_round_trips_and_rejects_partial_identities() {
    let me = ClientOwner::current().unwrap();
    let (pid, created) = ClientOwner::to_wire(Some(me));
    assert_eq!(ClientOwner::from_wire(pid, &created).unwrap(), Some(me));
    assert_eq!(ClientOwner::to_wire(None), (0, Vec::new()));
    assert_eq!(ClientOwner::from_wire(0, &[]).unwrap(), None);
    for (pid, created) in [
        (0, vec![1, 2]),
        (7, vec![]),
        (7, vec![1]),
        (7, vec![1, 2, 3]),
    ] {
        assert!(
            ClientOwner::from_wire(pid, &created).is_err(),
            "{pid} {created:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_child_is_alive_until_it_exits_and_gone_even_before_it_is_reaped() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id();
    let owner = ClientOwner::of_pid(pid).expect("the child is identified");
    assert_eq!(owner.liveness(), Liveness::Alive);
    child.kill().unwrap();
    // Not yet reaped: a zombie still has a process-table entry.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while owner.liveness() != Liveness::Gone {
        assert!(
            std::time::Instant::now() < deadline,
            "a killed child stays alive"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child.wait().unwrap();
    assert_eq!(owner.liveness(), Liveness::Gone);
}

fn by(alive: &'static [u32]) -> impl FnMut(&ClientOwner) -> Liveness {
    move |owner| {
        if alive.contains(&owner.pid) {
            Liveness::Alive
        } else {
            Liveness::Gone
        }
    }
}

#[test]
fn work_is_orphaned_only_when_every_owner_is_gone() {
    let mut owners = Owners::default();
    owners.submitted(&1u64, Some(owner(10, 1)), true);
    owners.submitted(&1, Some(owner(11, 1)), false);
    owners.submitted(&2, Some(owner(10, 1)), true);
    assert!(owners.orphaned(by(&[10, 11])).is_empty());
    assert_eq!(owners.orphaned(by(&[11])), vec![(2, vec![10])]);
    assert_eq!(owners.orphaned(by(&[])), vec![(1, vec![10, 11])]);
    assert!(
        owners.is_empty(),
        "orphaned work is forgotten once reported"
    );
}

#[test]
fn unknown_liveness_keeps_work_running() {
    let mut owners = Owners::default();
    owners.submitted(&1u64, Some(owner(10, 1)), true);
    assert!(owners.orphaned(|_| Liveness::Unknown).is_empty());
}

#[test]
fn a_detached_submission_pins_work_and_unowned_work_gains_no_owner() {
    let mut owners = Owners::default();
    owners.submitted(&1u64, Some(owner(10, 1)), true);
    owners.submitted(&1, None, false);
    owners.submitted(&2, None, true);
    owners.submitted(&2, Some(owner(10, 1)), false);
    assert!(owners.orphaned(by(&[])).is_empty());
    assert!(owners.is_empty());
}

#[test]
fn finished_work_is_forgotten() {
    let mut owners = Owners::default();
    owners.submitted(&1u64, Some(owner(10, 1)), true);
    owners.forget(&1);
    assert!(owners.orphaned(by(&[])).is_empty());
}

#[test]
fn releasing_cancels_only_when_the_last_owner_lets_go() {
    let mut owners = Owners::default();
    owners.submitted(&1u64, Some(owner(10, 1)), true);
    owners.submitted(&1, Some(owner(11, 1)), false);
    assert!(!owners.release(&1, &owner(12, 1)), "not an owner");
    assert!(!owners.release(&1, &owner(10, 1)), "another owner remains");
    assert!(owners.release(&1, &owner(11, 1)));
    assert!(owners.is_empty());
    owners.submitted(&2, None, true);
    assert!(
        !owners.release(&2, &owner(10, 1)),
        "detached work is never released"
    );
}
