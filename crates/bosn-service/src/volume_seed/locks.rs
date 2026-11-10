//! Exclusive volume leases use the same nonblocking kernel file-lock mechanism as cache leases.
use crate::ManifestVolumeResource;
use kernal_api::{async_engine, platform::fs};
use std::path::Path;

pub(crate) fn try_lock(state: &Path, name: &str) -> Result<Option<fs::OwnedFileLock>, String> {
    let directory = state.to_owned();
    if directory == crate::runners::cache_lock_dir() {
        // Long-state IPC sockets share this parent with the machine's volume locks.
        let parent = directory
            .parent()
            .ok_or("volume lock directory has no parent")?;
        kernal_api::platform::ipc::ensure_owner_private_directory(parent)
            .map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let file =
        fs::open_lock_file(&directory.join(format!("{name}.lock"))).map_err(|e| e.to_string())?;
    match fs::try_lock_exclusive_owned(file) {
        Ok(lock) => Ok(Some(lock)),
        Err(error) if fs::is_lock_conflict(&error) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// A run owns these locks until its ephemeral container has been removed.
/// Kept containers hold no lease while idle.
#[derive(Default)]
pub(crate) struct VolumeLease {
    locks: std::collections::BTreeMap<String, fs::OwnedFileLock>,
}

impl VolumeLease {
    pub(crate) fn holds(&self, name: &str) -> bool {
        self.locks.contains_key(name)
    }
}

pub(crate) async fn destinations(
    _state: &Path,
    volumes: &[ManifestVolumeResource],
    modes: &std::collections::BTreeMap<String, bosn_core::manifest::VolumeMode>,
    deadline: &async_engine::Deadline,
    cancellation: &async_engine::CancellationToken,
    holder: &str,
) -> Result<VolumeLease, crate::ephemeral_task::TaskContainerError> {
    if deadline.remaining().is_zero() || cancellation.is_cancelled() {
        return Err("manifest volume lease cancelled or exceeded deadline".into());
    }
    let names: std::collections::BTreeSet<_> = volumes
        .iter()
        .filter(|v| modes.get(&v.name) == Some(&bosn_core::manifest::VolumeMode::Exclusive))
        .map(|v| v.name.clone())
        .collect();
    let holder = holder.to_owned();
    async_engine::launch_blocking(move || {
        let directory = crate::runners::cache_lock_dir();
        let mut lease = VolumeLease::default();
        for name in names {
            let lock = try_lock(&directory, &name)?.ok_or_else(|| {
                crate::ephemeral_task::TaskContainerError::VolumeBusy {
                    volume: name.clone(),
                    holder: std::fs::read_to_string(directory.join(format!("{name}.holder")))
                        .unwrap_or_else(|_| format!("exclusive lock {name}")),
                }
            })?;
            std::fs::write(directory.join(format!("{name}.holder")), &holder)
                .map_err(|e| e.to_string())?;
            lease.locks.insert(name, lock);
        }
        Ok(lease)
    })
    .await
    .map_err(|e| e.to_string())?
}
