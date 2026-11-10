//! Effectful seed copy. A source is read-only, leased, and proven unmounted.
use super::observe::{OPTIONS, inspect, unmounted};
use crate::ManifestVolumeResource;
use bosn_engine::DockerEngine;
use bosn_registry::Resource;

// Existing pinned helper image, also used by Docker integration fixtures.
const IMAGE: &str =
    "alpine@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b";

fn action(engine: &DockerEngine, args: &[String]) -> Result<(), String> {
    let result = engine
        .with_args(args.iter().cloned())
        .capture(OPTIONS)
        .map_err(|e| e.to_string())?;
    result.require_complete().map_err(|e| e.to_string())?;
    if result.ok() {
        Ok(())
    } else {
        Err(format!("volume seed action exited {}", result.exit_code))
    }
}

fn create(
    engine: &DockerEngine,
    target: &ManifestVolumeResource,
    source: Option<&str>,
) -> Result<(), String> {
    let mut args = vec!["volume".into(), "create".into()];
    for (key, value) in &target.labels {
        args.extend(["--label".into(), format!("{key}={value}")]);
    }
    if let Some(source) = source {
        args.extend([
            "--label".into(),
            format!("{}.seeded-from={source}", bosn_core::NAMESPACE),
        ]);
    }
    args.push(target.name.clone());
    action(engine, &args)
}

pub(super) struct Outcome {
    pub result: Result<(), String>,
    pub helper_uncertain: bool,
}

pub(super) fn seed(
    engine: &DockerEngine,
    source: &Resource,
    target: &ManifestVolumeResource,
    state: &std::path::Path,
    budget: std::time::Duration,
) -> Outcome {
    let mut helper_uncertain = false;
    let result = seed_inner(engine, source, target, state, budget, &mut helper_uncertain);
    Outcome {
        result,
        helper_uncertain,
    }
}

fn seed_inner(
    engine: &DockerEngine,
    source: &Resource,
    target: &ManifestVolumeResource,
    state: &std::path::Path,
    budget: std::time::Duration,
    helper_uncertain: &mut bool,
) -> Result<(), String> {
    if inspect(engine, &target.name)?.is_some() {
        return Ok(());
    }
    let observed = inspect(engine, &source.name)?.ok_or("seed source disappeared")?;
    if observed
        .labels
        .get("com.zackees.bosn.setup-content-sha256")
        .map(String::as_str)
        != source.generation.strip_prefix("sha256:")
        || !unmounted(engine, &source.name)?
    {
        return Err("seed source changed or is mounted".into());
    }
    let ownership = crate::managed_retention::registered::RegisteredOwnership::load(state)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs_f64();
    if ownership
        .normalize(
            bosn_core::ResourceKind::Volume,
            &source.name,
            &observed.labels,
            now,
        )
        .is_none()
    {
        return Err("seed source ownership changed".into());
    }
    // Do not pull an image on the ensure path: an unavailable helper means cold.
    action(engine, &["image".into(), "inspect".into(), IMAGE.into()])?;
    let helper = format!("bosn-volume-seed-{}", target.name);
    let labels = helper_labels(state, target)?;
    let mut args = vec![
        "run".into(),
        "--name".into(),
        helper.clone(),
        bosn_engine::LOW_CPU_PRIORITY_ARG.into(),
        "--network=none".into(),
        "--mount".into(),
        format!(
            "type=volume,src={},dst=/source,readonly,volume-nocopy",
            source.name
        ),
        "--mount".into(),
        format!(
            "type=volume,src={},dst=/destination,volume-nocopy",
            target.name
        ),
        IMAGE.into(),
        "sh".into(),
        "-c".into(),
        "cp -a /source/. /destination/".into(),
    ];
    for (key, value) in labels.to_map() {
        args.splice(4..4, ["--label".into(), format!("{key}={value}")]);
    }
    // A full copy can be large, but every command remains bounded on the blocking lane.
    create(engine, target, Some(&source.name))?;
    *helper_uncertain = true;
    let options = bosn_engine::RunOptions::bounded(
        budget.min(std::time::Duration::from_secs(600)),
        1024 * 1024,
    );
    let result = engine
        .with_args(args)
        .capture(options)
        .map_err(|e| e.to_string())
        .and_then(|r| {
            r.require_complete().map_err(|e| e.to_string())?;
            if r.ok() {
                Ok(())
            } else {
                Err(format!("seed copy exited {}", r.exit_code))
            }
        });
    let cleaned = cleanup(engine, &helper, &labels.to_map());
    *helper_uncertain = cleaned.is_err();
    if result.is_err() {
        cleaned.as_ref().map_err(Clone::clone)?;
        // Throw away a partial copy, then recreate the ordinary cold volume.
        let observed =
            inspect(engine, &target.name)?.ok_or("partial seed destination disappeared")?;
        if target
            .labels
            .iter()
            .any(|(k, v)| observed.labels.get(k) != Some(v))
            || observed
                .labels
                .get(&format!("{}.seeded-from", bosn_core::NAMESPACE))
                != Some(&source.name)
            || !unmounted(engine, &target.name)?
        {
            return Err("partial seed destination could not be safely reset".into());
        }
        action(engine, &["volume".into(), "rm".into(), target.name.clone()])?;
        create(engine, target, None)?;
    }
    result.and(cleaned)
}

fn cleanup(
    engine: &DockerEngine,
    name: &str,
    expected: &std::collections::BTreeMap<&str, String>,
) -> Result<(), String> {
    let result = engine
        .with_args(["container", "inspect", name])
        .capture(OPTIONS)
        .map_err(|e| e.to_string())?;
    result.require_complete().map_err(|e| e.to_string())?;
    if result.reports_missing() {
        return Ok(());
    }
    if !result.ok() {
        return Err("seed helper cleanup inspection failed".into());
    }
    let inspected: Vec<HelperInspection> =
        serde_json::from_slice(&result.stdout).map_err(|e| e.to_string())?;
    let [helper] = inspected.as_slice() else {
        return Err("ambiguous seed helper inspection".into());
    };
    if helper.id.is_empty()
        || expected
            .iter()
            .any(|(k, v)| helper.config.labels.get(*k) != Some(v))
    {
        return Err("seed helper cleanup ownership mismatch".into());
    }
    action(
        engine,
        &[
            "container".into(),
            "rm".into(),
            "--force".into(),
            helper.id.clone(),
        ],
    )
}

fn helper_labels(
    state: &std::path::Path,
    target: &ManifestVolumeResource,
) -> Result<bosn_core::ResourceLabels, String> {
    let registry = bosn_registry::Registry::open_read_only(state.join("registry.sqlite3"))
        .map_err(|e| e.to_string())?
        .registry_id()
        .map_err(|e| e.to_string())?;
    bosn_core::ResourceLabels::new(
        &registry,
        bosn_core::ResourceKind::Container,
        &target.stack,
        &target.generation,
        bosn_core::Scope::Spec,
        &target.workspace,
        &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs_f64()
            .to_string(),
        Some(bosn_core::Retention::Warm),
    )
    .map_err(|e| format!("invalid seed helper labels: {e:?}"))
}

#[derive(serde::Deserialize)]
struct HelperInspection {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Config")]
    config: HelperConfig,
}
#[derive(serde::Deserialize)]
struct HelperConfig {
    #[serde(rename = "Labels")]
    labels: std::collections::BTreeMap<String, String>,
}
