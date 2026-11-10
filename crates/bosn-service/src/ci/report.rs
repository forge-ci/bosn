//! Verdicts: folding an engine outcome and a parsed tree into a run
//! conclusion, and the agent-facing failure report. Pure functions.

use super::{
    lifecycle::{CleanupEnd, EngineReport, ExecutionEnd},
    model::{ItemConclusion, RunTree},
    reply::{FailureReport, JobOutcomes, RemoteOnlyJob, RunReport},
    storage,
    wire::{Conclusion, RunRecord, SCHEMA_VERSION},
    workflow::Declared,
};

/// Fold the engine report into the tree and return the run conclusion.
/// Partial coverage and failed cleanup are never a pass.
pub fn conclude(
    outcome: &Result<EngineReport, String>,
    tree: &mut RunTree,
    declared: &Declared,
) -> (Conclusion, Option<String>) {
    let report = match outcome {
        Ok(report) => report,
        Err(error) => {
            tree.cancel_unfinished();
            return (Conclusion::Error, Some(error.clone()));
        }
    };
    let (mut conclusion, mut reason) = match &report.execution {
        ExecutionEnd::Exited(code) => {
            tree.settle_finished();
            tree.add_skipped_steps(&declared.steps);
            tree.mark_remote_only(&declared.remote_only);
            let failed_jobs = tree
                .jobs()
                .filter(|j| j.conclusion == Some(ItemConclusion::Failure))
                .count();
            if *code != 0 || failed_jobs > 0 {
                // The execution's own verdict, in its own words: a later
                // cleanup failure joins this reason (#481) instead of being
                // the only thing the run says.
                (
                    Conclusion::Failure,
                    Some(execution_reason(*code, failed_jobs)),
                )
            } else if !tree.unsupported_jobs().is_empty() {
                (
                    Conclusion::Incomplete,
                    Some(unsupported_summary(&tree.unsupported_reasons())),
                )
            } else if !declared.errors.is_empty() || tree.qualified_coverage_missing(declared) {
                (
                    Conclusion::Incomplete,
                    Some(if declared.errors.is_empty() {
                        "reusable workflows require qualified execution identity".into()
                    } else {
                        format!(
                            "workflow declarations are incomplete: {}",
                            declared.errors.join("; ")
                        )
                    }),
                )
            } else if !tree
                .jobs()
                .any(|j| j.conclusion == Some(ItemConclusion::Success))
            {
                (
                    Conclusion::Incomplete,
                    Some("no job ran for this trigger".into()),
                )
            } else {
                (Conclusion::Success, None)
            }
        }
        ExecutionEnd::TimedOut => {
            tree.cancel_unfinished();
            (
                Conclusion::TimedOut,
                Some("the run exceeded its timeout".into()),
            )
        }
        ExecutionEnd::Cancelled => {
            tree.cancel_unfinished();
            (Conclusion::Cancelled, None)
        }
        ExecutionEnd::EngineFailed(error)
        | ExecutionEnd::EngineLost {
            evidence: error, ..
        } => {
            tree.cancel_unfinished();
            (Conclusion::Error, Some(error.clone()))
        }
    };
    // A failure on nearly full storage says so: the step that tripped over
    // it (a free-space guard, ENOSPC) often explains nothing itself (#392).
    if matches!(conclusion, Conclusion::Failure | Conclusion::Error)
        && let Some(low) = report.storage.and_then(storage::failure_reason)
    {
        reason = Some(joined(reason, low));
    }
    if let CleanupEnd::Failed(error) = &report.cleanup {
        if matches!(conclusion, Conclusion::Success | Conclusion::Incomplete) {
            conclusion = Conclusion::Error;
        }
        reason = Some(joined(reason, format!("engine cleanup failed: {error}")));
    }
    (conclusion, reason)
}

/// Capture before act2 close removes run-* cgroups. Keep act's original code.
pub(super) async fn observe_exit(
    backend: &dyn super::engine::ActEngineBackend,
    lease: &super::shared_engine::Lease,
    plan: &super::lifecycle::EnginePlan,
    code: i32,
) -> ExecutionEnd {
    observe_end(
        backend,
        &lease.engine_id,
        &plan.intent.run_id,
        ExecutionEnd::Exited(code),
    )
    .await
}

pub(super) async fn observe_failure(
    backend: &dyn super::engine::ActEngineBackend,
    lease: &super::shared_engine::Lease,
    plan: &super::lifecycle::EnginePlan,
    error: String,
) -> ExecutionEnd {
    observe_end(
        backend,
        &lease.engine_id,
        &plan.intent.run_id,
        ExecutionEnd::EngineFailed(error),
    )
    .await
}

async fn observe_end(
    backend: &dyn super::engine::ActEngineBackend,
    engine: &str,
    run: &str,
    end: ExecutionEnd,
) -> ExecutionEnd {
    let evidence = backend.engine_evidence(engine).await;
    daemon_log!(
        "bosn ci: engine {engine} run {run} execution evidence: {}",
        evidence.json()
    );
    classify_engine_end(end, &evidence)
}

fn classify_engine_end(
    end: ExecutionEnd,
    evidence: &super::engine::EngineEvidence,
) -> ExecutionEnd {
    use super::engine::EngineObservation;
    let stopped = matches!(evidence.observation, EngineObservation::Stopped(_)) || evidence.missing;
    let lost = stopped
        || (matches!(end, ExecutionEnd::Exited(137))
            && !matches!(evidence.observation, EngineObservation::Running));
    if !lost {
        return end;
    }
    let (exit_code, original) = match end {
        ExecutionEnd::Exited(code) => (Some(code), None),
        ExecutionEnd::EngineFailed(error) => (None, Some(error)),
        other => return other,
    };
    let reason = format!("engine lost: {}", evidence.summary());
    ExecutionEnd::EngineLost {
        exit_code,
        evidence: original.map_or_else(|| reason.clone(), |error| format!("{reason}; {error}")),
    }
}

/// What the execution itself did: act's exit, the jobs that failed, or both.
/// Never mentions cleanup — that status joins this reason separately (#481).
fn execution_reason(code: i32, failed_jobs: usize) -> String {
    match (code != 0, failed_jobs > 0) {
        (true, true) => format!("act exit {code}; {failed_jobs} job(s) failed"),
        (true, false) => format!("act exit {code}"),
        (false, true) => format!("{failed_jobs} job(s) failed"),
        (false, false) => unreachable!("a failure reason needs a failure"),
    }
}

/// `next` after any earlier reason.
fn joined(reason: Option<String>, next: String) -> String {
    match reason {
        Some(first) => format!("{first}; {next}"),
        None => next,
    }
}

/// The `ci report --json` agent contract. `tail(job, section)` returns the
/// last lines of that one section.
/// `ui_origin` is the dashboard's origin while it serves; the report then
/// links straight to this run.
pub fn report(
    record: &RunRecord,
    ui_origin: Option<&str>,
    tail: impl FnOnce(&str, &str) -> Vec<String>,
) -> RunReport {
    let tree = &record.tree;
    let first_failure = tree.first_failure().map(|(job, section)| {
        let key = section.map(|s| format!("{}:{}", s.stage, s.id));
        FailureReport {
            job: job.key.clone(),
            job_name: job.name.clone(),
            tail: key
                .as_deref()
                .map(|k| tail(&job.key, k))
                .unwrap_or_default(),
            section: key,
            step: section.map(|s| s.name.clone()),
            exit_code: section.and_then(|s| s.exit_code),
        }
    });
    let count = |c: ItemConclusion| tree.jobs().filter(|j| j.conclusion == Some(c)).count();
    let unsupported = tree.unsupported_jobs();
    RunReport {
        schema_version: SCHEMA_VERSION,
        run: record.id.clone(),
        state: record.state,
        conclusion: record.conclusion,
        exit_code: record.conclusion.map(Conclusion::exit_code),
        reason: record.reason.clone(),
        sha: record.sha.clone(),
        dirty: record.dirty.clone(),
        workflow: record.workflow.clone(),
        trigger: record.trigger,
        mode: record.mode,
        actor: record.actor.clone(),
        cleanup: record.cleanup.clone(),
        stages: record.stages.clone(),
        first_failure,
        jobs: JobOutcomes {
            succeeded: count(ItemConclusion::Success),
            failed: count(ItemConclusion::Failure),
            cancelled: count(ItemConclusion::Cancelled),
            skipped: tree.skipped_jobs(),
            unsupported: unsupported.clone(),
            remote_only: tree
                .jobs()
                .filter(|j| j.conclusion == Some(ItemConclusion::RemoteOnly))
                .map(|j| RemoteOnlyJob {
                    job: j.key.clone(),
                    reason: j.reason.clone().unwrap_or_default(),
                })
                .collect(),
        },
        coverage_complete: unsupported.is_empty(),
        ui_url: ui_origin.map(|origin| format!("{origin}/ci/runs/{}", record.id)),
        logs_command: format!("bosn ci logs {}", record.id),
    }
}

/// The incomplete run's reason: every unsupported label and how to allow it.
fn unsupported_summary(reasons: &[String]) -> String {
    let base = "some jobs need runners bosn cannot supervise";
    if reasons.is_empty() {
        base.into()
    } else {
        format!("{base}: {}", reasons.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::model::{ActParser, RunTree, parse_act_list};

    #[test]
    fn the_report_lists_skipped_and_unsupported_jobs_and_links_the_dashboard() {
        let list = "Stage  Job ID  Job name  Workflow name  Workflow file  Events\n\
                    0      lin     lin       w              ci.yml         push\n\
                    0      mac     mac       w              ci.yml         push\n\
                    1      late    late      w              ci.yml         push\n";
        let mut parser = ActParser::new(RunTree::declared(&parse_act_list(list)));
        for (seq, line) in [
            r#"{"job":"w/mac","jobID":"mac","level":"info","msg":"🚧  Skipping unsupported platform -- Try running with `-P macos-latest=...`"}"#,
            r#"{"job":"w/lin","jobID":"lin","msg":"⭐ Run Main a","stage":"Main","step":"a","stepID":["0"]}"#,
            r#"{"job":"w/lin","jobID":"lin","msg":"  ✅  Success - Main a","stage":"Main","stepID":["0"],"stepResult":"success"}"#,
            r#"{"job":"w/lin","jobID":"lin","msg":"🏁  Job succeeded","jobResult":"success"}"#,
        ]
        .into_iter()
        .enumerate()
        {
            parser.feed(seq as u64 + 1, line);
        }
        parser.tree.settle_finished();
        let mut record = crate::ci::tests::sample_record("run-1");
        record.tree = parser.tree;
        record.finish(Conclusion::Failure, None);

        let report = report(&record, Some("http://127.0.0.1:7341"), |_, _| Vec::new());
        assert_eq!(report.jobs.succeeded, 1);
        assert_eq!(report.jobs.unsupported, ["w/mac"]);
        let reasons = record.tree.unsupported_reasons();
        assert_eq!(
            reasons,
            [bosn_core::runner_images::unsupported_message(
                "macos-latest"
            )]
        );
        assert!(
            unsupported_summary(&reasons).contains("runs-on macos-latest has no bosn runner image")
        );
        // A job that never ran keeps its declared key, the job ID.
        assert_eq!(report.jobs.skipped, ["late"]);
        assert!(
            !report.coverage_complete,
            "an unsupported job is partial coverage"
        );
        assert_eq!(
            report.ui_url.as_deref(),
            Some("http://127.0.0.1:7341/ci/runs/run-1")
        );
        assert_eq!(report.logs_command, "bosn ci logs run-1");
        let without_ui = super::report(&record, None, |_, _| Vec::new());
        assert_eq!(
            without_ui.ui_url, None,
            "no link while the dashboard is off"
        );
    }

    #[test]
    fn a_remote_only_job_is_reported_with_its_reason_and_never_fails_the_run() {
        use crate::ci::lifecycle::{CleanupEnd, EngineReport, ExecutionEnd};
        use crate::ci::workflow::Declared;
        let list = "Stage  Job ID  Job name         Workflow name  Workflow file  Events\n\
                    0      lin     lin              w              ci.yml         push\n\
                    1      timing  CI queue timing  w              ci.yml         push\n";
        let mut parser = ActParser::new(RunTree::declared(&parse_act_list(list)));
        for (seq, line) in [
            r#"{"job":"w/lin","jobID":"lin","msg":"🏁  Job succeeded","jobResult":"success"}"#,
            r#"{"job":"w/CI queue timing","jobID":"timing","msg":"⭐ Run Main Remote-only","stage":"Main","step":"Remote-only","stepID":["0"]}"#,
            r#"{"job":"w/CI queue timing","jobID":"timing","msg":"  ✅  Success - Main Remote-only","stage":"Main","stepID":["0"],"stepResult":"success"}"#,
            r#"{"job":"w/CI queue timing","jobID":"timing","msg":"🏁  Job succeeded","jobResult":"success"}"#,
        ]
        .into_iter()
        .enumerate()
        {
            parser.feed(seq as u64 + 1, line);
        }
        let mut tree = parser.tree;
        let declared = Declared {
            steps: Default::default(),
            remote_only: [(
                "timing".to_string(),
                "reads this run from the GitHub API".to_string(),
            )]
            .into(),
            ..Declared::default()
        };
        let (conclusion, reason) = conclude(
            &Ok(EngineReport {
                execution: ExecutionEnd::Exited(0),
                cleanup: CleanupEnd::Removed,
                engine_id: None,
                storage: None,
            }),
            &mut tree,
            &declared,
        );
        assert_eq!((conclusion, reason), (Conclusion::Success, None));
        let mut record = crate::ci::tests::sample_record("run-1");
        record.tree = tree;
        record.finish(conclusion, None);
        let report = report(&record, None, |_, _| Vec::new());
        assert_eq!(report.jobs.succeeded, 1, "the stub is no local evidence");
        assert_eq!(
            report.jobs.remote_only,
            [RemoteOnlyJob {
                job: "w/CI queue timing".into(),
                reason: "reads this run from the GitHub API".into(),
            }]
        );
        assert!(report.jobs.unsupported.is_empty() && report.coverage_complete);
    }
    fn exit_137(
        observation: super::super::engine::EngineObservation,
    ) -> (Conclusion, Option<String>) {
        let evidence =
            super::super::engine::EngineEvidence::observed("original-engine", observation);
        let execution = classify_engine_end(ExecutionEnd::Exited(137), &evidence);
        let report = EngineReport {
            execution,
            cleanup: CleanupEnd::Removed,
            engine_id: Some("original-engine".into()),
            storage: None,
        };
        conclude(
            &Ok(report),
            &mut RunTree::declared(&[]),
            &Declared::default(),
        )
    }

    #[test]
    fn exit_137_with_a_stopped_engine_is_an_engine_loss_error() {
        use super::super::engine::{EngineObservation, EngineState};
        let (conclusion, reason) = exit_137(EngineObservation::Stopped(EngineState {
            running: false,
            exit_code: 137,
            oom_killed: true,
            finished_at: "2026-11-10T00:00:00Z".into(),
            status: "exited".into(),
            error: Some(String::new()),
        }));
        assert_eq!(conclusion, Conclusion::Error);
        let reason = reason.unwrap();
        assert!(reason.contains("engine lost:"));
        assert!(reason.contains("OOMKilled=true"));
    }

    #[test]
    fn exit_137_with_a_running_engine_keeps_the_failure() {
        let (conclusion, reason) = exit_137(super::super::engine::EngineObservation::Running);
        assert_eq!(conclusion, Conclusion::Failure);
        assert_eq!(reason.as_deref(), Some("act exit 137"));
    }

    #[test]
    fn engine_loss_keeps_the_actual_exit_code() {
        let evidence = super::super::engine::EngineEvidence::observed(
            "original",
            super::super::engine::EngineObservation::Unknown("inspect deadline".into()),
        );
        assert!(matches!(
            classify_engine_end(ExecutionEnd::Exited(137), &evidence),
            ExecutionEnd::EngineLost {
                exit_code: Some(137),
                ..
            }
        ));
        assert_eq!(
            classify_engine_end(ExecutionEnd::Exited(0), &evidence),
            ExecutionEnd::Exited(0),
            "an uncertain read alone does not turn a normal exit into proven engine loss"
        );
    }
    #[test]
    fn proven_absence_reports_engine_loss_for_an_execution_error() {
        let mut evidence = super::super::engine::EngineEvidence::observed(
            "original",
            super::super::engine::EngineObservation::Unknown("No such container".into()),
        );
        evidence.missing = true;
        assert!(
            matches!(classify_engine_end(ExecutionEnd::EngineFailed("exec connection lost".into()), &evidence), ExecutionEnd::EngineLost { exit_code: None, evidence } if evidence.contains("engine lost:"))
        );
    }
}
