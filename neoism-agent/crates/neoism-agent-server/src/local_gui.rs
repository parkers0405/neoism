//! Automatic local GUI launch session. No daemon/operator secret is sent to JS.
//! Only a top-level, user/OS-initiated navigation on a loopback-bound listener
//! receives an HttpOnly session. Cross-site navigation, frames, forwarded hosts,
//! remote listeners and hosted credentials cannot bootstrap registry authority.
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use neoism_agent_service_api::server_registry::{
    RegistryError, ServerEntry, ServerRegistry,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
const COOKIE: &str = "neoism_local_gui";
const TTL: Duration = Duration::from_secs(12 * 60 * 60);
pub(crate) const REGISTRY_PATH: &str = "/__neoism/gui/servers";
pub(crate) const SHARE_TARGET_PATH: &str = "/__neoism/gui/share-target";
#[derive(Clone)]
pub(crate) struct LocalGui {
    port: u16,
    local_token: Option<String>,
    directory: PathBuf,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    launches: Arc<Mutex<HashMap<String, Instant>>>,
}
impl LocalGui {
    pub(crate) fn new(address: SocketAddr, directory: PathBuf) -> Option<Self> {
        address.ip().is_loopback().then(|| Self {
            port: address.port(),
            local_token: None,
            directory,
            sessions: Default::default(),
            launches: Default::default(),
        })
    }
    pub(crate) fn with_local_token(mut self, token: Option<String>) -> Self {
        self.local_token = token;
        self
    }
    fn local_request(&self, request: &Request<Body>) -> bool {
        let Some(ConnectInfo(peer)) =
            request.extensions().get::<ConnectInfo<SocketAddr>>()
        else {
            return false;
        };
        if !peer.ip().is_loopback() {
            return false;
        }
        let headers = request.headers();
        if ["forwarded", "x-forwarded-for", "x-forwarded-host"]
            .iter()
            .any(|h| headers.contains_key(*h))
        {
            return false;
        }
        let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
            return false;
        };
        let Ok(url) = url::Url::parse(&format!("http://{host}")) else {
            return false;
        };
        if !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            || url.port_or_known_default() != Some(self.port)
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return false;
        }
        if let Some(origin) = headers.get(header::ORIGIN) {
            if origin.to_str().ok() != Some(url.origin().ascii_serialization().as_str()) {
                return false;
            }
        }
        true
    }
    fn session(&self, request: &Request<Body>) -> bool {
        let value = request
            .headers()
            .get(header::COOKIE)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| {
                h.split(';')
                    .find_map(|part| part.trim().strip_prefix(&format!("{COOKIE}=")))
            });
        let Some(value) = value else {
            return false;
        };
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(value)
            .is_some_and(|time| time.elapsed() < TTL)
    }
    pub(crate) fn navigation_cookie(&self, request: &Request<Body>) -> Option<String> {
        if !self.local_request(request) || request.method() != Method::GET {
            return None;
        }
        let headers = request.headers();
        let supplied = headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));
        match self.local_token.as_deref() {
            Some(expected)
                if !self.session(request)
                    && !supplied.is_some_and(|value| {
                        crate::caller::constant_time_eq(
                            value.as_bytes(),
                            expected.as_bytes(),
                        )
                    }) =>
            {
                return None
            }
            None if headers.contains_key(header::AUTHORIZATION) => return None,
            _ => {}
        }
        if headers.get("sec-fetch-mode")?.to_str().ok()? != "navigate"
            || headers.get("sec-fetch-dest")?.to_str().ok()? != "document"
        {
            return None;
        }
        // Same-origin reloads retain a prior launch session. A remote page's
        // window.open/iframe cannot mint one merely by making the app load.
        let site = headers.get("sec-fetch-site")?.to_str().ok()?;
        if site != "none" && !(site == "same-origin" && self.session(request)) {
            return None;
        }
        Some(self.mint_cookie())
    }
    fn mint_cookie(&self) -> String {
        let token = rand::random::<[u8; 32]>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, time| time.elapsed() < TTL);
        if sessions.len() >= 128 {
            if let Some(key) = sessions
                .iter()
                .min_by_key(|(_, time)| **time)
                .map(|(key, _)| key.clone())
            {
                sessions.remove(&key);
            }
        }
        sessions.insert(token.clone(), Instant::now());
        // Restrict the cookie to this private GUI endpoint namespace; it must
        // not accompany unrelated localhost application/API requests.
        format!(
            "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/__neoism/gui; Max-Age={}",
            TTL.as_secs()
        )
    }

    /// Loopback operator GUI only: the sibling workspace-daemon HTTP origin.
    pub(crate) fn share_target(&self, request: &Request<Body>) -> Response {
        if !self.local_request(request) || request.method() != Method::GET {
            return StatusCode::FORBIDDEN.into_response();
        }
        let Some(port) = std::env::var("NEOISM_DAEMON_TCP_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port != 0)
        else {
            return Json(serde_json::json!({ "daemon": "http://127.0.0.1:7878" }))
                .into_response();
        };
        Json(serde_json::json!({ "daemon": format!("http://127.0.0.1:{port}") }))
            .into_response()
    }

    /// OS CLI handoff: existing local API auth is verified server-side. Only a
    /// one-use, 60-second GUI ticket enters the launch URL, never the API bearer.
    pub(crate) fn launch(&self, request: Request<Body>) -> Response {
        if !self.local_request(&request) {
            return StatusCode::FORBIDDEN.into_response();
        }
        let path = request.uri().path();
        let mut response = if path == "/__neoism/gui/launch"
            && request.method() == Method::POST
        {
            let supplied = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.strip_prefix("Bearer "));
            let authenticated = match &self.local_token {
                Some(expected) => supplied.is_some_and(|token| {
                    crate::caller::constant_time_eq(token.as_bytes(), expected.as_bytes())
                }),
                None => !request.headers().contains_key(header::AUTHORIZATION),
            };
            if !authenticated
                || request
                    .headers()
                    .get("x-neoism-launcher")
                    .and_then(|h| h.to_str().ok())
                    != Some("1")
            {
                return StatusCode::FORBIDDEN.into_response();
            }
            let ticket = rand::random::<[u8; 32]>()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let mut launches = self.launches.lock().unwrap_or_else(|e| e.into_inner());
            launches.retain(|_, time| time.elapsed() < Duration::from_secs(60));
            if launches.len() >= 128 {
                return StatusCode::TOO_MANY_REQUESTS.into_response();
            }
            launches.insert(ticket.clone(), Instant::now());
            Json(serde_json::json!({ "path": format!("/__neoism/gui/launch/{ticket}") }))
                .into_response()
        } else if let Some(ticket) = path
            .strip_prefix("/__neoism/gui/launch/")
            .filter(|_| request.method() == Method::GET)
        {
            let valid = self
                .launches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(ticket)
                .is_some_and(|time| time.elapsed() < Duration::from_secs(60));
            if !valid {
                return StatusCode::FORBIDDEN.into_response();
            }
            let mut response = StatusCode::SEE_OTHER.into_response();
            response
                .headers_mut()
                .insert(header::LOCATION, "/".parse().unwrap());
            response
                .headers_mut()
                .insert(header::SET_COOKIE, self.mint_cookie().parse().unwrap());
            response
        } else {
            StatusCode::NOT_FOUND.into_response()
        };
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        response
            .headers_mut()
            .insert("referrer-policy", "no-referrer".parse().unwrap());
        response
            .headers_mut()
            .insert("x-frame-options", "DENY".parse().unwrap());
        response
    }
    pub(crate) async fn registry(&self, request: Request<Body>) -> Response {
        if !self.local_request(&request)
            || request.headers().contains_key(header::AUTHORIZATION)
            || request
                .headers()
                .get("sec-fetch-site")
                .and_then(|h| h.to_str().ok())
                != Some("same-origin")
            || request
                .headers()
                .get("x-neoism-gui")
                .and_then(|h| h.to_str().ok())
                != Some("1")
            || !self.session(&request)
        {
            return (StatusCode::FORBIDDEN, "local GUI session required").into_response();
        }
        let method = request.method().clone();
        if !matches!(method, Method::GET | Method::POST | Method::DELETE) {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let bytes = match axum::body::to_bytes(request.into_body(), 65536).await {
            Ok(b) => b,
            Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        };
        let action = match method {
            Method::POST => match serde_json::from_slice::<Save>(&bytes) {
                Ok(s) => Action::Save(s),
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            },
            Method::DELETE => match serde_json::from_slice::<ServerEntry>(&bytes) {
                Ok(s) => Action::Remove(s),
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            },
            _ => Action::List,
        };
        let directory = self.directory.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut registry = ServerRegistry::load(directory).map_err(RegistryError::Io)?;
            match action {
                Action::List => Ok(serde_json::json!({ "capability": "neoism.operator.server-registry", "scope": "daemon-os-user", "servers": registry.entries() })),
                Action::Save(s) => registry.save_entry(s.entry, s.expected).map(|entry| serde_json::json!({ "entry": entry })),
                Action::Remove(s) => registry.remove_entry(s).map(|_| serde_json::json!({ "removed": true })),
            }
        }).await;
        let mut response = match result {
            Ok(Ok(value)) => Json(value).into_response(),
            Ok(Err(RegistryError::Conflict)) => (
                StatusCode::CONFLICT,
                "server changed; reload before editing",
            )
                .into_response(),
            Ok(Err(RegistryError::Invalid(message))) => {
                (StatusCode::BAD_REQUEST, message).into_response()
            }
            _ => (StatusCode::SERVICE_UNAVAILABLE, "saved servers unavailable")
                .into_response(),
        };
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        response
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    entry: ServerEntry,
    expected: Option<ServerEntry>,
}
enum Action {
    List,
    Save(Save),
    Remove(ServerEntry),
}
