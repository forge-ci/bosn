//! Phase-specific program and receipt authority for one journaled helper.
use super::{RecoveryPin, require_disposal, require_publication, require_release};
use crate::{
    RegistryActor,
    act_registry::{ActRegistryCommand, ActRegistryReply},
    ci::engine::ENGINE_CACHE,
};
use bosn_registry::{
    act::{ActEngineRecord, ActToolRecoveryPublicationProof},
    cache_helper::{CacheHelperRole, RecoveryHelperPhase},
};

pub(super) use bosn_registry::cache_helper::RecoveryHelperPhase as Operation;

#[derive(Debug)]
pub(super) enum Receipt {
    Published { generation: String, at: f64 },
    Released,
    Disposed,
}

impl Receipt {
    pub(super) async fn commit(
        self,
        registry: &RegistryActor,
        record: &ActEngineRecord,
    ) -> Result<(), String> {
        let intent = record
            .tool_recovery
            .clone()
            .ok_or("tool recovery intent missing")?;
        let run = record.intent.run_id.clone();
        let command = match self {
            // The caller consumes this verified receipt only after namespace
            // teardown. Do not persist/reuse a registry timestamp as producer
            // authority; every physical deletion retry runs the fence again.
            Self::Disposed => return Ok(()),
            Self::Published { generation, at } => ActRegistryCommand::ToolRecoveryPublished {
                run,
                proof: ActToolRecoveryPublicationProof { intent, generation },
                at,
            },
            Self::Released => ActRegistryCommand::ToolRecoveryReleased {
                run,
                intent,
                at: crate::ci::lifecycle::now_seconds().max(record.updated_at),
            },
        };
        match registry
            .act_registry(command)
            .await
            .map_err(|e| format!("tool recovery acknowledgement needs reconciliation: {e}"))?
        {
            ActRegistryReply::Committed => Ok(()),
            _ => Err("tool recovery acknowledgement reply mismatch".into()),
        }
    }
}

pub(super) fn already_completed(
    operation: Operation,
    record: &ActEngineRecord,
) -> Result<bool, String> {
    match operation {
        Operation::Publish => Ok(record.tool_recovery_completion.is_some()),
        Operation::Dispose => {
            if record.tool_recovery_source_disposal_at.is_none() {
                return Err("source disposal must be claimed before producer fencing".into());
            }
            Ok(false)
        }
        Operation::Release => record
            .tool_recovery_completion
            .as_ref()
            .map(|done| done.reference_released_at.is_some())
            .ok_or_else(|| "reference release requires durable publication".into()),
    }
}

pub(super) fn role(
    operation: Operation,
    record: &ActEngineRecord,
    command: String,
    source_mountpoint: Option<String>,
) -> Result<CacheHelperRole, String> {
    let run_id = record.intent.run_id.clone();
    let intent = record
        .tool_recovery
        .clone()
        .ok_or("tool recovery intent missing")?;
    Ok(match operation {
        Operation::Publish => CacheHelperRole::ToolRecoveryV1 {
            run_id,
            intent,
            command_sha256: Some(command),
            source_mountpoint,
        },
        Operation::Release => CacheHelperRole::ToolRecoveryReleaseV1 {
            run_id,
            intent,
            command_sha256: command,
        },
        Operation::Dispose => CacheHelperRole::ToolRecoveryDisposalV1 {
            run_id,
            intent,
            command_sha256: command,
        },
    })
}

pub(super) fn receipt(
    operation: Operation,
    text: &str,
    record: &ActEngineRecord,
) -> Result<Receipt, String> {
    let frozen = record
        .tool_recovery
        .as_ref()
        .ok_or("tool recovery intent missing")?;
    let pin = RecoveryPin::from_intent(frozen)?;
    let root = format!("{ENGINE_CACHE}/toolstore-v1");
    match operation {
        RecoveryHelperPhase::Publish => {
            let (generation, at) = require_publication(
                text,
                &pin,
                &root,
                record.updated_at,
                crate::ci::lifecycle::now_seconds(),
            )?;
            Ok(Receipt::Published { generation, at })
        }
        RecoveryHelperPhase::Release => {
            require_release(text, &pin, &root)?;
            Ok(Receipt::Released)
        }
        RecoveryHelperPhase::Dispose => {
            require_disposal(text, &pin, &root)?;
            Ok(Receipt::Disposed)
        }
    }
}
