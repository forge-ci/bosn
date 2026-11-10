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
fn manifest_seed_defaults_off_and_requires_typed_stack_opt_in() {
    let parse = |seed: &str| {
        bosn_core::manifest::parse_manifest_toml(
            &format!(
                "[stack.cook]\nimage = 'alpine'\n[stack.cook.volumes.target]\nscope = 'stack'\n{seed}"
            ),
            bosn_core::manifest::ManifestRoots::new("test", "/workspace", "/workspace"),
        )
    };
    assert!(!parse("").unwrap().stacks["cook"].volumes[0].seed);
    assert!(parse("seed = true").unwrap().stacks["cook"].volumes[0].seed);
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
fn completion_gate_resets_orphans_and_copying_but_uses_complete() {
    use bosn_registry::volume_seed::{SeedState, VolumeSeed};
    let mut labels = std::collections::BTreeMap::from([
        (gate::source_label(), "bosn-v-stack-source".into()),
        (gate::nonce_label(), "nonce".into()),
    ]);
    let mut seed = VolumeSeed {
        destination: "bosn-v-stack-destination".into(),
        source: "bosn-v-stack-source".into(),
        nonce: "nonce".into(),
        state: SeedState::Copying,
    };
    assert_eq!(
        gate::decision(None, &labels).unwrap(),
        gate::Decision::Reset
    );
    assert_eq!(
        gate::decision(Some(&seed), &labels).unwrap(),
        gate::Decision::Reset
    );
    seed.state = SeedState::Complete;
    assert_eq!(
        gate::decision(Some(&seed), &labels).unwrap(),
        gate::Decision::Use
    );
    labels.insert(gate::nonce_label(), "foreign".into());
    assert!(matches!(
        gate::decision(Some(&seed), &labels),
        Err(gate::SeedError::SeedDestinationUncertain { .. })
    ));
    assert_eq!(
        gate::decision(None, &Default::default()).unwrap(),
        gate::Decision::Use
    );
}
#[test]
fn budget_reserves_ensure_floor_and_caps_the_copy() {
    use std::time::Duration;
    assert!(budget(ENSURE_FLOOR).is_none());
    assert!(budget(ENSURE_FLOOR - Duration::from_secs(1)).is_none());
    assert_eq!(
        budget(Duration::from_secs(200)),
        Some(Duration::from_secs(140))
    );
    assert_eq!(
        budget(Duration::from_secs(4000)),
        Some(Duration::from_secs(600))
    );
}
