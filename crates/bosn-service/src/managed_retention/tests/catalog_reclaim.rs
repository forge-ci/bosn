use super::*;
use crate::managed_retention::{authority, catalog};

const GONE: &str = "aaaaaaaa-2222-4333-8444-555555555555";
const LIVE: &str = "bbbbbbbb-2222-4333-8444-555555555555";

fn peers(root: &std::path::Path, parent: &std::path::Path) -> std::path::PathBuf {
    let gone = parent.join("gone");
    let live = parent.join("live");
    for (dir, id) in [(&gone, GONE), (&live, LIVE)] {
        std::fs::create_dir_all(dir).unwrap();
        catalog::enroll(dir, id).unwrap();
    }
    std::fs::write(live.join("registry.sqlite3"), "").unwrap();
    std::fs::write(root.join(format!("{GONE}.abandoned-since")), "0").unwrap();
    gone
}

fn engine(parent: &std::path::Path, recreate: bool) -> DockerEngine {
    const SCRIPT: &str = r#"
case "$1" in
  ps|image) exit 0 ;;
  system) printf '%s' '{"Images":[],"Containers":[],"Volumes":[],"BuildCache":[]}'; exit 0 ;;
  volume)
    case "$2" in
      ls) case "$*" in *label=*) printf 'gone\nlive\n' ;; esac ;;
      inspect)
        if [ -f "$FD_PARENT/seen" ]; then
          case "$3" in gone) printf '%s' "$FD_GONE" ;; live) printf '%s' "$FD_LIVE" ;; esac
        else
          : > "$FD_PARENT/seen"
          if [ "$FD_RECREATE" = yes ]; then : > "$FD_PARENT/gone/registry.sqlite3"; fi
          printf '[%s,%s]' "$FD_GONE_OBJECT" "$FD_LIVE_OBJECT"
        fi ;;
      rm) printf '%s\n' "$3" >> "$FD_PARENT/removed" ;;
    esac ;;
esac
"#;
    let gone = owned_volume_inspect("gone").replace(REGISTRY_ID, GONE);
    let live = owned_volume_inspect("live").replace(REGISTRY_ID, LIVE);
    DockerEngine::synthetic_for_test("/bin/sh", ["-c", SCRIPT, "fake-docker"])
        .env("FD_PARENT", parent.as_os_str())
        .env("FD_RECREATE", if recreate { "yes" } else { "no" })
        .env("FD_GONE_OBJECT", &gone[1..gone.len() - 1])
        .env("FD_LIVE_OBJECT", &live[1..live.len() - 1])
        .env("FD_GONE", gone)
        .env("FD_LIVE", live)
}

#[test]
fn a_non_machine_pass_reclaims_abandoned_but_never_live_peer_volumes() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let root = temporary.path().join("catalog");
    let state = state_dir_with_registry("non-machine-reclaim");
    catalog::with_test_root(&root, || {
        peers(&root, temporary.path());
        let engine = engine(temporary.path(), false);
        let outcome = managed_retention_pass(&engine, &state, RetentionPolicy::default(), true);
        assert_eq!(outcome.summary.refused, None);
        assert_eq!(outcome.summary.removed, 1, "{:?}", outcome.summary);
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("removed")).unwrap(),
            "gone\n"
        );
        // The outcome keeps the reclaim lease even after empty-entry cleanup returned.
        let blocked = authority::Authority::load(&state, true);
        assert!(blocked.abandoned().is_empty());
        assert!(blocked.reclaim_guard.is_none());
        drop(outcome);
        assert!(
            authority::Authority::load(&state, true)
                .reclaim_guard
                .is_some()
        );
    });
}

#[test]
fn a_database_recreated_after_planning_protects_its_volume() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let root = temporary.path().join("catalog");
    let state = state_dir_with_registry("recreated-peer-db");
    catalog::with_test_root(&root, || {
        peers(&root, temporary.path());
        let engine = engine(temporary.path(), true);
        let outcome = managed_retention_pass(&engine, &state, RetentionPolicy::default(), true);
        assert_eq!(outcome.summary.planned, 1, "{:?}", outcome.summary);
        assert_eq!(outcome.summary.removed, 0, "{:?}", outcome.summary);
        assert_eq!(outcome.summary.failed, 1, "{:?}", outcome.summary);
        assert!(!temporary.path().join("removed").exists());
    });
}
