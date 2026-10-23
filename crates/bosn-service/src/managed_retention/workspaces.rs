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

use bosn_core::retention::EagerFacts;
use bosn_core::{LABEL_SCOPE, LABEL_WORKSPACE, ObservedArtifact, ResourceKind};

/// `.git` worktree pointers and `commondir` files are a single short line.
const POINTER_LIMIT: u64 = 4096;

pub(super) fn observe(artifacts: &[ObservedArtifact]) -> EagerFacts {
    let workspaces: BTreeSet<&str> = artifacts
        .iter()
        .filter(|artifact| artifact.kind == ResourceKind::Volume)
        .filter(|artifact| {
            matches!(
                artifact.labels.get(LABEL_SCOPE).map(String::as_str),
                Some("stack" | "spec")
            )
        })
        .filter_map(|artifact| artifact.labels.get(LABEL_WORKSPACE).map(String::as_str))
        .filter(|workspace| Path::new(workspace).is_absolute())
        .collect();
    let mut facts = EagerFacts::default();
    for workspace in workspaces {
        match std::fs::symlink_metadata(workspace) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                facts.gone_workspaces.insert(workspace.to_owned());
            }
            Ok(_) => {
                if let Some(family) = common_dir(Path::new(workspace)) {
                    facts
                        .families
                        .insert(workspace.to_owned(), family.to_string_lossy().into_owned());
                }
            }
            Err(_) => {}
        }
    }
    facts
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
        let facts = observe(&[volume(&plain), volume(&gone)]);
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
}
