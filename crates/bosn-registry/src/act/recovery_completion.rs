//! Publication and reference release are separate durable recovery transitions.
use super::*;

/// Receipt supplied by the trusted runtime after a complete, exact act2
/// publication report. This is not a serializable client request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActToolRecoveryPublicationProof {
    pub intent: ActToolRecoveryIntent,
    pub generation: String,
}

/// A committed publication excludes republishing this owner. Only a later,
/// exact reference-absence receipt permits early removal of the source disk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActToolRecoveryCompletion {
    pub generation: String,
    pub completed_at: f64,
    pub reference_released_at: Option<f64>,
}

impl ActEngineRecord {
    #[must_use]
    pub fn retains_tool_recovery_source(&self, at: f64) -> bool {
        self.tool_recovery.as_ref().is_some_and(|intent| {
            intent.is_live(at)
                && self
                    .tool_recovery_completion
                    .as_ref()
                    .is_none_or(|done| done.reference_released_at.is_none())
        })
    }

    pub(super) fn validate_tool_recovery(&self) -> Result<(), Error> {
        if self.tool_recovery_reserved_at.is_some_and(|at| {
            !at.is_finite()
                || at > self.updated_at
                || self.tool_recovery.as_ref().is_none_or(|intent| {
                    at < intent.created_at_seconds as f64 || at >= intent.expires_at_seconds as f64
                })
        }) {
            return Err(Error::BadRow("tool recovery reservation acknowledgement"));
        }
        if self.tool_recovery_source_stopped_at.is_some_and(|at| {
            !at.is_finite()
                || at > self.updated_at
                || self
                    .tool_recovery_reserved_at
                    .is_none_or(|reserved| at < reserved)
                || !matches!(
                    self.state,
                    ActEngineState::CleanupRequired | ActEngineState::Terminal
                )
        }) {
            return Err(Error::BadRow("tool recovery source stop snapshot"));
        }
        if let Some(intent) = &self.tool_recovery {
            intent.validate_record(self)?;
        }
        if let Some(done) = &self.tool_recovery_completion
            && (!hex(&done.generation, 64)
                || !done.completed_at.is_finite()
                || done.completed_at > self.updated_at
                || self
                    .tool_recovery_source_stopped_at
                    .is_none_or(|stopped| stopped > done.completed_at)
                || self
                    .tool_recovery
                    .as_ref()
                    .is_none_or(|intent| !intent.is_live(done.completed_at))
                || done.reference_released_at.is_some_and(|at| {
                    !at.is_finite() || at < done.completed_at || at > self.updated_at
                }))
        {
            return Err(Error::BadRow("tool recovery completion snapshot"));
        }
        if self.tool_recovery_source_disposal_at.is_some_and(|at| {
            !at.is_finite()
                || at > self.updated_at
                || self.tool_recovery.is_none()
                || self.retains_tool_recovery_source(at)
                || !matches!(
                    self.state,
                    ActEngineState::CleanupRequired | ActEngineState::Terminal
                )
        }) {
            return Err(Error::BadRow("tool recovery source disposal snapshot"));
        }
        if self.state == ActEngineState::Terminal
            && self.retains_tool_recovery_source(self.updated_at)
        {
            return Err(Error::BadRow("terminal engine retains recovery source"));
        }
        Ok(())
    }
}

impl Immediate<'_> {
    /// Persist publication before releasing the lower reference. Identical
    /// retries return the original completion, including after reference expiry.
    pub fn complete_act_tool_recovery(
        &mut self,
        run: &str,
        proof: &ActToolRecoveryPublicationProof,
        at: f64,
    ) -> Result<(), Error> {
        let mut record = self
            .act_record(run)?
            .ok_or(Error::BadRow("act intent missing"))?;
        record.check_time(at)?;
        if record.state != ActEngineState::CleanupRequired
            || record.tool_recovery.as_ref() != Some(&proof.intent)
            || record.tool_recovery_source_stopped_at.is_none()
            || !hex(&proof.generation, 64)
        {
            return Err(Error::BadRow("tool recovery publication proof"));
        }
        if let Some(done) = &record.tool_recovery_completion {
            return if done.generation == proof.generation {
                Ok(())
            } else {
                Err(Error::ResourceIdentityConflict)
            };
        }
        if !proof.intent.is_live(at) {
            return Err(Error::BadRow("tool recovery publication expired"));
        }
        record.tool_recovery_completion = Some(ActToolRecoveryCompletion {
            generation: proof.generation.clone(),
            completed_at: at,
            reference_released_at: None,
        });
        record.updated_at = at;
        self.store_act_record(&record)
    }

    /// Called only after act2 proves exact reference absence, never on a failed
    /// or partial release. A release cannot precede durable publication.
    pub fn release_act_tool_recovery(
        &mut self,
        run: &str,
        intent: &ActToolRecoveryIntent,
        at: f64,
    ) -> Result<(), Error> {
        let mut record = self
            .act_record(run)?
            .ok_or(Error::BadRow("act intent missing"))?;
        record.check_time(at)?;
        if !matches!(
            record.state,
            ActEngineState::CleanupRequired | ActEngineState::Terminal
        ) || record.tool_recovery.as_ref() != Some(intent)
        {
            return Err(Error::BadRow("tool recovery release identity"));
        }
        let done = record
            .tool_recovery_completion
            .as_mut()
            .ok_or(Error::BadRow("tool recovery release before publication"))?;
        if done.reference_released_at.is_some() {
            return Ok(());
        }
        done.reference_released_at = Some(at);
        record.updated_at = at;
        self.store_act_record(&record)
    }
}
