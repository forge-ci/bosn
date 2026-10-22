//! Machine-visible frozen source identity. A stopped metadata container survives
//! engine deletion and loss of the registry's owning daemon; it mounts no data.
use super::{ActEngineRecord, owned};
use crate::ci::machine::{EngineIdentity, Foreign};
use bosn_registry::act::ActToolSourceContext;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const NAMESPACE: &str = "com.zackees.bosn.act.source-context";
pub(super) const ENGINE: &str = "com.zackees.bosn.act.source-context-engine";
const RECORD: &str = "com.zackees.bosn.act.source-context-record";
const MAX_RECORD: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct SourceContext(pub(super) ActToolSourceContext);

impl std::ops::Deref for SourceContext {
    type Target = ActToolSourceContext;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SourceContext {
    pub(super) fn from_record(record: &ActEngineRecord) -> Result<Self, String> {
        ActToolSourceContext::from_record(record)
            .map(Self)
            .map_err(|e| e.to_string())
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        self.0.validate().map_err(|e| e.to_string())?;
        self.ownership()?;
        Ok(())
    }

    pub(super) fn name(&self) -> String {
        format!("bosn-act-source-{}", self.recovery.owner)
    }

    fn ownership(&self) -> Result<BTreeMap<String, String>, String> {
        Ok(bosn_core::ResourceLabels::new(
            &self.registry_id,
            bosn_core::ResourceKind::Container,
            "ci-source-context",
            &self.intent.run_id,
            bosn_core::Scope::Spec,
            "machine",
            &self.intent.created_at.to_string(),
            // Generic age/pressure GC must never discard recovery authority.
            // The dedicated source-disposal lifecycle removes this object only
            // after proving the source absent; this pin is not its expiry.
            Some(bosn_core::Retention::Pinned),
        )
        .map_err(|e| format!("source context ownership: {e:?}"))?
        .to_map()
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect())
    }

    fn labels(&self) -> Result<BTreeMap<String, String>, String> {
        self.validate()?;
        let encoded = serde_json::to_string(self).map_err(|e| e.to_string())?;
        if encoded.len() > MAX_RECORD {
            return Err("source context exceeds metadata bound".into());
        }
        let mut labels = self.ownership()?;
        labels.insert(NAMESPACE.into(), "v1".into());
        labels.insert(ENGINE.into(), self.recovery.engine_id.clone());
        labels.insert(RECORD.into(), encoded);
        Ok(labels)
    }

    pub(super) fn matches_engine(&self, engine: &Foreign) -> Result<(), String> {
        let profile = self
            .intent
            .creation_profile
            .as_ref()
            .ok_or("source context profile missing")?;
        if self.recovery.engine_id != engine.id
            || self.registry_id != engine.registry
            || self.intent.engine_name() != engine.name
            || engine.identity != EngineIdentity::of(&self.intent)
            || engine.storage_volume.as_deref() != Some(self.recovery.source_volume.as_str())
            || engine.created as f64 != self.intent.created_at.floor()
            || engine.memory_bytes != profile.memory_bytes
            || engine.nano_cpus != profile.nano_cpus
            || engine.pids != profile.pids
        {
            return Err("source context differs from observed engine".into());
        }
        Ok(())
    }

    pub(super) fn create_args(&self) -> Result<Vec<String>, String> {
        let mut args = owned(&[
            "create",
            "--name",
            &self.name(),
            "--pull",
            "never",
            bosn_engine::LOW_CPU_PRIORITY_ARG,
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--init=false",
            "--restart=no",
            "--tmpfs",
            "/var/lib/docker:ro,noexec,size=1m",
            "--entrypoint",
            "/bin/true",
        ]);
        for (key, value) in self.labels()? {
            args.extend(["--label".into(), format!("{key}={value}")]);
        }
        args.push(format!(
            "docker.io/library/docker@{}",
            self.intent.engine_image_digest
        ));
        Ok(args)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Container {
    id: String,
    name: String,
    state: State,
    config: Config,
    host_config: Host,
    mounts: Vec<serde::de::IgnoredAny>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct State {
    running: bool,
    status: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Config {
    image: String,
    labels: BTreeMap<String, String>,
    entrypoint: Vec<String>,
    cmd: Option<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Host {
    readonly_rootfs: bool,
    privileged: bool,
    network_mode: String,
    cap_drop: Vec<String>,
    tmpfs: BTreeMap<String, String>,
    auto_remove: bool,
    cpu_shares: u64,
    init: Option<bool>,
    restart_policy: Restart,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Restart {
    name: String,
    maximum_retry_count: u64,
}

pub(super) fn parse(document: &str) -> Result<(String, SourceContext), String> {
    if document.len() > 128 * 1024 {
        return Err("source context inspection exceeds bound".into());
    }
    let row: Container =
        serde_json::from_str(document).map_err(|e| format!("source context inspection: {e}"))?;
    let encoded = row
        .config
        .labels
        .get(RECORD)
        .filter(|text| text.len() <= MAX_RECORD)
        .ok_or("source context record missing or oversized")?;
    let context: SourceContext =
        serde_json::from_str(encoded).map_err(|e| format!("source context record: {e}"))?;
    context.validate()?;
    if row.config.labels != context.labels()?
        || row.name != format!("/{}", context.name())
        || row.id.len() != 64
        || !row
            .id
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        || row.state.running
        || row.state.status != "created"
        || row.config.image
            != format!(
                "docker.io/library/docker@{}",
                context.intent.engine_image_digest
            )
        || row.config.entrypoint != ["/bin/true"]
        || row.config.cmd.as_ref().is_some_and(|cmd| !cmd.is_empty())
        || !row.mounts.is_empty()
        || !row.host_config.readonly_rootfs
        || row.host_config.privileged
        || row.host_config.network_mode != "none"
        || row.host_config.cap_drop != ["ALL"]
        || row.host_config.tmpfs
            != BTreeMap::from([("/var/lib/docker".into(), "ro,noexec,size=1m".into())])
        || row.host_config.auto_remove
        || row.host_config.init != Some(false)
        || format!("--cpu-shares={}", row.host_config.cpu_shares)
            != bosn_engine::LOW_CPU_PRIORITY_ARG
        || row.host_config.restart_policy.name != "no"
        || row.host_config.restart_policy.maximum_retry_count != 0
    {
        return Err("source context identity or isolation differs".into());
    }
    Ok((row.id, context))
}

#[cfg(all(test, unix))]
#[path = "source_context_tests.rs"]
pub(in crate::ci::engine) mod tests;
