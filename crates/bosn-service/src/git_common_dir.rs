//! Shared repository-family observation. Call from a blocking worker.

use std::path::{Path, PathBuf};

/// `.git` worktree pointers and `commondir` files are a single short line.
const POINTER_LIMIT: u64 = 4096;

/// The canonical git common directory of the repository containing `workspace`, if any.
pub(crate) fn common_dir(workspace: &Path) -> Option<PathBuf> {
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
