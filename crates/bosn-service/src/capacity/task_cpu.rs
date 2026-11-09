//! Task CPU sizing is independent of CI admission and engine sizing.
use super::RunnerCapacity;
use bosn_core::manifest::CpuRequest;

pub(crate) fn resolve_task_cpus(
    request: Option<CpuRequest>,
    capacity: &RunnerCapacity,
    available: f64,
    other: f64,
) -> f64 {
    let base = capacity.cpus_per_slot;
    let wanted = match request {
        None => return base,
        Some(CpuRequest::Fixed(n)) => n.get(),
        Some(CpuRequest::Auto) => (available - other).max(base),
    };
    let max = capacity.max_task_cpus.unwrap_or(base);
    // Preserve the operator's existing cpus=0 (unlimited) policy.
    if max == 0.0 { wanted } else { wanted.min(max) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixed(n: f64) -> CpuRequest {
        let manifest =
            format!("[stack.dev]\nimage = 'test'\n[task.test]\ncmd = 'true'\ncpus = {n}");
        bosn_core::manifest::parse_manifest_toml(
            &manifest,
            bosn_core::manifest::ManifestRoots::new("/tmp", "/tmp", "test"),
        )
        .unwrap()
        .tasks["test"]
            .cpus
            .unwrap()
    }
    #[test]
    fn requests_respect_cap_and_busy_slots() {
        let mut c = RunnerCapacity::defaults(Some(16));
        assert_eq!(
            resolve_task_cpus(Some(CpuRequest::Auto), &c, 16.0, 0.0),
            4.0
        );
        c.max_task_cpus = Some(8.0);
        for (request, other, expected) in [
            (None, 0.0, 4.0),
            (Some(fixed(0.25)), 0.0, 0.25),
            (Some(fixed(16.0)), 0.0, 8.0),
            (Some(CpuRequest::Auto), 10.0, 6.0),
            (Some(CpuRequest::Auto), 15.0, 4.0),
        ] {
            assert_eq!(resolve_task_cpus(request, &c, 16.0, other), expected);
        }
    }
    #[test]
    fn operator_precedence_and_dynamic_default() {
        let mut c = RunnerCapacity::defaults(Some(16));
        c.apply_file("cpus = 2").unwrap();
        assert_eq!(
            resolve_task_cpus(Some(CpuRequest::Auto), &c, 16.0, 0.0),
            2.0
        );
        c.apply_file("max_task_cpus = 6").unwrap();
        c.apply_env(|k| (k == "BOSN_MAX_TASK_CPUS").then(|| "8".into()))
            .unwrap();
        assert_eq!(c.max_task_cpus, Some(8.0));
        c.set("max_task_cpus", "10").unwrap();
        assert_eq!(c.max_task_cpus, Some(10.0));
        for bad in ["0", "-1", "NaN", "inf", "no"] {
            assert!(c.set("max_task_cpus", bad).is_err());
        }
    }
}
