//! Exclusive volume leases use the same nonblocking kernel file-lock mechanism as cache leases.
use crate::ManifestVolumeResource;
use kernal_api::{async_engine, platform::fs};
use std::path::Path;

pub(crate) fn try_lock(state: &Path, name: &str) -> Result<Option<fs::OwnedFileLock>, String> {
    let directory = state.to_owned();
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let file =
        fs::open_lock_file(&directory.join(format!("{name}.lock"))).map_err(|e| e.to_string())?;
    match fs::try_lock_exclusive_owned(file) {
        Ok(lock) => Ok(Some(lock)),
        Err(error) if fs::is_lock_conflict(&error) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) async fn destinations(
    _state: &Path,
    volumes: &[ManifestVolumeResource],
    deadline: &async_engine::Deadline,
    cancellation: &async_engine::CancellationToken,
) -> Result<Vec<fs::OwnedFileLock>, String> {
    let mut names: Vec<_> = volumes
        .iter()
        .filter(|v| v.scope == bosn_core::Scope::Stack)
        .map(|v| v.name.clone())
        .collect();
    names.sort();
    names.dedup();
    let mut locks = Vec::new();
    for name in names {
        loop {
            if deadline.remaining().is_zero() || cancellation.is_cancelled() {
                return Err("manifest volume lease wait cancelled or exceeded deadline".into());
            }
            let name = name.clone();
            if let Some(lock) = async_engine::launch_blocking(move || {
                try_lock(&crate::runners::cache_lock_dir(), &name)
            })
            .await
            .map_err(|e| e.to_string())??
            {
                locks.push(lock);
                break;
            }
            async_engine::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    Ok(locks)
}
