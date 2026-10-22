//! Exact two-volume recovery identity; a nonce alone is insufficient.
use super::*;
use bosn_registry::cache_helper::CacheHelperIntent;

pub(super) fn fixture() -> (CacheHelperIntent, serde_json::Value) {
    let owner = "11111111-2222-4333-8444-555555555555";
    let nonce = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let intent: CacheHelperIntent = serde_json::from_value(serde_json::json!({
        "registry_id": owner, "nonce": nonce, "volume": "bosn-ci-cache-v1",
        "image": engine_image(), "created_at": 1.0,
        "role": {"tool_recovery_v1": {
            "run_id": "11111111-2222-4333-8444-555555555508",
            "command_sha256": "ae0ec6002965a1ecfc7799157a635844850854c81b71571d6443ee1e692daf34",
            "intent": {"schema_version": 1, "owner": "a".repeat(64),
                "engine_id": "b".repeat(64), "generation": "c".repeat(64),
                "source_volume": "bosn-source", "cache_volume": "bosn-ci-cache-v1",
                "created_at_seconds": 0, "expires_at_seconds": 3600}
        }}
    }))
    .unwrap();
    let labels = bosn_core::ResourceLabels::new(
        owner,
        bosn_core::ResourceKind::Container,
        "ci-cache-recovery",
        nonce,
        bosn_core::Scope::Spec,
        "machine",
        "1",
        Some(bosn_core::Retention::Warm),
    )
    .unwrap();
    let mut labels: BTreeMap<String, String> = labels
        .to_map()
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect();
    labels.insert("io.bosn.cache.recovery".into(), nonce.into());
    let document = serde_json::json!({
        "Id": "1".repeat(64), "Name": format!("/{}", intent.name()),
        "Config": {"Image": intent.image, "Labels": labels,
            "Entrypoint": ["/bin/sh"], "Cmd": ["-ec", "exec /tmp/bosn-recovery/bin/act --version"]},
        "HostConfig": {"ReadonlyRootfs": true, "Privileged": true, "NetworkMode": "none",
            "Memory": 536870912, "NanoCpus": 2000000000i64, "PidsLimit": 128, "CpuShares": 256,
            "Tmpfs": {"/tmp": "exec,size=64m"}, "Init": false, "PidMode": "", "AutoRemove": true,
            "RestartPolicy": {"Name":"no", "MaximumRetryCount":0}},
        "Mounts": [
            {"Type": "volume", "Name": "bosn-ci-cache-v1", "Destination": "/bosn/cache", "RW": true, "Driver": "local"},
            {"Type": "volume", "Name": "bosn-source", "Destination": "/var/lib/docker", "RW": true, "Driver": "local"}
        ]
    });
    (intent, document)
}
fn inspect(document: &serde_json::Value) -> String {
    serde_json::to_string(&vec![document]).unwrap()
}

#[test]
fn release_identity_is_source_free_and_drops_every_capability() {
    use bosn_registry::cache_helper::CacheHelperRole;
    let (mut intent, mut document) = fixture();
    let Some(CacheHelperRole::ToolRecoveryV1 {
        run_id,
        intent: frozen,
        command_sha256,
        ..
    }) = intent.role.take()
    else {
        panic!("recovery role missing");
    };
    intent.role = Some(CacheHelperRole::ToolRecoveryReleaseV1 {
        run_id,
        intent: frozen,
        command_sha256: command_sha256.unwrap(),
    });
    let identity = Identity::from_intent(&intent).unwrap();
    let mut labels = identity.ownership.clone();
    labels.insert(identity.label_key().into(), identity.nonce.clone());
    document["Name"] = format!("/{}", intent.name()).into();
    document["Config"]["Labels"] = serde_json::to_value(labels).unwrap();
    let source = document["Mounts"][1].clone();
    document["Mounts"].as_array_mut().unwrap().truncate(1);
    document["HostConfig"]["Privileged"] = false.into();
    document["HostConfig"]["CapDrop"] = serde_json::json!(["ALL"]);
    document["HostConfig"]["Tmpfs"]["/var/lib/docker"] = "ro,noexec,size=1m".into();
    assert!(identity.verify(&inspect(&document), &intent.volume).is_ok());
    for (field, value) in [
        ("Privileged", serde_json::json!(true)),
        ("CapDrop", serde_json::json!([])),
        ("CapDrop", serde_json::Value::Null),
        ("Init", serde_json::json!(true)),
        ("Tmpfs", serde_json::json!({"/tmp":"exec,size=64m"})),
    ] {
        let mut changed = document.clone();
        changed["HostConfig"][field] = value;
        assert!(
            identity.verify(&inspect(&changed), &intent.volume).is_err(),
            "{field}"
        );
    }
    document["Mounts"].as_array_mut().unwrap().push(source);
    assert!(
        identity
            .verify(&inspect(&document), &intent.volume)
            .is_err(),
        "source mount must be refused"
    );
}

#[test]
fn exact_recovery_identity_requires_both_volumes_and_frozen_pid1() {
    let (intent, document) = fixture();
    let identity = Identity::from_intent(&intent).unwrap();
    assert_eq!(
        identity
            .verify(&inspect(&document), &intent.volume)
            .unwrap(),
        "1".repeat(64)
    );
    assert!(
        identity
            .verify(&inspect(&document), "another-cache")
            .is_err()
    );
    for change in [
        "source",
        "cache",
        "readonly-source",
        "readonly-cache",
        "target",
        "driver",
        "missing-source",
        "extra-mount",
        "duplicate-source",
        "entrypoint",
        "command",
        "args",
        "privilege",
        "writable-root",
        "network",
        "memory",
        "cpus",
        "pids",
        "weight",
        "tmpfs",
        "nonce",
        "scope",
        "init",
        "pid-mode",
        "autoremove",
        "restart",
    ] {
        let mut bad = document.clone();
        mutate(&mut bad, change);
        assert!(
            identity.verify(&inspect(&bad), &intent.volume).is_err(),
            "{change}"
        );
    }
}

fn mutate(bad: &mut serde_json::Value, change: &str) {
    match change {
        "source" => bad["Mounts"][1]["Name"] = "foreign-source".into(),
        "cache" => bad["Mounts"][0]["Name"] = "foreign-cache".into(),
        "readonly-source" => bad["Mounts"][1]["RW"] = false.into(),
        "readonly-cache" => bad["Mounts"][0]["RW"] = false.into(),
        "target" => bad["Mounts"][1]["Destination"] = "/source".into(),
        "driver" => bad["Mounts"][1]["Driver"] = "foreign".into(),
        "missing-source" => {
            bad["Mounts"].as_array_mut().unwrap().pop();
        }
        "extra-mount" => bad["Mounts"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"Type":"bind","Name":"","Destination":"/host","RW":true})),
        "duplicate-source" => {
            let source = bad["Mounts"][1].clone();
            bad["Mounts"][0] = source;
        }
        "entrypoint" => bad["Config"]["Entrypoint"] = serde_json::json!(["sleep"]),
        "command" => bad["Config"]["Cmd"] = serde_json::json!(["300"]),
        "args" => bad["Config"]["Cmd"]
            .as_array_mut()
            .unwrap()
            .push("unfrozen".into()),
        "privilege" => bad["HostConfig"]["Privileged"] = false.into(),
        "writable-root" => bad["HostConfig"]["ReadonlyRootfs"] = false.into(),
        "network" => bad["HostConfig"]["NetworkMode"] = "host".into(),
        "memory" => bad["HostConfig"]["Memory"] = 0.into(),
        "cpus" => bad["HostConfig"]["NanoCpus"] = 0.into(),
        "pids" => bad["HostConfig"]["PidsLimit"] = serde_json::Value::Null,
        "weight" => bad["HostConfig"]["CpuShares"] = 1024.into(),
        "tmpfs" => bad["HostConfig"]["Tmpfs"] = serde_json::json!({"/tmp":"noexec,size=64m"}),
        "nonce" => bad["Config"]["Labels"]["io.bosn.cache.recovery"] = "foreign".into(),
        "scope" => {
            bad["Config"]["Labels"]
                .as_object_mut()
                .unwrap()
                .remove("io.bosn.cache.recovery");
        }
        "init" => bad["HostConfig"]["Init"] = true.into(),
        "pid-mode" => bad["HostConfig"]["PidMode"] = "host".into(),
        "autoremove" => bad["HostConfig"]["AutoRemove"] = false.into(),
        "restart" => bad["HostConfig"]["RestartPolicy"]["Name"] = "always".into(),
        _ => unreachable!(),
    }
}

#[test]
fn historical_missing_command_is_readable_but_never_adopted() {
    use bosn_registry::cache_helper::CacheHelperRole;
    let (mut intent, _) = fixture();
    let Some(CacheHelperRole::ToolRecoveryV1 { command_sha256, .. }) = &mut intent.role else {
        panic!("recovery role missing");
    };
    *command_sha256 = None;
    let encoded = serde_json::to_string(&intent).unwrap();
    assert!(!encoded.contains("command_sha256"));
    let decoded: CacheHelperIntent = serde_json::from_str(&encoded).unwrap();
    assert!(Identity::from_intent(&decoded).is_err());
}

#[test]
fn oversized_recovery_inspection_is_refused_before_decoding() {
    let (intent, mut document) = fixture();
    document["Padding"] = "x".repeat(64 << 10).into();
    let identity = Identity::from_intent(&intent).unwrap();
    assert!(
        identity
            .verify(&inspect(&document), &intent.volume)
            .unwrap_err()
            .contains("bound")
    );
}

#[test]
#[ignore = "requires an explicitly prepared, labelled Docker profile probe"]
fn actual_docker_recovery_profile_matches_its_frozen_intent() {
    let base = std::env::var("BOSN_RECOVERY_PROFILE_FIXTURE").unwrap();
    let intent: CacheHelperIntent =
        serde_json::from_slice(&std::fs::read(format!("{base}-intent.json")).unwrap()).unwrap();
    let document = std::fs::read_to_string(format!("{base}-inspection.json")).unwrap();
    let identity = Identity::from_intent(&intent).unwrap();
    let id = identity.verify(&document, &intent.volume).unwrap();
    assert_eq!(
        id,
        std::fs::read_to_string(format!("{base}-id"))
            .unwrap()
            .trim()
    );
}

#[test]
fn recovery_bind_identity_requires_exact_source_path_and_mount_mode() {
    use bosn_registry::cache_helper::CacheHelperRole;
    let (mut intent, mut document) = fixture();
    let path = "/var/lib/docker/volumes/bosn-source/_data";
    let Some(CacheHelperRole::ToolRecoveryV1 {
        source_mountpoint, ..
    }) = &mut intent.role
    else {
        panic!("recovery role missing");
    };
    *source_mountpoint = Some(path.into());
    document["Mounts"][1] = serde_json::json!({
        "Type":"bind", "Source":path, "Destination":"/var/lib/docker", "RW":true
    });
    let identity = Identity::from_intent(&intent).unwrap();
    assert!(identity.verify(&inspect(&document), &intent.volume).is_ok());
    for (field, value) in [
        (
            "Source",
            serde_json::json!("/var/lib/docker/volumes/foreign/_data"),
        ),
        ("RW", serde_json::json!(false)),
        ("Type", serde_json::json!("volume")),
        ("Name", serde_json::json!("bosn-source")),
        ("Driver", serde_json::json!("local")),
    ] {
        let mut changed = document.clone();
        changed["Mounts"][1][field] = value;
        assert!(
            identity.verify(&inspect(&changed), &intent.volume).is_err(),
            "{field}"
        );
    }
}
