use super::*;

fn request(args: &[&str]) -> ManifestAppTaskJobRequest {
    ManifestAppTaskJobRequest {
        clean: Default::default(),
        wait: false,
        workspace: PathBuf::from("/workspace"),
        manifest: "bosn.toml".into(),
        stack: "app".into(),
        task_name: "check".into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
        deadline: Duration::from_secs(30),
        output_limit: 4096,
    }
}

#[test]
fn argument_identity_joins_only_identical_arguments_and_jobs_show_them() {
    let first = request(&["a", "b c"]);
    let other = request(&["a b", "c"]);
    let mut jobs = Jobs::new(4);
    let submit = |jobs: &mut Jobs, request: &ManifestAppTaskJobRequest| {
        jobs.submit_class(
            "/workspace",
            &manifest_task_key(request),
            &manifest_app_task_digest(request),
            jobs::JobClass::Runner,
        )
        .unwrap()
    };
    let Submission::Started(id) = submit(&mut jobs, &first) else {
        panic!("started")
    };
    jobs.set_args(id, first.args.clone());
    assert_eq!(submit(&mut jobs, &first), Submission::Joined(id));
    let Submission::Started(other_id) = submit(&mut jobs, &other) else {
        panic!("distinct job")
    };
    assert_ne!(id, other_id);
    let view = jobs_view(&jobs, None);
    let row = view["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap();
    assert_eq!(row["args"], serde_json::json!(["a", "b c"]));
}

#[test]
fn previous_protocol_task_request_without_args_still_decodes() {
    let wire = Request {
        client_protocol: DAEMON_PROTOCOL_MIN,
        workspace: "/workspace".into(),
        stack: "app".into(),
        setup_config: "bosn.toml".into(),
        setup_task_name: "check".into(),
        setup_deadline_ms: 1000,
        setup_output_limit: 4096,
        ..Request::operation(23)
    };
    let decoded = Request::decode(wire.encode_to_vec().as_slice()).unwrap();
    assert!(decoded.task_args.is_empty());
    validate_manifest_app_task_request_wire(&decoded).unwrap();
    assert!(
        DaemonIdentity {
            release: "test".into(),
            protocol: DAEMON_PROTOCOL,
            protocol_min: DAEMON_PROTOCOL_MIN,
            build: None
        }
        .serves(decoded.client_protocol)
    );
}

#[test]
fn old_daemon_refuses_arguments_before_any_job_submission() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let server = async_engine::launch(
                Service::new(state.clone())
                    .with_protocol_window(1, 2)
                    .serve(),
            );
            let client = wait_for_client(&state).await;
            let error = client
                .submit_manifest_app_task(request(&["a"]))
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("task arguments require daemon protocol 3"),
                "{error}"
            );
            let stop = Request {
                client_protocol: 2,
                ..Request::operation(3)
            };
            let mut stream = AsyncStream::connect(&endpoint(&state).unwrap())
                .await
                .unwrap();
            write_frame(
                &mut stream,
                DaemonFrame::request(PAYLOAD_PROTOCOL, stop.encode_to_vec()),
            )
            .await
            .unwrap();
            read_frame(&mut stream).await.unwrap();
            stopped(server).await;
        });
}

#[test]
fn guest_shell_quoting_preserves_apostrophes() {
    assert_eq!(shell_single_quote("a'b"), "'a'\"'\"'b'");
}

#[test]
fn old_daemon_refuses_clean_before_any_job_submission() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let server = async_engine::launch(
                Service::new(state.clone())
                    .with_protocol_window(2, 3)
                    .serve(),
            );
            let client = wait_for_client(&state).await;
            let error = client
                .submit_manifest_app_task({
                    let mut r = request(&[]);
                    r.clean = CleanVolumes::Checkout;
                    r
                })
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("--clean requires daemon protocol 4"),
                "{error}"
            );
            let stop = Request {
                client_protocol: 3,
                ..Request::operation(3)
            };
            let mut stream = AsyncStream::connect(&endpoint(&state).unwrap())
                .await
                .unwrap();
            write_frame(
                &mut stream,
                DaemonFrame::request(PAYLOAD_PROTOCOL, stop.encode_to_vec()),
            )
            .await
            .unwrap();
            read_frame(&mut stream).await.unwrap();
            stopped(server).await;
        });
}

#[test]
fn wait_tag_round_trip_and_old_peer_refusal() {
    use prost::Message;
    let mut request = Request::operation(23);
    request.task_wait = Some(true);
    let decoded = Request::decode(request.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.task_wait, Some(true));
    for protocol in 0..5 {
        assert!(crate::task_wait::require_protocol(true, protocol).is_err());
        assert!(crate::task_wait::require_protocol(false, protocol).is_ok());
    }
    assert!(crate::task_wait::require_protocol(true, 5).is_ok());
}

#[test]
fn old_daemon_refuses_wait_before_any_job_submission() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().join("state");
    RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let server = async_engine::launch(
                Service::new(state.clone())
                    .with_protocol_window(3, 4)
                    .serve(),
            );
            let client = wait_for_client(&state).await;
            let error = client
                .submit_manifest_app_task({
                    let mut r = request(&[]);
                    r.wait = true;
                    r
                })
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("--wait requires daemon protocol 5"),
                "{error}"
            );
            let stop = Request {
                client_protocol: 4,
                ..Request::operation(3)
            };
            let mut stream = AsyncStream::connect(&endpoint(&state).unwrap())
                .await
                .unwrap();
            write_frame(
                &mut stream,
                DaemonFrame::request(PAYLOAD_PROTOCOL, stop.encode_to_vec()),
            )
            .await
            .unwrap();
            read_frame(&mut stream).await.unwrap();
            stopped(server).await;
        });
}

#[test]
fn wait_participates_in_job_identity_and_blocked_status_clears_on_cancel() {
    let mut first = request(&["a"]);
    let original = manifest_app_task_digest(&first);
    first.wait = true;
    assert_ne!(original, manifest_app_task_digest(&first));
    let mut jobs = Jobs::new(2);
    let Submission::Started(id) = jobs.submit("/workspace", "task", "digest").unwrap() else {
        panic!("started")
    };
    jobs.waiting_on(id, Some("probe-checkout".into()));
    assert_eq!(
        jobs.job(id).unwrap().waiting_on.as_deref(),
        Some("probe-checkout")
    );
    jobs.cancel(id).unwrap();
    jobs.settle(id, false).unwrap();
    assert!(jobs.job(id).unwrap().waiting_on.is_none());
}
