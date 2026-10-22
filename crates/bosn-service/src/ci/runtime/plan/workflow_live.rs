//! Real workflow execution through ordinary admission on a private Docker daemon.
use super::*;
use crate::ci::{
    engine::ActEngineBackend,
    lifecycle::tests::{
        OWNER,
        native_live::{Fixture, await_claim_release, fixture_backend, require_protection},
        with_registry,
    },
};

const WORKFLOW: &str = r#"name: native tools acceptance
on: push
jobs:
  install:
    runs-on: ubuntu-latest
    steps:
      - name: Preserve warm state and install completed tool
        shell: bash
        run: |
          set -euo pipefail
          test "$(cat "$RUNNER_TOOL_CACHE/Example/1/x64/tool")" = preserved-tool
          if [ "$(cat phase)" = recovered ]; then
            test "$(cat "$RUNNER_TOOL_CACHE/Workflow/1/x64/tool")" = workflow-update
            test "$(stat -c %a "$RUNNER_TOOL_CACHE/Workflow/1/x64/tool")" = 751
            test ! -e "$RUNNER_TOOL_CACHE/Unfinished"
          fi
          mkdir -p "$RUNNER_TOOL_CACHE/Workflow/1/x64" "$RUNNER_TOOL_CACHE/Unfinished/1/x64"
          printf workflow-update >"$RUNNER_TOOL_CACHE/Workflow/1/x64/tool"
          chmod 751 "$RUNNER_TOOL_CACHE/Workflow/1/x64/tool"
          touch "$RUNNER_TOOL_CACHE/Workflow/1/x64.complete"
          printf unfinished >"$RUNNER_TOOL_CACHE/Unfinished/1/x64/tool"
  reuse:
    needs: install
    runs-on: ubuntu-latest
    steps:
      - name: Reuse tool from preceding job
        shell: bash
        run: |
          set -euo pipefail
          test "$(cat "$RUNNER_TOOL_CACHE/Workflow/1/x64/tool")" = workflow-update
          test "$(stat -c %a "$RUNNER_TOOL_CACHE/Workflow/1/x64/tool")" = 751
          test -f "$RUNNER_TOOL_CACHE/Workflow/1/x64.complete"
"#;

async fn workflow_run(runtime: &CiRuntime, phase: &str, sha: char) -> RunRecord {
    let staging = crate::ci::new_uuid().await.unwrap();
    let source = runtime.staging_dir(&staging).join("source");
    let workflows = source.join(".github/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("ci.yml"), WORKFLOW).unwrap();
    std::fs::write(source.join("phase"), phase).unwrap();
    let mut request = crate::ci::tests::request(&staging, sha);
    request.timeout_secs = Some(900);
    let reply = runtime
        .handle(CiRequest::Submit {
            request: Box::new(request),
        })
        .await
        .unwrap();
    let submitted: SubmitReply = serde_json::from_value(reply).unwrap();
    async_engine::timeout(Duration::from_secs(960), async {
        loop {
            let record = runtime.record(&submitted.run).unwrap();
            if record.state == RunState::Done {
                assert_eq!(record.conclusion, Some(Conclusion::Success), "{record:?}");
                let view = RunView::of(record.clone(), true);
                assert_eq!(view.jobs.total, 2);
                assert_eq!(view.jobs.completed, 2);
                assert_eq!(view.jobs.failed, 0);
                return record;
            }
            async_engine::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("ordinary workflow must reach a terminal state")
}

#[test]
#[ignore = "requires a private Docker daemon, pinned runner/archive and exact cleanup"]
fn actual_runtime_workflows_recover_completed_tools_across_fresh_engines() {
    let path = std::env::var("BOSN_NATIVE_ADMISSION_FIXTURE").unwrap();
    let fixture: Fixture = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(fixture.act_version, crate::ci::engine::ACT_VERSION);
    with_registry(|registry, dir| async move {
        std::fs::write(
            dir.join("config.toml"),
            "[engine]\nmemory_gib=4\nstorage_gib=24\ncpus=2\n",
        )
        .unwrap();
        let backend = fixture_backend(&fixture).await;
        let stop = CancellationSource::new();
        let maintenance_backend = backend.clone();
        let maintenance_registry = registry.clone();
        let maintenance_stop = stop.token();
        let maintenance = async_engine::launch(async move {
            maintenance_backend
                .maintain_existing_cohort(&maintenance_registry, OWNER, &maintenance_stop)
                .await;
        });
        let runtime = CiRuntime::start(&dir, registry.clone(), backend.clone(), 1);
        let first = workflow_run(&runtime, "initial", 'a').await;
        let first_engine = runtime.shared.status().await.unwrap().0;
        require_protection(&registry, &first_engine).await;
        assert!(runtime.shared.retire_idle(Duration::ZERO).await);
        await_claim_release(&runtime.shared, &backend).await;
        let second = workflow_run(&runtime, "recovered", 'b').await;
        let second_engine = runtime.shared.status().await.unwrap().0;
        assert_ne!(first_engine, second_engine);
        require_protection(&registry, &second_engine).await;
        assert!(runtime.shared.retire_idle(Duration::ZERO).await);
        await_claim_release(&runtime.shared, &backend).await;
        runtime.close_shared().await;
        stop.cancel();
        let _ = maintenance.await;
        assert!(
            backend
                .machine()
                .unwrap()
                .engines()
                .await
                .unwrap()
                .is_empty()
        );
        std::fs::write(
            fixture.report,
            serde_json::to_vec(&serde_json::json!({
                "passed": true, "workflow": WORKFLOW,
                "first": first, "second": second,
                "first_engine": first_engine, "second_engine": second_engine,
                "claims_released": true,
            }))
            .unwrap(),
        )
        .unwrap();
    });
}
