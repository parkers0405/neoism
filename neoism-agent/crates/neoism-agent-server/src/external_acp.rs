use std::collections::HashMap;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

pub(crate) const ACP_PROTOCOL_VERSION: u16 = 1;
const JSONRPC: &str = "2.0";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
pub(crate) const PROMPT_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_TERMINAL_OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct AcpServerConfig {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: Vec<(String, String)>,
}

impl AcpServerConfig {
    pub(crate) fn new(
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

    pub(crate) fn args(
        mut self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) enum AcpEvent {
    Started {
        server_id: String,
        name: String,
        pid: Option<u32>,
    },
    SessionUpdate {
        server_id: String,
        session_id: String,
        update: Value,
    },
    Request {
        server_id: String,
        id: Value,
        method: String,
        params: Value,
    },
    Stderr {
        server_id: String,
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

#[derive(Clone, Debug)]
pub(crate) struct AcpRpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
}

type PendingMap =
    Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Value, AcpRpcError>>>>>;

#[derive(Clone)]
pub(crate) struct AcpClient {
    server_id: String,
    write_tx: mpsc::UnboundedSender<Value>,
    pending: PendingMap,
    next_request_id: Arc<AtomicU64>,
}

impl AcpClient {
    pub(crate) fn spawn(
        config: AcpServerConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<AcpEvent>), String> {
        let cwd = normalize_existing_dir(&config.cwd)
            .map_err(|err| format!("Invalid ACP cwd {}: {err}", config.cwd.display()))?;
        let mut command = tokio::process::Command::new(&config.command);
        command
            .args(&config.args)
            .envs(config.env.iter().map(|(key, value)| (key, value)))
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        crate::tool::process::set_new_process_group(&mut command);

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

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (write_tx, write_rx) = mpsc::unbounded_channel();
        let pending = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let client = Self {
            server_id: config.id.clone(),
            write_tx,
            pending: pending.clone(),
            next_request_id: Arc::new(AtomicU64::new(1)),
        };

        let _ = event_tx.send(AcpEvent::Started {
            server_id: config.id.clone(),
            name: config.name,
            pid,
        });
        spawn_writer(config.id.clone(), stdin, write_rx, event_tx.clone());
        spawn_reader(config.id.clone(), stdout, pending.clone(), event_tx.clone());
        spawn_stderr(config.id.clone(), stderr, event_tx.clone());
        spawn_waiter(config.id, child, pending, event_tx);

        Ok((client, event_rx))
    }

    pub(crate) async fn initialize(&self) -> Result<Value, AcpRpcError> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": ACP_PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": {
                            "readTextFile": true,
                            "writeTextFile": true,
                        },
                        "terminal": true,
                        "_meta": {
                            "terminal_output": true,
                        },
                    },
                    "clientInfo": {
                        "name": "neoism-agent",
                        "title": "Neoism",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
                DEFAULT_REQUEST_TIMEOUT,
            )
            .await?;
        Ok(result)
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        timeout_duration: Duration,
    ) -> Result<Value, AcpRpcError> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        if let Err(err) = self.write_tx.send(json!({
            "jsonrpc": JSONRPC,
            "id": id,
            "method": method,
            "params": params,
        })) {
            self.pending.lock().await.remove(&id);
            return Err(AcpRpcError {
                code: -32000,
                message: format!("ACP write queue closed: {err}"),
            });
        }
        match tokio::time::timeout(timeout_duration, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AcpRpcError {
                code: -32000,
                message: format!("ACP {method} response channel closed"),
            }),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(AcpRpcError {
                    code: -32000,
                    message: format!("ACP {method} timed out"),
                })
            }
        }
    }

    pub(crate) fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write_tx
            .send(json!({
                "jsonrpc": JSONRPC,
                "method": method,
                "params": params,
            }))
            .map_err(|err| format!("ACP write queue closed: {err}"))
    }

    pub(crate) fn respond(
        &self,
        id: Value,
        response: Result<Value, AcpRpcError>,
    ) -> Result<(), String> {
        let message = match response {
            Ok(result) => json!({
                "jsonrpc": JSONRPC,
                "id": id,
                "result": result,
            }),
            Err(err) => json!({
                "jsonrpc": JSONRPC,
                "id": id,
                "error": {
                    "code": err.code,
                    "message": err.message,
                },
            }),
        };
        self.write_tx
            .send(message)
            .map_err(|err| format!("ACP write queue closed: {err}"))
    }

    pub(crate) fn server_id(&self) -> &str {
        &self.server_id
    }
}

fn spawn_writer(
    server_id: String,
    mut stdin: tokio::process::ChildStdin,
    mut write_rx: mpsc::UnboundedReceiver<Value>,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
) {
    tokio::spawn(async move {
        while let Some(message) = write_rx.recv().await {
            let Ok(line) = serde_json::to_string(&message) else {
                let _ = event_tx.send(AcpEvent::Error {
                    server_id: server_id.clone(),
                    message: "Could not encode ACP JSON-RPC message".to_string(),
                });
                continue;
            };
            if let Err(err) = stdin.write_all(line.as_bytes()).await {
                let _ = event_tx.send(AcpEvent::Error {
                    server_id: server_id.clone(),
                    message: format!("ACP write failed: {err}"),
                });
                break;
            }
            if let Err(err) = stdin.write_all(b"\n").await {
                let _ = event_tx.send(AcpEvent::Error {
                    server_id: server_id.clone(),
                    message: format!("ACP newline write failed: {err}"),
                });
                break;
            }
            if let Err(err) = stdin.flush().await {
                let _ = event_tx.send(AcpEvent::Error {
                    server_id: server_id.clone(),
                    message: format!("ACP flush failed: {err}"),
                });
                break;
            }
        }
    });
}

fn spawn_reader(
    server_id: String,
    stdout: tokio::process::ChildStdout,
    pending: PendingMap,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(err) => {
                    let _ = event_tx.send(AcpEvent::Error {
                        server_id: server_id.clone(),
                        message: format!("ACP read failed: {err}"),
                    });
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let value = match serde_json::from_str::<Value>(&line) {
                Ok(value) => value,
                Err(err) => {
                    let _ = event_tx.send(AcpEvent::Error {
                        server_id: server_id.clone(),
                        message: format!("ACP JSON parse failed: {err}: {line}"),
                    });
                    continue;
                }
            };
            let method = value.get("method").and_then(Value::as_str);
            let id = value.get("id").cloned();
            match (id, method) {
                (Some(id), Some(method)) => {
                    let _ = event_tx.send(AcpEvent::Request {
                        server_id: server_id.clone(),
                        id,
                        method: method.to_string(),
                        params: value.get("params").cloned().unwrap_or(Value::Null),
                    });
                }
                (Some(id), None) => {
                    let Some(id) = id.as_u64() else {
                        continue;
                    };
                    let Some(tx) = pending.lock().await.remove(&id) else {
                        continue;
                    };
                    if let Some(result) = value.get("result") {
                        let _ = tx.send(Ok(result.clone()));
                    } else if let Some(error) = value.get("error") {
                        let _ = tx.send(Err(AcpRpcError {
                            code: error
                                .get("code")
                                .and_then(Value::as_i64)
                                .unwrap_or(-32000),
                            message: error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("ACP request failed")
                                .to_string(),
                        }));
                    }
                }
                (None, Some("session/update")) => {
                    let params = value.get("params").cloned().unwrap_or(Value::Null);
                    let session_id = params
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let update = params.get("update").cloned().unwrap_or(Value::Null);
                    let _ = event_tx.send(AcpEvent::SessionUpdate {
                        server_id: server_id.clone(),
                        session_id,
                        update,
                    });
                }
                (None, Some(method)) => {
                    let _ = event_tx.send(AcpEvent::Error {
                        server_id: server_id.clone(),
                        message: format!(
                            "Unhandled ACP notification {method}: {}",
                            value.get("params").cloned().unwrap_or(Value::Null)
                        ),
                    });
                }
                (None, None) => {
                    let _ = event_tx.send(AcpEvent::Error {
                        server_id: server_id.clone(),
                        message: "ACP message had neither id nor method".to_string(),
                    });
                }
            }
        }
    });
}

fn spawn_stderr(
    server_id: String,
    stderr: tokio::process::ChildStderr,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = event_tx.send(AcpEvent::Stderr {
                server_id: server_id.clone(),
                line,
            });
        }
    });
}

fn spawn_waiter(
    server_id: String,
    mut child: tokio::process::Child,
    pending: PendingMap,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
) {
    tokio::spawn(async move {
        let status = child.wait().await.ok().and_then(|status| status.code());
        let mut pending = pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(AcpRpcError {
                code: -32000,
                message: "ACP server exited".to_string(),
            }));
        }
        drop(pending);
        let _ = event_tx.send(AcpEvent::Exited { server_id, status });
    });
}

pub(crate) fn normalize_existing_dir(path: &Path) -> std::io::Result<PathBuf> {
    let path = crate::windows_process::canonicalize_path(path)?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is not a directory",
        ))
    }
}

pub(crate) fn workspace_path(
    cwd: &Path,
    raw: Option<&str>,
) -> Result<PathBuf, AcpRpcError> {
    let raw = raw.ok_or_else(|| AcpRpcError {
        code: -32602,
        message: "ACP request missing path".to_string(),
    })?;
    let path = normalize_absolute_path(Path::new(raw)).ok_or_else(|| AcpRpcError {
        code: -32602,
        message: format!("ACP path must be absolute: {raw}"),
    })?;
    if !path.is_absolute() {
        return Err(AcpRpcError {
            code: -32602,
            message: format!("ACP path must be absolute: {raw}"),
        });
    }

    let root =
        crate::windows_process::canonicalize_path(cwd).map_err(|err| AcpRpcError {
            code: -32000,
            message: format!("Could not resolve workspace {}: {err}", cwd.display()),
        })?;
    let existing_ancestor = nearest_existing_ancestor(
        path.parent().unwrap_or(path.as_path()),
    )
    .ok_or_else(|| AcpRpcError {
        code: -32000,
        message: format!("Could not resolve parent for {}", path.display()),
    })?;
    let existing_ancestor = crate::windows_process::canonicalize_path(&existing_ancestor)
        .map_err(|err| AcpRpcError {
            code: -32000,
            message: format!(
                "Could not resolve ancestor {}: {err}",
                existing_ancestor.display()
            ),
        })?;
    if !existing_ancestor.starts_with(&root) {
        return Err(AcpRpcError {
            code: -32000,
            message: format!(
                "ACP file access outside workspace is blocked: {}",
                path.display()
            ),
        });
    }
    Ok(path)
}

fn normalize_absolute_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    Some(out)
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if current.exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

#[derive(Clone)]
pub(crate) struct AcpTerminalManager {
    cwd: PathBuf,
    next_id: Arc<AtomicU64>,
    terminals: Arc<Mutex<HashMap<String, Arc<AcpTerminal>>>>,
}

impl AcpTerminalManager {
    pub(crate) fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            next_id: Arc::new(AtomicU64::new(1)),
            terminals: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn create(&self, params: &Value) -> Result<Value, AcpRpcError> {
        let command = params
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| AcpRpcError {
                code: -32602,
                message: "terminal/create missing command".to_string(),
            })?
            .to_string();
        let args = params
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| self.cwd.clone());
        let cwd = normalize_terminal_cwd(&self.cwd, &cwd)?;
        let output_limit = params
            .get("outputByteLimit")
            .and_then(Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_TERMINAL_OUTPUT_LIMIT);

        let mut child_cmd = Command::new(&command);
        child_cmd
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(env) = params.get("env").and_then(Value::as_array) {
            for item in env {
                if let (Some(name), Some(value)) = (
                    item.get("name").and_then(Value::as_str),
                    item.get("value").and_then(Value::as_str),
                ) {
                    child_cmd.env(name, value);
                }
            }
        }
        crate::windows_process::hide_std_command(&mut child_cmd);

        let mut child = child_cmd.spawn().map_err(|err| AcpRpcError {
            code: -32000,
            message: format!("Could not create ACP terminal `{command}`: {err}"),
        })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let terminal_id =
            format!("neoism-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let terminal = Arc::new(AcpTerminal::new(output_limit));
        if let Ok(mut terminals) = self.terminals.lock() {
            terminals.insert(terminal_id.clone(), terminal.clone());
        }
        if let Some(stdout) = stdout {
            terminal.capture(stdout);
        }
        if let Some(stderr) = stderr {
            terminal.capture(stderr);
        }
        terminal.wait(child);
        Ok(json!({ "terminalId": terminal_id }))
    }

    pub(crate) fn output(&self, params: &Value) -> Result<Value, AcpRpcError> {
        let terminal = self.terminal(params)?;
        let (output, truncated) = terminal.output();
        Ok(json!({
            "output": output,
            "truncated": truncated,
            "exitStatus": terminal.exit_status_json(),
        }))
    }

    pub(crate) fn wait_for_exit(&self, params: &Value) -> Result<Value, AcpRpcError> {
        Ok(self.terminal(params)?.wait_for_exit_json())
    }

    pub(crate) fn kill(&self, params: &Value) -> Result<Value, AcpRpcError> {
        self.terminal(params)?.kill();
        Ok(json!({}))
    }

    pub(crate) fn release(&self, params: &Value) -> Result<Value, AcpRpcError> {
        let id = params
            .get("terminalId")
            .and_then(Value::as_str)
            .ok_or_else(|| AcpRpcError {
                code: -32602,
                message: "terminal request missing terminalId".to_string(),
            })?;
        let terminal = self
            .terminals
            .lock()
            .ok()
            .and_then(|mut terminals| terminals.remove(id));
        if let Some(terminal) = terminal {
            terminal.kill();
        }
        Ok(json!({}))
    }

    fn terminal(&self, params: &Value) -> Result<Arc<AcpTerminal>, AcpRpcError> {
        let id = params
            .get("terminalId")
            .and_then(Value::as_str)
            .ok_or_else(|| AcpRpcError {
                code: -32602,
                message: "terminal request missing terminalId".to_string(),
            })?;
        self.terminals
            .lock()
            .ok()
            .and_then(|terminals| terminals.get(id).cloned())
            .ok_or_else(|| AcpRpcError {
                code: -32000,
                message: format!("Unknown ACP terminal id `{id}`"),
            })
    }
}

struct AcpTerminal {
    output: Mutex<String>,
    truncated: Mutex<bool>,
    limit: usize,
    exit_status: Mutex<Option<AcpTerminalExit>>,
    child: Mutex<Option<Child>>,
    condvar: Condvar,
}

#[derive(Clone, Debug)]
struct AcpTerminalExit {
    exit_code: Option<i32>,
    signal: Option<String>,
}

impl AcpTerminal {
    fn new(limit: usize) -> Self {
        Self {
            output: Mutex::new(String::new()),
            truncated: Mutex::new(false),
            limit,
            exit_status: Mutex::new(None),
            child: Mutex::new(None),
            condvar: Condvar::new(),
        }
    }

    fn capture(self: &Arc<Self>, mut reader: impl Read + Send + 'static) {
        let this = self.clone();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => this.append_output(&String::from_utf8_lossy(&buffer[..n])),
                    Err(_) => break,
                }
            }
        });
    }

    fn wait(self: &Arc<Self>, child: Child) {
        if let Ok(mut slot) = self.child.lock() {
            *slot = Some(child);
        }
        let this = self.clone();
        std::thread::spawn(move || {
            let status = loop {
                let maybe_status = {
                    let mut slot = match this.child.lock() {
                        Ok(slot) => slot,
                        Err(_) => return,
                    };
                    slot.as_mut()
                        .and_then(|child| child.try_wait().ok())
                        .flatten()
                };
                if maybe_status.is_some() {
                    break maybe_status;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            let exit = AcpTerminalExit {
                exit_code: status.as_ref().and_then(|status| status.code()),
                signal: status.as_ref().and_then(acp_exit_signal),
            };
            if let Ok(mut exit_status) = this.exit_status.lock() {
                *exit_status = Some(exit);
            }
            this.condvar.notify_all();
        });
    }

    fn append_output(&self, text: &str) {
        let Ok(mut output) = self.output.lock() else {
            return;
        };
        output.push_str(text);
        if output.len() > self.limit {
            let drop_bytes = output.len().saturating_sub(self.limit);
            let mut split = drop_bytes.min(output.len());
            while !output.is_char_boundary(split) && split < output.len() {
                split += 1;
            }
            output.drain(..split);
            if let Ok(mut truncated) = self.truncated.lock() {
                *truncated = true;
            }
        }
    }

    fn output(&self) -> (String, bool) {
        let output = self.output.lock().map(|s| s.clone()).unwrap_or_default();
        let truncated = self.truncated.lock().map(|value| *value).unwrap_or(false);
        (output, truncated)
    }

    fn exit_status_json(&self) -> Value {
        self.exit_status
            .lock()
            .ok()
            .and_then(|status| status.clone())
            .map(|status| {
                json!({
                    "exitCode": status.exit_code,
                    "signal": status.signal,
                })
            })
            .unwrap_or(Value::Null)
    }

    fn wait_for_exit_json(&self) -> Value {
        let mut guard = match self.exit_status.lock() {
            Ok(guard) => guard,
            Err(_) => return json!({ "exitCode": null, "signal": null }),
        };
        let started = Instant::now();
        while guard.is_none() {
            let Ok((next_guard, _)) =
                self.condvar.wait_timeout(guard, Duration::from_millis(100))
            else {
                return json!({ "exitCode": null, "signal": null });
            };
            guard = next_guard;
            if started.elapsed() > PROMPT_TIMEOUT {
                break;
            }
        }
        guard
            .clone()
            .map(|status| {
                json!({
                    "exitCode": status.exit_code,
                    "signal": status.signal,
                })
            })
            .unwrap_or_else(|| json!({ "exitCode": null, "signal": null }))
    }

    fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.kill();
            }
        }
    }
}

#[cfg(unix)]
fn acp_exit_signal(status: &std::process::ExitStatus) -> Option<String> {
    status.signal().map(|signal| signal.to_string())
}

#[cfg(not(unix))]
fn acp_exit_signal(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

fn normalize_terminal_cwd(workspace: &Path, cwd: &Path) -> Result<PathBuf, AcpRpcError> {
    let cwd =
        crate::windows_process::canonicalize_path(cwd).map_err(|err| AcpRpcError {
            code: -32000,
            message: format!("Could not resolve terminal cwd {}: {err}", cwd.display()),
        })?;
    let workspace =
        crate::windows_process::canonicalize_path(workspace).map_err(|err| {
            AcpRpcError {
                code: -32000,
                message: format!(
                    "Could not resolve workspace {}: {err}",
                    workspace.display()
                ),
            }
        })?;
    if cwd.starts_with(&workspace) {
        Ok(cwd)
    } else {
        Err(AcpRpcError {
            code: -32000,
            message: format!(
                "ACP terminal cwd outside workspace is blocked: {}",
                cwd.display()
            ),
        })
    }
}
