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
    Git,
    Config,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Family::Workspace => "Workspace",
            Family::Agent => "Agent",
            Family::Pty => "Pty",
            Family::Files => "Files",
            Family::Git => "Git",
            Family::Config => "Config",
        }
    }
}

#[derive(Debug)]
pub enum Inbound {
    Connecting,
    Connected,
    Disconnected(String),
    Agent(Box<AgentServerMessage>),
    /// Everything else stays untyped until a view needs it.
    Reply { family: String, request_id: u64, message: Value },
}

pub type Inbox = Arc<Mutex<VecDeque<Inbound>>>;

/// Cheap, cloneable sender the UI keeps in its state.
#[derive(Clone)]
pub struct Daemon {
    outbound: Sender<(Family, Value)>,
}

impl Daemon {
    /// Start the connection thread. `wake` asks the UI thread to drain `inbox`.
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> (Daemon, Inbox) {
        let inbox: Inbox = Arc::new(Mutex::new(VecDeque::new()));
        let (outbound, rx) = channel();
        let thread_inbox = inbox.clone();
        std::thread::Builder::new()
            .name("neoism-daemon".into())
            .spawn(move || run(rx, thread_inbox, wake))
            .expect("spawn daemon thread");
        (Daemon { outbound }, inbox)
    }

    pub fn send(&self, family: Family, message: Value) {
        let _ = self.outbound.send((family, message));
    }

    pub fn agent(&self, message: &AgentClientMessage) {
        if let Ok(value) = serde_json::to_value(message) {
            self.send(Family::Agent, value);
        }
    }
}

struct Endpoint {
    url: String,
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("neoism-{}", unsafe_uid())))
}

fn unsafe_uid() -> u32 {
    // The token directory name only needs the numeric uid the shell already uses.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
}

/// Same discovery as the GPUiX client: an explicit URL, else the published local daemon.
fn discover() -> Result<Endpoint, String> {
    let token = std::env::var("NEOISM_DAEMON_TOKEN").ok().or_else(|| {
        std::fs::read_to_string(runtime_dir().join("neoism").join("daemon-token")).ok().map(|t| t.trim().to_owned())
    });
    let base = if let Ok(url) = std::env::var("NEOISM_GPUIX_DAEMON_URL") {
        url
    } else {
        let published = std::fs::read_to_string(runtime_dir().join("neoism").join("daemon-endpoint.json"))
            .map_err(|_| "No local daemon is running (no daemon-endpoint.json)".to_owned())?;
        let record: Value = serde_json::from_str(&published).map_err(|e| e.to_string())?;
        let address = record["address"].as_str().ok_or("Published daemon has no address")?;
        format!("ws://{address}/session")
    };
    let url = match token {
        Some(token) if !token.is_empty() => {
            let sep = if base.contains('?') { '&' } else { '?' };
            format!("{base}{sep}token={token}")
        }
        _ => base,
    };
    Ok(Endpoint { url })
}

fn connect(endpoint: &Endpoint) -> Result<WebSocket<TcpStream>, String> {
    let rest = endpoint.url.strip_prefix("ws://").ok_or("Only ws:// daemons are supported")?;
    let host = rest.split('/').next().unwrap_or(rest);
    let stream = TcpStream::connect(host).map_err(|e| format!("Daemon connection failed: {e}"))?;
    stream.set_nodelay(true).ok();
    let mut config = tungstenite::protocol::WebSocketConfig::default();
    config.max_message_size = Some(64 << 20);
    config.max_frame_size = Some(64 << 20);
    let (socket, _) = tungstenite::client::client_with_config(endpoint.url.as_str(), stream, Some(config))
        .map_err(|e| format!("Daemon handshake failed: {e}"))?;
    socket.get_ref().set_read_timeout(Some(Duration::from_millis(8))).ok();
    Ok(socket)
}

fn push(inbox: &Inbox, wake: &dyn Fn(), event: Inbound) {
    inbox.lock().unwrap().push_back(event);
    wake();
}

fn run(rx: Receiver<(Family, Value)>, inbox: Inbox, wake: impl Fn()) {
    let mut request_id: u64 = 0;
    loop {
        push(&inbox, &wake, Inbound::Connecting);
        let result = discover().and_then(|endpoint| connect(&endpoint));
        let mut socket = match result {
            Ok(socket) => socket,
            Err(error) => {
                push(&inbox, &wake, Inbound::Disconnected(error));
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        let hello = json!({"Workspace": {"request_id": 0, "message": {"Hello": {"token": null, "client_name": "neoism-rune"}}}});
        if socket.send(Message::text(hello.to_string())).is_err() {
            continue;
        }
        let reason = 'session: loop {
            // Writes first: user actions should not wait behind a read timeout.
            loop {
                match rx.try_recv() {
                    Ok((family, message)) => {
                        request_id += 1;
                        let frame = json!({ family.name(): {"request_id": request_id, "message": message} });
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

/// `{"AgentReply": {"request_id": n, "message": {...}}}` and its siblings.
fn parse(text: &str) -> Option<Inbound> {
    let value: Value = serde_json::from_str(text).ok()?;
    let (tag, body) = value.as_object()?.iter().next()?;
    let family = tag.strip_suffix("Reply").unwrap_or(tag).to_owned();
    let request_id = body["request_id"].as_u64().unwrap_or(0);
    let message = body.get("message").cloned().unwrap_or_else(|| json!({ tag.clone(): body.clone() }));
    if family == "Agent" {
        if let Ok(typed) = serde_json::from_value::<AgentServerMessage>(message.clone()) {
            return Some(Inbound::Agent(Box::new(typed)));
        }
    }
    Some(Inbound::Reply { family, request_id, message })
}
