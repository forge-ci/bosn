//! Single-writer commands for durable manifest volume seeds and creation intents.
use crate::managed_retention::registered::read_all;
use crate::{ManifestVolumeResource, registry_records::put_manifest_volume_intents};
use bosn_registry::cache_helper::{CacheHelperIntent, CacheHelperRecord};
use bosn_registry::volume_seed::VolumeSeed;
use bosn_registry::{Registry, Resource, VolumeCreationIntent};

pub(crate) enum Command {
    PutIntents(Vec<ManifestVolumeResource>),
    Begin {
        source: Box<Resource>,
        seed: VolumeSeed,
        helper: Box<CacheHelperIntent>,
    },
    Complete(VolumeSeed),
    Delete(VolumeSeed),
    Snapshot,
    Helper(String),
}
pub(crate) struct Snapshot {
    pub owner: String,
    pub seeds: Vec<VolumeSeed>,
    pub intents: Vec<VolumeCreationIntent>,
}
pub(crate) enum Reply {
    Committed,
    Busy,
    Snapshot(Snapshot),
    Helper(Option<Box<CacheHelperRecord>>),
}
pub(crate) fn execute(
    registry: &mut Registry,
    command: Command,
) -> Result<Reply, bosn_registry::Error> {
    match command {
        Command::PutIntents(volumes) => put_manifest_volume_intents(registry, &volumes)?,
        Command::Snapshot => {
            return Ok(Reply::Snapshot(Snapshot {
                owner: registry.registry_id()?,
                seeds: registry.volume_seeds()?,
                intents: read_all(|at| registry.volume_creation_intents(at, 256))
                    .map_err(|_| bosn_registry::Error::BadRow("seed intent snapshot"))?,
            }));
        }
        Command::Helper(nonce) => {
            return Ok(Reply::Helper(registry.cache_helper(&nonce)?.map(Box::new)));
        }
        Command::Begin {
            source,
            seed,
            helper,
        } => {
            if !source_available(registry, &source)? {
                return Ok(Reply::Busy);
            }
            let mut tx = registry.begin_immediate()?;
            tx.begin_volume_seed(&seed, &helper)?;
            tx.commit()?;
        }
        Command::Complete(seed) => {
            let mut tx = registry.begin_immediate()?;
            tx.complete_volume_seed(&seed)?;
            tx.commit()?;
        }
        Command::Delete(seed) => {
            let mut tx = registry.begin_immediate()?;
            tx.delete_volume_seed(&seed.destination, &seed.nonce)?;
            tx.commit()?;
        }
    }
    Ok(Reply::Committed)
}
fn source_available(registry: &Registry, source: &Resource) -> Result<bool, bosn_registry::Error> {
    let incomplete = |_| bosn_registry::Error::BadRow("incomplete seed protection snapshot");
    Ok(registry
        .resource_by_kind_name(bosn_core::ResourceKind::Volume, &source.name)?
        .as_ref()
        == Some(source)
        && !registry
            .volume_seeds()?
            .iter()
            .any(|s| s.protects(&source.name))
        && !read_all(|at| registry.leases(at, 256))
            .map_err(incomplete)?
            .iter()
            .any(|l| l.resource_id == source.id || l.resource_id == source.name)
        && !read_all(|at| registry.execution_sessions(at, 256))
            .map_err(incomplete)?
            .iter()
            .any(|s| s.container_id == source.id || s.container_id == source.name)
        && !read_all(|at| registry.volume_creation_intents(at, 256))
            .map_err(incomplete)?
            .iter()
            .any(|i| i.name == source.name))
}
