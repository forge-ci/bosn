//! Different task names contend only when their manifest volume is exclusive.
use super::stack_task_docker::{StackWorkspace, stack_manifest};
use crate::support::setup_docker::*;
use bosn_service::{CleanVolumes, Client, ManifestAppTaskJobRequest};
use kernal_api::async_engine;
use std::time::Instant;

fn request(stack: &StackWorkspace, task: &str, wait: bool) -> ManifestAppTaskJobRequest {
    ManifestAppTaskJobRequest {
        workspace: stack.workspace.clone(),
        manifest: "bosn.toml".into(),
        stack: "live".into(),
        task_name: task.into(),
        args: vec![],
        clean: CleanVolumes::None,
        wait,
        deadline: Duration::from_secs(90),
        output_limit: 1024 * 1024,
    }
}

async fn status(client: &Client, job: u64, waiting: bool) -> bosn_service::JobStatus {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let status = client.job_status(job).await.unwrap();
        if (waiting && status.waiting_on.is_some())
            || (!waiting && matches!(status.state.as_str(), "Succeeded" | "Failed" | "Cancelled"))
        {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "job {job}: {status:?}; logs: {:?}",
            client.job_logs(job, 0, 256).await.unwrap()
        );
        async_engine::sleep(Duration::from_millis(100)).await;
    }
}

async fn ready(client: &Client, job: u64) {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let logs = client.job_logs(job, 0, 256).await.unwrap();
        if logs.records.iter().any(|r| r.line.contains("holder-ready")) {
            return;
        }
        assert!(Instant::now() < deadline, "holder did not start");
        async_engine::sleep(Duration::from_millis(100)).await;
    }
}

fn workspace(shared: bool) -> StackWorkspace {
    let marker = test_unique_suffix();
    let manifest = stack_manifest(
        &marker,
        &[
            (
                "holder",
                "echo holder-ready; while [ ! -e /ws/release ]; do sleep 0.1; done",
            ),
            ("other", "echo second-finished"),
        ],
    )
    .replace("keep = true", "keep = false")
    .replace(
        "[stack.live]",
        "[stack.live]\ndefault_target_volume = false",
    )
    .replace(
        "dockerfile = 'live.Dockerfile'",
        &format!("image = '{PINNED_ALPINE}'"),
    );
    let manifest = if shared {
        manifest
            .replace(
                "[stack.live]",
                &format!("[stack.live]\nfamily = '{marker}'"),
            )
            .replace(
                "scope = 'stack', destination = '/state'",
                "scope = 'machine', mode = 'shared', destination = '/state'",
            )
    } else {
        manifest
    };
    let stack = StackWorkspace::new(&manifest);
    stack.run_task("other");
    stack
}

// All objects carry this isolated registry's labels, including volumes after ephemeral removal.
fn cleanup_volumes(stack: &StackWorkspace) {
    let registry = Registry::open_read_only(stack.state.join("registry.sqlite3"))
        .unwrap()
        .registry_id()
        .unwrap();
    let output = docker_capture(
        &stack.engine,
        [
            "volume",
            "ls",
            "-q",
            "--filter",
            &format!("label={}={registry}", bosn_core::LABEL_REGISTRY),
        ],
    );
    assert!(output.ok());
    for name in String::from_utf8(output.stdout).unwrap().lines() {
        assert!(docker_capture(&stack.engine, ["volume", "rm", name]).ok());
    }
}

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_shared_machine_volume_allows_different_task_names() {
    let stack = workspace(true);
    RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let client = Client::for_state(&stack.state).unwrap();
            let a = client
                .submit_manifest_app_task(request(&stack, "holder", false))
                .await
                .unwrap();
            ready(&client, a).await;
            let b = client
                .submit_manifest_app_task(request(&stack, "other", false))
                .await
                .unwrap();
            let completed = status(&client, b, false).await;
            assert_eq!(completed.state, "Succeeded", "{completed:?}");
            assert_eq!(client.job_status(a).await.unwrap().state, "Running");
            std::fs::write(stack.workspace.join("release"), "release").unwrap();
            assert_eq!(status(&client, a, false).await.state, "Succeeded");
        });
    cleanup_volumes(&stack);
}

#[test]
#[ignore = "requires a local Docker daemon and the pinned Alpine image"]
fn live_docker_exclusive_stack_volume_refuses_or_waits_for_different_task_names() {
    let stack = workspace(false);
    RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let client = Client::for_state(&stack.state).unwrap();
            let a = client
                .submit_manifest_app_task(request(&stack, "holder", false))
                .await
                .unwrap();
            ready(&client, a).await;
            let b = client
                .submit_manifest_app_task(request(&stack, "other", false))
                .await
                .unwrap();
            let refused = status(&client, b, false).await;
            assert_eq!(refused.state, "Failed", "{refused:?}");
            let error = refused.error.unwrap();
            assert!(
                error.contains("volume") && error.contains("holder"),
                "{error}"
            );
            let b = client
                .submit_manifest_app_task(request(&stack, "other", true))
                .await
                .unwrap();
            assert!(
                status(&client, b, true)
                    .await
                    .waiting_on
                    .unwrap()
                    .contains("bosn-v-")
            );
            std::fs::write(stack.workspace.join("release"), "release").unwrap();
            assert_eq!(status(&client, a, false).await.state, "Succeeded");
            let completed = status(&client, b, false).await;
            assert_eq!(completed.state, "Succeeded", "{completed:?}");
            assert!(completed.waiting_on.is_none());
        });
    cleanup_volumes(&stack);
}
