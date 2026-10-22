use super::*;

const SLOTS: &str = include_str!("slots.sh");

fn holder(run: &str, pid: u32) -> Holder {
    Holder {
        run: run.into(),
        registry: "0a1b2c3d-4e5f-4a6b-8c7d-000000000001".into(),
        daemon: Daemon {
            boot: "b00t".into(),
            pid,
            start: 7,
        },
    }
}

#[test]
fn this_process_is_alive_and_a_reused_pid_is_not() {
    let me = Daemon::current();
    assert!(me.alive());
    let reused = Daemon {
        start: me.start + 1,
        ..me.clone()
    };
    assert!(!reused.alive(), "same pid, different start time");
    let elsewhere = Daemon {
        boot: "another-boot".into(),
        start: 0,
        ..me
    };
    assert!(elsewhere.alive(), "another boot cannot be checked");
}

#[test]
fn a_claim_round_trips_through_its_labels() {
    let claim = EngineClaim {
        id: "c1".into(),
        engine: "bosn-act-0a1b2c3d-4e5f-4a6b-8c7d-000000000002".into(),
        registry: "r".into(),
        release: Some(env!("CARGO_PKG_VERSION").into()),
        cpu_source: Some(super::super::reply::CiCpuSource::DefaultCeiling),
        daemon: Daemon::current(),
        created: 42,
    };
    assert_eq!(
        EngineClaim::from_labels("c1", &claim.labels()),
        Ok(claim.clone())
    );
    assert!(claim.ours("r"));
    assert!(!claim.ours("other"));
    let mut older = claim.labels();
    older.remove(&format!("{CLAIM_PREFIX}release"));
    older.remove(&format!("{CLAIM_PREFIX}cpu-source"));
    let older = EngineClaim::from_labels("c1", &older).unwrap();
    assert_eq!((older.release, older.cpu_source), (None, None));
    let mut bad_source = claim.labels();
    bad_source.insert(format!("{CLAIM_PREFIX}cpu-source"), "surprise".into());
    assert!(EngineClaim::from_labels("c1", &bad_source).is_err());
    let mut unknown_source = claim.labels();
    unknown_source.insert(format!("{CLAIM_PREFIX}cpu-source"), "unknown".into());
    assert_eq!(
        EngineClaim::from_labels("c1", &unknown_source)
            .unwrap()
            .cpu_source,
        Some(super::super::reply::CiCpuSource::Unknown)
    );
    let mut foreign = claim.labels();
    foreign.remove(CLAIM_LABEL);
    assert!(EngineClaim::from_labels("c1", &foreign).is_err());
}

#[test]
fn slot_replies_parse_eagerly() {
    assert_eq!(reply::leased("slot 3\n"), Ok(Leased::Slot(3)));
    assert_eq!(reply::leased("retiring"), Ok(Leased::Retiring));
    assert_eq!(reply::leased("full"), Ok(Leased::Full));
    assert!(reply::leased("slot x").is_err());
    let line = holder("run-a", 12).line();
    let survey = reply::survey(&format!("held 0 {line}\nidle 30\nrequested\n")).unwrap();
    assert_eq!(survey.held, vec![(0, holder("run-a", 12))]);
    assert_eq!(survey.idle_secs, 30);
    assert!(survey.requested && !survey.retiring);
    assert!(reply::survey("held 0 short\nidle 1").is_err());
    assert!(
        reply::survey("retiring").is_err(),
        "an idle time is required"
    );
}

/// The in-engine script itself, run by the host's `sh` against a private
/// slot directory: leases are distinct, retirement waits for the last slot,
/// and a retiring engine leases nothing.
#[cfg(target_os = "linux")]
#[test]
fn the_slot_script_leases_and_retires_under_one_lock() {
    if std::process::Command::new("flock")
        .arg("-V")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = SLOTS.replace("d=/run/bosn-slots", &format!("d={}", dir.path().display()));
    let run = |args: &[&str]| {
        let out = std::process::Command::new("sh")
            .arg("-ec")
            .arg(&script)
            .arg("sh")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let line = holder("run-a", 12).line();
    let mut args = vec!["lease", "2"];
    args.extend(line.split(' '));
    assert_eq!(reply::leased(&run(&args)), Ok(Leased::Preparing));
    run(&["ready"]);
    assert_eq!(reply::leased(&run(&args)), Ok(Leased::Slot(0)));
    assert_eq!(reply::leased(&run(&args)), Ok(Leased::Slot(1)));
    assert_eq!(reply::leased(&run(&args)), Ok(Leased::Full));
    run(&["release", "0"]);
    let survey = reply::survey(&run(&["retire"])).unwrap();
    assert_eq!(survey.held.len(), 1);
    assert!(!survey.retiring, "a held slot keeps the engine");
    run(&["release", "1"]);
    run(&["request"]);
    let survey = reply::survey(&run(&["retire"])).unwrap();
    assert!(survey.held.is_empty() && survey.retiring && survey.requested);
    assert_eq!(reply::leased(&run(&args)), Ok(Leased::Retiring));
}

#[cfg(target_os = "linux")]
#[test]
fn native_slot_cutoff_is_checked_under_the_lock_without_stopping_holders() {
    let dir = tempfile::tempdir().unwrap();
    let script = SLOTS.replace("d=/run/bosn-slots", &format!("d={}", dir.path().display()));
    let run = |now: u64, args: &[&str]| {
        std::process::Command::new("sh")
            .args([
                "-ec",
                &script.replace("date +%s", &format!("printf '{now}'")),
                "sh",
            ])
            .args(args)
            .output()
            .unwrap()
    };
    let text = |output: std::process::Output| {
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    text(run(100, &["ready"]));
    assert_eq!(
        text(run(100, &["lease-native", "2", "holder"])),
        "preparing\n"
    );
    assert_eq!(text(run(100, &["ready-native", "200"])), "ok\n");
    assert_eq!(text(run(100, &["ready-native", "200"])), "ok\n");
    assert!(!run(100, &["ready-native", "201"]).status.success());
    assert_eq!(text(run(100, &["lease-native", "2", "holder"])), "slot 0\n");
    for operation in ["lease-native", "lease"] {
        assert_eq!(text(run(200, &[operation, "2", "late"])), "retiring\n");
    }
    assert!(dir.path().join("requested").is_file());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("0/holder")).unwrap(),
        "holder\n"
    );
    assert!(!dir.path().join("1").exists(), "no late slot was allocated");
    assert!(!run(200, &["ready-native", "200"]).status.success());
}

#[test]
fn native_identity_labels_preserve_a_bounded_validated_binding() {
    let mut intent = crate::ci::lifecycle::tests::intent("0a1b2c3d-4e5f-4a6b-8c7d-000000000001");
    let owner = "0a1b2c3d-4e5f-4a6b-8c7d-000000000002";
    let legacy = crate::act_engine::creation_labels(&intent, owner).unwrap();
    let key = "com.zackees.bosn.act.tool-generation";
    assert!(!legacy.contains_key(key));
    assert_eq!(
        EngineIdentity::of(&intent),
        EngineIdentity::from_labels(&legacy)
    );
    let generation = ActToolGenerationBinding {
        id: "a".repeat(64),
        max_payload_bytes: 1 << 30,
        overlay_recipe_sha256: crate::ci::engine::tool_overlay_recipe_sha256(),
    };
    intent.creation_profile = Some(
        crate::act_engine::creation_profile_with_tools(
            crate::ci::limits::size_engine(
                crate::ci::lifecycle::tests::FAKE_HOST,
                Default::default(),
            )
            .unwrap(),
            Some(crate::ci::lifecycle::tests::test_cache().mount()),
            Some(generation.clone()),
        )
        .unwrap(),
    );
    let labels = crate::act_engine::creation_labels(&intent, owner).unwrap();
    assert_eq!(
        EngineIdentity::of(&intent),
        EngineIdentity::from_labels(&labels)
    );
    let mut missing = labels.clone();
    missing.remove(key);
    assert_ne!(
        EngineIdentity::of(&intent),
        EngineIdentity::from_labels(&missing)
    );
    let canonical = serde_json::to_string(&generation).unwrap();
    for bad in [
        " ".repeat(513),
        "{}".into(),
        canonical.replace(&"a".repeat(64), &"A".repeat(64)),
        canonical.replace("1073741824", "0"),
        canonical.replace("}", ",\"unknown\":true}"),
    ] {
        let mut invalid = labels.clone();
        invalid.insert(key.into(), bad);
        assert!(EngineIdentity::from_labels(&invalid).is_none());
    }
}
