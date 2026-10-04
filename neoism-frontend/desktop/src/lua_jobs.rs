use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use neoism_backend::event::{EventProxy, RioEvent, RioEventType};
use neoism_lua::{PluginJobSpawnRequest, PluginOwner};
use neoism_window::window::WindowId;

use crate::lua_async::{LuaAsyncSender, LuaAsyncToken};

const MAX_JOBS_PER_OWNER: usize = 16;
const MAX_JOBS_GLOBAL: usize = 64;
const DEFAULT_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STDIN_BYTES: usize = 256 * 1024;
const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const CHUNK_BYTES: usize = 16 * 1024;

struct ActiveJob {
    owner: PluginOwner,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    token: LuaAsyncToken,
    finished: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct LuaJobs {
    active: HashMap<String, ActiveJob>,
}

impl LuaJobs {
    pub(crate) fn spawn(
        &mut self,
        owner: PluginOwner,
        id: String,
        window_id: WindowId,
        workspace_root: &Path,
        request: PluginJobSpawnRequest,
        token: LuaAsyncToken,
        sender: LuaAsyncSender,
        event_proxy: EventProxy,
    ) -> Result<(), String> {
        self.reap();
        if self.active.len() >= MAX_JOBS_GLOBAL
            || self
                .active
                .values()
                .filter(|job| job.owner == owner)
                .count()
                >= MAX_JOBS_PER_OWNER
        {
            return Err("managed job limit reached".into());
        }
        validate_program(&request.program)?;
        if request.arguments.len() > 256
            || request
                .arguments
                .iter()
                .any(|argument| argument.len() > 64 * 1024)
        {
            return Err("managed job arguments exceed limits".into());
        }
        let cwd = resolve_cwd(workspace_root, request.cwd.as_deref())?;
        let timeout = Duration::from_millis(request.timeout_millis.unwrap_or(30_000));
        if timeout.is_zero() || timeout > MAX_TIMEOUT {
            return Err("managed job timeout is outside the allowed range".into());
        }
        let output_limit = request.max_output_bytes.unwrap_or(DEFAULT_OUTPUT_BYTES);
        if output_limit == 0 || output_limit > MAX_OUTPUT_BYTES {
            return Err("managed job output limit is outside the allowed range".into());
        }
        validate_env(&request.env)?;

        let mut command = Command::new(&request.program);
        command
            .args(&request.arguments)
            .current_dir(cwd)
            .env_clear()
            .envs(&request.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_process_group(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to spawn managed job: {error}"))?;
        let pid = child.id();
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = child
            .stdout
            .take()
            .ok_or("managed job stdout pipe is unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("managed job stderr pipe is unavailable")?;
        let finished = Arc::new(AtomicBool::new(false));
        self.active.insert(
            id.clone(),
            ActiveJob {
                owner: owner.clone(),
                stdin: stdin.clone(),
                token: token.clone(),
                finished: finished.clone(),
            },
        );

        std::thread::Builder::new().name(format!("lua-job-{pid}")).spawn(move || {
            let total = Arc::new(AtomicUsize::new(0));
            let truncated = Arc::new(AtomicBool::new(false));
            let stdout_thread = spawn_reader("stdout", stdout, owner.clone(), id.clone(), sender.clone(), total.clone(), truncated.clone(), output_limit, event_proxy.clone(), window_id);
            let stderr_thread = spawn_reader("stderr", stderr, owner.clone(), id.clone(), sender.clone(), total.clone(), truncated.clone(), output_limit, event_proxy.clone(), window_id);
            let started = Instant::now();
            let (status, timed_out) = loop {
                if token.is_cancelled() {
                    terminate_process_tree(pid, &mut child);
                    break (child.wait().ok(), false);
                }
                if started.elapsed() >= timeout {
                    terminate_process_tree(pid, &mut child);
                    break (child.wait().ok(), true);
                }
                match child.try_wait() {
                    Ok(Some(status)) => break (Some(status), false),
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    Err(error) => {
                        sender.fail(owner.clone(), id.clone(), "wait_failed", error.to_string());
                        wake(&event_proxy, window_id);
                        finished.store(true, Ordering::Release);
                        return;
                    }
                }
            };
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            drop(stdin.lock());
            if !token.is_cancelled() {
                if timed_out {
                    sender.fail(owner.clone(), id.clone(), "timeout", "managed job exceeded its timeout");
                } else {
                    sender.complete(owner.clone(), id.clone(), serde_json::json!({
                        "event": "exit",
                        "code": status.as_ref().and_then(std::process::ExitStatus::code),
                        "success": status.as_ref().is_some_and(std::process::ExitStatus::success),
                        "outputBytes": total.load(Ordering::Acquire),
                        "truncated": truncated.load(Ordering::Acquire),
                    }));
                }
                wake(&event_proxy, window_id);
            }
            finished.store(true, Ordering::Release);
        }).map_err(|error| format!("failed to start managed job worker: {error}"))?;
        Ok(())
    }

    pub(crate) fn stdin(
        &mut self,
        owner: &PluginOwner,
        id: &str,
        data: &str,
    ) -> Result<(), String> {
        if data.len() > MAX_STDIN_BYTES {
            return Err("managed job stdin chunk exceeds limit".into());
        }
        let job = self
            .active
            .get(id)
            .filter(|job| &job.owner == owner)
            .ok_or("managed job handle is stale or cross-owner")?;
        let mut guard = job
            .stdin
            .lock()
            .map_err(|_| "managed job stdin lock was poisoned")?;
        guard
            .as_mut()
            .ok_or("managed job stdin is closed")?
            .write_all(data.as_bytes())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn close_stdin(&mut self, owner: &PluginOwner, id: &str) -> bool {
        self.active
            .get(id)
            .filter(|job| &job.owner == owner)
            .and_then(|job| job.stdin.lock().ok())
            .map(|mut stdin| stdin.take())
            .is_some()
    }

    pub(crate) fn retire_inactive(&mut self, active: &HashSet<PluginOwner>) {
        for job in self.active.values() {
            if !active.contains(&job.owner) {
                job.token.cancel();
            }
        }
        self.reap();
    }

    fn reap(&mut self) {
        self.active
            .retain(|_, job| !job.finished.load(Ordering::Acquire));
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    stream: &'static str,
    mut reader: R,
    owner: PluginOwner,
    id: String,
    sender: LuaAsyncSender,
    total: Arc<AtomicUsize>,
    truncated: Arc<AtomicBool>,
    limit: usize,
    event_proxy: EventProxy,
    window_id: WindowId,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; CHUNK_BYTES];
        loop {
            let Ok(read) = reader.read(&mut buffer) else {
                break;
            };
            if read == 0 {
                break;
            }
            let accepted = reserve_output(&total, read, limit);
            if accepted < read {
                truncated.store(true, Ordering::Release);
            }
            if accepted > 0 {
                sender.progress(
                    owner.clone(),
                    id.clone(),
                    serde_json::json!({
                        "event": "output", "stream": stream,
                        "data": String::from_utf8_lossy(&buffer[..accepted]),
                    }),
                );
                wake(&event_proxy, window_id);
            }
        }
    })
}

fn reserve_output(total: &AtomicUsize, wanted: usize, limit: usize) -> usize {
    let mut current = total.load(Ordering::Acquire);
    loop {
        let accepted = wanted.min(limit.saturating_sub(current));
        match total.compare_exchange_weak(
            current,
            current + accepted,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return accepted,
            Err(actual) => current = actual,
        }
    }
}

fn validate_program(program: &str) -> Result<(), String> {
    if program.is_empty()
        || program.len() > 256
        || program.contains('/')
        || program.contains('\\')
        || program == "."
        || program == ".."
    {
        Err("managed jobs require a bounded executable name, not a path".into())
    } else {
        Ok(())
    }
}

fn validate_env(env: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
    if env.len() > 64 {
        return Err("managed job environment exceeds limit".into());
    }
    for (key, value) in env {
        if key.is_empty()
            || key.len() > 128
            || value.len() > 16 * 1024
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || matches!(
                key.as_str(),
                "PATH"
                    | "HOME"
                    | "LD_PRELOAD"
                    | "LD_LIBRARY_PATH"
                    | "DYLD_INSERT_LIBRARIES"
            )
        {
            return Err(format!(
                "managed job environment key `{key}` is not allowed"
            ));
        }
    }
    Ok(())
}

fn resolve_cwd(root: &Path, requested: Option<&str>) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("workspace root is unavailable: {error}"))?;
    let relative = requested.unwrap_or(".");
    let path = Path::new(relative);
    if path.is_absolute() {
        return Err("managed job cwd must be workspace-relative".into());
    }
    let cwd = root
        .join(path)
        .canonicalize()
        .map_err(|error| format!("managed job cwd is unavailable: {error}"))?;
    if !cwd.starts_with(&root) {
        return Err("managed job cwd escapes the workspace".into());
    }
    Ok(cwd)
}

fn wake(proxy: &EventProxy, window_id: WindowId) {
    proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id);
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(
        windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP
            | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
    );
}

#[cfg(not(any(unix, windows)))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_tree(pid: u32, child: &mut std::process::Child) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(windows)]
fn terminate_process_tree(pid: u32, child: &mut std::process::Child) {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new("taskkill");
    command
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    let _ = command.status();
    let _ = child.kill();
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_tree(_pid: u32, child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_output_reservation_never_exceeds_bound() {
        let total = AtomicUsize::new(0);
        assert_eq!(reserve_output(&total, 7, 10), 7);
        assert_eq!(reserve_output(&total, 7, 10), 3);
        assert_eq!(reserve_output(&total, 1, 10), 0);
        assert_eq!(total.load(Ordering::Acquire), 10);
    }

    #[test]
    fn cwd_and_environment_policy_reject_escape_and_loader_injection() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-job-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(resolve_cwd(&root, Some(".")).is_ok());
        assert!(resolve_cwd(&root, Some("../")).is_err());
        let mut env = std::collections::BTreeMap::new();
        env.insert("LD_PRELOAD".into(), "evil".into());
        assert!(validate_env(&env).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn real_process_output_is_truncated_at_the_aggregate_bound() {
        #[cfg(unix)]
        {
            let output = Command::new("sh")
                .args(["-c", "printf 123456789; printf abcdefghi >&2"])
                .output()
                .unwrap();
            let total = AtomicUsize::new(0);
            let stdout = reserve_output(&total, output.stdout.len(), 10);
            let stderr = reserve_output(&total, output.stderr.len(), 10);
            assert_eq!(stdout + stderr, 10);
            assert_eq!(total.load(Ordering::Acquire), 10);
        }
    }
}
