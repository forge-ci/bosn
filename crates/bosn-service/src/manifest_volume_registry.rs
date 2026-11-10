//! Blocking registry operations for manifest volumes.
use crate::{ManifestVolumeResource, registry_records::put_manifest_volume_intents};

pub(crate) enum Command {
    PutIntents(Vec<ManifestVolumeResource>),
}

pub(crate) fn execute(
    registry: &mut bosn_registry::Registry,
    command: Command,
) -> Result<bool, bosn_registry::Error> {
    match command {
        Command::PutIntents(volumes) => {
            put_manifest_volume_intents(registry, &volumes)?;
            Ok(true)
        }
    }
}
