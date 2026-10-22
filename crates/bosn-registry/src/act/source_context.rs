//! Self-contained frozen source identity, independent of its owning registry.
use super::{ActEngineIntent, ActEngineRecord, ActToolRecoveryIntent};
use crate::Error;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActToolSourceContext {
    pub schema_version: u32,
    pub registry_id: String,
    pub intent: ActEngineIntent,
    pub recovery: ActToolRecoveryIntent,
}

impl ActToolSourceContext {
    pub fn from_record(record: &ActEngineRecord) -> Result<Self, Error> {
        let recovery = record
            .tool_recovery
            .clone()
            .ok_or(Error::BadRow("source recovery intent missing"))?;
        recovery.validate_record(record)?;
        let context = Self {
            schema_version: 1,
            registry_id: record.registry_id.clone(),
            intent: record.intent.clone(),
            recovery,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.schema_version != 1 || self.recovery.source_volume == self.recovery.cache_volume {
            return Err(Error::BadRow("source context schema differs"));
        }
        self.intent.required_labels(&self.registry_id)?;
        self.recovery
            .validate_engine_intent(&self.registry_id, &self.intent)
    }
}
