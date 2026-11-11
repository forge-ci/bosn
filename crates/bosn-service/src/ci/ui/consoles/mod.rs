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

pub async fn respond(
    ci: &CiRuntime,
    route: ConsoleRoute,
    request: http_server::Request,
) -> http_server::Response {
    match route {
        ConsoleRoute::Page => return super::page::response("/console/"),
        ConsoleRoute::Script => return super::page::response("/console/console.js"),
        ConsoleRoute::Style => return super::page::response("/console/console.css"),
        ConsoleRoute::Viewer => return super::page::response("/console/novnc.js"),
        ConsoleRoute::License => return super::page::response("/console/LICENSE.txt"),
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
