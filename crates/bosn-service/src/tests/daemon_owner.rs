//! Work owned by a client process ends when that process does (2026-10-16).

use super::*;

struct BlockingTask {
    started: async_engine::Sender<String>,
}
impl ManifestAppTaskExecutor for BlockingTask {
    fn execute<'a>(
        &'a self,
        request: ManifestAppTaskJobRequest,
        cancellation: &'a async_engine::CancellationToken,
        _logs: &'a crate::raw_run_log::JobLogSink,
        _session: &'a dyn ManifestAppTaskSessionRecorder,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.started.send(request.task_name).await;
            cancellation.cancelled().await;
            Err("cancelled".into())
        })
    }
}

/// The daemon is the second line of defense: a client killed outright never
/// cancels its work, so the daemon notices the owner's exit and cancels the
/// work itself, running or queued, well inside the follow lease. Work owned
/// by a live process, and detached work, keep running.
#[test]
fn a_dead_owners_jobs_are_cancelled_and_a_live_owners_are_not() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let (started, mut started_wait) = async_engine::channel(8);
    let fake = Arc::new(BlockingTask { started });
    let request = |task: &str| ManifestAppTaskJobRequest {
        workspace: workspace.clone(),
        manifest: "bosn.toml".into(),
        stack: "app".into(),
        task_name: task.into(),
        deadline: Duration::from_secs(600),
        output_limit: 4096,
    };
    let mut doomed = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .unwrap();
    let doomed_owner = ClientOwner::of_pid(doomed.id()).unwrap();
    RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let server = async_engine::launch(
                Service::new(state.clone())
                    .with_manifest_app_task_executor(fake.clone())
                    .serve(),
            );
            let client = wait_for_client(&state).await;
            let submit = |task: &'static str, owner: Option<ClientOwner>| {
                client.submit_manifest_app_task_with_lease(request(task), None, owner)
            };
            let running = submit("running", Some(doomed_owner)).await.unwrap();
            assert_eq!(next_start(&mut started_wait).await, "running");
            let queued = submit("queued", Some(doomed_owner)).await.unwrap();
            async_engine::sleep(Duration::from_secs(1)).await;
            assert_eq!(client.job_status(running).await.unwrap().state, "Running");

            // SIGKILL: the client gets no chance to cancel anything.
            doomed.kill().unwrap();
            let killed = Instant::now();
            wait_for_job_state(&client, running, "Cancelled").await;
            wait_for_job_state(&client, queued, "Cancelled").await;
            assert!(
                killed.elapsed() < Duration::from_secs(3),
                "cancelled within a few sweeps, not at a lease or deadline"
            );
            let log = client.job_logs(running, 0, 16).await.unwrap();
            let pid = format!("pid {}", doomed_owner.pid());
            assert!(
                log.records
                    .iter()
                    .any(|record| record.line.contains("exited") && record.line.contains(&pid)),
                "the job log names the client that went away"
            );

            for owner in [ClientOwner::current(), None] {
                let task = if owner.is_some() { "owned" } else { "detached" };
                let id = submit(task, owner).await.unwrap();
                assert_eq!(next_start(&mut started_wait).await, task);
                async_engine::sleep(Duration::from_secs(2)).await;
                assert_eq!(client.job_status(id).await.unwrap().state, "Running");
                client.cancel_job(id).await.unwrap();
                wait_for_job_state(&client, id, "Cancelled").await;
            }

            client.shutdown().await.unwrap();
            stopped(server).await;
        });
    doomed.wait().unwrap();
}

async fn next_start(wait: &mut async_engine::Receiver<String>) -> String {
    async_engine::timeout(Duration::from_secs(5), wait.recv())
        .await
        .unwrap()
        .unwrap()
}
