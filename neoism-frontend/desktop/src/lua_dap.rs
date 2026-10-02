use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use neoism_backend::event::{EventProxy, RioEvent, RioEventType};
use neoism_lua::{PluginDapStartRequest, PluginOwner};
use neoism_window::window::WindowId;
use crate::lua_async::{LuaAsyncSender, LuaAsyncToken};

struct Session { owner: PluginOwner, stdin: Arc<Mutex<Option<ChildStdin>>>, token: LuaAsyncToken }
#[derive(Default)] pub(crate) struct LuaDap { sessions: HashMap<String, Session> }

impl LuaDap {
    pub(crate) fn start(&mut self, owner: PluginOwner, id: String, window: WindowId, root: &Path, request: PluginDapStartRequest, token: LuaAsyncToken, sender: LuaAsyncSender, proxy: EventProxy) -> Result<(), String> {
        if self.sessions.len() >= 16 || self.sessions.values().filter(|session| session.owner == owner).count() >= 4 { return Err("debug adapter session limit reached".into()); }
        if request.command.is_empty() || request.command.len() > 256 || request.command[0].contains(['/', '\\']) { return Err("debug adapter requires a bounded executable name".into()); }
        let cwd = contained_cwd(root, request.cwd.as_deref())?;
        let mut command = Command::new(&request.command[0]); command.args(&request.command[1..]).current_dir(cwd).env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()); configure_process_group(&mut command);
        let mut child = command.spawn().map_err(|error| error.to_string())?; let pid = child.id();
        let stdin = Arc::new(Mutex::new(child.stdin.take())); let stdout = child.stdout.take().ok_or("debug adapter stdout is unavailable")?; let stderr = child.stderr.take().ok_or("debug adapter stderr is unavailable")?;
        self.sessions.insert(id.clone(), Session { owner: owner.clone(), stdin: stdin.clone(), token: token.clone() });
        let output_sender = sender.clone(); let output_owner = owner.clone(); let output_id = id.clone(); let output_proxy = proxy.clone();
        std::thread::spawn(move || read_dap(stdout, output_owner, output_id, window, output_sender, output_proxy));
        let error_sender = sender.clone(); let error_owner = owner.clone(); let error_id = id.clone(); let error_proxy = proxy.clone();
        std::thread::spawn(move || { let mut reader = BufReader::new(stderr); let mut line = String::new(); while reader.read_line(&mut line).ok().is_some_and(|size| size > 0) { error_sender.progress(error_owner.clone(), error_id.clone(), serde_json::json!({ "event": "stderr", "data": line })); error_proxy.send_event(RioEventType::Rio(RioEvent::Render), window); line.clear(); } });
        std::thread::spawn(move || { loop { if token.is_cancelled() { terminate(pid, &mut child); let _ = child.wait(); break; } match child.try_wait() { Ok(Some(status)) => { sender.complete(owner, id, serde_json::json!({ "event": "exit", "code": status.code() })); proxy.send_event(RioEventType::Rio(RioEvent::Render), window); break; }, Ok(None) => std::thread::sleep(Duration::from_millis(10)), Err(error) => { sender.fail(owner, id, "debug_wait", error.to_string()); proxy.send_event(RioEventType::Rio(RioEvent::Render), window); break; } } } drop(stdin.lock()); });
        Ok(())
    }
    pub(crate) fn send(&mut self, owner: &PluginOwner, id: &str, message: &serde_json::Value) -> Result<(), String> {
        let bytes = serde_json::to_vec(message).map_err(|error| error.to_string())?; if bytes.len() > 4 * 1024 * 1024 { return Err("DAP message exceeds 4 MiB".into()); }
        let session = self.sessions.get(id).filter(|session| &session.owner == owner).ok_or("debug session is stale or cross-owner")?;
        let mut stdin = session.stdin.lock().map_err(|_| "debug adapter stdin lock was poisoned")?; let stdin = stdin.as_mut().ok_or("debug adapter stdin is closed")?;
        write!(stdin, "Content-Length: {}\r\n\r\n", bytes.len()).and_then(|_| stdin.write_all(&bytes)).and_then(|_| stdin.flush()).map_err(|error| error.to_string())
    }
    pub(crate) fn stop(&mut self, owner: &PluginOwner, id: &str) -> bool { let Some(session) = self.sessions.get(id).filter(|session| &session.owner == owner) else { return false }; session.token.cancel(); self.sessions.remove(id); true }
    pub(crate) fn retire_inactive(&mut self, active: &HashSet<PluginOwner>) { for session in self.sessions.values() { if !active.contains(&session.owner) { session.token.cancel(); } } self.sessions.retain(|_, session| active.contains(&session.owner)); }
}

fn read_dap<R: Read>(reader: R, owner: PluginOwner, id: String, window: WindowId, sender: LuaAsyncSender, proxy: EventProxy) {
    let mut reader = BufReader::new(reader);
    loop {
        let mut length = None; let mut line = String::new();
        loop { line.clear(); let Ok(size) = reader.read_line(&mut line) else { return }; if size == 0 { return; } if line == "\r\n" || line == "\n" { break; } if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = value.trim().parse::<usize>().ok(); } }
        let Some(length) = length.filter(|length| *length <= 4 * 1024 * 1024) else { sender.fail(owner.clone(), id.clone(), "dap_frame", "invalid DAP content length"); return };
        let mut body = vec![0; length]; if reader.read_exact(&mut body).is_err() { return; }
        match serde_json::from_slice::<serde_json::Value>(&body) { Ok(message) => sender.progress(owner.clone(), id.clone(), serde_json::json!({ "event": "message", "message": message })), Err(error) => { sender.fail(owner.clone(), id.clone(), "dap_json", error.to_string()); return; } }
        proxy.send_event(RioEventType::Rio(RioEvent::Render), window);
    }
}

fn contained_cwd(root: &Path, requested: Option<&str>) -> Result<PathBuf, String> { let root = root.canonicalize().map_err(|error| error.to_string())?; let relative = Path::new(requested.unwrap_or(".")); if relative.is_absolute() { return Err("debug adapter cwd must be workspace-relative".into()); } let cwd = root.join(relative).canonicalize().map_err(|error| error.to_string())?; if !cwd.starts_with(&root) { return Err("debug adapter cwd escapes the workspace".into()); } Ok(cwd) }
#[cfg(unix)] fn configure_process_group(command: &mut Command) { use std::os::unix::process::CommandExt; unsafe { command.pre_exec(|| if libc::setpgid(0, 0) == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }); } }
#[cfg(windows)] fn configure_process_group(command: &mut Command) { use std::os::windows::process::CommandExt; command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW); }
#[cfg(not(any(unix, windows)))] fn configure_process_group(_: &mut Command) {}
#[cfg(unix)] fn terminate(pid: u32, child: &mut std::process::Child) { unsafe { libc::kill(-(pid as i32), libc::SIGKILL); } let _ = child.kill(); }
#[cfg(windows)] fn terminate(_: u32, child: &mut std::process::Child) { let _ = Command::new("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).status(); let _ = child.kill(); }
#[cfg(not(any(unix, windows)))] fn terminate(_: u32, child: &mut std::process::Child) { let _ = child.kill(); }