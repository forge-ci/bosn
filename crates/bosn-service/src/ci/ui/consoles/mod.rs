//! Authenticated Dell consoles: typed targets, embedded viewer, disposable TLS bridge.
mod catalog;
mod transport;

use super::{
    CiRuntime,
    routes::{Route, RouteError},
};
use kernal_api::{async_engine, http_server};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

macro_rules! asset {
    ($name:literal) => {
        include_str!(concat!("../assets/", $name))
    };
}
const HTML: &str = asset!("console.html");
const SCRIPT: &str = asset!("console.js");
const STYLE: &str = asset!("console.css");
const VIEWER: &str = asset!("novnc.js");
const LICENSE: &str = asset!("novnc.LICENSE.txt");
pub struct SessionLease(Arc<AtomicUsize>);
impl SessionLease {
    fn acquire(activity: Arc<AtomicUsize>) -> Option<Self> {
        activity
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 4).then_some(n + 1)
            })
            .ok()
            .map(|_| Self(activity))
    }
}
impl Drop for SessionLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, PartialEq)]
pub enum ConsoleRoute {
    List,
    Page,
    Script,
    Style,
    Viewer,
    License,
    Socket { id: String },
}

/// Called only after the main router's closed set, preserving method refusals.
pub fn route(method: &str, path: &str) -> Result<Route, RouteError> {
    let segments: Vec<_> = path.trim_matches('/').split('/').collect();
    let console = match segments.as_slice() {
        ["v1", "consoles"] => ConsoleRoute::List,
        ["v1", "consoles", id, "socket"] if catalog::valid_id(id) => {
            ConsoleRoute::Socket { id: (*id).into() }
        }
        ["console", "console.js"] => ConsoleRoute::Script,
        ["console", "console.css"] => ConsoleRoute::Style,
        ["console", "novnc.js"] => ConsoleRoute::Viewer,
        ["console", "LICENSE.txt"] => ConsoleRoute::License,
        ["console", id] if catalog::valid_id(id) => ConsoleRoute::Page,
        _ => return Err(RouteError::NotFound),
    };
    if method != "GET" {
        return Err(RouteError::MethodNotAllowed);
    }
    Ok(Route::Console(console))
}

fn asset_response(text: &str, content_type: &str) -> http_server::Response {
    http_server::Response::new(200, text.as_bytes().to_vec())
        .and_then(|r| r.with_header("content-type", content_type))
        .and_then(|r| r.with_header("cache-control", "no-store"))
        .unwrap_or_else(|_| super::text(500, "asset unavailable"))
}

pub async fn respond(
    ci: &CiRuntime,
    route: ConsoleRoute,
    request: http_server::Request,
) -> http_server::Response {
    match route {
        ConsoleRoute::Page => return asset_response(HTML, "text/html; charset=utf-8"),
        ConsoleRoute::Script => return asset_response(SCRIPT, "text/javascript; charset=utf-8"),
        ConsoleRoute::Style => return asset_response(STYLE, "text/css; charset=utf-8"),
        ConsoleRoute::Viewer => return asset_response(VIEWER, "text/javascript; charset=utf-8"),
        ConsoleRoute::License => return asset_response(LICENSE, "text/plain; charset=utf-8"),
        _ => {}
    }
    let state = ci.state_dir().to_owned();
    let targets = match async_engine::launch_blocking(move || catalog::load(&state)).await {
        Ok(Ok(targets)) => targets,
        _ => return super::json(400, r#"{"code":"invalid_server_config","message":"Check servers.toml in the Bosn state directory."}"#.into()),
    };
    match route {
        ConsoleRoute::List => {
            let summaries: Vec<_> = targets.iter().map(catalog::Target::summary).collect();
            match serde_json::to_string(&summaries) {
                Ok(json) => super::json(200, json),
                Err(_) => super::text(500, "server list unavailable"),
            }
        }
        ConsoleRoute::Socket { id } => {
            let Some(target) = targets.into_iter().find(|target| target.id == id) else {
                return super::text(404, "no such console target");
            };
            let admission = ci.admission();
            let _guard = admission.lock().await;
            if ci.admission_closed().load(Ordering::Acquire) {
                return super::text(503, "daemon is exiting");
            }
            let Some(activity) = ci.console_activity() else {
                return super::text(503, "UI is starting");
            };
            let Some(lease) = SessionLease::acquire(activity) else {
                return super::text(429, "four consoles are already open");
            };
            let Ok(registry) = ci.console_registry_id().await else {
                return super::text(503, "registry unavailable");
            };
            let workspace = ci.state_dir().to_string_lossy().into_owned();
            let Ok(upgrade) = request.into_websocket() else {
                return super::text(400, "WebSocket upgrade required");
            };
            upgrade
                .on_upgrade(http_server::WebSocketLimits::default(), move |socket| {
                    transport::serve(socket, target, registry, workspace, lease)
                })
                .unwrap_or_else(|_| super::text(400, "invalid WebSocket upgrade"))
        }
        _ => unreachable!("static console assets returned above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_known_targets_and_authenticated_same_origin_upgrades_are_routable() {
        let route = route("GET", "/v1/consoles/rack-1/socket").unwrap();
        assert!(route.needs_session());
        assert!(
            route.is_write(),
            "WebSocket GET needs the same Origin check as a write"
        );
        assert_eq!(
            super::route("POST", "/v1/consoles"),
            Err(RouteError::MethodNotAllowed)
        );
        assert_eq!(
            super::route("GET", "/v1/consoles/../socket"),
            Err(RouteError::NotFound)
        );
        assert_eq!(
            super::route("GET", "/console/../../password"),
            Err(RouteError::NotFound)
        );
    }
}
