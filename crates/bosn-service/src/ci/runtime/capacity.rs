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
    /// Prefer the frozen limits of an engine already running; otherwise size
    /// the next engine using a fresh Docker-host resource sample.
    pub(super) async fn engine_capacity(&self) -> Option<CiEngineCapacity> {
        if let Some((cpus, memory_bytes, source)) = self.shared.capacity_limits().await {
            return Some(CiEngineCapacity {
                cpus,
                memory_bytes,
                cpu_source: source.unwrap_or(CiCpuSource::Unknown),
                running: true,
            });
        }
        // A different daemon may have made the machine's engine before this
        // daemon leases a slot in it. Its Docker limits are already frozen.
        if let Some(machine) = self.backend.machine()
            && let Some(claim) = machine.claim().await.ok()?
            && let Some(engine) = machine.inspect(&claim.engine).await.ok()?
            && engine.running
            && engine.registry == claim.registry
            && engine.identity.is_some()
        {
            return Some(CiEngineCapacity {
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
                .ok()?
                .ok()?;
        let cpu_source = if config.cpus.is_some() {
            CiCpuSource::Config
        } else {
            CiCpuSource::DefaultCeiling
        };
        let host = self.backend.host_resources().await.ok()?;
        let reserved = if let Some(machine) = self.backend.machine() {
            limits::reserved_engine_memory(&machine.engines().await.ok()?, None, host.total_memory)
        } else {
            0
        };
        let limits = limits::size_engine_with_reserved(host, config, reserved).ok()?;
        Some(CiEngineCapacity {
            cpus: limits.nano_cpus / 1_000_000_000,
            memory_bytes: limits.memory_bytes,
            cpu_source,
            running: false,
        })
    }
}
