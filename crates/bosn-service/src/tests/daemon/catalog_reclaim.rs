use super::*;
use crate::managed_retention::catalog;

const GONE: &str = "aaaaaaaa-2222-4333-8444-555555555555";
const LIVE: &str = "bbbbbbbb-2222-4333-8444-555555555555";
const UNKNOWN: &str = "cccccccc-2222-4333-8444-555555555555";

fn catalog_peers(root: &Path, parent: &Path) {
    catalog::with_test_root(root, || {
        for (name, id) in [("gone", GONE), ("live", LIVE)] {
            let dir = parent.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            catalog::enroll(&dir, id).unwrap();
            if id == LIVE {
                std::fs::write(dir.join("registry.sqlite3"), "").unwrap();
            }
        }
    });
}

#[test]
fn abandoned_and_live_catalog_peers_do_not_block_startup() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let root = temporary.path().join("catalog");
    catalog_peers(&root, temporary.path());
    let state = temporary.path().join("new");
    let prior = PriorIdentity {
        objects: vec![
            prior_volume("abandoned", GONE, 1),
            prior_volume("peer", LIVE, 1),
        ],
        unreadable: Vec::new(),
    };
    catalog::with_test_root(&root, || {
        RuntimeBuilder::current_thread()
            .enable_all()
            .build()
            .unwrap()
            .run(async {
                let server = async_engine::launch(
                    Service::new(state.clone())
                        .with_identity_probe(Arc::new(FixedIdentityProbe(prior)))
                        .serve(),
                );
                let client = wait_for_client(&state).await;
                assert!(!client.status().await.unwrap().registry_id.is_empty());
                client.shutdown().await.unwrap();
                stopped(server).await;
            });
    });
}

#[test]
fn only_pre_catalog_objects_refuse_with_exact_recovery_lines() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let root = temporary.path().join("catalog");
    catalog_peers(&root, temporary.path());
    let state = temporary.path().join("new");
    let mut prior = PriorIdentity {
        objects: vec![
            prior_volume("unknown", UNKNOWN, 1),
            prior_volume("abandoned", GONE, 1),
            prior_volume("peer", LIVE, 1),
        ],
        unreadable: Vec::new(),
    };
    catalog::with_test_root(&root, || {
        let result = RuntimeBuilder::current_thread()
            .enable_all()
            .build()
            .unwrap()
            .run(
                Service::new(state.clone())
                    .with_identity_probe(Arc::new(FixedIdentityProbe(prior.clone())))
                    .serve(),
            );
        assert!(matches!(result, Err(Error::Protocol(_))));
        assert!(!state.join("registry.sqlite3").exists());
        let known = catalog::known_registries();
        prior
            .objects
            .retain(|object| !known.contains(&object.registry_id));
        let message = prior_registry_refusal(&prior, &state.join("registry.sqlite3"));
        let lines: Vec<_> = message
            .lines()
            .filter(|line| line.starts_with("If registry"))
            .collect();
        assert_eq!(
            lines,
            [format!(
                "If registry {UNKNOWN}'s state directory is gone (1 object(s) above): bosn gc release-registry {UNKNOWN} --yes"
            )]
        );
    });
}
