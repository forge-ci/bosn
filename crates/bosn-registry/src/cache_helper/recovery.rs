//! Bind recovery helpers to exact source intent and phase admission authority.
use super::*;

impl Immediate<'_> {
    pub(super) fn validate_recovery_helper_admission(
        &mut self,
        helper: &CacheHelperIntent,
    ) -> Result<(), Error> {
        if let Some(CacheHelperRole::ToolSourceFinalizationV1 { context, .. }) = &helper.role {
            if self
                .recovery_helper_for_source(&context.recovery.source_volume)?
                .is_some()
            {
                return Err(Error::BadRow("source finalization helper already pending"));
            }
            return Ok(());
        }
        let Some(binding) = helper
            .role
            .as_ref()
            .and_then(CacheHelperRole::recovery_binding)
        else {
            return Ok(());
        };
        let intent = binding.intent;
        let record = self
            .act_record(binding.run_id)?
            .ok_or(Error::BadRow("recovery helper engine missing"))?;
        if record.tool_recovery.as_ref() != Some(intent)
            || (binding.phase != RecoveryHelperPhase::Dispose
                && (record.tool_recovery_reserved_at.is_none()
                    || record.tool_recovery_source_stopped_at.is_none()))
            || helper.created_at < record.updated_at
            || helper.image
                != format!(
                    "docker.io/library/docker@{}",
                    record.intent.engine_image_digest
                )
        {
            return Err(Error::BadRow("recovery helper source not admitted"));
        }
        match binding.phase {
            RecoveryHelperPhase::Publish => {
                if record.state != act::ActEngineState::CleanupRequired
                    || record.tool_recovery_completion.is_some()
                    || record.tool_recovery_source_disposal_at.is_some()
                    || !intent.is_live(helper.created_at)
                    || self
                        .recovery_helper_for_source(&intent.source_volume)?
                        .is_some()
                {
                    return Err(Error::BadRow(
                        "recovery source not available for publication",
                    ));
                }
            }
            RecoveryHelperPhase::Release => {
                if !matches!(
                    record.state,
                    act::ActEngineState::CleanupRequired | act::ActEngineState::Terminal
                ) || record
                    .tool_recovery_completion
                    .as_ref()
                    .is_none_or(|done| done.reference_released_at.is_some())
                    || self.pending_reference_release(&intent.owner)?.is_some()
                {
                    return Err(Error::BadRow(
                        "recovery reference not available for release",
                    ));
                }
            }
            RecoveryHelperPhase::Dispose => {
                // This is the registry's admission fence, not the producer's
                // receipt. Lost reservation acknowledgements may still need
                // disposal after expiry; the producer decides eligibility.
                if record.state != act::ActEngineState::CleanupRequired
                    || record.tool_recovery_source_disposal_at.is_none()
                {
                    return Err(Error::BadRow("source disposal not claimed"));
                }
            }
        }
        Ok(())
    }

    /// A pending or acknowledged helper excludes source disposal even after the
    /// lower reference expires. An uncertain create is still a possible reader.
    pub fn recovery_helper_for_source(
        &mut self,
        source: &str,
    ) -> Result<Option<CacheHelperRecord>, Error> {
        act::ActEngineCacheVolume {
            name: source.into(),
            target: "/source".into(),
        }
        .validate()?;
        self.pending_helper(
            "SELECT 'ci.cache-helper.v1:'||id,detail FROM ci_current WHERE family=2 AND pending=1 AND source=? LIMIT 1",
            source,
        )
    }

    fn pending_reference_release(
        &mut self,
        owner: &str,
    ) -> Result<Option<CacheHelperRecord>, Error> {
        self.pending_helper(
            "SELECT 'ci.cache-helper.v1:'||id,detail FROM ci_current WHERE family=2 AND pending=1 AND release_owner=? LIMIT 1",
            owner,
        )
    }

    fn pending_helper(&mut self, sql: &str, key: &str) -> Result<Option<CacheHelperRecord>, Error> {
        let rows = self.transaction.query(
            sql,
            &[Value::Text(key.into())],
            QueryLimits {
                max_rows: 1,
                max_bytes: RECORD_BYTES + 128,
            },
        )?;
        rows.first()
            .map(|row| {
                let event = text(row, 0)?;
                let nonce = event
                    .strip_prefix(PREFIX)
                    .ok_or(Error::BadRow("cache helper event kind"))?;
                decode(&text(row, 1)?, nonce, &self.helper_owner()?)
            })
            .transpose()
    }
}
