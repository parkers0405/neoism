use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use neoism_backend::event::{EventProxy, RioEvent, RioEventType};
use neoism_lua::{PluginOwner, PluginPtyCreateRequest};
use neoism_terminal_pty::{PtySession, PtySessionConfig};
use neoism_window::window::WindowId;

use crate::lua_async::{LuaAsyncSender, LuaAsyncToken};

const MAX_PTYS_PER_OWNER: usize = 8;
const MAX_PTYS_GLOBAL: usize = 32;
const MAX_WRITE: usize = 256 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

struct ActivePty {
    owner: PluginOwner,
    session: Arc<Mutex<PtySession>>,
    token: LuaAsyncToken,
}

#[derive(Default)]
pub(crate) struct LuaPtys {
    active: HashMap<String, ActivePty>,
}

impl LuaPtys {
    pub(crate) fn create(
        &mut self,
        owner: PluginOwner,
        id: String,
        window_id: WindowId,
        root: &Path,
        request: PluginPtyCreateRequest,
        token: LuaAsyncToken,
        sender: LuaAsyncSender,
        proxy: EventProxy,
    ) -> Result<(), String> {
        self.reap();
        if self.active.len() >= MAX_PTYS_GLOBAL
            || self
                .active
                .values()
                .filter(|pty| pty.owner == owner)
                .count()
                >= MAX_PTYS_PER_OWNER
        {
            return Err("plugin PTY limit reached".into());
        }
        if request.arguments.len() > 256
            || request
                .arguments
                .iter()
                .any(|value| value.len() > 64 * 1024)
        {
            return Err("PTY arguments exceed limits".into());
        }
        if request.cols == 0
            || request.rows == 0
            || request.cols > 1_000
            || request.rows > 1_000
        {
            return Err("PTY dimensions are invalid".into());
        }
        let cwd = contained_cwd(root, request.cwd.as_deref())?;
        let session = PtySession::spawn(PtySessionConfig {
            shell: request.program,
            args: request.arguments,
            cwd: Some(cwd),
            env: Vec::new(),
            cols: request.cols,
            rows: request.rows,
        })
        .map_err(|error| error.to_string())?;
        let session = Arc::new(Mutex::new(session));
        self.active.insert(
            id.clone(),
            ActivePty {
                owner: owner.clone(),
                session: session.clone(),
                token: token.clone(),
            },
        );
        std::thread::Builder::new().name("lua-pty-reader".into()).spawn(move || {
            let mut buffer = [0u8; 16 * 1024]; let mut total = 0usize;
            loop {
                if token.is_cancelled() { break; }
                let read = match session.lock() { Ok(mut session) => session.read(&mut buffer), Err(_) => break };
                match read {
                    Ok(0) => {
                        let code = session.lock().ok().and_then(|session| session.exit_code());
                        sender.complete(owner.clone(), id.clone(), serde_json::json!({ "event": "exit", "code": code }));
                        proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id); break;
                    }
                    Ok(size) => {
                        total = total.saturating_add(size);
                        if total > MAX_OUTPUT { sender.fail(owner.clone(), id.clone(), "output_limit", "PTY output exceeded 8 MiB"); proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id); break; }
                        sender.progress(owner.clone(), id.clone(), serde_json::json!({ "event": "output", "data": String::from_utf8_lossy(&buffer[..size]) }));
                        proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(8)),
                    Err(error) => { sender.fail(owner.clone(), id.clone(), "pty_read", error.to_string()); proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id); break; }
                }
            }
        }).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(crate) fn write(
        &mut self,
        owner: &PluginOwner,
        id: &str,
        data: &str,
    ) -> Result<(), String> {
        if data.len() > MAX_WRITE {
            return Err("PTY write exceeds 256 KiB".into());
        }
        let pty = self
            .active
            .get(id)
            .filter(|pty| &pty.owner == owner)
            .ok_or("PTY handle is stale or cross-owner")?;
        pty.session
            .lock()
            .map_err(|_| "PTY lock was poisoned")?
            .write(data.as_bytes())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn resize(
        &mut self,
        owner: &PluginOwner,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        if cols == 0 || rows == 0 || cols > 1_000 || rows > 1_000 {
            return Err("PTY dimensions are invalid".into());
        }
        let pty = self
            .active
            .get(id)
            .filter(|pty| &pty.owner == owner)
            .ok_or("PTY handle is stale or cross-owner")?;
        pty.session
            .lock()
            .map_err(|_| "PTY lock was poisoned")?
            .resize(cols, rows)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn close(&mut self, owner: &PluginOwner, id: &str) -> bool {
        let Some(pty) = self.active.get(id).filter(|pty| &pty.owner == owner) else {
            return false;
        };
        pty.token.cancel();
        self.active.remove(id);
        true
    }

    pub(crate) fn status(
        &self,
        owner: &PluginOwner,
        id: &str,
    ) -> Result<serde_json::Value, String> {
        let pty = self
            .active
            .get(id)
            .filter(|pty| &pty.owner == owner)
            .ok_or("PTY handle is stale or cross-owner")?;
        let exit_code = pty
            .session
            .lock()
            .map_err(|_| "PTY lock was poisoned")?
            .exit_code();
        Ok(
            serde_json::json!({ "pty": id, "running": exit_code.is_none(), "exitCode": exit_code }),
        )
    }

    pub(crate) fn retire_inactive(&mut self, active: &HashSet<PluginOwner>) {
        for pty in self.active.values() {
            if !active.contains(&pty.owner) {
                pty.token.cancel();
            }
        }
        self.active.retain(|_, pty| active.contains(&pty.owner));
    }
    fn reap(&mut self) {
        self.active.retain(|_, pty| !pty.token.is_cancelled());
    }
}

fn contained_cwd(root: &Path, requested: Option<&str>) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let relative = Path::new(requested.unwrap_or("."));
    if relative.is_absolute() {
        return Err("PTY cwd must be workspace-relative".into());
    }
    let cwd = root
        .join(relative)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !cwd.starts_with(&root) {
        return Err("PTY cwd escapes the workspace".into());
    }
    Ok(cwd)
}
