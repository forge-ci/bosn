//! Independent WAL work. Its connection closes before its writer lease drops.
use super::*;

pub struct RegistryCheckpointer {
    connection: Connection,
    _writer: std::sync::Arc<fs::OwnedFileLock>,
    path: PathBuf,
    identity: Option<fs::FileIdentity>,
}

#[derive(Debug)]
pub struct RegistryCheckpointReport {
    pub blocked: bool,
    pub remaining_frames: u64,
    pub usage: RegistryStorageUsage,
}

impl Registry {
    /// Take the sole checkpoint worker. Run it on a separate blocking worker;
    /// it cannot submit business mutations and retains this owner's lease.
    pub fn take_storage_checkpointer(&mut self) -> Option<RegistryCheckpointer> {
        self.checkpoint_connection
            .take()
            .map(|connection| RegistryCheckpointer {
                connection,
                _writer: std::sync::Arc::clone(&self._writer),
                path: self.storage_path.clone(),
                identity: self.storage_identity,
            })
    }
}

impl RegistryCheckpointer {
    /// Copies outstanding WAL off the registry actor. Lock waits are zero;
    /// copy/sync work is not bounded, and callers must await it at shutdown.
    pub fn run_pass(&mut self) -> Result<RegistryCheckpointReport, Error> {
        verify_database_identity(&self.path, self.identity)?;
        // PASSIVE permits normal writers during a large copy. Truncation is
        // attempted only after this pass has caught up with its observed WAL.
        let rows = self.connection.query(
            "PRAGMA wal_checkpoint(PASSIVE)",
            &[],
            QueryLimits {
                max_rows: 1,
                max_bytes: 256,
            },
        )?;
        let row = rows
            .first()
            .ok_or(Error::BadRow("passive checkpoint result"))?;
        let busy = integer(row, 0)?;
        let frames = integer(row, 1)?;
        let completed = integer(row, 2)?;
        if busy < 0 || frames < 0 || completed < 0 || completed > frames {
            return Err(Error::BadRow("passive checkpoint result"));
        }
        let remaining_frames = (frames - completed) as u64;
        let blocked = if busy != 0 || remaining_frames != 0 {
            true
        } else {
            self.connection.checkpoint()?.busy
        };
        verify_database_identity(&self.path, self.identity)?;
        Ok(RegistryCheckpointReport {
            blocked,
            remaining_frames,
            usage: registry_storage_usage(&self.path),
        })
    }
}
