//! Daemon-side agent proxy.
//!
//! Two layers live here:
//!
//! 1. The original **direct Claude proxy** that POSTs to
//!    `https://api.anthropic.com/v1/messages` with `stream=true`,
//!    parses SSE `data:` lines, and forwards each event as an
//!    [`AgentServerMessage`]. This is preserved verbatim so the web
//!    frontend keeps working in the no-agent-server / no-API-key
//!    fallback path.
//! 2. The **agent-server proxy** that forwards every new
//!    [`AgentClientMessage`] variant (session lifecycle, prompts,
//!    permissions, edits, config, catalogs, maintenance) to the
//!    embedded `neoism-agent-server` HTTP API on
//!    `http://127.0.0.1:4096` and translates its SSE event stream
//!    into typed [`AgentServerMessage`] variants. This mirrors the
//!    desktop pane's `neoism::agent::api` + `updates` pipeline but
//!    runs inside the daemon so the web chrome can drive the full
//!    Claude-Code-style runtime over the WebSocket.
//!
//! API-key gating only applies to layer 1 — the agent-server brings
//! its own auth/provider story, so layer-2 envelopes pass through
//! even when `NEOISM_AGENT_API_KEY` is unset.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use neoism_protocol::agent::{
    AgentClientMessage, AgentInfo, AgentServerMessage, Attachment, CompactionPhase,
    ContentKind, HistoryMessage, HistoryMessageKind, ModelInfo, NoticeLevel,
    PermissionDecision, ProviderInfo, Role, SkillInfo, StreamingState, SubagentStatus,
    ThreadSummary, TodoItem, ToolStatus, Usage,
};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

/// Anthropic streaming endpoint (layer 1 — direct proxy).
const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
/// Anthropic API version pin (matches their docs as of 2026-Q2).
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Default per-request output budget. Conservative; the model can
/// always end the turn earlier with `end_turn`.
const DEFAULT_MAX_TOKENS: u32 = 4096;
/// Default agent-server base URL when `NEOISM_AGENT_SERVER` is unset.
const DEFAULT_AGENT_SERVER: &str = "http://127.0.0.1:4096";
const AGENT_SERVER_HEALTH_PATH: &str = "/global/health";
const AGENT_SERVER_READY_TIMEOUT: Duration = Duration::from_millis(1500);
const AGENT_SERVER_READY_POLL: Duration = Duration::from_millis(50);

/// One-shot guard that boots the embedded agent-server exactly once
/// per process. Mirrors `frontends/neoism/src/agent_server.rs::ensure_started`.
static AGENT_SERVER_STARTED: OnceLock<()> = OnceLock::new();

/// Spawn the embedded `neoism-agent-server` on `127.0.0.1:4096` if
/// `NEOISM_AGENT_SERVER` points at a local host and the daemon hasn't
/// already booted one. Safe to call repeatedly — the first call wins.
/// Each WebSocket connection invokes this so an agent-server is
/// guaranteed to be reachable before the first envelope is routed.
pub fn ensure_agent_server_started() {
    if AGENT_SERVER_STARTED.get().is_some() {
        return;
    }
    let server = configured_agent_server();
    // Re-publish the resolved URL so child crates (e.g. agent-server
    // helpers in this module) read the same value.
    std::env::set_var("NEOISM_SERVER", &server);
    let Some((hostname, port)) = local_bind_target(&server) else {
        tracing::warn!(
            target: "neoism_workspace_daemon::agent",
            server,
            "skipping embedded agent-server start (non-local NEOISM_AGENT_SERVER)"
        );
        let _ = AGENT_SERVER_STARTED.set(());
        return;
    };
    if AGENT_SERVER_STARTED.set(()).is_err() {
        return;
    }
    // Supervisor rather than a one-shot: when another process (usually
    // the desktop app) already owns the port, `listen` exits with
    // AddrInUse immediately — that's fine while the desktop serves the
    // same API, but the daemon must take the port over once the
    // desktop exits or the web agent goes dark. Probe health between
    // attempts so we never fight a healthy owner.
    tokio::spawn(async move {
        let health_url = format!("{server}{AGENT_SERVER_HEALTH_PATH}");
        let probe = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .ok();
        loop {
            let healthy = match &probe {
                Some(client) => matches!(
                    client.get(&health_url).send().await,
                    Ok(resp) if resp.status().is_success()
                ),
                None => false,
            };
            if !healthy {
                let options = neoism_agent_server::ServerOptions {
                    hostname: hostname.clone(),
                    port,
                    cors: Vec::new(),
                };
                if let Err(error) = neoism_agent_server::listen(options).await {
                    tracing::warn!(
                        target: "neoism_workspace_daemon::agent",
                        %error,
                        "embedded Neoism Agent server exited; retrying"
                    );
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

pub(crate) fn configured_agent_server() -> String {
    std::env::var("NEOISM_AGENT_SERVER")
        .ok()
        .or_else(|| std::env::var("NEOISM_SERVER").ok())
        .map(|server| server.trim().trim_end_matches('/').to_string())
        .filter(|server| !server.is_empty())
        .unwrap_or_else(|| DEFAULT_AGENT_SERVER.to_string())
}

fn local_bind_target(server: &str) -> Option<(String, u16)> {
    let rest = server.strip_prefix("http://")?;
    let host_port = rest.split('/').next().unwrap_or(rest);
    let (host, port_str) = host_port.split_once(':').unwrap_or((host_port, "4096"));
    let port = port_str.parse::<u16>().ok()?;
    if matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
        Some(("127.0.0.1".to_string(), port))
    } else {
        None
    }
}

/// Handle for a per-connection agent session. Cheap to clone via
/// `Arc` — every clone manipulates the same task slot and turn
/// history.
#[derive(Clone)]
pub struct AgentSession {
    inner: Arc<AgentInner>,
}

pub(crate) struct AgentInner {
    /// `None` when `NEOISM_AGENT_API_KEY` was unset at spawn time.
    api_key: Option<String>,
    /// Model id (e.g. `"claude-opus-4-5-20250929"`). Empty string =
    /// fall back to the Anthropic default the SDK ships.
    model: String,
    /// Outbound channel; cloned into the spawned task so it can
    /// stream events back without holding the session lock.
    tx: UnboundedSender<AgentServerMessage>,
    /// HTTP client built once on spawn and reused for direct-proxy
    /// turns and agent-server traffic alike.
    http: reqwest::Client,
    /// Conversation history accumulated across direct-proxy turns.
    history: Mutex<Vec<HistoryTurn>>,
    /// Handle to the most recently spawned direct-proxy turn.
    /// Replaced on every `send_message`; `cancel` aborts it via the
    /// join handle.
    current: Mutex<Option<JoinHandle<()>>>,
    /// Base URL for the embedded agent-server (layer 2). Resolved
    /// once on spawn so per-envelope dispatch doesn't re-read the
    /// env var.
    agent_server: String,
    /// Per-agent-server-session SSE forwarder handles. Inserted by
    /// `start_event_stream`, dropped on `stop_event_stream` /
    /// session-shutdown.
    stream_handles: Mutex<HashMap<String, JoinHandle<()>>>,
    /// Per-session in-flight HTTP request handles (prompts, etc.).
    /// `CancelInflight` / `Cancel` aborts the matching entry so the
    /// request stops attempting retries.
    inflight: Mutex<HashMap<String, JoinHandle<()>>>,
}

#[derive(Clone, Debug)]
struct HistoryTurn {
    role: &'static str,
    content: String,
}

impl AgentSession {
    /// Spawn a new agent session. If `NEOISM_AGENT_API_KEY` is unset
    /// the session emits exactly one [`AgentServerMessage::Disabled`]
    /// on `tx` before parking; the chrome surfaces this as an inline
    /// "no key" banner.
    ///
    /// `model` may be the empty string to defer to Anthropic's
    /// server-side default.
    pub fn spawn(
        api_key: Option<String>,
        model: String,
        tx: UnboundedSender<AgentServerMessage>,
    ) -> AgentSession {
        if api_key.as_ref().map(String::is_empty).unwrap_or(true) {
            // Tell the chrome why the direct proxy is parked. The
            // agent-server-backed layer can still serve envelopes —
            // the chrome paints this as informational chrome rather
            // than fatal.
            let _ = tx.send(AgentServerMessage::Disabled {
                reason: "set NEOISM_AGENT_API_KEY".to_string(),
            });
        }
        let http = reqwest::Client::builder()
            .pool_idle_timeout(Some(std::time::Duration::from_secs(30)))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        AgentSession {
            inner: Arc::new(AgentInner {
                api_key: api_key.filter(|s| !s.is_empty()),
                model,
                tx,
                http,
                history: Mutex::new(Vec::new()),
                current: Mutex::new(None),
                agent_server: configured_agent_server(),
                stream_handles: Mutex::new(HashMap::new()),
                inflight: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Send a user prompt to the direct Claude proxy (layer 1).
    pub fn send_message(&self, text: String, _attachments: Vec<Attachment>) {
        if self.inner.api_key.is_none() {
            tracing::warn!("agent send_message dropped: no API key");
            let _ = self.inner.tx.send(AgentServerMessage::Disabled {
                reason: "set NEOISM_AGENT_API_KEY".to_string(),
            });
            return;
        }
        if let Some(prev) = self.inner.current.lock().take() {
            prev.abort();
        }
        self.inner.history.lock().push(HistoryTurn {
            role: "user",
            content: text,
        });
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move { run_turn(inner).await });
        *self.inner.current.lock() = Some(handle);
    }

    /// Reset the direct-proxy conversation history.
    pub fn new_thread(&self) {
        if let Some(prev) = self.inner.current.lock().take() {
            prev.abort();
        }
        self.inner.history.lock().clear();
    }

    /// Abort the in-flight direct-proxy request, if any. Does not
    /// touch agent-server sessions — those are cancelled via
    /// [`AgentClientMessage::CancelInflight`].
    pub fn cancel(&self) {
        if let Some(prev) = self.inner.current.lock().take() {
            prev.abort();
        }
    }
}

pub(crate) mod dispatcher;
pub(crate) mod events;
pub(crate) mod handlers;
pub(crate) mod http;
pub(crate) mod turn;

pub(crate) use events::*;
pub(crate) use handlers::*;
pub(crate) use http::*;
pub(crate) use turn::*;

pub use dispatcher::dispatch;

#[cfg(test)]
mod tests;
