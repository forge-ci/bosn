use super::*;

/// Stop waiting if preparation finishes a run without reaching execution.
pub(super) async fn wait_for_executions(runtime: &CiRuntime, backend: &FakeBackend, runs: &[&str]) {
    let expected = u32::try_from(runs.len()).unwrap();
    let wait = async {
        while *backend.executions.lock().unwrap() < expected {
            for run in runs {
                let record = runtime.record(run).unwrap();
                assert_ne!(
                    record.state,
                    RunState::Done,
                    "run finished before execution began: {record:?}"
                );
            }
            async_engine::sleep(Duration::from_millis(5)).await;
        }
    };
    async_engine::timeout(Duration::from_secs(30), wait)
        .await
        .expect("execution must begin within the run budget");
}
