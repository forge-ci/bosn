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
pub fn format_ci_capacity(
    limit: Option<usize>,
    engine: Option<&CiEngineCapacity>,
    error: Option<&str>,
) -> String {
    let admission = limit.map_or_else(|| "unknown".into(), |limit| limit.to_string());
    let Some(engine) = engine else {
        return match error {
            Some(error) => {
                format!("CI lane: admission limit {admission}; engine limits unavailable: {error}")
            }
            None => format!("CI lane: admission limit {admission}; engine limits unavailable"),
        };
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

/// Same placement and kernel values in doctor and jobs.
pub fn format_slice_cpu(report: Option<&bosn_engine::SliceCpu>) -> String {
    let Some(report) = report else {
        return "CPU placement: unavailable; cpu.max: unavailable; cpu.weight: unavailable".into();
    };
    let maximum = report.cpu_max.as_ref().map_or_else(
        || "unavailable".into(),
        |maximum| match maximum.cpus() {
            Some(cpus) => format!(
                "{} {} ({cpus} CPUs)",
                maximum.quota.unwrap_or_default(),
                maximum.period
            ),
            None => format!("max {} (unlimited)", maximum.period),
        },
    );
    let weight = report
        .cpu_weight
        .map_or_else(|| "unavailable".into(), |weight| weight.to_string());
    format!(
        "CPU placement: {}; cpu.max: {maximum}; cpu.weight: {weight}",
        report.placement
    )
}

#[cfg(test)]
mod slice_tests {
    use super::*;
    use bosn_service::ci::reply::{DoctorReply, RunnersReply};

    #[test]
    fn doctor_and_jobs_render_slice_cpu_text_and_json() {
        let slice = serde_json::json!({
            "placement": "bosn.slice",
            "cpu_max": {"quota": 200000, "period": 100000},
            "cpu_weight": 10
        });
        let doctor: DoctorReply = serde_json::from_value(serde_json::json!({
            "engine": null, "report": null, "slice_cpu": slice
        }))
        .unwrap();
        let runners: RunnersReply = serde_json::from_value(serde_json::json!({
            "runners": {"limit": 2, "running": 0, "queued": 0, "drained": false,
                "engine": "act", "act_version": "act2.19", "widget": "absent"},
            "pruned_runs": null, "slice_cpu": slice
        }))
        .unwrap();
        for report in [doctor.slice_cpu.as_ref(), runners.slice_cpu.as_ref()] {
            assert_eq!(
                format_slice_cpu(report),
                "CPU placement: bosn.slice; cpu.max: 200000 100000 (2 CPUs); cpu.weight: 10"
            );
        }
        assert_eq!(serde_json::to_value(doctor).unwrap()["slice_cpu"], slice);
        assert_eq!(serde_json::to_value(runners).unwrap()["slice_cpu"], slice);
    }
}
