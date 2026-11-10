use super::*;
use bosn_core::{ResourceLabels, Retention, Scope};
const OWNER: &str = "11111111-2222-4333-8444-555555555555";

fn labels(retention: Retention) -> BTreeMap<String, String> {
    ResourceLabels::new(
        OWNER,
        ResourceKind::Volume,
        "app",
        "gen",
        Scope::Stack,
        "/workspace",
        "1",
        Some(retention),
    )
    .unwrap()
    .to_map()
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}

#[test]
fn explicit_clean_waives_age_but_never_mount_pin_or_foreign_gates() {
    let check = |labels, in_use| {
        verdict(
            "volume",
            ResourceKind::Volume,
            authority::Proven {
                labels,
                age: 0.0,
                in_use,
            },
            OWNER,
        )
    };
    assert!(check(labels(Retention::Warm), false).is_ok());
    assert!(
        check(labels(Retention::Warm), true)
            .unwrap_err()
            .to_string()
            .contains("in-use")
    );
    assert!(
        check(labels(Retention::Pinned), false)
            .unwrap_err()
            .to_string()
            .contains("pinned")
    );
    let mut foreign = labels(Retention::Warm);
    foreign.insert(
        bosn_core::LABEL_REGISTRY.into(),
        "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into(),
    );
    assert!(
        check(foreign, false)
            .unwrap_err()
            .to_string()
            .contains("foreign")
    );
    assert!(check(BTreeMap::new(), false).is_err());
}

#[test]
fn clean_never_expires_a_recorded_lease_and_names_its_holder() {
    let root = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let mut registry =
        bosn_registry::Registry::create_writer(root.path().join("registry.sqlite3"), OWNER)
            .unwrap();
    let target = target();
    let mut tx = registry.begin_immediate().unwrap();
    tx.put_resource(&bosn_registry::Resource {
        id: target.id.clone(),
        kind: ResourceKind::Volume,
        name: target.name.clone(),
        stack: target.stack.clone(),
        generation: target.generation.clone(),
        scope: target.scope,
        workspace: target.workspace.clone(),
        created_at: 1.0,
        last_used: 1.0,
        state: bosn_core::ResourceState::Active,
        retention: Retention::Warm,
    })
    .unwrap();
    tx.put_lease(&bosn_registry::Lease {
        id: "holder-lease".into(),
        resource_id: target.id.clone(),
        pid: 42,
        proc_start: None,
        acquired_at: 1.0,
        heartbeat_at: 1.0,
        ttl_seconds: 0.0,
    })
    .unwrap();
    tx.commit().unwrap();
    let error = check_lease(root.path(), &target).unwrap_err().to_string();
    assert!(
        error.contains("holder-lease") && error.contains("42"),
        "{error}"
    );
}

#[test]
fn exclusive_volume_lock_refuses_without_waiting() {
    let root = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let _holder = crate::volume_seed::locks::try_lock(root.path(), "volume")
        .unwrap()
        .unwrap();
    assert!(
        crate::volume_seed::locks::try_lock(root.path(), "volume")
            .unwrap()
            .is_none()
    );
}

fn target() -> ManifestVolumeResource {
    ManifestVolumeResource {
        id: "manifest-volume:volume".into(),
        name: "volume".into(),
        stack: "app".into(),
        generation: "sha256:abc".into(),
        scope: Scope::Stack,
        workspace: "/workspace".into(),
        retention: Retention::Warm,
        target: "/state".into(),
        labels: BTreeMap::new(),
        seed_name: None,
    }
}

#[test]
fn cold_task_recording_preserves_a_pin_on_an_unselected_machine_volume() {
    let root = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let mut registry =
        bosn_registry::Registry::create_writer(root.path().join("registry.sqlite3"), OWNER)
            .unwrap();
    let mut target = target();
    target.scope = Scope::Machine;
    target.retention = Retention::Pinned;
    crate::manifest_volume_registry::execute(
        &mut registry,
        crate::manifest_volume_registry::Command::RecordVolumes(vec![target.clone()]),
    )
    .unwrap();
    target.retention = Retention::Warm;
    crate::manifest_volume_registry::execute(
        &mut registry,
        crate::manifest_volume_registry::Command::RecordVolumes(vec![target]),
    )
    .unwrap();
    assert_eq!(
        registry.resources(0, 256).unwrap().items[0].retention,
        Retention::Pinned
    );
}
