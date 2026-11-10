use super::*;
use bosn_core::{ResourceState, Retention};

fn target() -> ManifestVolumeResource {
    ManifestVolumeResource {
        seed_name: Some("target".into()),
        id: "manifest-volume:new".into(),
        name: "new".into(),
        stack: "cook".into(),
        generation: "sha256:new".into(),
        scope: Scope::Stack,
        workspace: "/repo/worktree".into(),
        retention: Retention::Warm,
        target: "/target".into(),
        labels: Default::default(),
    }
}
fn candidate(name: &str, last_used: f64) -> Candidate {
    Candidate {
        resource: bosn_registry::Resource {
            id: format!("manifest-volume:{name}"),
            name: name.into(),
            kind: ResourceKind::Volume,
            scope: Scope::Stack,
            stack: "cook".into(),
            workspace: format!("/repo/{name}"),
            generation: "sha256:source".into(),
            created_at: 1.0,
            last_used,
            state: ResourceState::Active,
            retention: Retention::Warm,
        },
        family: Some("/repo/.git".into()),
        volume: "target".into(),
        idle: true,
        leased: false,
        mounted: false,
    }
}
#[test]
fn selects_most_recent_idle_sibling() {
    let candidates = [
        candidate("older", 2.0),
        candidate("newer", 3.0),
        candidate("oldest", 1.0),
    ];
    assert_eq!(
        select(Some("/repo/.git"), &target(), &candidates, false)
            .unwrap()
            .resource
            .name,
        "newer"
    );
}
#[test]
fn rejects_unsafe_or_unrelated_siblings() {
    let original = candidate("source", 2.0);
    let mut cases = vec![original.clone(); 9];
    cases[0].leased = true;
    cases[1].mounted = true;
    cases[2].idle = false;
    cases[3].family = Some("/other/.git".into());
    cases[4].resource.stack = "other".into();
    cases[5].volume = "other".into();
    cases[6].resource.scope = Scope::Spec;
    cases[7].resource.workspace = target().workspace;
    cases[8].resource.last_used = f64::NAN;
    for case in cases {
        assert!(select(Some("/repo/.git"), &target(), &[case], false).is_none());
    }
}
#[test]
fn no_repository_existing_volume_and_opt_out_never_seed() {
    let candidates = [candidate("source", 2.0)];
    assert!(select(None, &target(), &candidates, false).is_none());
    assert!(select(Some("/repo/.git"), &target(), &candidates, true).is_none());
    let mut target = target();
    target.seed_name = None;
    assert!(select(Some("/repo/.git"), &target, &candidates, false).is_none());
}
#[test]
fn manifest_seed_defaults_true_and_accepts_typed_opt_out() {
    let parse = |seed: &str| {
        bosn_core::manifest::parse_manifest_toml(
            &format!(
                "[stack.cook]\nimage = 'alpine'\n[stack.cook.volumes.target]\nscope = 'stack'\n{seed}"
            ),
            bosn_core::manifest::ManifestRoots::new("test", "/workspace", "/workspace"),
        )
    };
    assert!(parse("").unwrap().stacks["cook"].volumes[0].seed);
    assert!(!parse("seed = false").unwrap().stacks["cook"].volumes[0].seed);
    assert!(parse("seed = 'false'").is_err());
}
#[test]
fn exclusive_volume_lock_never_waits() {
    let root = tempfile::tempdir().unwrap();
    let first = locks::try_lock(root.path(), "source").unwrap().unwrap();
    assert!(locks::try_lock(root.path(), "source").unwrap().is_none());
    drop(first);
    assert!(locks::try_lock(root.path(), "source").unwrap().is_some());
}

#[test]
fn registry_seed_lease_is_exclusive_and_released_without_schema_changes() {
    use crate::manifest_volume_registry::{Command, execute};
    let root = tempfile::tempdir().unwrap();
    let mut registry =
        bosn_registry::Registry::create_writer(root.path().join("registry.sqlite3"), "seed-test")
            .unwrap();
    let resource = candidate("source", 2.0).resource;
    let mut tx = registry.begin_immediate().unwrap();
    tx.put_resource(&resource).unwrap();
    tx.commit().unwrap();
    let lease = bosn_registry::Lease {
        id: "volume-seed:new".into(),
        resource_id: resource.id.clone(),
        pid: std::process::id(),
        proc_start: None,
        acquired_at: 3.0,
        heartbeat_at: 3.0,
        ttl_seconds: 0.0,
    };
    assert!(
        execute(
            &mut registry,
            Command::AcquireSeed {
                source: Box::new(resource.clone()),
                lease: lease.clone()
            }
        )
        .unwrap()
    );
    assert!(
        !execute(
            &mut registry,
            Command::AcquireSeed {
                source: Box::new(resource.clone()),
                lease: lease.clone()
            }
        )
        .unwrap()
    );
    assert!(execute(&mut registry, Command::ReleaseSeed(lease.id.clone())).unwrap());
    assert!(
        execute(
            &mut registry,
            Command::AcquireSeed {
                source: Box::new(resource),
                lease
            }
        )
        .unwrap()
    );
}
