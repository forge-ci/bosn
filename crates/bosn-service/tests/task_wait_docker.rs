//! Isolated named-container contention; every object belongs to StackWorkspace.
use super::stack_task_docker::{StackWorkspace, stack_manifest};
use crate::support::setup_docker::*;
use bosn_service::{CleanVolumes, Client, ManifestAppTaskJobRequest};
use kernal_api::async_engine;
use std::time::Instant;

fn request(stack: &StackWorkspace, arg: &str, wait: bool) -> ManifestAppTaskJobRequest {
    ManifestAppTaskJobRequest {
        workspace: stack.workspace.clone(),
        manifest: "bosn.toml".into(),
        stack: "live".into(),
        task_name: "probe".into(),
        args: vec![arg.into()],
        clean: CleanVolumes::None,
        wait,
        deadline: Duration::from_secs(90),
        output_limit: 1024 * 1024,
    }
}

async fn until(client: &Client, job: u64, waiting: bool) -> bosn_service::JobStatus {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let status = client.job_status(job).await.unwrap();
        if (waiting && status.waiting_on.is_some())
            || (!waiting && matches!(status.state.as_str(), "Succeeded" | "Failed" | "Cancelled"))
        {
            return status;
        }
        assert!(Instant::now() < deadline, "job {job}: {status:?}");
        async_engine::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_task_wait_resumes_and_cancellation_leaves_holder_alone() {
    let manifest = stack_manifest(&test_unique_suffix(), &[("probe",
        "if [ \"$1\" = hold ]; then echo holder-ready; while [ ! -e /ws/release ]; do sleep 0.1; done; fi; echo finished")])
        .replace("keep = true", "keep = false");
    let stack = StackWorkspace::new(&manifest);
    // Warm preparation before the holder is admitted.
    stack
        .bosn_run("probe")
        .args(["--", "warm"])
        .output()
        .map(|o| StackWorkspace::output_of("probe", o))
        .unwrap();
    RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let client = Client::for_state(&stack.state).unwrap();
            let a = client
                .submit_manifest_app_task(request(&stack, "hold", false))
                .await
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                let logs = client.job_logs(a, 0, 256).await.unwrap();
                if logs.records.iter().any(|r| r.line.contains("holder-ready")) {
                    break;
                }
                assert!(Instant::now() < deadline, "holder did not start");
                async_engine::sleep(Duration::from_millis(100)).await;
            }
            let b = client
                .submit_manifest_app_task(request(&stack, "cancel", true))
                .await
                .unwrap();
            let blocked = until(&client, b, true).await;
            assert!(blocked.waiting_on.unwrap().starts_with("probe-"));
            let jobs = client.jobs().await.unwrap();
            assert!(jobs.to_string().contains("waiting_on"), "{jobs}");
            let c = client
                .submit_manifest_app_task(request(&stack, "refuse", false))
                .await
                .unwrap();
            let refused = until(&client, c, false).await;
            assert_eq!(refused.state, "Failed");
            assert!(refused.error.unwrap().contains("is busy"));
            client.cancel_job(b).await.unwrap();
            let cancelled = until(&client, b, false).await;
            assert_eq!(cancelled.state, "Cancelled");
            assert!(cancelled.waiting_on.is_none());
            assert_eq!(client.job_status(a).await.unwrap().state, "Running");
            let d = client
                .submit_manifest_app_task(request(&stack, "resume", true))
                .await
                .unwrap();
            until(&client, d, true).await;
            std::fs::write(stack.workspace.join("release"), "release").unwrap();
            assert_eq!(until(&client, a, false).await.state, "Succeeded");
            let resumed = until(&client, d, false).await;
            assert_eq!(resumed.state, "Succeeded", "{resumed:?}");
            assert!(resumed.waiting_on.is_none());
            let raced = until(&client, e, false).await;
            assert_eq!(raced.state, "Succeeded", "{raced:?}");
        });
}
