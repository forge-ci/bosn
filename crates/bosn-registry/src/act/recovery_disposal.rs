//! Atomic helper exclusion before the trusted runtime removes a private source.
use super::*;

impl Immediate<'_> {
    /// Commit before Docker volume removal. A successful absence lookup alone
    /// cannot exclude a delayed helper admission after that lookup.
    pub fn claim_act_tool_source_disposal(&mut self, run: &str, at: f64) -> Result<(), Error> {
        let mut record = self
            .act_record(run)?
            .ok_or(Error::BadRow("act intent missing"))?;
        if record.tool_recovery.is_none() {
            return Ok(());
        }
        record.check_time(at)?;
        if record.state != ActEngineState::CleanupRequired
            || record.retains_tool_recovery_source(at)
        {
            return Err(Error::BadRow("recovery source disposal not admitted"));
        }
        if record.tool_recovery_source_disposal_at.is_some() {
            return Ok(());
        }
        let source = record
            .intent
            .storage_volume_name()
            .ok_or(Error::BadRow("recovery source storage missing"))?;
        if self.recovery_helper_for_source(&source)?.is_some() {
            return Err(Error::BadRow("recovery helper retains source storage"));
        }
        record.tool_recovery_source_disposal_at = Some(at);
        record.updated_at = at;
        self.store_act_record(&record)
    }
}
