//! Fair, bounded recovery of already-reserved native sources and references.
use crate::{
    RegistryActor,
    act_registry::{ActRegistryCommand, ActRegistryReply},
    ci::engine::DockerActBackend,
};
use kernal_api::async_engine::{self, CancellationToken};
use std::{collections::VecDeque, time::Duration};

const PASS_BUDGET: Duration = Duration::from_secs(900);
const INTERVAL: Duration = Duration::from_secs(10);

#[derive(Default, Debug)]
pub(in crate::ci::engine) struct RecoveryTick {
    pub next_run_id: Option<String>,
    pub advanced: Option<String>,
    pub deferred: Option<String>,
}

impl DockerActBackend {
    /// At most 512 bounded snapshots and one external mutation per pass.
    /// Failed records advance the same keyset cursor as successful records.
    pub(in crate::ci::engine) async fn retry_tool_recovery(
        &self,
        registry: &RegistryActor,
        owner: &str,
        mut cursor: Option<String>,
    ) -> Result<RecoveryTick, String> {
        for _ in 0..8 {
            let start = cursor.clone();
            let reply = registry
                .act_registry(ActRegistryCommand::ToolRecoveryPending {
                    after_run_id: cursor.clone(),
                    limit: 64,
                })
                .await
                .map_err(|e| e.to_string())?;
            let ActRegistryReply::Recovery(page) = reply else {
                return Err("tool recovery page reply mismatch".into());
            };
            for record in page.items {
                let run = record.intent.run_id.clone();
                cursor = Some(run.clone());
                if record.registry_id != owner {
                    return Err("tool recovery registry owner mismatch".into());
                }
                let frozen = record
                    .tool_recovery
                    .as_ref()
                    .ok_or("tool recovery queue lost intent")?;
                let result = if record.tool_recovery_completion.is_some() {
                    self.release_tool_with_helper(registry, &run, owner).await
                } else if frozen.is_live(crate::ci::lifecycle::now_seconds()) {
                    self.recover_tool_with_helper(registry, &run, owner).await
                } else {
                    // Expiry revokes publication authority. The ordinary cleanup
                    // worker owns source disposal, including its helper fence.
                    continue;
                };
                return Ok(RecoveryTick {
                    next_run_id: cursor,
                    advanced: result.is_ok().then_some(run),
                    deferred: result.err(),
                });
            }
            match page.next_run_id {
                Some(next)
                    if start.as_ref().is_none_or(|old| old < &next)
                        && cursor.as_ref().is_none_or(|old| old <= &next) =>
                {
                    cursor = Some(next)
                }
                Some(_) => return Err("tool recovery cursor did not advance".into()),
                None => return Ok(RecoveryTick::default()),
            }
        }
        Ok(RecoveryTick {
            next_run_id: cursor,
            ..Default::default()
        })
    }

    /// Independent of enrollment discovery and ordinary cleanup's budgets.
    /// Cancellation drops the owned Docker client; the journal retains all
    /// uncertain creates/readers for the existing helper reaper after restart.
    pub(in crate::ci::engine) async fn supervise_tool_recovery(
        &self,
        registry: &RegistryActor,
        owner: &str,
        stop: &CancellationToken,
    ) {
        let mut cursor = None;
        let mut deferrals = VecDeque::new();
        loop {
            let result = async_engine::cancellable(
                stop,
                async_engine::timeout(
                    PASS_BUDGET,
                    self.retry_tool_recovery(registry, owner, cursor.clone()),
                ),
            )
            .await;
            match result {
                Err(_) => return,
                Ok(Err(_)) => {
                    daemon_log!("bosn tool recovery pass deadline exceeded; journal retained")
                }
                Ok(Ok(Err(error))) => daemon_log!("bosn tool recovery query failed: {error}"),
                Ok(Ok(Ok(tick))) => {
                    cursor = tick.next_run_id;
                    if let Some(run) = tick.advanced {
                        deferrals.retain(|(id, _)| id != &run);
                        daemon_log!("bosn tool recovery advanced {run}");
                    }
                    if let Some(error) = tick.deferred {
                        let key = (cursor.clone().unwrap_or_default(), error);
                        if !deferrals.contains(&key) {
                            daemon_log!("bosn tool recovery deferred for {}: {}", key.0, key.1);
                            if deferrals.len() == 64 {
                                deferrals.pop_front();
                            }
                            deferrals.push_back(key);
                        }
                    }
                }
            }
            if async_engine::cancellable(stop, async_engine::sleep(INTERVAL))
                .await
                .is_err()
            {
                return;
            }
        }
    }
}
