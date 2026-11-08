//! The CI client's refusal classification.

use super::*;

#[test]
fn only_a_definite_refusal_discards_the_staged_snapshot() {
    let refused: Result<(), Error> = Err(Error::Ci {
        code: "refused".into(),
        message: "no".into(),
    });
    assert!(daemon_refused(&refused));
    // The daemon may have accepted the run before the reply was lost.
    assert!(!daemon_refused::<()>(&Err(Error::Deadline)));
    assert!(!daemon_refused::<()>(&Err(Error::Protocol(
        "ci reply decode"
    ))));
    assert!(!daemon_refused(&Ok(())));
}

#[test]
fn ambiguous_loss_retries_only_allowlisted_reads_without_reply_bytes() {
    for operation in 1..=40 {
        let request = Request::operation(operation);
        let eof = Err(Error::Protocol("eof"));
        assert_eq!(
            retry::should_retry(&request, &eof, false),
            matches!(operation, 1 | 2 | 5 | 7 | 11 | 12 | 13 | 37)
        );
        assert!(!retry::should_retry(&request, &eof, true));
        let reset = Err(Error::Io(std::io::ErrorKind::ConnectionReset.into()));
        assert_eq!(
            retry::should_retry(&request, &reset, false),
            matches!(operation, 1 | 2 | 5 | 7 | 11 | 12 | 13 | 37)
        );
        assert!(!retry::should_retry(&request, &reset, true));
        assert!(retry::should_retry(
            &request,
            &Err(Error::DaemonExiting),
            true
        ));
    }
    for op in [
        "submit",
        "retry",
        "cancel",
        "release",
        "widget_poll",
        "ui_grant",
    ] {
        let request = Request {
            ci_request: format!("{{\"op\":\"{op}\"}}"),
            ..Request::operation(36)
        };
        assert!(!retry::should_retry(
            &request,
            &Err(Error::Protocol("eof")),
            false
        ));
    }
}

#[cfg(unix)]
#[test]
fn exiting_run_is_retried_but_eof_run_is_not_and_eof_ping_is() {
    fake_retry(4, Some(handoff::CODE_EXITING), true);
    fake_retry(4, None, false);
    fake_retry(1, None, true);
}

#[cfg(unix)]
fn fake_retry(operation: u32, code: Option<u32>, retries: bool) {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().to_path_buf();
    fake_startup_helper(&state);
    let runtime = async_engine::RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let listener = AsyncListener::bind_owner_only(&endpoint(&state).unwrap()).unwrap();
        let peer = async_engine::launch(async move {
            let mut stream = listener.accept().await.unwrap();
            let frame = read_frame(&mut stream).await.unwrap();
            if let Some(code) = code {
                fake_reply(&mut stream, &frame, code).await;
            }
            drop(stream);
            if !retries {
                assert!(
                    async_engine::timeout(Duration::from_millis(100), listener.accept())
                        .await
                        .is_err()
                );
                return;
            }
            let mut stream = listener.accept().await.unwrap();
            let frame = read_frame(&mut stream).await.unwrap();
            assert_eq!(
                Request::decode(frame.payload()).unwrap().operation,
                operation
            );
            // Only this second request is dispatched, and exactly once.
            fake_reply(&mut stream, &frame, if operation == 1 { 10 } else { 40 }).await;
            assert!(
                async_engine::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let result = Client::for_state(&state)
            .unwrap()
            .call(Request::operation(operation))
            .await;
        if retries {
            assert!(result.is_ok(), "{:?}", result.err());
        } else {
            assert!(matches!(result, Err(Error::Protocol("eof"))));
        }
        async_engine::timeout(Duration::from_secs(5), peer)
            .await
            .unwrap()
            .unwrap();
    });
}

#[cfg(unix)]
async fn fake_reply(stream: &mut AsyncStream, frame: &DaemonFrame, code: u32) {
    let mut payload = Vec::new();
    ReplyWire {
        code,
        ..Default::default()
    }
    .encode(&mut payload)
    .unwrap();
    write_frame(stream, DaemonFrame::response_to(frame, payload))
        .await
        .unwrap();
}

#[cfg(unix)]
fn fake_startup_helper(state: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir(state.join("daemon")).unwrap();
    let executable = state.join("daemon/bosn-daemon");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
#[test]
fn shutdown_probe_does_not_restart_on_eof() {
    let temporary = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
    let state = temporary.path().to_path_buf();
    fake_startup_helper(&state);
    let runtime = async_engine::RuntimeBuilder::multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.run(async {
        let listener = AsyncListener::bind_owner_only(&endpoint(&state).unwrap()).unwrap();
        let peer = async_engine::launch(async move {
            let mut stream = listener.accept().await.unwrap();
            let frame = read_frame(&mut stream).await.unwrap();
            assert_eq!(Request::decode(frame.payload()).unwrap().operation, 3);
            fake_reply(&mut stream, &frame, 30).await;
            drop(stream);
            let mut stream = listener.accept().await.unwrap();
            let frame = read_frame(&mut stream).await.unwrap();
            assert_eq!(Request::decode(frame.payload()).unwrap().operation, 1);
            drop(stream);
            assert!(
                async_engine::timeout(Duration::from_millis(200), listener.accept())
                    .await
                    .is_err()
            );
        });
        Client::for_state(&state)
            .unwrap()
            .shutdown_and_wait(Duration::from_secs(2))
            .await
            .unwrap();
        peer.await.unwrap();
    });
}
