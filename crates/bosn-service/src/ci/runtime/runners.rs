//! Runner management: list, drain/resume, the live limit, pruning finished
//! runs, and the machine-wide cache volume.

use super::*;

/// One clear: the volume deletion (`DELETE`) with slack — the outer bound
/// that keeps `clearing_cache` from holding admission forever (#445, #453).
const CLEAR_CACHE_BUDGET: Duration = Duration::from_secs(240);

/// Releases the cache-clear admission hold on every exit path (#453).
struct ClearingHold(Arc<Mutex<CiState>>);
impl Drop for ClearingHold {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clearing_cache = false;
    }
}

impl CiRuntime {
    pub(super) async fn runners(&self, action: RunnerAction) -> Result<RunnersReply, CiError> {
        let (mut pruned, mut cache) = (None, None);
        let maintenance = if matches!(action, RunnerAction::CacheUsage) {
            use super::super::maintenance_status::MaintenanceStatus;
            use crate::act_registry::{ActRegistryCommand, ActRegistryReply};
            Some(
                match self
                    .registry
                    .act_registry(ActRegistryCommand::MaintenanceLatest)
                    .await
                {
                    Ok(ActRegistryReply::Maintenance(Some(snapshot))) => snapshot.into(),
                    Ok(ActRegistryReply::Maintenance(None)) => MaintenanceStatus::NeverObserved,
                    _ => MaintenanceStatus::Unavailable,
                },
            )
        } else {
            None
        };
        match &action {
            RunnerAction::PruneCache {
                older_than_secs,
                max_bytes,
            } => pruned = Some(self.prune(*older_than_secs, *max_bytes).await?),
            RunnerAction::CacheUsage => cache = Some(self.cache_usage().await?),
            RunnerAction::ClearCache => cache = Some(self.clear_cache().await?),
            RunnerAction::List
            | RunnerAction::Drain
            | RunnerAction::Resume
            | RunnerAction::SetLimit { .. } => {}
        }
        {
            let mut state = self.lock();
            match action {
                RunnerAction::List
                | RunnerAction::PruneCache { .. }
                | RunnerAction::CacheUsage
                | RunnerAction::ClearCache => {}
                RunnerAction::Drain => state.scheduler.set_drained(true),
                RunnerAction::Resume => state.scheduler.set_drained(false),
                RunnerAction::SetLimit { limit } => {
                    if !(1..=256).contains(&limit) {
                        return Err(CiError::refused("limit must be between 1 and 256"));
                    }
                    state.scheduler.set_limit(limit);
                }
            }
            self.store.save_settings(&Settings {
                limit: state.scheduler.limit(),
                drained: state.scheduler.drained(),
            });
        }
        // A dead dispatcher is rebuilt here, before the status is read, so
        // the reply reports the recovered admission state (#453).
        self.kick();
        let status = self.runner_status(&self.lock());
        let engine_capacity = self.engine_capacity().await;
        Ok(RunnersReply {
            runners: status,
            engine_capacity,
            pruned_runs: pruned,
            cache,
            maintenance,
        })
    }

    async fn cache_usage(&self) -> Result<CacheUsage, CiError> {
        let owner = self
            .registry
            .status()
            .await
            .map_err(|error| CiError::new("internal", error.to_string()))?
            .registry_id;
        Ok(self
            .backend
            .tracked_cache_usage(CACHE_VOLUME, &self.registry, &owner)
            .await
            .unwrap_or_else(|error| CacheUsage {
                volume: CACHE_VOLUME.into(),
                partial: true,
                errors: vec![error.chars().take(1024).collect()],
                ..CacheUsage::default()
            }))
    }

    /// Remove the machine-wide cache volume. Refused while a run executes;
    /// dispatch holds queued runs until the volume is gone. The hold is a
    /// guard released on every exit path — panic, cancellation or an
    /// over-budget clear included — so `clearing_cache` can never outlive
    /// the clear and silently strand admission (#453); the clear runs under
    /// one deadline so a stuck Docker call cannot hold it forever (#445).
    async fn clear_cache(&self) -> Result<CacheUsage, CiError> {
        let hold = {
            let mut state = self.lock();
            if state.scheduler.running() > 0 {
                return Err(CiError::refused(
                    "runs are executing; let them finish or cancel them, then clear the cache",
                ));
            }
            state.clearing_cache = true;
            ClearingHold(Arc::clone(&self.state))
        };
        let work = self.backend.remove_cache(CACHE_VOLUME);
        let cleared = match async_engine::timeout(CLEAR_CACHE_BUDGET, work).await {
            Err(_) => Err(CiError::new(
                "internal",
                "cache clear exceeded its deadline",
            )),
            Ok(Err(error)) => Err(CiError::new("internal", error)),
            // Measure the removed volume before dispatch may start a run that
            // recreates it. Otherwise the reply races that run.
            Ok(Ok(())) => self.cache_usage().await,
        };
        drop(hold);
        self.kick();
        cleared
    }

    /// Remove finished runs by age, total size, or beyond the retention
    /// counts, and drop sources of all but the newest few. Live runs are
    /// kept. Sizes are measured and files deleted off the state lock.
    pub(super) async fn prune(
        &self,
        older_than: Option<u64>,
        max_bytes: Option<u64>,
    ) -> Result<Vec<String>, CiError> {
        let sizes = match max_bytes {
            None => BTreeMap::new(),
            Some(_) => {
                let finished = self.lock().finished_newest_first();
                let store = self.store.clone();
                blocking(move || {
                    finished
                        .into_iter()
                        .map(|id| {
                            let size = store.run_bytes(&id);
                            (id, size)
                        })
                        .collect()
                })
                .await?
            }
        };
        let plan = self.lock().plan_prune(older_than, max_bytes, &sizes);
        let (store, writer) = (self.store.clone(), self.writer.clone());
        let pruned = plan.runs.clone();
        blocking(move || plan.apply(&store, &writer)).await?;
        Ok(pruned)
    }
}

/// What one prune pass deletes: decided under the lock, applied outside it.
struct PrunePlan {
    runs: Vec<String>,
    sources: Vec<String>,
}
impl PrunePlan {
    fn apply(&self, store: &Store, writer: &RecordWriter) {
        for id in &self.runs {
            writer.remove_run(id);
        }
        for id in &self.sources {
            store.remove_source(id);
        }
    }
}

impl CiState {
    fn finished_newest_first(&self) -> Vec<String> {
        self.order
            .iter()
            .rev()
            .filter(|id| self.runs[*id].record.state == RunState::Done)
            .cloned()
            .collect()
    }

    /// Drop pruned runs from the state now; their files go in `apply`.
    fn plan_prune(
        &mut self,
        older_than: Option<u64>,
        max_bytes: Option<u64>,
        sizes: &BTreeMap<String, u64>,
    ) -> PrunePlan {
        let now = lifecycle::now_seconds();
        let mut plan = PrunePlan {
            runs: Vec::new(),
            sources: Vec::new(),
        };
        let mut kept_bytes = 0;
        for (newest_first, id) in self.finished_newest_first().into_iter().enumerate() {
            let age = now - self.runs[&id].record.created_at;
            let size = sizes.get(&id).copied().unwrap_or(0);
            if newest_first >= KEEP_RUNS
                || older_than.is_some_and(|s| age > s as f64)
                || max_bytes.is_some_and(|m| kept_bytes + size > m)
            {
                self.runs.remove(&id);
                plan.runs.push(id);
                continue;
            }
            if newest_first >= KEEP_SOURCES {
                plan.sources.push(id);
            }
            kept_bytes += size;
        }
        self.order.retain(|id| self.runs.contains_key(id));
        plan
    }
}
