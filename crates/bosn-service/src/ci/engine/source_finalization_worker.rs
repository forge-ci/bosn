//! Machine-wide retained source recovery independent of the owning registry.
use super::discovery::SourceCursor;
use crate::{
    RegistryActor,
    act_registry::{ActRegistryCommand, ActRegistryReply},
    ci::engine::{CONTROL_DEADLINE, DockerActBackend, owned},
};
use bosn_registry::act::ActEngineState;
use kernal_api::async_engine::{self, CancellationToken};
use std::{collections::VecDeque, time::Duration};

#[derive(Default, Debug)]
pub(super) struct SourceTick {
    pub(super) advanced: Option<String>,
    pub(super) deferred: Vec<(String, String)>,
}

impl DockerActBackend {
    pub(super) async fn retry_retained_sources(
        &self,
        registry: &RegistryActor,
        owner: &str,
        cursor: &mut SourceCursor,
    ) -> Result<SourceTick, String> {
        cursor.begin_pass();
        let mut queries = 8;
        let mut tick = SourceTick::default();
        for _ in 0..32 {
            let Some(id) = cursor.next(self, &mut queries).await? else {
                break;
            };
            let context = match self.observed_source_context(&id).await {
                Ok((observed, context)) if observed == id => context,
                result => {
                    tick.deferred
                        .push((id, format!("source context invalid: {result:?}")));
                    continue;
                }
            };
            if context.registry_id == owner {
                match registry
                    .act_registry(ActRegistryCommand::Get {
                        run: context.intent.run_id.clone(),
                    })
                    .await
                    .map_err(|e| e.to_string())?
                {
                    ActRegistryReply::Record(Some(record))
                        if record.state != ActEngineState::Terminal =>
                    {
                        continue;
                    }
                    ActRegistryReply::Record(_) => (),
                    _ => return Err("retained source local registry reply differs".into()),
                }
            }
            // A live engine is expected, so skip it quietly. Every absence and
            // deterministic engine name are checked again by the helper path.
            let probe = self
                .run(
                    owned(&["container", "inspect", &context.recovery.engine_id]),
                    CONTROL_DEADLINE,
                )
                .await?;
            if probe.ok() {
                continue;
            }
            let detail = String::from_utf8_lossy(&probe.stderr).to_ascii_lowercase();
            if !probe.reports_missing()
                || !(detail.contains("no such container:") || detail.contains("no such object:"))
            {
                tick.deferred
                    .push((id, "retained source engine inspection failed".into()));
                continue;
            }
            match self
                .dispose_retained_source(registry, owner, &id, &context)
                .await
            {
                Ok(()) => tick.advanced = Some(id),
                Err(error) => tick.deferred.push((id, error)),
            }
            // One external source lifecycle per pass; invalid or live records
            // still advance discovery instead of hiding older sources.
            break;
        }
        Ok(tick)
    }

    pub(in crate::ci::engine) async fn supervise_retained_sources(
        &self,
        registry: &RegistryActor,
        owner: &str,
        stop: &CancellationToken,
    ) {
        let mut cursor = SourceCursor::default();
        let mut reported = VecDeque::new();
        loop {
            let result = async_engine::cancellable(
                stop,
                async_engine::timeout(
                    Duration::from_secs(900),
                    // Heap-own each bounded pass as well as the supervisor:
                    // daemon startup composes several large async lanes.
                    Box::pin(self.retry_retained_sources(registry, owner, &mut cursor)),
                ),
            )
            .await;
            match result {
                Err(_) => return,
                Ok(Err(_)) => {
                    daemon_log!("bosn retained source pass deadline exceeded; journal retained")
                }
                Ok(Ok(Err(error))) => {
                    daemon_log!("bosn retained source discovery failed: {}", bounded(&error))
                }
                Ok(Ok(Ok(tick))) => {
                    if let Some(id) = tick.advanced {
                        reported.retain(|(key, _)| key != &id);
                        daemon_log!("bosn retained source finalized {id}");
                    }
                    for (id, error) in tick.deferred {
                        let key = (id, bounded(&error));
                        if !reported.contains(&key) {
                            daemon_log!("bosn retained source deferred for {}: {}", key.0, key.1);
                            if reported.len() == 64 {
                                reported.pop_front();
                            }
                            reported.push_back(key);
                        }
                    }
                }
            }
            if async_engine::cancellable(stop, async_engine::sleep(Duration::from_secs(10)))
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

fn bounded(text: &str) -> String {
    text.chars().take(512).collect()
}
