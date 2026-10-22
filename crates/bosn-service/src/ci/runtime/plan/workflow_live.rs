//! Real workflow execution through ordinary admission on a private Docker daemon.
use super::*;
use crate::ci::{
    engine::ActEngineBackend,
    lifecycle::tests::{
        OWNER,
        native_live::{
            Fixture, await_claim_release, fixture_backend, fixture_engine_exec, require_protection,
        },
        with_registry,
    },
};

const WORKFLOW: &str = r#"name: native tools and archives acceptance
on: push
jobs:
  install:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/cache@v4
        id: archives
        with:
          path: .acceptance-cache
          key: bosn-508-workflow-archives-v1
      - name: Compile and package new state or verify its restored provenance
        shell: bash
        env:
          CACHE_HIT: ${{ steps.archives.outputs.cache-hit }}
        run: |
          set -euo pipefail
          test -f phase
          if [ "$(cat phase)" = recovered ]; then
            test "$CACHE_HIT" = true
            (cd .acceptance-cache && sha256sum -c provenance.sha256)
          else
            test "$CACHE_HIT" != true
            mkdir -p .acceptance-cache/package
            printf '#include <stdio.h>\nint main(void){puts("compiled-by-workflow-508");}\n' >.acceptance-cache/source.c
            gcc .acceptance-cache/source.c -o .acceptance-cache/compiled
            printf '{"name":"bosn-508-acceptance","version":"1.0.0"}\n' >.acceptance-cache/package/package.json
            printf workflow-package-508 >.acceptance-cache/package/payload
            (cd .acceptance-cache/package && npm pack --ignore-scripts)
            (cd .acceptance-cache && sha256sum source.c compiled package/*.tgz >provenance.sha256)
          fi
          test "$(.acceptance-cache/compiled)" = compiled-by-workflow-508
          test "$(tar -xOf .acceptance-cache/package/*.tgz package/payload)" = workflow-package-508
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
      - uses: actions/checkout@v4
      - uses: actions/cache@v4
        id: archives
        with:
          path: .acceptance-cache
          key: bosn-508-workflow-archives-v1
      - name: Verify archive bytes saved by the preceding job
        shell: bash
        env:
          CACHE_HIT: ${{ steps.archives.outputs.cache-hit }}
        run: |
          set -euo pipefail
          test "$CACHE_HIT" = true
          (cd .acceptance-cache && sha256sum -c provenance.sha256)
          test "$(.acceptance-cache/compiled)" = compiled-by-workflow-508
          test "$(tar -xOf .acceptance-cache/package/*.tgz package/payload)" = workflow-package-508
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
                assert_eq!(
                    record.conclusion,
                    Some(Conclusion::Success),
                    "{record:?}\n{}",
                    serde_json::to_string(
                        &runtime
                            .logs(
                                &record.id,
                                Default::default(),
                                record.log_records.saturating_sub(200),
                                200,
                                128 << 10,
                            )
                            .unwrap()
                    )
                    .unwrap(),
                );
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

async fn require_archive_source_reclaimed(engine: &str, record: &RunRecord) {
    let namespace = crate::ci::cache_cohort::Namespace::parse(&record.cache_namespace()).unwrap();
    let script = format!(
        r#"set -eu
source={}
test -f "$source"
snapshot=
for candidate in "${{source%/*}}"/."${{source##*/}}".archive-enrollment-*; do
    test ! -L "$candidate"
    [ -d "$candidate" ] || continue
    test -z "$snapshot"
    snapshot=$candidate
done
test -n "$snapshot"
test -f "$snapshot/.archive-source-identity-v1"
remaining=$(find "$snapshot" -mindepth 1 -maxdepth 1 ! -name .archive-source-identity-v1 -exec printf x \;)
if [ -n "$remaining" ]; then
    printf 'archive source still retains payload entries\n' >&2
    exit 1
fi
printf archive-source-reclaimed
"#,
        namespace.legacy_path()
    );
    assert_eq!(
        fixture_engine_exec(engine, &script).await,
        "archive-source-reclaimed"
    );
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
        require_archive_source_reclaimed(&first_engine, &first).await;
        require_protection(&registry, &first_engine).await;
        assert!(runtime.shared.retire_idle(Duration::ZERO).await);
        await_claim_release(&runtime.shared, &backend).await;
        let second = workflow_run(&runtime, "recovered", 'b').await;
        let second_engine = runtime.shared.status().await.unwrap().0;
        require_archive_source_reclaimed(&second_engine, &second).await;
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
                "archive_sources_reclaimed": true,
            }))
            .unwrap(),
        )
        .unwrap();
    });
}
