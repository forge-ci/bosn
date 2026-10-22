//! Physical registry samples and incremental reclamation under the writer fence.
use super::*;

mod checkpoint;
#[cfg(test)]
mod tests;
pub use checkpoint::*;

pub(crate) const CI_METADATA_LIMIT_BYTES: u64 = 1 << 30;

pub(crate) struct StorageCheck {
    path: PathBuf,
    identity: Option<fs::FileIdentity>,
    limit: u64,
}

impl StorageCheck {
    pub(crate) fn admit(&self, transaction: &mut Transaction<'_>) -> Result<(), Error> {
        verify_database_identity(&self.path, self.identity)?;
        let usage = registry_storage_usage(&self.path);
        let allocated_bytes = usage.allocated_bytes.ok_or(Error::BadRow(
            "CI metadata allocation unavailable; new authority refused",
        ))?;
        verify_database_identity(&self.path, self.identity)?;
        // Include pages newly allocated inside this transaction even before
        // SQLite flushes them. This is a conservative projection, separately
        // named from the physical filesystem sample.
        let rows = transaction.query(
            "SELECT page_count*page_size FROM pragma_page_count,pragma_page_size",
            &[],
            QueryLimits {
                max_rows: 1,
                max_bytes: 128,
            },
        )?;
        let database_bytes = rows
            .first()
            .ok_or(Error::BadRow("CI metadata page count"))?;
        let database_bytes = u64::try_from(integer(database_bytes, 0)?)
            .map_err(|_| Error::BadRow("CI metadata page count"))?;
        // A checkpoint needs database pages alongside the still-present WAL;
        // include sidecar extents, even before their allocation is flushed.
        let projected_bytes = usage
            .files
            .iter()
            .filter(|file| file.kind != RegistryFileKind::Database)
            .try_fold(database_bytes, |total, file| total.checked_add(file.bytes?))
            .ok_or(Error::BadRow("CI metadata projection unavailable"))?;
        if allocated_bytes >= self.limit || projected_bytes >= self.limit {
            return Err(Error::CiMetadataStoragePressure {
                allocated_bytes,
                database_bytes,
                projected_bytes,
                limit_bytes: self.limit,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryFileKind {
    Database,
    Wal,
    SharedMemory,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RegistryFileStorage {
    pub kind: RegistryFileKind,
    pub bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
}

/// A bounded, non-atomic sample of three known files, never a directory walk.
/// Unknown files make totals unknown rather than contributing zero.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RegistryStorageUsage {
    pub files: Vec<RegistryFileStorage>,
    pub bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    /// The platform facade may fall back to logical length on Windows.
    pub allocation_may_use_logical_fallback: bool,
    pub partial: bool,
}

fn sample(path: &Path, required: bool) -> std::io::Result<Option<(fs::FileIdentity, u64, u64)>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.is_symlink() {
        return Err(std::io::Error::other(
            "registry storage is not a regular file",
        ));
    }
    let identity = fs::path_identity(path)?
        .ok_or_else(|| std::io::Error::other("file identity unavailable"))?;
    let allocated = fs::allocated_bytes(path, &metadata);
    let after = std::fs::symlink_metadata(path)?;
    if fs::path_identity(path)? != Some(identity) || !after.is_file() || after.is_symlink() {
        return Err(std::io::Error::other(
            "registry storage changed during sample",
        ));
    }
    // A checkpoint can run concurrently. Use the larger observations for a
    // conservative interval sample rather than rejecting benign size changes.
    Ok(Some((
        identity,
        metadata.len().max(after.len()),
        allocated.max(fs::allocated_bytes(path, &after)),
    )))
}

/// Includes database, WAL and shared-memory files, with inode deduplication.
/// Does not open SQLite, checkpoint, migrate or read any file contents.
pub fn registry_storage_usage(path: &Path) -> RegistryStorageUsage {
    let mut report = RegistryStorageUsage {
        files: Vec::with_capacity(3),
        bytes: Some(0),
        allocated_bytes: Some(0),
        allocation_may_use_logical_fallback: cfg!(windows),
        partial: false,
    };
    let mut seen = BTreeSet::new();
    for (kind, suffix) in [
        (RegistryFileKind::Database, ""),
        (RegistryFileKind::Wal, "-wal"),
        (RegistryFileKind::SharedMemory, "-shm"),
    ] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let sampled = sample(Path::new(&name), suffix.is_empty());
        let (bytes, allocated_bytes) = match sampled {
            Ok(Some((identity, bytes, allocated))) => {
                let unique = seen.insert((identity.device, identity.file));
                (
                    Some(if unique { bytes } else { 0 }),
                    Some(if unique { allocated } else { 0 }),
                )
            }
            Ok(None) => (Some(0), Some(0)),
            Err(_) => (None, None),
        };
        report.bytes = report.bytes.zip(bytes).and_then(|(a, b)| a.checked_add(b));
        report.allocated_bytes = report
            .allocated_bytes
            .zip(allocated_bytes)
            .and_then(|(a, b)| a.checked_add(b));
        report.files.push(RegistryFileStorage {
            kind,
            bytes,
            allocated_bytes,
        });
    }
    report.partial = report.bytes.is_none() || report.allocated_bytes.is_none();
    report
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryStorageMaintenance {
    pub checkpoint_blocked: bool,
    /// WAL exceeds the actor's 4 MiB checkpoint-I/O budget.
    pub checkpoint_deferred: bool,
    pub free_pages: u64,
    pub page_size: u64,
    /// Older databases need an explicit offline rebuild to enable incremental vacuum.
    pub incremental_vacuum: bool,
    pub usage: RegistryStorageUsage,
}

impl Registry {
    /// Lower this writer's CI admission budget. The default and maximum are
    /// 1 GiB; existing recovery stays admitted. This override is not persisted.
    pub fn set_ci_metadata_limit_bytes(&mut self, bytes: u64) -> Result<(), Error> {
        if !(64 * 1024..=CI_METADATA_LIMIT_BYTES).contains(&bytes) {
            return Err(Error::BadRow("CI metadata budget bounds"));
        }
        self.ci_metadata_limit_bytes = bytes;
        Ok(())
    }

    pub(crate) fn storage_check(&self) -> StorageCheck {
        StorageCheck {
            path: self.storage_path.clone(),
            identity: self.storage_identity,
            limit: self.ci_metadata_limit_bytes,
        }
    }

    pub fn storage_usage(&self) -> Result<RegistryStorageUsage, Error> {
        verify_database_identity(&self.storage_path, self.storage_identity)?;
        let report = registry_storage_usage(&self.storage_path);
        verify_database_identity(&self.storage_path, self.storage_identity)?;
        Ok(report)
    }

    fn storage_pragma(&self, sql: &str) -> Result<u64, Error> {
        let rows = self.connection.query(
            sql,
            &[],
            QueryLimits {
                max_rows: 1,
                max_bytes: 128,
            },
        )?;
        let value = rows
            .first()
            .ok_or(Error::BadRow("registry storage pragma"))?;
        u64::try_from(integer(value, 0)?).map_err(|_| Error::BadRow("registry storage pragma"))
    }

    /// Reclaim at most 64 free database pages and try WAL truncation without
    /// waiting for readers. Never removes SQLite's sidecar files manually.
    pub fn reclaim_storage(&mut self) -> Result<RegistryStorageMaintenance, Error> {
        self.require_ci_ready()?;
        verify_database_identity(&self.storage_path, self.storage_identity)?;
        self.connection.query(
            "PRAGMA busy_timeout=0",
            &[],
            QueryLimits {
                max_rows: 1,
                max_bytes: 128,
            },
        )?;
        let result = self.reclaim_storage_now();
        // Restore the ordinary writer policy even when reclamation fails.
        self.connection.query(
            "PRAGMA busy_timeout=5000",
            &[],
            QueryLimits {
                max_rows: 1,
                max_bytes: 128,
            },
        )?;
        result
    }

    fn reclaim_storage_now(&mut self) -> Result<RegistryStorageMaintenance, Error> {
        let incremental_vacuum = self.storage_pragma("PRAGMA auto_vacuum")? == 2;
        if incremental_vacuum {
            self.connection.query(
                "PRAGMA incremental_vacuum(64)",
                &[],
                QueryLimits {
                    max_rows: 64,
                    max_bytes: 4096,
                },
            )?;
        }
        let wal_bytes = self
            .storage_usage()?
            .files
            .into_iter()
            .find(|file| file.kind == RegistryFileKind::Wal)
            .and_then(|file| file.bytes)
            .ok_or(Error::BadRow("registry WAL allocation unavailable"))?;
        // This connection is the sole application writer. No other admitted
        // write can grow the WAL between this sample and the checkpoint.
        let checkpoint_deferred = wal_bytes > 4 * 1024 * 1024;
        let checkpoint_blocked = if checkpoint_deferred {
            false
        } else {
            self.connection.checkpoint()?.busy
        };
        Ok(RegistryStorageMaintenance {
            checkpoint_blocked,
            checkpoint_deferred,
            free_pages: self.storage_pragma("PRAGMA freelist_count")?,
            page_size: self.storage_pragma("PRAGMA page_size")?,
            incremental_vacuum,
            usage: self.storage_usage()?,
        })
    }
}
