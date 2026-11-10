//! Completion gate: never allow a workload to see an uncommitted seed.
use super::{SeedContext, helper, observe};
use crate::{
    ManifestVolumeResource,
    manifest_volume_registry::{Command, Reply, Snapshot},
};
use bosn_registry::volume_seed::{SeedState, VolumeSeed};
use std::fmt;

#[derive(Debug)]
pub(crate) enum SeedError {
    SeedDestinationUncertain { destination: String, detail: String },
    Operation(String),
}
impl fmt::Display for SeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SeedDestinationUncertain {
                destination,
                detail,
            } => write!(f, "SeedDestinationUncertain: {destination}: {detail}"),
            Self::Operation(detail) => f.write_str(detail),
        }
    }
}
impl From<String> for SeedError {
    fn from(s: String) -> Self {
        Self::Operation(s)
    }
}
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Decision {
    Use,
    Reset,
}
pub(super) fn decision(
    seed: Option<&VolumeSeed>,
    labels: &std::collections::BTreeMap<String, String>,
) -> Result<Decision, SeedError> {
    match seed {
        Some(seed) if seed.state == SeedState::Copying => Ok(Decision::Reset),
        Some(seed)
            if labels.get(&nonce_label()) == Some(&seed.nonce)
                && labels.get(&source_label()) == Some(&seed.source) =>
        {
            Ok(Decision::Use)
        }
        Some(seed) => Err(uncertain(
            &seed.destination,
            "completed seed labels do not match its registry record",
        )),
        None if labels.contains_key(&nonce_label()) || labels.contains_key(&source_label()) => {
            Ok(Decision::Reset)
        }
        None => Ok(Decision::Use),
    }
}
pub(super) fn source_label() -> String {
    bosn_core::LABEL_SEEDED_FROM.into()
}
pub(super) fn nonce_label() -> String {
    bosn_core::LABEL_SEED_NONCE.into()
}
pub(super) fn uncertain(destination: &str, detail: impl Into<String>) -> SeedError {
    SeedError::SeedDestinationUncertain {
        destination: destination.into(),
        detail: detail.into(),
    }
}
pub(super) async fn op(context: &SeedContext<'_>, command: Command) -> Result<Reply, SeedError> {
    context
        .registry
        .manifest_volume_command(command)
        .await
        .map_err(|e| SeedError::Operation(e.to_string()))
}
pub(super) async fn snapshot(context: &SeedContext<'_>) -> Result<Snapshot, SeedError> {
    match op(context, Command::Snapshot).await? {
        Reply::Snapshot(snapshot) => Ok(snapshot),
        _ => Err("seed snapshot reply mismatch".to_string().into()),
    }
}
pub(super) fn verify_owner(
    destination: &str,
    labels: &std::collections::BTreeMap<String, String>,
    owner: &str,
) -> Result<(), SeedError> {
    if [
        bosn_core::LABEL_SEEDED_FROM,
        bosn_core::LABEL_SEED_NONCE,
        bosn_core::LABEL_SEED_REGISTRY,
    ]
    .iter()
    .any(|key| labels.contains_key(*key))
        && labels
            .get(bosn_core::LABEL_SEED_REGISTRY)
            .map(String::as_str)
            != Some(owner)
    {
        return Err(uncertain(
            destination,
            "seed belongs to another or unknown registry",
        ));
    }
    Ok(())
}
// Returns true after recovery/reset, suppressing another seed during the same ensure.
pub(super) async fn ensure_safe(
    context: &SeedContext<'_>,
    target: &ManifestVolumeResource,
) -> Result<bool, SeedError> {
    let snapshot = snapshot(context).await?;
    let seed = snapshot
        .seeds
        .into_iter()
        .find(|s| s.destination == target.name);
    let engine = context.engine.clone();
    let name = target.name.clone();
    let observed =
        kernal_api::async_engine::launch_blocking(move || observe::inspect(&engine, &name))
            .await
            .map_err(|e| SeedError::Operation(e.to_string()))??;
    if let Some(volume) = &observed {
        verify_owner(&target.name, &volume.labels, &snapshot.owner)?;
    }
    if let Some(seed) = &seed {
        if seed.state == SeedState::Copying {
            settle(context, seed)
                .await
                .map_err(|e| uncertain(&target.name, e.to_string()))?;
            reset(context, target, Some(seed)).await?;
            op(context, Command::Delete(seed.clone())).await?;
            recreate_cold(context, target).await?;
            return Ok(true);
        }
        if observed.is_none() {
            op(context, Command::Delete(seed.clone())).await?;
            return Ok(false);
        }
    }
    let Some(observed) = observed else {
        return Ok(false);
    };
    if decision(seed.as_ref(), &observed.labels)? == Decision::Reset {
        // An orphan nonce can still have a live helper: settle it before touching bytes.
        if let Some(nonce) = observed.labels.get(&nonce_label()) {
            if let Reply::Helper(Some(record)) = op(context, Command::Helper(nonce.clone())).await?
            {
                helper::settle(context, &record)
                    .await
                    .map_err(|e| uncertain(&target.name, e))?;
            }
        }
        reset(context, target, None).await?;
        recreate_cold(context, target).await?;
        return Ok(true);
    }
    Ok(false)
}
pub(super) async fn settle(context: &SeedContext<'_>, seed: &VolumeSeed) -> Result<(), SeedError> {
    match op(context, Command::Helper(seed.nonce.clone())).await? {
        Reply::Helper(Some(record)) => helper::settle(context, &record)
            .await
            .map_err(SeedError::Operation),
        _ => Err(uncertain(
            &seed.destination,
            "durable helper intent missing",
        )),
    }
}
pub(super) async fn reset(
    context: &SeedContext<'_>,
    target: &ManifestVolumeResource,
    seed: Option<&VolumeSeed>,
) -> Result<(), SeedError> {
    let engine = context.engine.clone();
    let target = target.clone();
    let seed = seed.cloned();
    let name = target.name.clone();
    kernal_api::async_engine::launch_blocking(move || {
        if let Some(observed) = observe::inspect(&engine, &target.name)? {
            if target
                .labels
                .iter()
                .any(|(k, v)| observed.labels.get(k) != Some(v))
                || seed.as_ref().is_some_and(|s| {
                    observed.labels.get(&nonce_label()) != Some(&s.nonce)
                        || observed.labels.get(&source_label()) != Some(&s.source)
                })
            {
                return Err("destination ownership or seed nonce changed".into());
            }
            if !observe::unmounted(&engine, &target.name)? {
                return Err("destination is attached to a container".into());
            }
            helper::action_blocking(
                &engine,
                &["volume".into(), "rm".into(), target.name.clone()],
            )?;
            if observe::inspect(&engine, &target.name)?.is_some() {
                return Err("destination absence unproven".into());
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| uncertain(&name, e.to_string()))?
    .map_err(|e| uncertain(&name, e))
}

// Recreate only after Copying was deleted. A crash before that delete leaves
// absence (replayable), never an unlabelled volume that looks like a nonce swap.
pub(super) async fn recreate_cold(
    context: &SeedContext<'_>,
    target: &ManifestVolumeResource,
) -> Result<(), SeedError> {
    let engine = context.engine.clone();
    let target = target.clone();
    let name = target.name.clone();
    kernal_api::async_engine::launch_blocking(move || helper::create_volume(&engine, &target, None))
        .await
        .map_err(|e| uncertain(&name, e.to_string()))?
        .map_err(|e| uncertain(&name, e))
}

/// Registry authority travels in the private setup receipt so its final Docker
/// inspection also refuses a volume swapped after the earlier completion gate.
pub(crate) async fn authorize_plan(
    state: Option<&std::path::Path>,
    plan: &mut bosn_setup::SetupPlan,
) -> Result<(), String> {
    let Some(state) = state else {
        return Ok(());
    };
    let path = state.join("registry.sqlite3");
    let authority = kernal_api::async_engine::launch_blocking(move || {
        if !path.try_exists().map_err(|e| e.to_string())? {
            return Ok(None);
        }
        let registry = bosn_registry::Registry::open_read_only(&path).map_err(|e| e.to_string())?;
        Ok::<_, String>(Some((
            registry.registry_id().map_err(|e| e.to_string())?,
            registry.volume_seeds().map_err(|e| e.to_string())?,
        )))
    })
    .await
    .map_err(|e| e.to_string())??;
    let Some((owner, seeds)) = authority else {
        return Ok(());
    };
    for volume in &mut plan.named_volumes {
        for key in [
            bosn_core::LABEL_SEEDED_FROM,
            bosn_core::LABEL_SEED_NONCE,
            bosn_core::LABEL_SEED_REGISTRY,
        ] {
            volume.labels.remove(key);
        }
        if let Some(seed) = seeds
            .iter()
            .find(|s| s.destination == volume.name && s.state == SeedState::Complete)
        {
            volume.labels.extend([
                (bosn_core::LABEL_SEEDED_FROM.into(), seed.source.clone()),
                (bosn_core::LABEL_SEED_NONCE.into(), seed.nonce.clone()),
                (bosn_core::LABEL_SEED_REGISTRY.into(), owner.clone()),
            ]);
        }
    }
    Ok(())
}
