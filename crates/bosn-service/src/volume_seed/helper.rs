//! Adapter to the existing durable helper identity and pending-helper recovery.
use super::{SeedContext, observe::OPTIONS};
use crate::{
    ManifestVolumeResource,
    ci::engine::{CacheHelperIdentity, DockerActBackend},
};
use bosn_engine::DockerEngine;
use bosn_registry::cache_helper::{CacheHelperIntent, CacheHelperRecord, CacheHelperState};
use bosn_registry::volume_seed::VolumeSeed;

pub(super) fn action_blocking(engine: &DockerEngine, args: &[String]) -> Result<(), String> {
    let result = engine
        .with_args(args.iter().cloned())
        .capture(OPTIONS)
        .map_err(|e| e.to_string())?;
    result.require_complete().map_err(|e| e.to_string())?;
    if result.ok() {
        Ok(())
    } else {
        Err(format!(
            "seed Docker action failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ))
    }
}
pub(super) fn create_volume(
    engine: &DockerEngine,
    target: &ManifestVolumeResource,
    seed: Option<(&VolumeSeed, &str)>,
) -> Result<(), String> {
    let mut args = vec!["volume".into(), "create".into()];
    for (k, v) in &target.labels {
        args.extend(["--label".into(), format!("{k}={v}")]);
    }
    if let Some((seed, owner)) = seed {
        args.extend([
            "--label".into(),
            format!("{}={owner}", bosn_core::LABEL_SEED_REGISTRY),
            "--label".into(),
            format!("{}={}", super::gate::source_label(), seed.source),
            "--label".into(),
            format!("{}={}", super::gate::nonce_label(), seed.nonce),
        ]);
    }
    args.push(target.name.clone());
    action_blocking(engine, &args)
}
pub(super) fn args(
    identity: &CacheHelperIdentity,
    intent: &CacheHelperIntent,
    source: &str,
) -> Vec<String> {
    let mut args = vec![
        "create".into(),
        "--rm".into(),
        bosn_engine::LOW_CPU_PRIORITY_ARG.into(),
        "--name".into(),
        identity.name.clone(),
        "--label".into(),
        identity.label(),
        "--pull=never".into(),
        "--network=none".into(),
        "--read-only".into(),
        "--tmpfs".into(),
        format!(
            "/var/lib/docker:{}",
            bosn_registry::cache_helper::VOLUME_SEED_TMPFS
        ),
        "--init=false".into(),
        "--restart=no".into(),
        "--memory=512m".into(),
        "--cpus=2".into(),
        "--pids-limit=128".into(),
        "--mount".into(),
        format!("type=volume,source={source},target=/source,readonly,volume-nocopy"),
        "--mount".into(),
        format!(
            "type=volume,source={},target=/destination,volume-nocopy",
            intent.volume
        ),
        "--entrypoint".into(),
        "/bin/sh".into(),
        intent.image.clone(),
        "-ec".into(),
        bosn_registry::cache_helper::VOLUME_SEED_COMMAND.into(),
    ];
    args.splice(1..1, identity.ownership_args());
    args
}
pub(super) async fn settle(
    context: &SeedContext<'_>,
    record: &CacheHelperRecord,
) -> Result<(), String> {
    if record.state == CacheHelperState::Removed {
        return Ok(());
    }
    DockerActBackend::new(context.engine.clone())
        .reconcile_measurement(context.registry, record)
        .await
}

pub(super) fn destination_labels_match(
    target: &ManifestVolumeResource,
    seed: &VolumeSeed,
    owner: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> bool {
    target.labels.iter().all(|(k, v)| labels.get(k) == Some(v))
        && labels.get(bosn_core::LABEL_SEEDED_FROM) == Some(&seed.source)
        && labels.get(bosn_core::LABEL_SEED_NONCE) == Some(&seed.nonce)
        && labels
            .get(bosn_core::LABEL_SEED_REGISTRY)
            .map(String::as_str)
            == Some(owner)
}
pub(super) fn prove_destination(
    engine: &DockerEngine,
    target: &ManifestVolumeResource,
    seed: &VolumeSeed,
    owner: &str,
) -> Result<(), String> {
    let observed =
        super::observe::inspect(engine, &target.name)?.ok_or("seed destination disappeared")?;
    if !destination_labels_match(target, seed, owner, &observed.labels) {
        return Err("seed destination ownership or nonce changed".into());
    }
    Ok(())
}
