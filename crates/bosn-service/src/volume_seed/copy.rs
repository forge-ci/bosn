//! Registry-first copy; only a settled helper can authorize Complete.
use super::{
    SeedContext,
    gate::{self, SeedError},
    helper, observe,
};
use crate::{
    ci::engine::{CacheHelperIdentity, CacheHelperTracker},
    manifest_volume_registry::{Command, Reply},
};
use bosn_registry::{
    Resource,
    cache_helper::{CacheHelperIntent, CacheHelperRole},
    volume_seed::{SeedState, VolumeSeed},
};
use kernal_api::async_engine::{self, CancellationToken, Deadline};
use std::time::Duration;

pub(super) async fn seed(
    context: &SeedContext<'_>,
    target: &crate::ManifestVolumeResource,
    source: Resource,
    budget: Duration,
    cancel: &CancellationToken,
) -> Result<(), SeedError> {
    let deadline = Deadline::after(budget);
    async_engine::timeout_at(
        deadline,
        seed_inner(context, target, source, budget, cancel),
    )
    .await
    .map_err(|_| {
        gate::uncertain(
            &target.name,
            "seed budget exhausted; durable recovery required",
        )
    })?
}

async fn seed_inner(
    context: &SeedContext<'_>,
    target: &crate::ManifestVolumeResource,
    source: Resource,
    budget: Duration,
    cancel: &CancellationToken,
) -> Result<(), SeedError> {
    let snapshot = gate::snapshot(context).await?;
    let nonce = crate::ci::new_uuid()
        .await
        .map_err(|e| SeedError::Operation(e.to_string()))?;
    let seed = VolumeSeed {
        destination: target.name.clone(),
        source: source.name.clone(),
        nonce: nonce.clone(),
        state: SeedState::Copying,
    };
    let intent = CacheHelperIntent {
        registry_id: snapshot.owner,
        nonce,
        volume: target.name.clone(),
        image: crate::ci::engine::engine_image(),
        created_at: crate::ci::lifecycle::now_seconds(),
        role: Some(CacheHelperRole::VolumeSeedV1 {
            source: source.name.clone(),
            destination: target.name.clone(),
        }),
    };
    // Missing local helper images are a cold create, before any durable attempt.
    let image = context
        .engine
        .with_args(["image", "inspect", &intent.image])
        .capture_async(bosn_engine::RunOptions::bounded(
            Duration::from_secs(15),
            1024 * 1024,
        ))
        .await;
    if !image.is_ok_and(|r| r.ok() && r.require_complete().is_ok()) {
        crate::daemon_log!(
            "bosn volume seed: {}: cold create: pinned helper image unavailable",
            target.name
        );
        return Ok(());
    }
    let identity = CacheHelperIdentity::from_intent(&intent).map_err(SeedError::Operation)?;
    if !matches!(
        gate::op(
            context,
            Command::Begin {
                source: Box::new(source),
                seed: seed.clone(),
                helper: Box::new(intent.clone())
            }
        )
        .await?,
        Reply::Committed
    ) {
        crate::daemon_log!(
            "bosn volume seed: {}: cold create: sibling became busy",
            target.name
        );
        return Ok(());
    }
    let deadline = Deadline::after(budget.saturating_sub(Duration::from_secs(30)));
    let outcome = run(
        context, target, &seed, &intent, &identity, &deadline, cancel,
    )
    .await;
    // Recovery verifies the full frozen profile, removes by immutable ID, and
    // never finishes an absent unacknowledged create inside its late-create window.
    gate::settle(context, &seed)
        .await
        .map_err(|e| gate::uncertain(&target.name, e.to_string()))?;
    match outcome {
        Ok(()) => {
            gate::op(context, Command::Complete(seed)).await?;
        }
        Err(error) => {
            gate::reset(context, target, Some(&seed)).await?;
            gate::op(context, Command::Delete(seed)).await?;
            gate::recreate_cold(context, target).await?;
            crate::daemon_log!("bosn volume seed: {}: cold create: {error}", target.name);
        }
    }
    Ok(())
}
async fn run(
    context: &SeedContext<'_>,
    target: &crate::ManifestVolumeResource,
    seed: &VolumeSeed,
    intent: &CacheHelperIntent,
    identity: &CacheHelperIdentity,
    deadline: &Deadline,
    cancel: &CancellationToken,
) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Err("seed cancelled before create".into());
    }
    let engine = context.engine.clone();
    let target_copy = target.clone();
    let owner = intent.registry_id.clone();
    let seed_copy = seed.clone();
    async_engine::launch_blocking(move || {
        if observe::inspect(&engine, &target_copy.name)?.is_some() {
            return Err("destination appeared before seed create".into());
        }
        if !observe::unmounted(&engine, &seed_copy.source)? {
            return Err("source is mounted".into());
        }
        helper::create_volume(&engine, &target_copy, Some((&seed_copy, &owner)))?;
        helper::prove_destination(&engine, &target_copy, &seed_copy, &owner)
    })
    .await
    .map_err(|e| e.to_string())??;
    let created = checked(
        context,
        helper::args(identity, intent, &seed.source),
        deadline,
        cancel,
    )
    .await?;
    let document = checked(
        context,
        vec!["container".into(), "inspect".into(), identity.name.clone()],
        deadline,
        cancel,
    )
    .await?;
    let id = identity.verify(&document, &intent.volume)?;
    if created.trim() != id {
        return Err("seed helper acknowledgement differs from verified ID".into());
    }
    let tracker = CacheHelperTracker::new(context.registry, &intent.nonce);
    tracker.register(&id).await?;
    let engine = context.engine.clone();
    let target = target.clone();
    let seed = seed.clone();
    let owner = intent.registry_id.clone();
    async_engine::launch_blocking(move || {
        helper::prove_destination(&engine, &target, &seed, &owner)
    })
    .await
    .map_err(|e| e.to_string())??;

    checked(
        context,
        vec!["start".into(), "--attach".into(), id],
        deadline,
        cancel,
    )
    .await?;
    Ok(())
}
async fn checked(
    context: &SeedContext<'_>,
    args: Vec<String>,
    deadline: &Deadline,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let options = bosn_engine::RunOptions::bounded(deadline.remaining(), 1024 * 1024);
    let result = async_engine::cancellable(
        cancel,
        context.engine.with_args(args).capture_async(options),
    )
    .await
    .map_err(|_| "seed cancelled".to_string())?
    .map_err(|e| e.to_string())?;
    result.require_complete().map_err(|e| e.to_string())?;
    if !result.ok() {
        return Err(format!(
            "seed helper action failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    String::from_utf8(result.stdout).map_err(|e| e.to_string())
}
