//! The default build-output volume under a stack's source mount (#666).

use super::*;

/// Name of the synthesized volume.
pub(crate) const DEFAULT_TARGET_VOLUME: &str = "target";

/// Return `stack` with a `scope = "stack"`, exclusive volume named `target`
/// at `<source destination>/target`, where the source mount is the bind of
/// the workspace root (`source = "."`). It recycles cargo's default output
/// directory across runs and hides the host's own `target/` from the
/// container.
///
/// Nothing is added when the stack opts out (`default_target_volume =
/// false`), has no source mount, already declares a volume named `target`,
/// or already mounts something at that destination. A read-only source mount
/// gets the volume only when `target/` already exists on the host: Docker
/// cannot create the mount point inside a read-only bind.
pub(crate) fn with_default_target_volume(
    stack: &bosn_core::manifest::Stack,
    workspace: &Path,
    mounts: &[bosn_core::WorkspaceMount],
) -> bosn_core::manifest::Stack {
    let mut stack = stack.clone();
    if !stack.default_target_volume
        || stack
            .volumes
            .iter()
            .any(|volume| volume.name == DEFAULT_TARGET_VOLUME)
    {
        return stack;
    }
    let Some(source) = mounts.iter().find(|mount| mount.source == ".") else {
        return stack;
    };
    let host_target_is_dir = || {
        fs::context_path_metadata_no_follow(&workspace.join(DEFAULT_TARGET_VOLUME))
            .is_ok_and(|metadata| metadata.kind == fs::ContextPathKind::Directory)
    };
    if source.readonly && !host_target_is_dir() {
        return stack;
    }
    let destination = if source.target == "/" {
        format!("/{DEFAULT_TARGET_VOLUME}")
    } else {
        format!("{}/{DEFAULT_TARGET_VOLUME}", source.target)
    };
    let taken = stack
        .volumes
        .iter()
        .map(bosn_core::manifest::Volume::mount_at)
        .chain(mounts.iter().map(|mount| mount.target.clone()))
        .chain(stack.tmpfs.iter().map(|tmpfs| tmpfs.destination.clone()))
        .any(|existing| existing == destination);
    if taken {
        return stack;
    }
    stack.volumes.push(bosn_core::manifest::Volume {
        name: DEFAULT_TARGET_VOLUME.into(),
        scope: Scope::Stack,
        destination: Some(destination),
        retention: Retention::Warm,
        mode: bosn_core::manifest::VolumeMode::Exclusive,
    });
    stack
}
