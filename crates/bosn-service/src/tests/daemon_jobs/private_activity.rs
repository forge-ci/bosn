use super::*;

struct PrivateTicking {
    started: async_engine::Sender<String>,
}
impl ManifestAppTaskExecutor for PrivateTicking {
    fn execute<'a>(
        &'a self,
        request: ManifestAppTaskJobRequest,
        cancellation: &'a async_engine::CancellationToken,
        logs: &'a crate::raw_run_log::JobLogSink,
        _session: &'a dyn ManifestAppTaskSessionRecorder,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let _ = self
                .started
                .send(request.workspace.to_string_lossy().into_owned())
                .await;
            loop {
                if async_engine::cancellable(
                    cancellation,
                    async_engine::sleep(Duration::from_millis(200)),
                )
                .await
                .is_err()
                {
                    return Err("cancelled".into());
                }
                logs.activity().await?;
            }
        })
    }
}

#[test]
fn private_setup_progress_prevents_stall_without_public_log_spam() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let (started, mut started_wait) = async_engine::channel(4);
    let fake = Arc::new(PrivateTicking { started });
    let state = temporary.path().join("state");
    RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let server = async_engine::launch(
                Service::new(state.clone())
                    .with_manifest_app_task_executor(fake)
                    .with_runner_capacity(test_capacity(4, Some(Duration::from_secs(1))))
                    .serve(),
            );
            let client = wait_for_client(&state).await;
            let id = client
                .submit_manifest_app_task(app_task("/w/private"))
                .await
                .unwrap();
            next_task_start(&mut started_wait).await;
            async_engine::sleep(Duration::from_secs(3)).await;
            assert_eq!(client.job_status(id).await.unwrap().state, "Running");
            let page = client.job_logs(id, 0, 64).await.unwrap();
            assert!(
                !page
                    .records
                    .iter()
                    .any(|r| r.line.contains("tick") || r.line.contains("stalled"))
            );
            client.cancel_job(id).await.unwrap();
            wait_for_job_state(&client, id, "Cancelled").await;
            client.shutdown().await.unwrap();
            stopped(server).await;
        });
}
