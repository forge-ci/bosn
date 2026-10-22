//! Intent-derived private storage, distinct from the pinned machine cache.
use super::*;
use bosn_core::{ResourceKind, ResourceLabels, Retention, Scope};
use bosn_engine::DockerEngine;
use bosn_registry::act::ActEngineRecord;
use serde::Deserialize;

pub(super) fn labels(
    intent: &ActEngineIntent,
    owner: &str,
) -> Result<BTreeMap<String, String>, ActEngineError> {
    // A native source can be unmounted while a recovering reader still has
    // authority to open it. Generic pressure/age GC lacks that authority's
    // producer fence, so only the dedicated lifecycle may remove this source.
    let retention = if intent
        .creation_profile
        .as_ref()
        .is_some_and(|profile| profile.tool_generation.is_some())
    {
        Retention::Pinned
    } else {
        Retention::Warm
    };
    Ok(ResourceLabels::new(
        owner,
        ResourceKind::Volume,
        "act-storage",
        &intent.run_id,
        Scope::Spec,
        &intent.run_id,
        &intent.created_at.to_string(),
        Some(retention),
    )
    .map_err(|_| ActEngineError("invalid private storage labels".into()))?
    .to_map()
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct DockerVolume {
    pub(super) name: String,
    pub(super) driver: String,
    pub(super) scope: String,
    pub(super) options: Option<BTreeMap<String, String>>,
    pub(super) labels: Option<BTreeMap<String, String>>,
    pub(super) mountpoint: Option<String>,
}

/// The one predicate that decides whether an observed volume is the private
/// local volume a bosn intent owns. Ambiguous, absent or unparsable output is
/// always a refusal; the expected name comes from the caller, never from
/// Docker, so a rename can never substitute a different volume.
pub(super) fn verify_owned_local_volume(
    document: &[u8],
    expected_name: &str,
    expected_labels: &BTreeMap<String, String>,
) -> Result<(), ActEngineError> {
    owned_local_volume(document, expected_name, expected_labels).map(|_| ())
}

pub(super) fn owned_local_volume(
    document: &[u8],
    expected_name: &str,
    expected_labels: &BTreeMap<String, String>,
) -> Result<DockerVolume, ActEngineError> {
    let mut volumes: Vec<DockerVolume> =
        serde_json::from_slice(document).map_err(|e| ActEngineError(e.to_string()))?;
    if volumes.len() != 1 {
        return Err(ActEngineError("ambiguous private storage volume".into()));
    }
    let volume = volumes
        .pop()
        .ok_or_else(|| ActEngineError("missing private storage volume".into()))?;
    if volume.name != expected_name
        || volume.driver != "local"
        || volume.scope != "local"
        || volume
            .options
            .as_ref()
            .is_some_and(|options| !options.is_empty())
        || volume.labels.as_ref() != Some(expected_labels)
    {
        return Err(ActEngineError(
            "private storage volume ownership does not match".into(),
        ));
    }
    Ok(volume)
}

fn verify(document: &[u8], intent: &ActEngineIntent, owner: &str) -> Result<(), ActEngineError> {
    let expected = intent
        .storage_volume_name()
        .ok_or_else(|| ActEngineError("private storage volume ownership does not match".into()))?;
    verify_owned_local_volume(document, &expected, &labels(intent, owner)?)
}

async fn present(engine: &DockerEngine, name: &str) -> Result<bool, ActEngineError> {
    let bytes = storage_control(
        engine,
        vec![
            "volume".into(),
            "ls".into(),
            "--filter".into(),
            format!("name={name}"),
            "--format".into(),
            "{{.Name}}".into(),
        ],
    )
    .await?;
    let output = std::str::from_utf8(&bytes)
        .map_err(|_| ActEngineError("invalid storage volume list encoding".into()))?;
    let names: Vec<_> = output.split_whitespace().collect();
    match names.as_slice() {
        [] => Ok(false),
        [found] if *found == name => Ok(true),
        _ => Err(ActEngineError(
            "ambiguous private storage volume list".into(),
        )),
    }
}

pub(crate) async fn ensure_storage_volume(
    engine: &DockerEngine,
    intent: &ActEngineIntent,
    owner: &str,
) -> Result<(), ActEngineError> {
    let Some(name) = intent.storage_volume_name() else {
        return Ok(());
    };
    if !present(engine, &name).await? {
        let mut args = vec![
            "volume".into(),
            "create".into(),
            "--driver".into(),
            "local".into(),
        ];
        for (key, value) in labels(intent, owner)? {
            args.extend(["--label".into(), format!("{key}={value}")]);
        }
        args.push(name.clone());
        let created = storage_control(engine, args).await?;
        if std::str::from_utf8(&created).ok().map(str::trim) != Some(name.as_str()) {
            return Err(ActEngineError(
                "storage creation returned an unexpected identity".into(),
            ));
        }
    }
    let document = storage_control(engine, vec!["volume".into(), "inspect".into(), name]).await?;
    verify(&document, intent, owner)
}

/// Remove a retiring engine's private storage, unless native tool recovery
/// still needs it.
///
/// A record carrying a live unreleased recovery intent keeps its source volume.
/// Verified publication and reference release must both be committed before
/// early deletion; expiry permits attempting abandonment. Neither expiry nor
/// a registry timestamp replaces producer exclusion. Exact ownership and
/// Docker absence are required on either path.
pub(crate) async fn remove_storage_volume(
    registry: &RegistryActor,
    engine: &DockerEngine,
    record: &ActEngineRecord,
    owner: &str,
) -> Result<f64, ActEngineError> {
    if super::source_retained_by_recovery(record) {
        return Err(ActEngineError(
            "native tool recovery retains the private source volume".into(),
        ));
    }
    let mut disposal_at = record.updated_at;
    if record.tool_recovery.is_some() {
        disposal_at = crate::act_runtime::now().max(record.updated_at);
        match registry
            .act_registry(ActRegistryCommand::ToolRecoverySourceDisposal {
                run: record.intent.run_id.clone(),
                at: disposal_at,
            })
            .await
            .map_err(|e| ActEngineError(format!("recovery source disposal: {e}")))?
        {
            ActRegistryReply::Committed => (),
            _ => {
                return Err(ActEngineError(
                    "recovery source disposal reply differs".into(),
                ));
            }
        }
    }
    if record.tool_recovery.is_some() {
        crate::ci::engine::DockerActBackend::new(engine.clone())
            .fence_tool_source_with_helper(registry, &record.intent.run_id, owner)
            .await
            .map_err(|e| ActEngineError(format!("producer source disposal: {e}")))?;
    }
    remove_private_storage(engine, &record.intent, owner).await?;
    Ok(disposal_at)
}

/// Exact named storage only. Attached volumes are refused by Docker; removal
/// is never forced. A successful exact listing is required before receipt.
/// Module-private so [`remove_storage_volume`] stays the only reachable path.
async fn remove_private_storage(
    engine: &DockerEngine,
    intent: &ActEngineIntent,
    owner: &str,
) -> Result<(), ActEngineError> {
    let Some(name) = intent.storage_volume_name() else {
        return Ok(());
    };
    if !present(engine, &name).await? {
        return Ok(());
    }
    let document = storage_control(
        engine,
        vec!["volume".into(), "inspect".into(), name.clone()],
    )
    .await?;
    verify(&document, intent, owner)?;
    super::create::docker_control_budget(
        engine,
        vec!["volume".into(), "rm".into(), name.clone()],
        super::budgets::DELETE,
    )
    .await?;
    if present(engine, &name).await? {
        return Err(ActEngineError(
            "private storage absence is not established".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::act_engine::tests::{OWNER, disk_intent};
    #[test]
    fn disk_pressure_cannot_reclaim_a_native_recovery_source() {
        use bosn_core::retention::{RetentionPolicy, plan_managed};
        let mut intent = disk_intent();
        intent.creation_profile.as_mut().unwrap().tool_generation =
            Some(bosn_registry::act::ActToolGenerationBinding {
                id: "a".repeat(64),
                max_payload_bytes: 1 << 30,
                overlay_recipe_sha256: crate::ci::engine::tool_overlay_recipe_sha256(),
            });
        let mut artifact = bosn_core::ObservedArtifact {
            id: intent.storage_volume_name().unwrap(),
            kind: ResourceKind::Volume,
            labels: labels(&intent, OWNER).unwrap(),
            signals: bosn_core::Signals::default(),
            bytes: Some(1 << 30),
            age_seconds: Some(7200.0),
        };
        let plan = plan_managed(
            &[artifact.clone()],
            Some(OWNER),
            RetentionPolicy::default().under_disk_pressure(),
        );
        assert!(
            plan.candidates.is_empty(),
            "pressure selected a protected native source"
        );
        intent.creation_profile.as_mut().unwrap().tool_generation = None;
        artifact.labels = labels(&intent, OWNER).unwrap();
        let legacy = plan_managed(
            &[artifact],
            Some(OWNER),
            RetentionPolicy::default().under_disk_pressure(),
        );
        assert_eq!(
            legacy.candidates.len(),
            1,
            "ordinary private storage still expires"
        );
    }
    #[test]
    fn private_storage_rejects_foreign_retention_and_options() {
        let intent = disk_intent();
        let doc = serde_json::json!([{"Name":intent.storage_volume_name().unwrap(),"Driver":"local","Scope":"local","Options":null,"Labels":labels(&intent,OWNER).unwrap()}]);
        assert!(verify(&serde_json::to_vec(&doc).unwrap(), &intent, OWNER).is_ok());
        for field in ["name", "registry", "retention"] {
            let mut changed = doc.clone();
            match field {
                "name" => changed[0]["Name"] = serde_json::json!("bosn-ci-cache-v1"),
                "registry" => {
                    changed[0]["Labels"][bosn_core::LABEL_REGISTRY] =
                        serde_json::json!("00000000-0000-4000-8000-000000000000")
                }
                _ => changed[0]["Labels"][bosn_core::LABEL_RETENTION] = serde_json::json!("pinned"),
            }
            assert!(verify(&serde_json::to_vec(&changed).unwrap(), &intent, OWNER).is_err());
        }
        let mut changed = doc;
        changed[0]["Options"] = serde_json::json!({"type":"none","device":"/home","o":"bind"});
        assert!(verify(&serde_json::to_vec(&changed).unwrap(), &intent, OWNER).is_err());
    }
}

#[cfg(all(test, unix))]
mod transport_tests {
    use super::*;
    use crate::act_engine::tests::{OWNER, disk_intent};
    use kernal_api::{async_engine::RuntimeBuilder, platform::fs::TemporaryDirectory};
    const SCRIPT: &str = r#"import pathlib, sys, time
artifact, state, log, name, mode = sys.argv[1:6]
mode, _, delay = mode.partition(':')
state, log = pathlib.Path(state), pathlib.Path(log)
args = sys.argv[6:]
if args[:2] == ['volume', 'ls']:
    if state.exists(): print(name)
elif args[:2] == ['volume', 'inspect']:
    if mode == 'unreadable': sys.exit(1)
    print(pathlib.Path(artifact).read_text())
elif args[:2] == ['volume', 'rm']:
    log.write_text(args[2])
    if mode == 'slow-remove': time.sleep(float(delay))
    if mode == 'attached': sys.exit(1)
    if mode != 'persists': state.rename(state.with_name('removed'))
    if mode == 'lost_ack': sys.exit(1)
    print(name)
else: sys.exit(2)
"#;
    #[test]
    fn removal_errors_and_uncertain_absence_remain_retryable() {
        for mode in [
            "success",
            "slow-remove",
            "attached",
            "unreadable",
            "lost_ack",
            "persists",
        ] {
            let dir = TemporaryDirectory::new().unwrap();
            let intent = disk_intent();
            let name = intent.storage_volume_name().unwrap();
            let artifact = dir.path().join("inspect.json");
            let state = dir.path().join("present");
            let log = dir.path().join("removed-name");
            let script = dir.path().join("docker.py");
            std::fs::write(&script, SCRIPT).unwrap();
            // Outlasts one storage control call, well inside the delete budget.
            let delay = super::super::budgets::STORAGE_CONTROL + std::time::Duration::from_secs(2);
            assert!(delay < super::super::budgets::DELETE);
            let fixture_mode = format!("{mode}:{}", delay.as_secs_f64());
            std::fs::write(&state, "present").unwrap();
            let document = serde_json::json!([{"Name":name,"Driver":"local","Scope":"local","Options":null,"Labels":labels(&intent,OWNER).unwrap()}]);
            std::fs::write(&artifact, serde_json::to_vec(&document).unwrap()).unwrap();
            let engine = DockerEngine::synthetic_for_test(
                "python3",
                [
                    script.to_string_lossy().into_owned(),
                    artifact.to_string_lossy().into_owned(),
                    state.to_string_lossy().into_owned(),
                    log.to_string_lossy().into_owned(),
                    name.clone(),
                    fixture_mode.clone(),
                ],
            );
            RuntimeBuilder::multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
                .run(async {
                    let result = remove_private_storage(&engine, &intent, OWNER).await;
                    assert_eq!(
                        result.is_ok(),
                        matches!(mode, "success" | "slow-remove"),
                        "{mode}"
                    );
                    if mode == "lost_ack" {
                        assert!(!state.exists());
                        assert!(
                            remove_private_storage(&engine, &intent, OWNER)
                                .await
                                .is_ok(),
                            "a successful later list may establish absence"
                        );
                    }
                    if matches!(mode, "attached" | "persists" | "unreadable") {
                        assert!(state.exists());
                    }
                    if mode == "unreadable" {
                        assert!(!log.exists(), "failed inspect must not authorize deletion");
                    } else {
                        assert_eq!(
                            std::fs::read_to_string(&log).unwrap(),
                            name,
                            "only the exact private volume can be removed"
                        );
                    }
                });
        }
    }
}

async fn storage_control(
    engine: &DockerEngine,
    args: Vec<String>,
) -> Result<Vec<u8>, ActEngineError> {
    super::create::docker_control_budget(engine, args, super::budgets::STORAGE_CONTROL).await
}
