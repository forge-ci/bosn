//! Accepted CI cancellation stays visible while engine teardown completes.

use super::*;

impl CiRuntime {
    pub(super) fn cancel(&self, run: &str) -> Result<CancelReply, CiError> {
        self.cancel_because(run, "cancelled while queued")
    }

    /// Cancel a run; a queued one ends now with `queued_reason`.
    pub(super) fn cancel_because(
        &self,
        run: &str,
        queued_reason: &str,
    ) -> Result<CancelReply, CiError> {
        let mut state = self.lock();
        if state.scheduler.cancel_queued(run) {
            state.owners.forget(&run.to_string());
            drop(state);
            self.update(run, |slot| {
                slot.record
                    .finish(Conclusion::Cancelled, Some(queued_reason.into()));
            });
            return Ok(CancelReply {
                run: run.into(),
                cancelled: true,
                state: RunState::Done,
            });
        }
        let slot = state
            .runs
            .get_mut(run)
            .ok_or_else(|| CiError::new("not_found", format!("no run {run}")))?;
        let state_now = slot.record.state;
        let source = slot.cancel.clone();
        let (cancelled, save) = if let (Some(source), RunState::Running) = (source, state_now) {
            let save = if slot.record.cancel_requested_at.is_none() {
                slot.record.cancel_requested_at = Some(lifecycle::now_seconds());
                Some(self.announce(slot))
            } else {
                None
            };
            source.cancel();
            (true, save)
        } else {
            (false, None)
        };
        drop(state);
        if let Some(save) = save {
            self.writer.save_detached(save);
        }
        Ok(CancelReply {
            run: run.into(),
            cancelled,
            state: state_now,
        })
    }
}
