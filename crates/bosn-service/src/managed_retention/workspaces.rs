//! The workspace observations eager retention needs (#659), read off the filesystem once per
//! pass for each workspace a stack or spec volume names.
//!
//! A workspace is *gone* only when the path is proven absent (`NotFound`); an unreadable path
//! proves nothing and stays held. A present workspace joins a repository family when a `.git`
//! directory or worktree file is found at it or above it; the family is the canonical git
//! common directory, so every worktree of one clone shares it. Nothing is guessed: a workspace
//! outside any repository has no family and is only subject to the gone rule and the age gate.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static ABSENT: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

use bosn_core::retention::EagerFacts;
use bosn_core::{LABEL_SCOPE, LABEL_WORKSPACE, ObservedArtifact, ResourceKind};

/// `.git` worktree pointers and `commondir` files are a single short line.
const POINTER_LIMIT: u64 = 4096;

pub(super) fn observe(artifacts: &[ObservedArtifact]) -> EagerFacts {
    let Ok(mut absent) = ABSENT.get_or_init(Mutex::default).lock() else {
        return EagerFacts::default();
    };
    observe_with(artifacts, &mut absent)
}

fn observe_with(artifacts: &[ObservedArtifact], absent: &mut BTreeSet<String>) -> EagerFacts {
    let workspaces: BTreeSet<&str> = artifacts
        .iter()
        .filter(|artifact| matches!(artifact.kind, ResourceKind::Volume | ResourceKind::Image))
        .filter(|artifact| {
            artifact.kind == ResourceKind::Image
                || matches!(
                    artifact.labels.get(LABEL_SCOPE).map(String::as_str),
                    Some("stack" | "spec")
                )
        })
        .filter_map(|artifact| artifact.labels.get(LABEL_WORKSPACE).map(String::as_str))
        .filter(|workspace| Path::new(workspace).is_absolute())
        .collect();
    let mut next_absent = BTreeSet::new();
    let mut facts = EagerFacts::default();
    for workspace in workspaces {
        match std::fs::symlink_metadata(workspace) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if Path::new(workspace).parent().is_some_and(Path::is_dir) {
                    if absent.contains(workspace) {
                        facts.gone_workspaces.insert(workspace.to_owned());
                    }
                    next_absent.insert(workspace.to_owned());
                }
            }
            Ok(_) => {
                if clean_checkout(Path::new(workspace)) {
                    facts.clean_workspaces.insert(workspace.to_owned());
                }
                if let Some(family) = common_dir(Path::new(workspace)) {
                    facts
                        .families
                        .insert(workspace.to_owned(), family.to_string_lossy().into_owned());
                }
            }
            Err(_) => {}
        }
    }
    *absent = next_absent;
    facts
}

/// Uncertainty, detached HEAD, and a missing upstream all keep the workspace.
fn clean_checkout(directory: &Path) -> bool {
    clean_with(|args| super::merged_branches::command("git", directory, args))
}

fn clean_with(mut git: impl FnMut(&[&str]) -> std::io::Result<String>) -> bool {
    let attached = git(&["symbolic-ref", "--quiet", "HEAD"]).is_ok_and(|branch| !branch.is_empty());
    attached
        && git(&[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])
        .is_ok_and(|status| status.is_empty())
        && git(&["rev-list", "--count", "@{upstream}..HEAD"]).is_ok_and(|count| count == "0")
}

/// Repeat workspace safety immediately before an eager removal without advancing observations.
pub(super) fn recheck(
    artifact: &ObservedArtifact,
    reason: bosn_core::retention::EagerReason,
) -> bool {
    use bosn_core::retention::EagerReason;
    let Some(workspace) = artifact.labels.get(LABEL_WORKSPACE) else {
        return false;
    };
    let path = Path::new(workspace);
    match reason {
        EagerReason::WorkspaceGone => {
            path.symlink_metadata()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                && path.parent().is_some_and(Path::is_dir)
                && ABSENT
                    .get()
                    .and_then(|state| state.lock().ok())
                    .is_some_and(|state| state.contains(workspace))
        }
        EagerReason::BranchMerged => {
            clean_checkout(path) && super::merged_branches::still_merged(path)
        }
        EagerReason::FamilyCap => clean_checkout(path),
    }
}

/// The canonical git common directory of the repository containing `workspace`, if any.
pub(super) fn common_dir(workspace: &Path) -> Option<PathBuf> {
    let dot = workspace
        .ancestors()
        .map(|directory| directory.join(".git"))
        .find(|dot| dot.symlink_metadata().is_ok())?;
    if dot.is_dir() {
        return dot.canonicalize().ok();
    }
    // A worktree: `.git` is a file naming its private git directory, whose `commondir` names
    // the clone's shared one (both may be relative).
    let base = dot.parent()?;
    let gitdir = base.join(pointer(&dot)?.strip_prefix("gitdir:")?.trim());
    let common = match pointer(&gitdir.join("commondir")) {
        Some(relative) => gitdir.join(relative.trim()),
        None => gitdir,
    };
    common.canonicalize().ok()
}

fn pointer(path: &Path) -> Option<String> {
    if path.metadata().ok()?.len() > POINTER_LIMIT {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn worktrees_and_their_clone_share_one_family() {
        let root = tempfile::tempdir().unwrap();
        let clone = root.path().join("clone");
        std::fs::create_dir_all(clone.join(".git/worktrees/wt")).unwrap();
        write(&clone.join(".git/worktrees/wt/commondir"), "../..\n");
        let worktree = root.path().join("wt");
        write(
            &worktree.join(".git"),
            &format!("gitdir: {}\n", clone.join(".git/worktrees/wt").display()),
        );
        std::fs::create_dir_all(clone.join("sub")).unwrap();
        let family = clone.join(".git").canonicalize().unwrap();
        assert_eq!(common_dir(&clone), Some(family.clone()));
        assert_eq!(common_dir(&clone.join("sub")), Some(family.clone()));
        assert_eq!(common_dir(&worktree), Some(family));
    }

    #[test]
    fn only_a_missing_workspace_is_gone_and_a_repoless_one_has_no_family() {
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let gone = root.path().join("deleted");
        let volume = |workspace: &Path| ObservedArtifact {
            id: workspace.display().to_string(),
            kind: ResourceKind::Volume,
            labels: [
                (LABEL_SCOPE.to_owned(), "stack".to_owned()),
                (LABEL_WORKSPACE.to_owned(), workspace.display().to_string()),
            ]
            .into(),
            signals: bosn_core::Signals {
                in_use: false,
                dangling: false,
                anonymous: false,
            },
            bytes: None,
            age_seconds: None,
        };
        let mut absent = BTreeSet::new();
        let artifacts = [volume(&plain), volume(&gone)];
        assert!(
            observe_with(&artifacts, &mut absent)
                .gone_workspaces
                .is_empty()
        );
        let facts = observe_with(&artifacts, &mut absent);
        assert_eq!(
            facts.gone_workspaces,
            BTreeSet::from([gone.display().to_string()])
        );
        // The temp dir may itself sit inside a checkout; only assert no family was invented
        // for a path whose ancestors hold no `.git`.
        if common_dir(&plain).is_none() {
            assert!(facts.families.is_empty());
        }
    }
    #[test]
    fn missing_parent_and_interrupted_absence_keep_work() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("unmounted/work");
        let item = ObservedArtifact {
            id: "v".into(),
            kind: ResourceKind::Volume,
            labels: [
                (LABEL_SCOPE.into(), "stack".into()),
                (LABEL_WORKSPACE.into(), missing.display().to_string()),
            ]
            .into(),
            signals: bosn_core::Signals {
                in_use: false,
                dangling: false,
                anonymous: false,
            },
            bytes: None,
            age_seconds: None,
        };
        let mut absent = BTreeSet::new();
        for _ in 0..2 {
            assert!(
                observe_with(std::slice::from_ref(&item), &mut absent)
                    .gone_workspaces
                    .is_empty()
            );
        }
        std::fs::create_dir(missing.parent().unwrap()).unwrap();
        assert!(
            observe_with(std::slice::from_ref(&item), &mut absent)
                .gone_workspaces
                .is_empty()
        );
        assert_eq!(
            observe_with(std::slice::from_ref(&item), &mut absent)
                .gone_workspaces
                .len(),
            1
        );
        std::fs::create_dir(&missing).unwrap();
        assert!(
            observe_with(std::slice::from_ref(&item), &mut absent)
                .gone_workspaces
                .is_empty()
        );
        assert!(absent.is_empty());
    }

    #[test]
    fn clean_checkout_requires_attached_clean_and_pushed() {
        for (attached, status, ahead, expected) in [
            (true, "", "0", true),
            (true, "?? new", "0", false),
            (true, " M tracked", "0", false),
            (true, "", "1", false),
            (false, "", "0", false),
            (true, "", "", false),
        ] {
            assert_eq!(
                clean_with(|args| match args[0] {
                    "symbolic-ref" if attached => Ok("refs/heads/work".into()),
                    "status" => {
                        assert!(args.contains(&"--ignore-submodules=none"));
                        Ok(status.into())
                    }
                    "rev-list" => Ok(ahead.into()),
                    _ => Err(std::io::Error::other("unknown")),
                }),
                expected
            );
        }
    }
}
