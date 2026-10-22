//! Durable intents for temporary cache accounting/maintenance containers.
//! Docker observation and absence proof belong to the trusted daemon, not a
//! client. Unacknowledged creates remain pending even when currently absent.

use super::*;
use serde::{Deserialize, Serialize};

mod recovery;

pub(crate) const PREFIX: &str = "ci.cache-helper.v1:";
pub(crate) const RECORD_BYTES: usize = 24_576;
pub const MAX_HELPER_PAGE: usize = 64;
/// Docker control deadline (30 s) plus ten minutes for a late create to settle.
pub const LATE_CREATE_WINDOW_SECS: f64 = 630.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CacheHelperRole {
    MaintenanceV1,
    /// Complete foreign source authority; the helper journal belongs to its
    /// executing daemon, independently of the source's original registry.
    ToolSourceFinalizationV1 {
        context: Box<act::ActToolSourceContext>,
        context_id: String,
        command_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_mountpoint: Option<String>,
    },
    ToolRecoveryV1 {
        run_id: String,
        intent: act::ActToolRecoveryIntent,
        /// Absent only in pre-transport recovery intents; never adopt their process.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_sha256: Option<String>,
        /// Frozen Docker-side local volume path; absent only in older helpers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_mountpoint: Option<String>,
    },
    ToolRecoveryReleaseV1 {
        run_id: String,
        intent: act::ActToolRecoveryIntent,
        command_sha256: String,
    },
    ToolRecoveryDisposalV1 {
        run_id: String,
        intent: act::ActToolRecoveryIntent,
        command_sha256: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryHelperPhase {
    Publish,
    Release,
    Dispose,
}

pub struct RecoveryHelperBinding<'a> {
    pub run_id: &'a str,
    pub intent: &'a act::ActToolRecoveryIntent,
    pub command_sha256: Option<&'a str>,
    pub phase: RecoveryHelperPhase,
    pub source_mountpoint: Option<&'a str>,
}

impl CacheHelperRole {
    pub fn recovery_binding(&self) -> Option<RecoveryHelperBinding<'_>> {
        match self {
            Self::MaintenanceV1 | Self::ToolSourceFinalizationV1 { .. } => None,
            Self::ToolRecoveryV1 {
                run_id,
                intent,
                command_sha256,
                source_mountpoint,
            } => Some(RecoveryHelperBinding {
                run_id,
                intent,
                command_sha256: command_sha256.as_deref(),
                phase: RecoveryHelperPhase::Publish,
                source_mountpoint: source_mountpoint.as_deref(),
            }),
            Self::ToolRecoveryReleaseV1 {
                run_id,
                intent,
                command_sha256,
            } => Some(RecoveryHelperBinding {
                run_id,
                intent,
                command_sha256: Some(command_sha256),
                phase: RecoveryHelperPhase::Release,
                source_mountpoint: None,
            }),
            Self::ToolRecoveryDisposalV1 {
                run_id,
                intent,
                command_sha256,
            } => Some(RecoveryHelperBinding {
                run_id,
                intent,
                command_sha256: Some(command_sha256),
                phase: RecoveryHelperPhase::Dispose,
                source_mountpoint: None,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheHelperIntent {
    pub registry_id: String,
    pub nonce: String,
    pub volume: String,
    pub image: String,
    pub created_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<CacheHelperRole>,
}

impl CacheHelperIntent {
    pub fn name(&self) -> String {
        let role = match &self.role {
            None => "measure",
            Some(CacheHelperRole::MaintenanceV1) => "maintain",
            Some(CacheHelperRole::ToolSourceFinalizationV1 { .. }) => "finalize-source",
            Some(CacheHelperRole::ToolRecoveryV1 { .. }) => "recover",
            Some(CacheHelperRole::ToolRecoveryReleaseV1 { .. }) => "release",
            Some(CacheHelperRole::ToolRecoveryDisposalV1 { .. }) => "dispose",
        };
        format!("bosn-cache-{role}-{}", self.nonce)
    }

    pub fn validate(&self) -> Result<(), Error> {
        let digest = self.image.strip_prefix("docker.io/library/docker@sha256:");
        if !is_uuid(&self.registry_id)
            || kind(&self.nonce).is_err()
            || !self.created_at.is_finite()
            || self.created_at < 0.0
            || digest.is_none_or(|digest| !hex_id(digest))
        {
            return Err(Error::BadRow("cache helper intent"));
        }
        if let Some(CacheHelperRole::ToolSourceFinalizationV1 {
            context,
            context_id,
            command_sha256,
            source_mountpoint,
        }) = &self.role
        {
            context.validate()?;
            if !hex_id(context_id)
                || !hex_id(command_sha256)
                || source_mountpoint
                    .as_ref()
                    .is_some_and(|path| !valid_recovery_source_mountpoint(path))
                || self.volume != context.recovery.cache_volume
                || self.created_at < context.intent.created_at
                || self.image
                    != format!(
                        "docker.io/library/docker@{}",
                        context.intent.engine_image_digest
                    )
            {
                return Err(Error::BadRow("source finalization helper identity"));
            }
        }
        if let Some(binding) = self
            .role
            .as_ref()
            .and_then(CacheHelperRole::recovery_binding)
        {
            let intent = binding.intent;
            if binding
                .source_mountpoint
                .is_some_and(|path| !valid_recovery_source_mountpoint(path))
                || binding.command_sha256.is_some_and(|hash| !hex_id(hash))
                || !is_uuid(binding.run_id)
                || intent.schema_version != 1
                || !hex_id(&intent.owner)
                || !hex_id(&intent.generation)
                || !hex_id(&intent.engine_id)
                || intent.cache_volume != self.volume
                || intent.source_volume == self.volume
                || intent.created_at_seconds < 0
                || intent.expires_at_seconds <= intent.created_at_seconds
                || intent.expires_at_seconds - intent.created_at_seconds > 86400
                || intent.expires_at_seconds > (1_i64 << 53)
            {
                return Err(Error::BadRow("recovery helper frozen source"));
            }
            act::ActEngineCacheVolume {
                name: intent.source_volume.clone(),
                target: "/source".into(),
            }
            .validate()?;
        }
        act::ActEngineCacheVolume {
            name: self.volume.clone(),
            target: "/cache".into(),
        }
        .validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheHelperState {
    Pending,
    Created,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheHelperRecord {
    pub schema_version: u32,
    pub intent: CacheHelperIntent,
    pub state: CacheHelperState,
    pub container_id: Option<String>,
    pub updated_at: f64,
}

#[derive(Debug)]
pub struct CacheHelperPage {
    pub items: Vec<CacheHelperRecord>,
    pub next_nonce: Option<String>,
}

fn kind(nonce: &str) -> Result<String, Error> {
    if !is_uuid(nonce) || nonce != nonce.to_ascii_lowercase() {
        return Err(Error::BadRow("cache helper nonce"));
    }
    Ok(format!("{PREFIX}{nonce}"))
}

fn hex_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn decode(detail: &str, nonce: &str, owner: &str) -> Result<CacheHelperRecord, Error> {
    let record: CacheHelperRecord =
        serde_json::from_str(detail).map_err(|_| Error::BadRow("cache helper snapshot JSON"))?;
    record.intent.validate()?;
    if record.schema_version != 1
        || record.intent.nonce != nonce
        || !record.updated_at.is_finite()
        || record.updated_at < record.intent.created_at
        || match record.state {
            CacheHelperState::Pending => record.container_id.is_some(),
            CacheHelperState::Created => record.container_id.is_none(),
            CacheHelperState::Removed => false,
        }
        || record.container_id.as_ref().is_some_and(|id| !hex_id(id))
    {
        return Err(Error::BadRow("cache helper snapshot"));
    }
    if record.intent.registry_id != owner {
        return Err(Error::ResourceIdentityConflict);
    }
    Ok(record)
}

impl Immediate<'_> {
    fn helper_owner(&mut self) -> Result<String, Error> {
        let rows = self.transaction.query(
            "SELECT value FROM meta WHERE key='registry_id'",
            &[],
            QueryLimits {
                max_rows: 1,
                max_bytes: 128,
            },
        )?;
        rows.first()
            .ok_or(Error::BadRow("registry_id"))
            .and_then(|row| text(row, 0))
    }

    pub(super) fn helper_record(
        &mut self,
        nonce: &str,
    ) -> Result<Option<CacheHelperRecord>, Error> {
        let rows = self.transaction.query(
            "SELECT detail FROM ci_current WHERE family=2 AND id=?",
            &[Value::Text(
                kind(nonce)?.strip_prefix(PREFIX).unwrap().into(),
            )],
            QueryLimits {
                max_rows: 1,
                max_bytes: RECORD_BYTES + 1024,
            },
        )?;
        let owner = self.helper_owner()?;
        rows.first()
            .map(|row| decode(&text(row, 0)?, nonce, &owner))
            .transpose()
    }

    fn store_helper(&mut self, record: &CacheHelperRecord) -> Result<(), Error> {
        self.write_helper(record, ci_state::WriteMode::Event)
    }

    fn write_helper(
        &mut self,
        record: &CacheHelperRecord,
        mode: ci_state::WriteMode,
    ) -> Result<(), Error> {
        let detail =
            serde_json::to_string(record).map_err(|_| Error::BadRow("cache helper JSON"))?;
        decode(&detail, &record.intent.nonce, &self.helper_owner()?)?;
        if detail.len() > RECORD_BYTES {
            return Err(Error::BadRow("cache helper snapshot size"));
        }
        ci_state::append(
            &mut self.transaction,
            record.updated_at,
            &kind(&record.intent.nonce)?,
            &detail,
            mode,
            &self.storage,
        )
    }

    /// Commit before issuing Docker create. A nonce can never be reused, even
    /// after terminal removal, so an old receipt cannot authorize a new helper.
    pub fn begin_cache_helper(&mut self, intent: &CacheHelperIntent) -> Result<(), Error> {
        intent.validate()?;
        if self.helper_record(&intent.nonce)?.is_some() {
            return Err(Error::BadRow("cache helper nonce already used"));
        }
        self.validate_recovery_helper_admission(intent)?;
        let mode = if intent
            .role
            .as_ref()
            .and_then(CacheHelperRole::recovery_binding)
            .is_some()
        {
            ci_state::WriteMode::RecoveryHelper
        } else {
            ci_state::WriteMode::Event
        };
        self.write_helper(
            &CacheHelperRecord {
                schema_version: 1,
                intent: intent.clone(),
                state: CacheHelperState::Pending,
                container_id: None,
                updated_at: intent.created_at,
            },
            mode,
        )
    }

    /// Record the acknowledged or independently verified immutable ID before
    /// starting/removing it. Conflicting IDs never replace the original claim.
    pub fn register_cache_helper(&mut self, nonce: &str, id: &str, at: f64) -> Result<(), Error> {
        let mut record = self
            .helper_record(nonce)?
            .ok_or(Error::BadRow("cache helper intent missing"))?;
        if !hex_id(id)
            || !at.is_finite()
            || at < record.updated_at
            || record.state == CacheHelperState::Removed
            || record
                .container_id
                .as_ref()
                .is_some_and(|existing| existing != id)
        {
            return Err(Error::BadRow("cache helper registration"));
        }
        // Recovery can observe the same immutable ID repeatedly while removal
        // is deferred. Only an actual state transition belongs in the audit;
        // keep its original timestamp and full nonce/identity evidence.
        if record.state == CacheHelperState::Created {
            return Ok(());
        }
        record.container_id = Some(id.into());
        record.state = CacheHelperState::Created;
        record.updated_at = at;
        self.store_helper(&record)
    }

    /// The trusted daemon supplies explicit absence of the registered ID.
    /// Mere name absence for an uncertain create is deliberately insufficient.
    pub fn finish_cache_helper(&mut self, nonce: &str, id: &str, at: f64) -> Result<(), Error> {
        let mut record = self
            .helper_record(nonce)?
            .ok_or(Error::BadRow("cache helper intent missing"))?;
        if record.state != CacheHelperState::Created
            || record.container_id.as_deref() != Some(id)
            || !at.is_finite()
            || at < record.updated_at
        {
            return Err(Error::BadRow("cache helper removal proof"));
        }
        record.state = CacheHelperState::Removed;
        record.updated_at = at;
        self.store_helper(&record)
    }

    /// A trusted Docker name probe may finish an unacknowledged create only
    /// after its bounded late-create window. No immutable ID is fabricated.
    pub fn finish_absent_cache_helper(&mut self, nonce: &str, at: f64) -> Result<(), Error> {
        let mut record = self
            .helper_record(nonce)?
            .ok_or(Error::BadRow("cache helper intent missing"))?;
        if record.state != CacheHelperState::Pending
            || record.container_id.is_some()
            || !at.is_finite()
            || at < record.intent.created_at + LATE_CREATE_WINDOW_SECS
        {
            return Err(Error::BadRow("cache helper late-create absence proof"));
        }
        record.state = CacheHelperState::Removed;
        record.updated_at = at;
        self.store_helper(&record)
    }
}

impl Registry {
    pub fn cache_helper(&self, nonce: &str) -> Result<Option<CacheHelperRecord>, Error> {
        self.require_ci_ready()?;
        read_helper(&self.connection, &self.registry_id()?, nonce)
    }

    pub fn pending_cache_helpers(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<CacheHelperPage, Error> {
        if limit == 0 || limit > MAX_HELPER_PAGE {
            return Err(Error::BadRow("cache helper page limit"));
        }
        self.require_ci_ready()?;
        let after_kind = after
            .map(|id| kind(id).map(|_| id.to_owned()))
            .transpose()?
            .unwrap_or_default();
        let rows = self.connection.query("SELECT 'ci.cache-helper.v1:'||id,detail FROM ci_current WHERE family=2 AND pending=1 AND id>? ORDER BY id LIMIT ?",
            &[Value::Text(after_kind), Value::Integer((limit + 1) as i64)],
            QueryLimits { max_rows: limit + 1, max_bytes: (limit + 1) * (RECORD_BYTES + 1024) })?;
        let owner = self.registry_id()?;
        let items = rows
            .iter()
            .take(limit)
            .map(|row| {
                let event = text(row, 0)?;
                let nonce = event
                    .strip_prefix(PREFIX)
                    .ok_or(Error::BadRow("cache helper event kind"))?;
                decode(&text(row, 1)?, nonce, &owner)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let next_nonce =
            (rows.len() > limit).then(|| items.last().expect("nonempty page").intent.nonce.clone());
        Ok(CacheHelperPage { items, next_nonce })
    }
}

fn read_helper(
    connection: &Connection,
    owner: &str,
    nonce: &str,
) -> Result<Option<CacheHelperRecord>, Error> {
    let rows = connection.query(
        "SELECT detail FROM ci_current WHERE family=2 AND id=?",
        &[Value::Text(
            kind(nonce)?.strip_prefix(PREFIX).unwrap().into(),
        )],
        QueryLimits {
            max_rows: 1,
            max_bytes: RECORD_BYTES + 1024,
        },
    )?;
    rows.first()
        .map(|row| decode(&text(row, 0)?, nonce, owner))
        .transpose()
}

impl ReadOnlyRegistry {
    pub fn cache_helper(&self, nonce: &str) -> Result<Option<CacheHelperRecord>, Error> {
        read_helper(&self.connection, &self.registry_id()?, nonce)
    }
}

#[cfg(test)]
mod tests;

/// Docker-side POSIX path, independent of the daemon client's host platform.
/// Commas/quotes would alter --mount CSV parsing; no normalization is inferred.
pub fn valid_recovery_source_mountpoint(path: &str) -> bool {
    path.len() <= 4096
        && path.starts_with('/')
        && path != "/"
        && !path
            .chars()
            .any(|c| c.is_control() || matches!(c, ',' | '"' | '\\'))
        && path
            .split('/')
            .skip(1)
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
