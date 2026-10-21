//! Bind a recovery helper to one stopped source before Docker can create it.
use super::*;

impl Immediate<'_> {
    pub(super) fn validate_recovery_helper_admission(
        &mut self,
        helper: &CacheHelperIntent,
    ) -> Result<(), Error> {
        let Some(CacheHelperRole::ToolRecoveryV1 { run_id, intent, .. }) = &helper.role else {
            return Ok(());
        };
        let record = self
            .act_record(run_id)?
            .ok_or(Error::BadRow("recovery helper engine missing"))?;
        if record.state != act::ActEngineState::CleanupRequired
            || record.tool_recovery.as_ref() != Some(intent)
            || record.tool_recovery_reserved_at.is_none()
            || record.tool_recovery_source_stopped_at.is_none()
            || record.tool_recovery_completion.is_some()
            || record.tool_recovery_source_disposal_at.is_some()
            || helper.created_at < record.updated_at
            || !intent.is_live(helper.created_at)
            || helper.image
                != format!(
                    "docker.io/library/docker@{}",
                    record.intent.engine_image_digest
                )
        {
            return Err(Error::BadRow("recovery helper source not admitted"));
        }
        if self
            .recovery_helper_for_source(&intent.source_volume)?
            .is_some()
        {
            return Err(Error::BadRow(
                "recovery source already has a pending helper",
            ));
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
        let rows = self.transaction.query(
            "SELECT e.kind,e.detail FROM events e JOIN (SELECT MAX(id) AS id FROM events WHERE kind GLOB 'ci.cache-helper.v1:*' GROUP BY kind) latest ON latest.id=e.id WHERE json_extract(e.detail,'$.intent.role.tool_recovery_v1.intent.source_volume')=? AND (json_extract(e.detail,'$.state') IS NULL OR json_extract(e.detail,'$.state')<>'removed') LIMIT 1",
            &[Value::Text(source.into())],
            QueryLimits { max_rows: 1, max_bytes: RECORD_BYTES + 128 },
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
