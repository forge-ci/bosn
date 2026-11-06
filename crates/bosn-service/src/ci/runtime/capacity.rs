//! Effective shared-engine capacity for operator reports.

use super::*;
use crate::ci::{
    limits::{self, EngineConfig},
    reply::{CiCpuSource, CiEngineCapacity, TaskLaneCapacity},
};

impl CiRuntime {
    /// The task runner's limits are fixed when the daemon starts.
    pub fn configure_task_capacity(&self, capacity: &crate::capacity::RunnerCapacity) {
        let _ = self.task_capacity.set(TaskLaneCapacity {
            slots: capacity.runner_slots,
            cpus_per_slot: capacity.cpus_per_slot,
            host_logical_cpus: crate::capacity::host_cpus(),
            control_slots: capacity.control_slots,
            stall_seconds: capacity.stall_after.map(|duration| duration.as_secs()),
            docker_proxy: capacity.docker_proxy,
        });
    }
    /// [`Self::probe_engine_capacity`], split for a reply: the limits or why there are none.
    pub(super) async fn engine_capacity(&self) -> (Option<CiEngineCapacity>, Option<String>) {
        match self.probe_engine_capacity().await {
            Ok(capacity) => (Some(capacity), None),
            Err(reason) => (None, Some(reason)),
        }
    }

    /// Prefer the frozen limits of an engine already running; otherwise size
    /// the next engine using a fresh Docker-host resource sample. A failure
    /// names itself: this is when an operator most needs the explanation (#767).
    async fn probe_engine_capacity(&self) -> Result<CiEngineCapacity, String> {
        if let Some((cpus, memory_bytes, source)) = self.shared.capacity_limits().await {
            return Ok(CiEngineCapacity {
                cpus,
                memory_bytes,
                cpu_source: source.unwrap_or(CiCpuSource::Unknown),
                running: true,
            });
        }
        // A different daemon may have made the machine's engine before this
        // daemon leases a slot in it. Its Docker limits are already frozen.
        if let Some(machine) = self.backend.machine()
            && let Some(claim) = machine
                .claim()
                .await
                .map_err(|error| format!("cannot read the engine claim: {error}"))?
            && let Some(engine) = machine
                .inspect(&claim.engine)
                .await
                .map_err(|error| format!("cannot inspect the engine: {error}"))?
            && engine.running
            && engine.registry == claim.registry
            && engine.identity.is_some()
        {
            return Ok(CiEngineCapacity {
                cpus: engine.nano_cpus / 1_000_000_000,
                memory_bytes: engine.memory_bytes,
                cpu_source: claim.cpu_source.unwrap_or(CiCpuSource::Unknown),
                running: true,
            });
        }
        let state_dir = self.state_dir.clone();
        let config: EngineConfig =
            async_engine::launch_blocking(move || super::super::config::load_engine(&state_dir))
                .await
                .map_err(|_| "the config read did not finish".to_owned())?
                .map_err(|error| format!("cannot read the engine config: {error}"))?;
        let cpu_source = if config.cpus.is_some() {
            CiCpuSource::Config
        } else {
            CiCpuSource::DefaultCeiling
        };
        let host = self
            .backend
            .host_resources()
            .await
            .map_err(|error| format!("cannot read host resources: {error}"))?;
        let reserved = if let Some(machine) = self.backend.machine() {
            let engines = machine
                .engines()
                .await
                .map_err(|error| format!("cannot list engines: {error}"))?;
            limits::reserved_engine_memory(&engines, None, host.total_memory)
        } else {
            0
        };
        let limits = limits::size_engine_with_reserved(host, config, reserved)?;
        Ok(CiEngineCapacity {
            cpus: limits.nano_cpus / 1_000_000_000,
            memory_bytes: limits.memory_bytes,
            cpu_source,
            running: false,
        })
    }
}
