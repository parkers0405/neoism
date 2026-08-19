use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::paths::normalize_existing_dir;
use super::rpc::{spawn_reader, spawn_stderr, spawn_waiter, spawn_writer};
use super::terminal::AcpTerminalManager;

pub(super) const JSONRPC: &str = "2.0";
pub(super) const ACP_PROTOCOL_VERSION: u16 = 1;
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
pub(super) const PROMPT_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug)]
pub struct AcpServerConfig {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

impl AcpServerConfig {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        command: impl Into<String>,
        cwd: PathBuf,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            cwd,
            env: Vec::new(),
        }
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum AcpUiEvent {
    Started {
        server_id: String,
        name: String,
        pid: Option<u32>,
    },
    Initialized {
        server_id: String,
        agent_capabilities: Value,
        auth_methods: Value,
    },
    SessionCreated {
        server_id: String,
        session_id: String,
        cwd: PathBuf,
        response: Value,
    },
    SessionUpdate {
        server_id: String,
        session_id: String,
        update: Value,
    },
    PromptFinished {
        server_id: String,
        session_id: String,
        stop_reason: Option<String>,
    },
    FileRead {
        server_id: String,
        session_id: Option<String>,
        path: PathBuf,
    },
    FileWritten {
        server_id: String,
        session_id: Option<String>,
        path: PathBuf,
        bytes: usize,
    },
    PermissionRequested {
        server_id: String,
        session_id: Option<String>,
        request_id: u64,
        tool_call: Value,
        options: Value,
    },
    TerminalCreated {
        server_id: String,
        session_id: Option<String>,
        terminal_id: String,
        command: String,
    },
    Stderr {
        server_id: String,
        line: String,
    },
    DebugLine {
        server_id: String,
        direction: AcpDebugDirection,
        line: String,
    },
    Exited {
        server_id: String,
        status: Option<i32>,
    },
    Error {
        server_id: String,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpDebugDirection {
    Incoming,
    Outgoing,
}

#[derive(Clone)]
pub struct AcpClientHandle {
    server_id: String,
    cwd: PathBuf,
    pub(super) shared: Arc<AcpShared>,
}

pub(super) struct AcpShared {
    pub(super) write_tx: mpsc::Sender<String>,
    pub(super) pending: Mutex<HashMap<u64, mpsc::Sender<Result<Value, AcpRpcError>>>>,
    pub(super) next_request_id: AtomicU64,
    pub(super) terminals: AcpTerminalManager,
    pub(super) pending_permissions: Mutex<HashMap<u64, mpsc::Sender<Option<String>>>>,
    pub(super) next_permission_id: AtomicU64,
    pub(super) ui_tx: mpsc::Sender<AcpUiEvent>,
    pub(super) wake: Arc<dyn Fn() + Send + Sync + 'static>,
}

#[derive(Clone, Debug)]
pub(super) struct AcpRpcError {
    pub(super) code: i64,
    pub(super) message: String,
}

impl AcpClientHandle {
    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub fn spawn(
        config: AcpServerConfig,
        ui_tx: mpsc::Sender<AcpUiEvent>,
        wake: Arc<dyn Fn() + Send + Sync + 'static>,
    ) -> Result<Self, String> {
        let cwd = normalize_existing_dir(&config.cwd)
            .map_err(|err| format!("Invalid ACP cwd {}: {err}", config.cwd.display()))?;

        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .envs(config.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
            );
        }

        let mut child = command.spawn().map_err(|err| {
            format!("Could not start ACP server `{}`: {err}", config.command)
        })?;
        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ACP server stdin was not available".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ACP server stdout was not available".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "ACP server stderr was not available".to_string())?;

        let (write_tx, write_rx) = mpsc::channel::<String>();
        let shared = Arc::new(AcpShared {
            write_tx,
            pending: Mutex::new(HashMap::new()),
            next_request_id: AtomicU64::new(1),
            terminals: AcpTerminalManager::new(cwd.clone()),
            pending_permissions: Mutex::new(HashMap::new()),
            next_permission_id: AtomicU64::new(1),
            ui_tx: ui_tx.clone(),
            wake,
        });

        emit(
            &ui_tx,
            &shared.wake,
            AcpUiEvent::Started {
                server_id: config.id.clone(),
                name: config.name.clone(),
                pid: Some(pid),
            },
        );

        spawn_writer(
            config.id.clone(),
            stdin,
            write_rx,
            ui_tx.clone(),
            shared.wake.clone(),
        );
        spawn_reader(config.id.clone(), cwd.clone(), stdout, shared.clone());
        spawn_stderr(
            config.id.clone(),
            stderr,
            ui_tx.clone(),
            shared.wake.clone(),
        );
        spawn_waiter(config.id.clone(), child, shared.clone());

        Ok(Self {
            server_id: config.id,
            cwd,
            shared,
        })
    }

    pub fn start_default_session(&self, initial_prompt: Option<String>) {
        let handle = self.clone();
        thread::Builder::new()
            .name(format!("neoism-acp-{}-session", self.server_id))
            .spawn(move || {
                if let Err(err) = handle.initialize_and_create_session(initial_prompt) {
                    handle.emit_error(err);
                }
            })
            .ok();
    }

    #[allow(dead_code)]
    pub fn send_prompt(&self, session_id: String, prompt: String) {
        let handle = self.clone();
        thread::Builder::new()
            .name(format!("neoism-acp-{}-prompt", self.server_id))
            .spawn(move || {
                if let Err(err) = handle.prompt(session_id, prompt) {
                    handle.emit_error(err);
                }
            })
            .ok();
    }

    #[allow(dead_code)]
    pub fn cancel(&self, session_id: &str) {
        let message = json!({
            "jsonrpc": JSONRPC,
            "method": "session/cancel",
            "params": {
                "sessionId": session_id,
            },
        });
        self.write_json(message);
    }

    pub fn respond_permission(&self, request_id: u64, option_id: Option<String>) -> bool {
        let Some(tx) = self
            .shared
            .pending_permissions
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&request_id))
        else {
            return false;
        };
        tx.send(option_id).is_ok()
    }

    fn initialize_and_create_session(
        &self,
        initial_prompt: Option<String>,
    ) -> Result<String, String> {
        let initialize = self
            .send_request(
                "initialize",
                json!({
                    "protocolVersion": ACP_PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": {
                            "readTextFile": true,
                            "writeTextFile": true,
                        },
                        "terminal": true,
                    },
                    "clientInfo": {
                        "name": "neoism",
                        "title": "Neoism",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
                REQUEST_TIMEOUT,
            )
            .map_err(|err| format!("ACP initialize failed: {}", err.message))?;

        emit(
            &self.shared.ui_tx,
            &self.shared.wake,
            AcpUiEvent::Initialized {
                server_id: self.server_id.clone(),
                agent_capabilities: initialize
                    .get("agentCapabilities")
                    .cloned()
                    .unwrap_or(Value::Null),
                auth_methods: initialize
                    .get("authMethods")
                    .cloned()
                    .unwrap_or(Value::Null),
            },
        );

        let auth_methods = initialize
            .get("authMethods")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let response = match self.new_session_request() {
            Ok(response) => response,
            Err(err) if err.code == -32000 => {
                self.authenticate_with_first_method(&auth_methods)?;
                self.new_session_request()
                    .map_err(|err| format!("ACP session/new failed: {}", err.message))?
            }
            Err(err) => return Err(format!("ACP session/new failed: {}", err.message)),
        };
        let session_id = response
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "ACP session/new response did not include sessionId".to_string()
            })?
            .to_string();

        emit(
            &self.shared.ui_tx,
            &self.shared.wake,
            AcpUiEvent::SessionCreated {
                server_id: self.server_id.clone(),
                session_id: session_id.clone(),
                cwd: self.cwd.clone(),
                response,
            },
        );

        if let Some(prompt) = initial_prompt {
            self.prompt(session_id.clone(), prompt)?;
        }

        Ok(session_id)
    }

    fn new_session_request(&self) -> Result<Value, AcpRpcError> {
        self.send_request(
            "session/new",
            json!({
                "cwd": self.cwd.display().to_string(),
                "mcpServers": [],
            }),
            REQUEST_TIMEOUT,
        )
    }

    fn authenticate_with_first_method(
        &self,
        auth_methods: &[Value],
    ) -> Result<(), String> {
        let method_id = auth_methods
            .first()
            .and_then(|method| method.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "ACP agent requires authentication but did not advertise an auth method"
                    .to_string()
            })?;
        self.send_request(
            "authenticate",
            json!({
                "methodId": method_id,
            }),
            PROMPT_TIMEOUT,
        )
        .map(|_| ())
        .map_err(|err| format!("ACP authenticate failed: {}", err.message))
    }

    fn prompt(&self, session_id: String, prompt: String) -> Result<(), String> {
        let response = self
            .send_request(
                "session/prompt",
                json!({
                    "sessionId": session_id.clone(),
                    "prompt": [
                        {
                            "type": "text",
                            "text": prompt,
                        }
                    ],
                }),
                PROMPT_TIMEOUT,
            )
            .map_err(|err| format!("ACP session/prompt failed: {}", err.message))?;
        emit(
            &self.shared.ui_tx,
            &self.shared.wake,
            AcpUiEvent::PromptFinished {
                server_id: self.server_id.clone(),
                session_id,
                stop_reason: response
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
        );
        Ok(())
    }

    fn send_request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, AcpRpcError> {
        let id = self.shared.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.shared
            .pending
            .lock()
            .map_err(|_| AcpRpcError {
                code: -32603,
                message: "ACP pending request map poisoned".to_string(),
            })?
            .insert(id, tx);

        self.write_json(json!({
            "jsonrpc": JSONRPC,
            "id": id,
            "method": method,
            "params": params,
        }));

        match rx.recv_timeout(timeout) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(err),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = self
                    .shared
                    .pending
                    .lock()
                    .map(|mut pending| pending.remove(&id));
                Err(AcpRpcError {
                    code: -32000,
                    message: format!("ACP {method} timed out"),
                })
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(AcpRpcError {
                code: -32000,
                message: format!("ACP {method} response channel closed"),
            }),
        }
    }

    fn write_json(&self, value: Value) {
        match serde_json::to_string(&value) {
            Ok(line) => {
                emit(
                    &self.shared.ui_tx,
                    &self.shared.wake,
                    AcpUiEvent::DebugLine {
                        server_id: self.server_id.clone(),
                        direction: AcpDebugDirection::Outgoing,
                        line: line.clone(),
                    },
                );
                if let Err(err) = self.shared.write_tx.send(line) {
                    self.emit_error(format!("ACP write queue closed: {err}"));
                }
            }
            Err(err) => {
                self.emit_error(format!("Could not encode ACP JSON-RPC message: {err}"))
            }
        }
    }

    fn emit_error(&self, message: String) {
        emit(
            &self.shared.ui_tx,
            &self.shared.wake,
            AcpUiEvent::Error {
                server_id: self.server_id.clone(),
                message,
            },
        );
    }
}

pub(super) fn emit(
    tx: &mpsc::Sender<AcpUiEvent>,
    wake: &Arc<dyn Fn() + Send + Sync + 'static>,
    event: AcpUiEvent,
) {
    let _ = tx.send(event);
    wake();
}
