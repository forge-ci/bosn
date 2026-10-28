use super::*;

#[test]
fn private_setup_output_is_recorded_and_refreshes_activity_without_public_lines() {
    let runtime = async_engine::RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let id = "00000000-0000-4000-8000-000000000001";
    let raw = crate::raw_run_log::RawRunLog::create(dir.path(), id).unwrap();
    let (text, mut public) = async_engine::channel(8);
    let activity = std::sync::Arc::new(std::sync::Mutex::new(None));
    let logs = crate::raw_run_log::JobLogSink::durable(text, raw).with_progress(activity.clone());
    let (sender, receiver) = async_engine::channel(8);
    runtime.run(async {
        sender
            .send(EngineEvent::Stderr(b"private setup SECRET-".to_vec()))
            .await
            .unwrap();
        sender
            .send(EngineEvent::Stderr(b"CANARY diagnostic".to_vec()))
            .await
            .unwrap();
        drop(sender);
        drain_private_setup_output(receiver, &logs, SecretMasker::new(["SECRET-CANARY"]))
            .await
            .unwrap();
        assert!(activity.lock().unwrap().is_some());
    });
    drop(logs);
    assert!(runtime.run(public.recv()).is_none());
    let chunks = crate::raw_run_log::read_since(dir.path(), id, 0, 8).unwrap();
    let bytes: Vec<_> = chunks.into_iter().flat_map(|chunk| chunk.bytes).collect();
    assert_eq!(bytes, b"private setup *** diagnostic");
    runtime.run(async {
        let server = crate::run_http::start(dir.path()).await.unwrap();
        let token = std::fs::read_to_string(dir.path().join("run-http.token")).unwrap();
        let port = server.local_addr.port();
        let request = format!("GET /v1/runs/{id}/logs/stderr HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n");
        let response = async_engine::launch_blocking(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut data = Vec::new();
            stream.read_to_end(&mut data).unwrap();
            data
        }).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 206"), "{}", String::from_utf8_lossy(&response));
        assert!(response.ends_with(b"private setup *** diagnostic"));
        assert!(!response.windows(b"SECRET-CANARY".len()).any(|w| w == b"SECRET-CANARY"));
    });
}
