//! Runs owned by client processes: a run whose every owner has exited, or
//! whose last owner releases it on its way out, is cancelled at once, which
//! closes its scope and removes its containers. Detached runs carry no owner.

use super::*;
use crate::{ClientOwner, owner};

/// How often the owners of unfinished runs are looked up.
const OWNER_SWEEP: Duration = Duration::from_millis(500);

impl CiRuntime {
    /// `owner`'s client is exiting (`bosn ci run` interrupted or terminated):
    /// cancel the run if no other owner still holds it.
    pub(super) fn release(
        &self,
        run: &str,
        owner: Option<ClientOwner>,
    ) -> Result<CancelReply, CiError> {
        let owner = owner.ok_or_else(|| CiError::refused("release names no owner"))?;
        let last = self.lock().owners.release(&run.to_string(), &owner);
        if last {
            self.cancel_orphaned(run, &[owner.pid()])
        } else {
            let state = self.record(run)?.state;
            Ok(CancelReply {
                run: run.into(),
                cancelled: false,
                state,
            })
        }
    }

    /// Cancel the runs whose every owner has exited, once per [`OWNER_SWEEP`]:
    /// the second line of defense, for a client killed outright (SIGKILL, a
    /// crash, a closed terminal) that never released its run.
    pub(super) fn start_owner_sweep(&self) {
        let runtime = self.clone();
        async_engine::launch(async move {
            loop {
                async_engine::sleep(OWNER_SWEEP).await;
                let orphaned = {
                    let mut state = runtime.lock();
                    if state.owners.is_empty() {
                        continue;
                    }
                    state.owners.orphaned(ClientOwner::liveness)
                };
                for (run, pids) in orphaned {
                    let _ = runtime.cancel_orphaned(&run, &pids);
                }
            }
        })
        .detach();
    }

    fn cancel_orphaned(&self, run: &str, pids: &[u32]) -> Result<CancelReply, CiError> {
        let message = owner::orphaned_message(pids);
        let reply = self.cancel_because(run, &message)?;
        if reply.cancelled {
            daemon_log!("bosn ci: run {run} {message}");
        }
        Ok(reply)
    }
}
