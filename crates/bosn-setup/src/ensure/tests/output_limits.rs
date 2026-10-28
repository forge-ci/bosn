use super::*;

#[test]
fn output_limit_does_not_charge_private_observations() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(workspace.join("src")).unwrap();
    let plan = plan(&workspace);
    let image = prepared(&plan);
    let engine = FakeEngine::with_results([Ok(SetupEnsureResponse::Inspection(
        Some(observed(&plan, true)),
        result(0, vec![b'x'; 8192], []),
    ))]);
    let cancellation = CancellationSource::new();
    let result = run(
        &engine,
        &plan,
        &workspace,
        &image,
        &cancellation.token(),
        RunOptions::streaming(Duration::from_secs(2), 4096),
    );
    assert!(
        result.is_ok(),
        "private inspection exceeded caller budget: {result:?}"
    );
}

#[cfg(all(unix, feature = "native-test-helper"))]
#[test]
fn structured_inspection_environment_is_captured_privately_not_forwarded() {
    let engine = DockerEngine::synthetic_for_test(
        "/bin/sh",
        [
            "-c",
            "printf '%s' '{\"Config\":{\"Env\":[\"SECRET=private-canary\"]}}'",
            "sh",
        ],
    );
    let cancellation = CancellationSource::new();
    let (events, mut receiver) = channel(8);
    let result = runtime()
        .run(SetupEnsureEngine::stream(
            &engine,
            SetupEnsureCommand::ImageInspect {
                image_identity: IDENTITY.into(),
            },
            RunOptions::streaming(Duration::from_secs(2), 4096),
            &cancellation.token(),
            &events,
        ))
        .unwrap();
    let SetupEnsureResponse::Command(result) = result else {
        panic!("expected private command receipt");
    };
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("private-canary")
    );
    drop(events);
    assert!(runtime().run(receiver.recv()).is_none());
}

struct CaptureBudget {
    inner: FakeEngine,
    limits: Mutex<Vec<(SetupEnsureCommand, usize)>>,
}

impl SetupEnsureEngine for CaptureBudget {
    type StreamFuture<'a> = Ready<Result<SetupEnsureResponse, CommandError>>;

    fn stream<'a>(
        &'a self,
        command: SetupEnsureCommand,
        options: RunOptions,
        cancellation: &'a CancellationToken,
        events: &'a Sender<EngineEvent>,
    ) -> Self::StreamFuture<'a> {
        self.limits
            .lock()
            .unwrap()
            .push((command.clone(), options.output_limit));
        self.inner.stream(command, options, cancellation, events)
    }
}

#[test]
fn control_receipts_use_fixed_budget_even_with_one_byte_caller_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(workspace.join("src")).unwrap();
    let plan = plan(&workspace);
    let image = prepared(&plan);
    let engine = CaptureBudget {
        inner: FakeEngine::with_results([absent(), command(CONTAINER_ID), command("started")]),
        limits: Mutex::new(Vec::new()),
    };
    let cancellation = CancellationSource::new();
    let (events, _receiver) = channel(8);
    let result = runtime().run(ensure_setup_app(
        &engine,
        SetupEnsureRequest {
            plan: &plan,
            workspace_root: workspace,
            prepared_image: &image,
            options: RunOptions::streaming(Duration::from_secs(2), 1),
            cancellation: &cancellation.token(),
            events: &events,
        },
    ));
    assert!(
        result.is_ok(),
        "control protocol must not use caller budget: {result:?}"
    );
    let limits = engine.limits.lock().unwrap();
    assert!(
        limits
            .iter()
            .any(|(c, _)| matches!(c, SetupEnsureCommand::Create { .. }))
    );
    assert!(
        limits
            .iter()
            .any(|(c, _)| matches!(c, SetupEnsureCommand::Start { .. }))
    );
    assert!(
        limits
            .iter()
            .all(|(_, n)| *n == crate::creation::MAX_OBSERVATION_BYTES)
    );
}

#[cfg(all(unix, feature = "native-test-helper"))]
#[test]
fn control_mutation_receipts_are_not_forwarded_to_job_events() {
    let engine =
        DockerEngine::synthetic_for_test("/bin/sh", ["-c", "printf private-control-canary", "sh"]);
    let cancellation = CancellationSource::new();
    let (events, mut receiver) = channel(8);
    let response = runtime()
        .run(SetupEnsureEngine::stream(
            &engine,
            SetupEnsureCommand::Start {
                container_name: "ignored".into(),
            },
            RunOptions::streaming(Duration::from_secs(2), 4096),
            &cancellation.token(),
            &events,
        ))
        .unwrap();
    let SetupEnsureResponse::Command(result) = response else {
        panic!("expected receipt")
    };
    assert_eq!(result.stdout, b"private-control-canary");
    drop(events);
    assert!(runtime().run(receiver.recv()).is_none());
}

#[test]
fn private_control_failure_does_not_expose_error_output() {
    let response = result(9, [], b"private-secret-canary".to_vec());
    let error = action_failed("container create", &response).to_string();
    assert!(error.contains("exit 9"));
    assert!(!error.contains("private-secret-canary"));
    let conflict = result(1, [], b"private-canary is already in use".to_vec());
    let error = action_failed("container create", &conflict).to_string();
    assert!(error.contains("is already in use"));
    assert!(!error.contains("private-canary"));
}
