//! Blocking registry operations for manifest volumes.
use crate::{ManifestVolumeResource, registry_records::put_manifest_volume_intents};

pub(crate) enum Command {
    PutIntents(Vec<ManifestVolumeResource>),
    AcquireSeed {
        source: Box<bosn_registry::Resource>,
        lease: bosn_registry::Lease,
    },
    ReleaseSeed(String),
}

pub(crate) fn execute(
    registry: &mut bosn_registry::Registry,
    command: Command,
) -> Result<bool, bosn_registry::Error> {
    match command {
        Command::AcquireSeed { source, lease } => acquire(registry, &source, &lease),
        Command::ReleaseSeed(id) => {
            let mut tx = registry.begin_immediate()?;
            tx.delete_lease(&id)?;
            tx.commit()?;
            Ok(true)
        }
        Command::PutIntents(volumes) => {
            put_manifest_volume_intents(registry, &volumes)?;
            Ok(true)
        }
    }
}

// The sole writer serializes this check-and-insert. Any recorded lease protects a
// source, without TTL expiry, exactly as retention requires (#777).
fn acquire(
    registry: &mut bosn_registry::Registry,
    source: &bosn_registry::Resource,
    lease: &bosn_registry::Lease,
) -> Result<bool, bosn_registry::Error> {
    use crate::managed_retention::registered::read_all;
    let bad_read = |_| bosn_registry::Error::BadRow("incomplete seed protection snapshot");
    if registry
        .resource_by_kind_name(bosn_core::ResourceKind::Volume, &source.name)?
        .as_ref()
        != Some(source)
        || read_all(|at| registry.leases(at, 256))
            .map_err(bad_read)?
            .iter()
            .any(|l| l.resource_id == source.id || l.resource_id == source.name)
        || read_all(|at| registry.execution_sessions(at, 256))
            .map_err(bad_read)?
            .iter()
            .any(|s| s.container_id == source.id || s.container_id == source.name)
        || read_all(|at| registry.volume_creation_intents(at, 256))
            .map_err(bad_read)?
            .iter()
            .any(|i| i.name == source.name)
    {
        return Ok(false);
    }
    let mut tx = registry.begin_immediate()?;
    tx.put_lease(lease)?;
    tx.commit()?;
    Ok(true)
}
