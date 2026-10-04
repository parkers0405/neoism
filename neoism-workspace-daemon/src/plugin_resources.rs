use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use neoism_extensions::trust::{ApprovalScope, ApprovalState, ExtensionTrustStore};
use neoism_protocol::plugin_resource::{
    PluginResourceReply as Reply, PluginResourceRequest as Request,
    RemotePluginOwner as Owner, RemotePluginTrust, RemoteResourceKind as Kind,
    RemoteResourceTarget as Target,
};

struct Binding {
    owner: Owner,
    generation: u64,
    workspace_id: String,
    value: Resource,
}
enum Resource {
    Path {
        path: PathBuf,
        kind: Kind,
    },
    Process {
        child: Child,
        stdin: Option<ChildStdin>,
        output: Arc<Mutex<Vec<u8>>>,
        kind: Kind,
    },
    Pty {
        session: Arc<Mutex<Option<neoism_terminal_pty::PtySession>>>,
        output: Arc<Mutex<Vec<u8>>>,
    },
}

#[derive(Default)]
pub(crate) struct PluginResourceBroker {
    resources: BTreeMap<String, Binding>,
}

impl PluginResourceBroker {
    pub(crate) fn handle(
        &mut self,
        root: &Path,
        active_workspace: Option<&str>,
        request: Request,
    ) -> Reply {
        self.handle_result(root, active_workspace, request)
            .unwrap_or_else(|message| Reply::Error {
                code: "resource_rejected".into(),
                message,
            })
    }

    fn handle_result(
        &mut self,
        root: &Path,
        active_workspace: Option<&str>,
        request: Request,
    ) -> Result<Reply, String> {
        match request {
            Request::Allocate {
                owner,
                workspace_id,
                generation,
                trust,
                resource_kind,
                target,
            } => {
                validate_owner(&owner, generation, &workspace_id)?;
                if active_workspace != Some(workspace_id.as_str()) {
                    return Err(
                        "resource workspace identity is not active on this socket".into(),
                    );
                }
                verify_trust(&owner, &workspace_id, &trust)?;
                let retired = self
                    .resources
                    .iter()
                    .filter_map(|(id, binding)| {
                        (binding.owner.plugin_id == owner.plugin_id
                            && (binding.owner.revision != owner.revision
                                || binding.generation != generation))
                            .then_some(id.clone())
                    })
                    .collect::<Vec<_>>();
                for id in retired {
                    if let Some(mut binding) = self.resources.remove(&id) {
                        cancel(&mut binding);
                    }
                }
                let value = self.allocate(
                    root,
                    &owner,
                    generation,
                    &workspace_id,
                    resource_kind,
                    target,
                )?;
                let resource = opaque_id();
                self.resources.insert(
                    resource.clone(),
                    Binding {
                        owner,
                        generation,
                        workspace_id,
                        value,
                    },
                );
                Ok(Reply::Allocated {
                    resource,
                    resource_kind,
                })
            }
            Request::Invoke {
                owner,
                generation,
                resource,
                operation,
                payload,
            } => {
                let binding = self.binding_mut(&owner, generation, &resource)?;
                let value = invoke(binding, &operation, payload)?;
                Ok(Reply::Result { resource, value })
            }
            Request::Cancel {
                owner,
                generation,
                resource,
            } => {
                let binding = self.binding_mut(&owner, generation, &resource)?;
                cancel(binding);
                Ok(Reply::Closed { resource })
            }
            Request::Close {
                owner,
                generation,
                resource,
            } => {
                self.binding(&owner, generation, &resource)?;
                if let Some(mut binding) = self.resources.remove(&resource) {
                    cancel(&mut binding);
                }
                Ok(Reply::Closed { resource })
            }
            Request::CloseOwner { owner, generation } => {
                validate_owner(&owner, generation, "owner-close")?;
                let ids = self
                    .resources
                    .iter()
                    .filter_map(|(id, binding)| {
                        (&binding.owner == &owner && binding.generation == generation)
                            .then_some(id.clone())
                    })
                    .collect::<Vec<_>>();
                for id in &ids {
                    if let Some(mut binding) = self.resources.remove(id) {
                        cancel(&mut binding);
                    }
                }
                Ok(Reply::OwnerClosed { count: ids.len() })
            }
        }
    }

    fn allocate(
        &self,
        root: &Path,
        owner: &Owner,
        generation: u64,
        workspace: &str,
        kind: Kind,
        target: Target,
    ) -> Result<Resource, String> {
        match target {
            Target::WorkspaceRoot
                if kind == Kind::Workspace || kind == Kind::Directory =>
            {
                Ok(Resource::Path {
                    path: canonical_root(root)?,
                    kind,
                })
            }
            Target::Child { parent, name } => {
                validate_child_name(&name)?;
                let parent = self.binding(owner, generation, &parent)?;
                if parent.workspace_id != workspace {
                    return Err("resource belongs to another workspace".into());
                }
                let Resource::Path { path, .. } = &parent.value else {
                    return Err("parent resource is not a directory".into());
                };
                let candidate = path.join(name);
                let resolved = candidate.canonicalize().map_err(|e| e.to_string())?;
                if !resolved.starts_with(canonical_root(root)?) {
                    return Err("resource escapes workspace".into());
                }
                let actual = if resolved.is_dir() {
                    Kind::Directory
                } else {
                    Kind::File
                };
                if !matches!(
                    (kind, actual),
                    (Kind::File, Kind::File)
                        | (Kind::Directory, Kind::Directory)
                        | (Kind::Watch, _)
                ) {
                    return Err("resource kind does not match target".into());
                }
                Ok(Resource::Path {
                    path: resolved,
                    kind,
                })
            }
            Target::Command {
                program,
                arguments,
                cwd,
            } if matches!(kind, Kind::Task | Kind::Test | Kind::Dap) => {
                spawn_process(root, program, arguments, cwd, kind)
            }
            Target::Command {
                program,
                arguments,
                cwd,
            } if kind == Kind::Pty => spawn_pty_process(root, program, arguments, cwd),
            _ => Err("resource target is incompatible with requested kind".into()),
        }
    }

    fn binding(
        &self,
        owner: &Owner,
        generation: u64,
        id: &str,
    ) -> Result<&Binding, String> {
        self.resources
            .get(id)
            .filter(|item| &item.owner == owner && item.generation == generation)
            .ok_or_else(|| {
                "resource is stale, cross-owner, cross-socket, or cross-generation".into()
            })
    }
    fn binding_mut(
        &mut self,
        owner: &Owner,
        generation: u64,
        id: &str,
    ) -> Result<&mut Binding, String> {
        self.resources
            .get_mut(id)
            .filter(|item| &item.owner == owner && item.generation == generation)
            .ok_or_else(|| {
                "resource is stale, cross-owner, cross-socket, or cross-generation".into()
            })
    }
}

impl Drop for PluginResourceBroker {
    fn drop(&mut self) {
        for binding in self.resources.values_mut() {
            cancel(binding);
        }
    }
}

fn invoke(
    binding: &mut Binding,
    operation: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match (&mut binding.value, operation) {
        (
            Resource::Path {
                path,
                kind: Kind::File,
            },
            "read",
        ) => {
            let max = payload
                .get("maxBytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(1024 * 1024)
                .clamp(1, 4 * 1024 * 1024) as usize;
            let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            file.take((max + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > max {
                return Err("file exceeds bounded read limit".into());
            }
            Ok(
                serde_json::json!({"text": String::from_utf8(bytes).map_err(|_| "file is not UTF-8")?}),
            )
        }
        (
            Resource::Path {
                path,
                kind: Kind::File,
            },
            "write",
        ) => {
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("file write requires text")?;
            if text.len() > 4 * 1024 * 1024 {
                return Err("file write exceeds limit".into());
            }
            let temp = path.with_extension("neoism-plugin-tmp");
            std::fs::write(&temp, text)
                .and_then(|_| std::fs::rename(temp, path))
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({"written": text.len()}))
        }
        (
            Resource::Path {
                path,
                kind: Kind::Watch,
            },
            "poll",
        ) => {
            let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|v| v.as_millis() as u64);
            Ok(serde_json::json!({"modifiedMillis": modified, "length": metadata.len()}))
        }
        (Resource::Process { stdin, .. }, "stdin") => {
            let data = payload
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or("stdin requires data")?;
            if data.len() > 256 * 1024 {
                return Err("stdin payload exceeds limit".into());
            }
            stdin
                .as_mut()
                .ok_or("process stdin is closed")?
                .write_all(data.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({"accepted": data.len()}))
        }
        (
            Resource::Process {
                child,
                output,
                kind,
                ..
            },
            "status",
        ) => {
            let status = child.try_wait().map_err(|e| e.to_string())?;
            let mut bytes = output.lock().map_err(|_| "process output lock poisoned")?;
            let take = bytes.len().min(1024 * 1024);
            let chunk = bytes.drain(..take).collect::<Vec<_>>();
            Ok(
                serde_json::json!({"kind": format!("{kind:?}").to_ascii_lowercase(), "running": status.is_none(), "exitCode": status.and_then(|v| v.code()), "output": String::from_utf8_lossy(&chunk)}),
            )
        }
        (Resource::Process { child, .. }, "cancel") => {
            let _ = child.kill();
            let _ = child.wait();
            Ok(serde_json::json!({"cancelled": true}))
        }
        (Resource::Pty { session, .. }, "stdin") => {
            let data = payload
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or("PTY stdin requires data")?;
            if data.len() > 256 * 1024 {
                return Err("PTY stdin exceeds limit".into());
            }
            let mut guard = session.lock().map_err(|_| "PTY lock poisoned")?;
            let accepted = guard
                .as_mut()
                .ok_or("PTY is closed")?
                .write(data.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({"accepted": accepted}))
        }
        (Resource::Pty { session, .. }, "resize") => {
            let cols = payload.get("cols").and_then(|v| v.as_u64()).unwrap_or(80);
            let rows = payload.get("rows").and_then(|v| v.as_u64()).unwrap_or(24);
            if cols == 0 || rows == 0 || cols > 1000 || rows > 1000 {
                return Err("PTY dimensions are invalid".into());
            }
            session
                .lock()
                .map_err(|_| "PTY lock poisoned")?
                .as_mut()
                .ok_or("PTY is closed")?
                .resize(cols as u16, rows as u16)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({"resized": true}))
        }
        (Resource::Pty { session, output }, "status") => {
            let code = session
                .lock()
                .map_err(|_| "PTY lock poisoned")?
                .as_ref()
                .and_then(|value| value.exit_code());
            let mut bytes = output.lock().map_err(|_| "PTY output lock poisoned")?;
            let take = bytes.len().min(1024 * 1024);
            let chunk = bytes.drain(..take).collect::<Vec<_>>();
            Ok(
                serde_json::json!({"running": code.is_none(), "exitCode": code, "output": String::from_utf8_lossy(&chunk)}),
            )
        }
        (Resource::Pty { session, .. }, "cancel") => {
            if let Some(value) = session.lock().map_err(|_| "PTY lock poisoned")?.take() {
                value.close();
            }
            Ok(serde_json::json!({"cancelled": true}))
        }
        _ => Err("operation is not supported by this resource".into()),
    }
}

fn spawn_process(
    root: &Path,
    program: String,
    arguments: Vec<String>,
    cwd: Option<String>,
    kind: Kind,
) -> Result<Resource, String> {
    if program.is_empty()
        || program.contains('/')
        || program.contains('\\')
        || program.contains(':')
        || arguments.len() > 256
        || arguments.iter().any(|value| value.len() > 64 * 1024)
    {
        return Err("process program must be a host-resolved command name and arguments must be bounded".into());
    }
    let cwd = contained_cwd(root, cwd.as_deref())?;
    let mut child = Command::new(program)
        .args(arguments)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdin = child.stdin.take();
    let output = Arc::new(Mutex::new(Vec::new()));
    for mut stream in [
        child.stdout.take().map(Stream::Out),
        child.stderr.take().map(Stream::Err),
    ]
    .into_iter()
    .flatten()
    {
        let output = output.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            if let Ok(mut target) = output.lock() {
                let remaining = (4 * 1024 * 1024usize).saturating_sub(target.len());
                target.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
            }
        });
    }
    Ok(Resource::Process {
        child,
        stdin,
        output,
        kind,
    })
}
fn spawn_pty_process(
    root: &Path,
    program: String,
    arguments: Vec<String>,
    cwd: Option<String>,
) -> Result<Resource, String> {
    if program.is_empty()
        || program.contains('/')
        || program.contains('\\')
        || program.contains(':')
        || arguments.len() > 256
    {
        return Err("PTY program must be a host-resolved command name".into());
    }
    let cwd = contained_cwd(root, cwd.as_deref())?;
    let pty =
        neoism_terminal_pty::PtySession::spawn(neoism_terminal_pty::PtySessionConfig {
            shell: Some(program),
            args: arguments,
            cwd: Some(cwd),
            env: Vec::new(),
            cols: 80,
            rows: 24,
        })
        .map_err(|e| e.to_string())?;
    let session = Arc::new(Mutex::new(Some(pty)));
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_session = session.clone();
    let reader_output = output.clone();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let read = {
                let Ok(mut guard) = reader_session.lock() else {
                    break;
                };
                let Some(session) = guard.as_mut() else { break };
                session.read(&mut buffer)
            };
            match read {
                Ok(0) => break,
                Ok(size) => {
                    let Ok(mut bytes) = reader_output.lock() else {
                        break;
                    };
                    let remaining = (4 * 1024 * 1024usize).saturating_sub(bytes.len());
                    bytes.extend_from_slice(&buffer[..size.min(remaining)]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(8))
                }
                Err(_) => break,
            }
        }
    });
    Ok(Resource::Pty { session, output })
}
enum Stream {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}
impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Out(v) => v.read(buffer),
            Self::Err(v) => v.read(buffer),
        }
    }
}
fn cancel(binding: &mut Binding) {
    match &mut binding.value {
        Resource::Process { child, .. } => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Resource::Pty { session, .. } => {
            if let Ok(mut value) = session.lock() {
                if let Some(session) = value.take() {
                    session.close();
                }
            }
        }
        Resource::Path { .. } => {}
    }
}
fn verify_trust(
    owner: &Owner,
    workspace: &str,
    trust: &RemotePluginTrust,
) -> Result<(), String> {
    let scope = if trust.user_scope {
        ApprovalScope::User
    } else {
        ApprovalScope::Workspace(workspace.into())
    };
    let approval = ExtensionTrustStore::managed()
        .exact(
            &owner.plugin_id,
            &owner.revision,
            &trust.package_digest,
            &trust.artifact_digest,
            trust.abi_version,
            &trust.capabilities,
            &scope,
        )
        .map_err(|e| e.to_string())?
        .ok_or("remote resource owner is not approved by the host")?;
    if approval.state != ApprovalState::Approved {
        return Err("remote resource approval is not active".into());
    }
    Ok(())
}
fn validate_owner(owner: &Owner, generation: u64, workspace: &str) -> Result<(), String> {
    if owner.plugin_id.is_empty()
        || owner.revision.is_empty()
        || generation == 0
        || workspace.is_empty()
    {
        Err("remote resource identity is incomplete".into())
    } else {
        Ok(())
    }
}
fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    root.canonicalize().map_err(|e| e.to_string())
}
fn validate_child_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
        Err("resource child name is invalid".into())
    } else {
        Ok(())
    }
}
fn contained_cwd(root: &Path, cwd: Option<&str>) -> Result<PathBuf, String> {
    let root = canonical_root(root)?;
    let Some(cwd) = cwd else { return Ok(root) };
    let mut current = root.clone();
    for name in cwd.split('/').filter(|v| !v.is_empty()) {
        validate_child_name(name)?;
        current.push(name);
    }
    let current = current.canonicalize().map_err(|e| e.to_string())?;
    if !current.starts_with(&root) || !current.is_dir() {
        return Err("process cwd escapes workspace".into());
    }
    Ok(current)
}
fn opaque_id() -> String {
    format!("pr_{}", uuid::Uuid::new_v4().simple())
}
