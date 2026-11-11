//! Workspace daemon client. One background thread owns the WebSocket: it parses every frame there
//! and hands typed events to the UI thread through an inbox, so daemon traffic (megabyte history
//! pages included) never runs on the frame path.

use neoism_protocol::agent::{AgentClientMessage, AgentServerMessage};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tungstenite::{Message, WebSocket};

/// Wire families the daemon multiplexes over one socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Workspace,
    Agent,
    Pty,
    Files,
    Search,
    Editor,
    Git,
    Config,
    Database,
    /// Shared documents and presence. Never scoped to a workspace root.
    Crdt,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Family::Workspace => "Workspace",
            Family::Agent => "Agent",
            Family::Pty => "Pty",
            Family::Files => "Files",
            Family::Search => "Search",
            Family::Editor => "Editor",
            Family::Git => "Git",
            Family::Config => "Config",
            Family::Database => "Database",
            Family::Crdt => "Crdt",
        }
    }
}

#[derive(Debug)]
pub enum Inbound {
    Connecting,
    Connected,
    Disconnected(String),
    Agent(Box<AgentServerMessage>),
    Editor(Box<neoism_protocol::editor::EditorServerMessage>),
    Database { request_id: u64, message: Result<Box<neoism_protocol::database::DatabaseReply>, String> },
    /// A shared-document or presence message (`request_id` 0 for broadcasts).
    Crdt { request_id: u64, message: Box<neoism_protocol::crdt::CrdtServerMessage> },
    /// Everything else stays untyped until a view needs it.
    Reply { family: String, request_id: u64, message: Value },
}

pub type Inbox = Arc<Mutex<VecDeque<Inbound>>>;

/// A request: its family, message, and the workspace it is scoped to (if any).
enum Outbound {
    Request(u64, Family, Value, Option<String>),
    Close(u64, String, Sender<()>),
}

/// Cheap, cloneable sender the UI keeps in its state.
#[derive(Clone)]
pub struct Daemon {
    outbound: Sender<Outbound>,
    sequence: Arc<std::sync::atomic::AtomicU64>,
    _lifetime: Arc<()>,
}

impl Daemon {
    /// Start a connection to the local daemon. `wake` asks the UI thread to drain `inbox`.
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> (Daemon, Inbox) { Daemon::start_to(None, wake) }

    /// Start a connection thread to `target` (`None`: the published local daemon). Each
    /// workspace owns one, so a window can hold workspaces on several servers at once.
    pub fn start_to(target: Option<Target>, wake: impl Fn() + Send + Sync + 'static) -> (Daemon, Inbox) {
        let inbox: Inbox = Arc::new(Mutex::new(VecDeque::new()));
        let (outbound, rx) = channel();
        let thread_inbox = inbox.clone();
        let lifetime = Arc::new(());
        let alive = Arc::downgrade(&lifetime);
        std::thread::Builder::new()
            .name("neoism-daemon".into())
            .spawn(move || run(rx, thread_inbox, wake, target, alive))
            .expect("spawn daemon thread");
        (Daemon { outbound, sequence: Arc::new(std::sync::atomic::AtomicU64::new(1)), _lifetime: lifetime }, inbox)
    }

    pub fn send(&self, family: Family, message: Value) {
        self.request(family, message, None);
    }

    /// A request for one workspace (`Files`, `Search`, `Git`, `Editor`, `Database`).
    pub fn send_scoped(&self, family: Family, message: Value, workspace_root: &str) {
        self.request(family, message, Some(workspace_root.to_owned()));
    }

    pub fn request(&self, family: Family, message: Value, scope: Option<String>) -> u64 {
        let id = self.sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.outbound.send(Outbound::Request(id, family, message, scope));
        id
    }

    /// Flush earlier terminal closes, release this socket's workspace claim, then disconnect.
    pub fn close(&self, workspace: &str) -> Receiver<()> {
        let (done, rx) = channel();
        let id = self.sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.outbound.send(Outbound::Close(id, workspace.to_owned(), done));
        rx
    }

    pub fn agent(&self, message: &AgentClientMessage) {
        if let Ok(value) = serde_json::to_value(message) {
            self.send(Family::Agent, value);
        }
    }
}

struct Endpoint {
    url: String,
    /// Sent in `Hello` (a hosted server's pairing password).
    pairing_token: Option<String>,
}

/// A daemon other than the published local one: a saved or hosted server, a Fast Server
/// invite, or a system reached over SSH (`ServerSelection`). `auth_token` rides the URL's
/// `token` query (`authToken`); `pairing_token` goes in `Hello` (`pairingToken`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub auth_token: Option<String>,
    pub pairing_token: Option<String>,
}

fn with_token(base: &str, token: Option<&str>) -> String {
    match token.filter(|t| !t.is_empty()) {
        Some(token) => {
            let sep = if base.contains('?') { '&' } else { '?' };
            format!("{base}{sep}token={token}")
        }
        None => base.to_owned(),
    }
}

fn endpoint_for(target: &Target) -> Endpoint {
    Endpoint { url: with_token(&target.url, target.auth_token.as_deref()), pairing_token: target.pairing_token.clone() }
}

/// Dial `target` and complete the handshake, then hang up (`prepareServerConnection`'s probe):
/// a host that can't be reached or refuses the credentials leaves the current connection as is.
pub fn probe(target: &Target, timeout: Duration) -> Result<(), String> {
    let endpoint = endpoint_for(target);
    let mut socket = connect(&endpoint)?;
    let hello = json!({"Workspace": {"request_id": 0, "message": {"Hello": {"token": endpoint.pairing_token, "client_name": "neoism-rune"}}}});
    socket.send(Message::text(hello.to_string())).map_err(|e| format!("Server handshake failed: {e}"))?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if std::time::Instant::now() >= deadline { return Err("The selected server did not complete its handshake".into()); }
        match socket.read() {
            Ok(Message::Text(text)) => {
                let value: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                let Some((_, body)) = value.as_object().and_then(|o| o.iter().next()) else { continue };
                let message = body.get("message").unwrap_or(body);
                if message.get("HelloAck").is_some() { let _ = socket.close(None); return Ok(()); }
                if let Some(error) = message.get("Error") {
                    return Err(error["message"].as_str().or_else(|| error.as_str()).unwrap_or("The server rejected the connection").to_owned());
                }
            }
            Ok(Message::Close(frame)) => {
                let reason = frame.map(|f| f.reason.to_string()).filter(|r| !r.is_empty());
                return Err(reason.unwrap_or_else(|| "The server rejected the connection".into()));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("Server disconnected: {e}")),
        }
    }
}

/// Where the local daemon publishes its endpoint and token: the one path the daemon, the agent
/// and every client agree on (`daemon_credential::canonical_daemon_token_path`), which differs
/// per system.
fn token_file() -> PathBuf { neoism_agent_service_api::daemon_credential::canonical_daemon_token_path() }
fn endpoint_file() -> PathBuf { token_file().with_file_name("daemon-endpoint.json") }

/// The daemon address without credentials (`publicEndpoint`), for the Servers list.
pub fn public_endpoint() -> Option<String> {
    if let Ok(url) = std::env::var("NEOISM_GPUIX_DAEMON_URL") {
        return Some(url.split(['?', '#']).next().unwrap_or(&url).to_owned());
    }
    let published = std::fs::read_to_string(endpoint_file()).ok()?;
    let record: Value = serde_json::from_str(&published).ok()?;
    Some(format!("ws://{}/session", record["address"].as_str()?))
}

/// The window's server, else the same discovery as the GPUiX client: an explicit URL, else
/// the published local daemon.
fn discover(target: Option<&Target>) -> Result<Endpoint, String> {
    if let Some(target) = target { return Ok(endpoint_for(target)); }
    let token = std::env::var("NEOISM_DAEMON_TOKEN").ok().or_else(|| {
        std::fs::read_to_string(token_file()).ok().map(|t| t.trim().to_owned())
    });
    let base = if let Ok(url) = std::env::var("NEOISM_GPUIX_DAEMON_URL") {
        url
    } else {
        let published = std::fs::read_to_string(endpoint_file())
            .map_err(|_| "No local daemon is running (no daemon-endpoint.json)".to_owned())?;
        let record: Value = serde_json::from_str(&published).map_err(|e| e.to_string())?;
        let address = record["address"].as_str().ok_or("Published daemon has no address")?;
        format!("ws://{address}/session")
    };
    Ok(Endpoint { url: with_token(&base, token.as_deref()), pairing_token: None })
}

type Socket = WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

/// `ws://` locally and over SSH tunnels; `wss://` for Fast Server invites (rustls, web roots).
fn connect(endpoint: &Endpoint) -> Result<Socket, String> {
    let parsed = url::Url::parse(&endpoint.url).map_err(|_| "Invalid server address".to_owned())?;
    let secure = match parsed.scheme() { "ws" => false, "wss" => true, _ => return Err("Server addresses start with ws:// or wss://".into()) };
    let host = parsed.host_str().ok_or("Server address has no host")?.trim_matches(['[', ']']).to_owned();
    let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });
    use std::net::ToSocketAddrs;
    let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().map_err(|e| format!("Daemon connection failed: {e}"))?.collect();
    let stream = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_secs(8)).ok())
        .ok_or_else(|| format!("Daemon connection failed: could not reach {host}:{port}"))?;
    stream.set_nodelay(true).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    // The handshake gets its own deadline; the session then polls on a short read timeout.
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut config = tungstenite::protocol::WebSocketConfig::default();
    config.max_message_size = Some(64 << 20);
    config.max_frame_size = Some(64 << 20);
    let (socket, _) = tungstenite::client_tls_with_config(endpoint.url.as_str(), stream, Some(config), None)
        .map_err(|e| format!("Daemon handshake failed: {e}"))?;
    match socket.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => { s.set_read_timeout(Some(Duration::from_millis(8))).ok(); }
        tungstenite::stream::MaybeTlsStream::Rustls(s) => { s.get_ref().set_read_timeout(Some(Duration::from_millis(8))).ok(); }
        _ => {}
    }
    Ok(socket)
}

fn push(inbox: &Inbox, wake: &dyn Fn(), event: Inbound) {
    inbox.lock().unwrap().push_back(event);
    wake();
}

fn run(rx: Receiver<Outbound>, inbox: Inbox, wake: impl Fn(), target: Option<Target>, alive: std::sync::Weak<()>) {
    while alive.strong_count() > 0 {
        push(&inbox, &wake, Inbound::Connecting);
        let result = discover(target.as_ref()).and_then(|endpoint| connect(&endpoint).map(|socket| (socket, endpoint.pairing_token)));
        let (mut socket, pairing_token) = match result {
            Ok(connected) => connected,
            Err(error) => {
                if alive.strong_count() == 0 { return; }
                // Nothing answered locally: bring up this installation's own daemon, then
                // keep looking for the endpoint it publishes.
                let error = match target.as_ref() {
                    None => crate::startup::ensure_local_daemon().err().unwrap_or(error),
                    Some(_) => error,
                };
                push(&inbox, &wake, Inbound::Disconnected(error));
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        let hello = json!({"Workspace": {"request_id": 0, "message": {"Hello": {"token": pairing_token, "client_name": "neoism-rune"}}}});
        if socket.send(Message::text(hello.to_string())).is_err() {
            continue;
        }
        let reason = 'session: loop {
            // Writes first: user actions should not wait behind a read timeout.
            loop {
                match rx.try_recv() {
                    Ok(Outbound::Close(id, workspace, done)) => {
                        close_socket(&mut socket, id, &workspace);
                        let _ = done.send(());
                        return;
                    }
                    Ok(Outbound::Request(request_id, family, message, workspace_root)) => {
                        let mut envelope = json!({"request_id": request_id, "message": message});
                        if let Some(root) = workspace_root { envelope["workspace_root"] = json!(root); }
                        let frame = json!({ family.name(): envelope });
                        if let Err(e) = socket.send(Message::text(frame.to_string())) {
                            break 'session format!("Daemon write failed: {e}");
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    if let Some(event) = parse(text.as_str()) {
                        if matches!(&event, Inbound::Reply { family, message, .. } if family == "Workspace" && message.get("HelloAck").is_some()) {
                            push(&inbox, &wake, Inbound::Connected);
                        }
                        push(&inbox, &wake, event);
                    }
                }
                Ok(Message::Close(_)) => break 'session "Daemon closed the connection".into(),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => break 'session format!("Daemon disconnected: {e}"),
            }
        };
        push(&inbox, &wake, Inbound::Disconnected(reason));
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// A reply is the barrier: dropping the socket before it can lose the close request.
fn close_socket(socket: &mut Socket, id: u64, workspace: &str) {
    let frame = json!({"Workspace":{"request_id":id,"message":{"CloseWindowWorkspace":{"workspace_id":workspace}}}});
    if socket.send(Message::text(frame.to_string())).is_err() { return; }
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        match socket.read() {
            Ok(Message::Text(text)) => {
                if matches!(parse(text.as_str()), Some(Inbound::Reply { request_id, family, .. }) if request_id == id && family == "Workspace") { break; }
            }
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    let _ = socket.close(None);
}

/// `{"AgentReply": {"request_id": n, "message": {...}}}` and its siblings.
fn parse(text: &str) -> Option<Inbound> {
    let value: Value = serde_json::from_str(text).ok()?;
    let (tag, body) = value.as_object()?.iter().next()?;
    if std::env::var_os("NEOISM_RUNE_TRACE_DAEMON").is_some() {
        let inner = body.get("message").and_then(|m| m.as_object()).and_then(|m| m.keys().next().cloned()).unwrap_or_default();
        eprintln!("[daemon] {tag} {inner} {}", text.len());
        if std::env::var_os("NEOISM_RUNE_TRACE_BODIES").is_some() && text.len() < 4000 { eprintln!("[body] {text}"); }
    }
    // PTY output and lifecycle arrive unsolicited, without a reply envelope.
    if matches!(tag.as_str(), "PtyCreated" | "PtyOutput" | "PtyClosed" | "SessionCwd" | "SessionProgram" | "TerminalDirectoryChanged" | "TerminalDirectoryChangeFailed") {
        return Some(Inbound::Reply { family: "Pty".into(), request_id: 0, message: json!({ tag.clone(): body.clone() }) });
    }
    let family = tag.strip_suffix("Reply").unwrap_or(tag).to_owned();
    let request_id = body["request_id"].as_u64().unwrap_or(0);
    let message = body.get("message").cloned().unwrap_or_else(|| json!({ tag.clone(): body.clone() }));
    if family == "Database" {
        return Some(Inbound::Database { request_id, message: serde_json::from_value(message).map(Box::new).map_err(|e| e.to_string()) });
    }
    if family == "Crdt" {
        if let Ok(typed) = serde_json::from_value(message.clone()) { return Some(Inbound::Crdt { request_id, message: Box::new(typed) }); }
    }
    if family == "Editor" {
        if let Ok(typed) = serde_json::from_value(message.clone()) { return Some(Inbound::Editor(Box::new(typed))); }
    }
    if family == "Agent" {
        if let Ok(typed) = serde_json::from_value::<AgentServerMessage>(message.clone()) {
            return Some(Inbound::Agent(Box::new(typed)));
        }
    }
    Some(Inbound::Reply { family, request_id, message })
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    #[test]
    fn closing_a_join_flushes_terminal_close_and_waits_for_workspace_reply() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/session", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut socket = tungstenite::accept(tcp).unwrap();
            socket.read().unwrap(); // Hello
            socket.send(Message::text(r#"{"WorkspaceReply":{"request_id":0,"message":{"HelloAck":{}}}}"#)).unwrap();
            let terminal: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert!(terminal["Pty"]["message"].get("ClosePty").is_some());
            let close: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(close["Workspace"]["message"]["CloseWindowWorkspace"]["workspace_id"], "joined");
            let id = close["Workspace"]["request_id"].as_u64().unwrap();
            socket.send(Message::text(json!({"WorkspaceReply":{"request_id":id,"message":{"HostWorkspaceTree":{}}}}).to_string())).unwrap();
            assert!(matches!(socket.read(), Ok(Message::Close(_))));
        });
        let (daemon, inbox) = Daemon::start_to(Some(Target { url, ..Default::default() }), || {});
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if inbox.lock().unwrap().iter().any(|e| matches!(e, Inbound::Connected)) { break; }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        daemon.send(Family::Pty, json!({"ClosePty":{"session_id":"owned-shell"}}));
        let done = daemon.close("joined");
        drop(daemon);
        done.recv_timeout(Duration::from_secs(3)).unwrap();
        server.join().unwrap();
    }
}
