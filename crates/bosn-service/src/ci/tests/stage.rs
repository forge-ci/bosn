//! Setup stage reporting over the runtime.

use super::*;

#[test]
fn ci_stage_timeouts_reach_status_and_report() {
    with_registry(|registry, dir| async move {
        let backend = Arc::new(FakeBackend::with(Faults {
            prepare_delay: Duration::from_millis(1500),
            ..Faults::default()
        }));
        let runtime = CiRuntime::start(&dir, registry, backend, 1);
        let staging = new_uuid().await.unwrap();
        staged(&runtime, &staging);
        let mut request = request(&staging, '7');
        request.timeout_secs = Some(30);
        let submitted: SubmitReply = call(
            &runtime,
            CiRequest::Submit {
                request: Box::new(request),
            },
        )
        .await;
        let run = submitted.run;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let observed = runtime.record(&run).unwrap();
            if observed.stages.iter().any(|stage| {
                stage.kind == StageKind::EnginePrepare && stage.state == StageState::Running
            }) {
                assert!(observed.stages.last().unwrap().elapsed_ms.is_some());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "running preparation was not observed"
            );
            async_engine::sleep(Duration::from_millis(10)).await;
        }
        let done = wait_done(&runtime, &run).await;
        let stage = done
            .stages
            .iter()
            .find(|stage| stage.kind == StageKind::EnginePrepare)
            .expect("engine preparation observed");
        assert_eq!(stage.state, StageState::Succeeded);
        assert!(stage.elapsed_ms.is_some());
        assert!(stage.run_remaining_ms.is_some());
        let view: RunView = call(
            &runtime,
            CiRequest::Show {
                run: run.clone(),
                tree: None,
            },
        )
        .await;
        let report: RunReport = call(&runtime, CiRequest::Report { run, tail: None }).await;
        assert_eq!(view.record.stages, done.stages);
        assert_eq!(report.stages, done.stages);
        runtime.close_shared().await;
    });
}

#[test]
fn ci_stage_timeouts_cancelled_stage_has_measured_elapsed() {
    with_registry(|registry, dir| async move {
        let backend = Arc::new(FakeBackend::with(Faults {
            prepare_delay: Duration::from_secs(10),
            ..Faults::default()
        }));
        let runtime = CiRuntime::start(&dir, registry, backend, 1);
        let staging = new_uuid().await.unwrap();
        staged(&runtime, &staging);
        let mut request = request(&staging, '8');
        request.timeout_secs = Some(30);
        let submitted: SubmitReply = call(
            &runtime,
            CiRequest::Submit {
                request: Box::new(request),
            },
        )
        .await;
        let run = submitted.run;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let observed = runtime.record(&run).unwrap();
            if observed.stages.iter().any(|stage| {
                stage.kind == StageKind::EnginePrepare && stage.state == StageState::Running
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "running preparation was not observed"
            );
            async_engine::sleep(Duration::from_millis(10)).await;
        }
        let _: CancelReply = call(&runtime, CiRequest::Cancel { run: run.clone() }).await;
        let done = wait_done(&runtime, &run).await;
        let stage = done
            .stages
            .iter()
            .find(|stage| stage.kind == StageKind::EnginePrepare)
            .unwrap();
        assert_eq!(stage.state, StageState::Cancelled);
        assert_eq!(stage.reason, Some(StageReason::Cancelled));
        assert!(stage.elapsed_ms.is_some());
        assert!(
            done.cleanup
                .as_deref()
                .unwrap_or_default()
                .starts_with("unconfirmed")
        );
        runtime.close_shared().await;
    });
}
