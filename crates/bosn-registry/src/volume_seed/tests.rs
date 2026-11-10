use super::*;
use crate::cache_helper::{
    CacheHelperIntent, CacheHelperRole, CacheHelperState, LATE_CREATE_WINDOW_SECS,
};
const OWNER: &str = "aabbccdd-0000-4000-8000-000000000327";
const NONCE: &str = "aabbccdd-0000-4000-8000-000000000328";
const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn seed() -> VolumeSeed {
    VolumeSeed {
        destination: "bosn-v-stack-destination".into(),
        source: "bosn-v-stack-source".into(),
        nonce: NONCE.into(),
        state: SeedState::Copying,
    }
}
fn helper() -> CacheHelperIntent {
    CacheHelperIntent {
        registry_id: OWNER.into(),
        nonce: NONCE.into(),
        volume: seed().destination,
        image: format!("docker.io/library/docker@sha256:{ID}"),
        created_at: 1.0,
        role: Some(CacheHelperRole::VolumeSeedV1 {
            source: seed().source,
            destination: seed().destination,
        }),
    }
}
#[test]
fn copying_protects_both_volumes_without_ttl_complete_releases_source() {
    let root = fs::TemporaryDirectory::new().unwrap();
    let mut registry =
        Registry::create_writer(root.path().join("registry.sqlite3"), OWNER).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_volume_seed(&seed(), &helper()).unwrap();
    tx.commit().unwrap();
    let row = registry.volume_seeds().unwrap().pop().unwrap();
    for _age in [0.0, 1.0, 1e15] {
        assert!(row.protects(&row.source));
        assert!(row.protects(&row.destination));
        assert!(!row.protects("bosn-v-stack-other"));
    }
    assert!(registry.leases(0, 10).unwrap().items.is_empty());
    let mut tx = registry.begin_immediate().unwrap();
    assert!(tx.complete_volume_seed(&row).is_err());
    tx.register_cache_helper(NONCE, ID, 2.0).unwrap();
    tx.finish_cache_helper(NONCE, ID, 3.0).unwrap();
    tx.complete_volume_seed(&row).unwrap();
    tx.commit().unwrap();
    let row = registry.volume_seeds().unwrap().pop().unwrap();
    assert_eq!(row.state, SeedState::Complete);
    assert!(!row.protects(&row.source));
    assert!(!row.protects(&row.destination));
    assert_eq!(
        registry.cache_helper(NONCE).unwrap().unwrap().state,
        CacheHelperState::Removed
    );
}
#[test]
fn restart_keeps_copying_until_late_create_absence_is_proven() {
    let root = fs::TemporaryDirectory::new().unwrap();
    let path = root.path().join("registry.sqlite3");
    let mut registry = Registry::create_writer(&path, OWNER).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_volume_seed(&seed(), &helper()).unwrap();
    tx.commit().unwrap();
    drop(registry);
    let mut registry = Registry::open_writer(&path).unwrap();
    assert_eq!(registry.volume_seeds().unwrap(), vec![seed()]);
    let mut tx = registry.begin_immediate().unwrap();
    assert!(
        tx.finish_absent_cache_helper(NONCE, LATE_CREATE_WINDOW_SECS)
            .is_err()
    );
    assert!(tx.delete_volume_seed(&seed().destination, NONCE).is_err());
    tx.finish_absent_cache_helper(NONCE, 1.0 + LATE_CREATE_WINDOW_SECS)
        .unwrap();
    tx.delete_volume_seed(&seed().destination, NONCE).unwrap();
    tx.commit().unwrap();
    assert!(registry.volume_seeds().unwrap().is_empty());
}
#[test]
fn seed_role_binds_safe_distinct_stack_volumes_and_nonce() {
    let mut intent = helper();
    intent.validate().unwrap();
    intent.volume = "bosn-v-stack-other".into();
    assert!(intent.validate().is_err());
    let mut same = seed();
    same.source = same.destination.clone();
    assert!(validate_names(&same.source, &same.destination).is_err());
}

#[test]
fn copying_keeps_settled_helper_proof_until_reset_even_under_metadata_retention() {
    let root = fs::TemporaryDirectory::new().unwrap();
    let mut registry =
        Registry::create_writer(root.path().join("registry.sqlite3"), OWNER).unwrap();
    let mut tx = registry.begin_immediate().unwrap();
    tx.begin_volume_seed(&seed(), &helper()).unwrap();
    tx.register_cache_helper(NONCE, ID, 2.0).unwrap();
    tx.finish_cache_helper(NONCE, ID, 3.0).unwrap();
    tx.commit().unwrap();
    for _ in 0..4 {
        registry.maintain_ci_state(1e9, 64).unwrap();
    }
    assert!(registry.cache_helper(NONCE).unwrap().is_some());
    let mut tx = registry.begin_immediate().unwrap();
    tx.complete_volume_seed(&seed()).unwrap();
    tx.commit().unwrap();
    for _ in 0..4 {
        registry.maintain_ci_state(1e9 + 1.0, 64).unwrap();
    }
    let mut tx = registry.begin_immediate().unwrap();
    tx.delete_volume_seed(&seed().destination, NONCE).unwrap();
    tx.commit().unwrap();
    assert!(registry.volume_seeds().unwrap().is_empty());
}
