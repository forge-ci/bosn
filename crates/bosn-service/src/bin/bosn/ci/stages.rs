//! Concise setup phase diagnostics.

pub(super) fn print_setup_stages(stages: &[bosn_service::ci::wire::StageTiming]) {
    for stage in stages {
        let elapsed = stage
            .elapsed_ms
            .map_or("unavailable".to_owned(), |ms| format!("{ms}ms"));
        let reason = stage.reason.map_or(String::new(), |reason| {
            format!("; reason {}", reason.as_str())
        });
        let command = stage
            .command_limit_secs
            .map_or(String::new(), |secs| format!("; command limit {secs}s"));
        let act2 = stage
            .act2_operation_limit_secs
            .map_or(String::new(), |secs| {
                format!("; act2 operation limit {secs}s")
            });
        let nested = if stage.shorter_internal_limits_possible {
            "; shorter internal limits possible"
        } else {
            ""
        };
        let remaining = stage
            .run_remaining_ms
            .map_or(String::new(), |ms| format!("; run budget at start {ms}ms"));
        println!(
            "  setup {}: {} ({elapsed}; outer limit {}s{command}{act2}{nested}{remaining}{reason})",
            stage.kind.as_str(),
            stage.state.as_str(),
            stage.outer_limit_secs
        );
    }
}
