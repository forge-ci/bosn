//! Real HTTP authorization boundaries, before any Docker or iDRAC access.
use super::*;

#[test]
fn consoles_require_a_session_and_websocket_gets_require_the_same_origin() {
    with_registry(|registry, dir| async move {
        std::fs::write(dir.join("servers.toml"), format!(
            "[[servers]]\nid='rack'\nname='Dell rack'\nhost='192.0.2.1'\nport=5901\ncertificate_sha256='{}'\n", "ab".repeat(32)
        )).unwrap();
        let (ci, _server, port) = listener(&dir, registry).await;
        let host = format!("127.0.0.1:{port}");
        for path in [
            "/console/rack",
            "/console/novnc.js",
            "/v1/consoles",
            "/v1/consoles/rack/socket",
        ] {
            assert_eq!(raw(port, get(port, path, &host, None)).await.status, 401);
        }
        let cookie = sign_in(&ci, port).await;
        let list = raw(port, get(port, "/v1/consoles", &host, Some(&cookie))).await;
        assert_eq!(list.status, 200);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&list.body).unwrap(),
            serde_json::json!([{"id":"rack", "name":"Dell rack"}])
        );
        assert!(!list.body.contains("192.0.2.1"));
        for origin in [None, Some("http://evil.invalid")] {
            let request = get(port, "/v1/consoles/rack/socket", &host, Some(&cookie));
            let request = origin.map_or(request.clone(), |origin| {
                request.replace(
                    "Connection: close",
                    &format!("Origin: {origin}\r\nConnection: close"),
                )
            });
            assert_eq!(raw(port, request).await.status, 403);
        }
        let request = get(port, "/v1/consoles/rack/socket", &host, Some(&cookie)).replace(
            "Connection: close",
            &format!("Origin: http://{host}\r\nConnection: close"),
        );
        assert_eq!(
            raw(port, request).await.status,
            400,
            "no WebSocket handshake, no Docker work"
        );
        assert_eq!(
            ci.active_consoles(),
            0,
            "a failed upgrade releases its session reservation"
        );
        let page = raw(port, get(port, "/console/rack", &host, Some(&cookie))).await;
        assert_eq!(page.status, 200);
        assert!(page.body.contains("/console/console.js"));
    });
}
