use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::client::{emit, AcpRpcError, AcpShared, AcpUiEvent, PROMPT_TIMEOUT};
use super::paths::canonicalize_path;

const DEFAULT_TERMINAL_OUTPUT_LIMIT: usize = 1024 * 1024;

pub(super) struct AcpTerminalManager {
    cwd: PathBuf,
    next_id: AtomicU64,
    terminals: Mutex<HashMap<String, Arc<AcpTerminal>>>,
}

impl AcpTerminalManager {
    pub(super) fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            next_id: AtomicU64::new(1),
            terminals: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn create(
        &self,
        server_id: &str,
        params: &Value,
        shared: &Arc<AcpShared>,
    ) -> Result<Value, AcpRpcError> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
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

        emit(
            &shared.ui_tx,
            &shared.wake,
            AcpUiEvent::TerminalCreated {
                server_id: server_id.to_string(),
                session_id,
                terminal_id: terminal_id.clone(),
                command,
            },
        );

        Ok(json!({ "terminalId": terminal_id }))
    }

    pub(super) fn output(&self, params: &Value) -> Result<Value, AcpRpcError> {
        let terminal = self.terminal(params)?;
        let (output, truncated) = terminal.output();
        Ok(json!({
            "output": output,
            "truncated": truncated,
            "exitStatus": terminal.exit_status_json(),
        }))
    }

    pub(super) fn wait_for_exit(&self, params: &Value) -> Result<Value, AcpRpcError> {
        let terminal = self.terminal(params)?;
        Ok(terminal.wait_for_exit_json())
    }

    pub(super) fn kill(&self, params: &Value) -> Result<Value, AcpRpcError> {
        let terminal = self.terminal(params)?;
        terminal.kill();
        Ok(json!({}))
    }

    pub(super) fn release(&self, params: &Value) -> Result<Value, AcpRpcError> {
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

pub(super) struct AcpTerminal {
    output: Mutex<String>,
    truncated: Mutex<bool>,
    limit: usize,
    exit_status: Mutex<Option<AcpTerminalExit>>,
    child: Mutex<Option<Child>>,
    condvar: Condvar,
}

#[derive(Clone, Debug)]
pub(super) struct AcpTerminalExit {
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
        thread::spawn(move || {
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
        thread::spawn(move || {
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
                thread::sleep(Duration::from_millis(50));
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
        let truncated = self.truncated.lock().map(|v| *v).unwrap_or(false);
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
    let cwd = canonicalize_path(cwd).map_err(|err| AcpRpcError {
        code: -32000,
        message: format!("Could not resolve terminal cwd {}: {err}", cwd.display()),
    })?;
    let workspace = canonicalize_path(workspace).map_err(|err| AcpRpcError {
        code: -32000,
        message: format!("Could not resolve workspace {}: {err}", workspace.display()),
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
