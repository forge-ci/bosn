//! Shared human-readable capacity lines for `bosn jobs` and `bosn ci doctor`.

use bosn_service::ci::reply::{CiCpuSource, CiEngineCapacity, TaskLaneCapacity};

pub fn format_task_capacity(capacity: Option<&TaskLaneCapacity>) -> String {
    let Some(capacity) = capacity else {
        return "task lane: capacity unavailable".into();
    };
    let cpus = if capacity.cpus_per_slot > 0.0 {
        format!("{} CPUs", capacity.cpus_per_slot)
    } else {
        "unlimited CPUs".into()
    };
    format!(
        "task lane: {} slots x {cpus} per slot (host {} logical CPUs), control slots: {}, stall teardown: {}, docker proxy: {}",
        capacity.slots,
        capacity
            .host_logical_cpus
            .map_or_else(|| "unknown".into(), |cpus| cpus.to_string()),
        capacity.control_slots,
        capacity
            .stall_seconds
            .map_or_else(|| "off".into(), |s| format!("{s}s")),
        if capacity.docker_proxy { "on" } else { "off" },
    )
}

/// One wording for CI capacity wherever the CLI reports it.
pub fn format_ci_capacity(limit: Option<usize>, engine: Option<&CiEngineCapacity>) -> String {
    let admission = limit.map_or_else(|| "unknown".into(), |limit| limit.to_string());
    let Some(engine) = engine else {
        return format!("CI lane: admission limit {admission}; engine limits unavailable");
    };
    let source = match engine.cpu_source {
        CiCpuSource::Config => "[engine] cpus",
        CiCpuSource::DefaultCeiling => "default 8-CPU ceiling",
        CiCpuSource::Unknown => "source unknown (older engine)",
    };
    let state = if engine.running { "running" } else { "next" };
    format!(
        "CI lane: admission limit {admission}; {state} engine {} CPUs ({source}), {} GiB memory",
        engine.cpus,
        engine.memory_bytes / (1 << 30),
    )
}
