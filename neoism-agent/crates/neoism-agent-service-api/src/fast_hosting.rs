//! Owned temporary dedicated daemons. This is lifecycle isolation, NOT an
//! execution sandbox: invited collaborators' shells/agents run as the host user.
use std::{
    ffi::OsString,
    io::{self, Read},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[cfg(unix)]
mod process_owner;

const TAIL_BYTES: usize = 16 * 1024;
const LINE_BYTES: usize = 8 * 1024;
pub const FAST_READINESS_PROTOCOL: &str = "neoism-fast-ready-v1";

/// stdout JSON-line contract shared with the standalone runner. This proves
/// startup only; the parent still performs authenticated service checks.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FastStartupAcknowledgement {
    pub protocol: String,
    pub nonce: String,
    pub address: SocketAddr,
}

#[derive(Default)]
struct CapturedOutput {
    stdout: String,
    stderr: String,
    ack: Option<FastStartupAcknowledgement>,
    ack_error: Option<&'static str>,
}

// Never expose an environment dump, a full URL (including WS token URLs), or
// credential-bearing lines. Redact before retaining the bounded diagnostic tail.
fn safe_output(text: &str, private_token: &str) -> String {
    let text = if private_token.is_empty() {
        text.to_owned()
    } else {
        text.replace(private_token, "[redacted]")
    };
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if lower.contains("bearer")
                || lower.contains("token")
                || line.contains("NEOISM_")
                || line.contains("HOME=")
                || line.split_whitespace().any(|word| {
                    word.split_once('=').is_some_and(|(key, _)| {
                        !key.is_empty()
                            && key.bytes().all(|b| {
                                b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit()
                            })
                    })
                })
            {
                return "[redacted credential/environment output]".to_owned();
            }
            line.split_whitespace()
                .map(|word| {
                    if word.contains("://") {
                        "[redacted URL]".to_owned()
                    } else {
                        word.chars().filter(|c| !c.is_control()).collect()
                    }
                })
                .collect::<Vec<String>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn append_tail(tail: &mut String, line: &str) {
    tail.push_str(line);
    tail.push('\n');
    if tail.len() > TAIL_BYTES {
        let mut cut = tail.len() - TAIL_BYTES;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
}

fn capture_line(
    output: &mut CapturedOutput,
    line: &[u8],
    stdout: bool,
    nonce: &str,
    address: SocketAddr,
    token: &str,
) {
    let line = String::from_utf8_lossy(line);
    if stdout && line.contains(FAST_READINESS_PROTOCOL) {
        match serde_json::from_str::<FastStartupAcknowledgement>(line.trim()) {
            Ok(ack) if ack.protocol == FAST_READINESS_PROTOCOL && ack.nonce == nonce && ack.address == address => {
                output.ack = Some(ack);
            }
            _ => output.ack_error = Some("Invalid Fast daemon readiness acknowledgement (protocol, nonce or address mismatch)"),
        }
        return;
    }
    // Catch a structurally valid acknowledgement with an unsupported version.
    if stdout
        && serde_json::from_str::<serde_json::Value>(line.trim())
            .ok()
            .is_some_and(|v| {
                v.get("protocol").is_some()
                    && v.get("nonce").is_some()
                    && v.get("address").is_some()
            })
    {
        output.ack_error = Some("Unsupported Fast daemon readiness protocol");
        return;
    }
    let safe = safe_output(&line, token);
    append_tail(
        if stdout {
            &mut output.stdout
        } else {
            &mut output.stderr
        },
        &safe,
    );
}

#[cfg(unix)]
fn nonblocking_pipe(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn drain_output(
    reader: impl Read + Send + 'static,
    output: Arc<Mutex<CapturedOutput>>,
    cancelled: Arc<AtomicBool>,
    stdout: bool,
    nonce: String,
    address: SocketAddr,
    token: String,
) -> io::Result<()> {
    // No JoinHandle is retained: teardown never waits on a pipe held open by a
    // descendant. Readers hold no TempDir or process ownership guards.
    std::thread::Builder::new()
        .name("fast-daemon-output".into())
        .spawn(move || {
            let mut reader = reader;
            let mut chunk = [0u8; 4096];
            let mut line = Vec::with_capacity(LINE_BYTES);
            let mut oversized = false;
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let count = match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => {
                        let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
                        append_tail(
                            if stdout {
                                &mut output.stdout
                            } else {
                                &mut output.stderr
                            },
                            "[output capture read failed]",
                        );
                        break;
                    }
                };
                for &byte in &chunk[..count] {
                    if byte == b'\n' {
                        let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
                        if oversized {
                            append_tail(
                                if stdout {
                                    &mut output.stdout
                                } else {
                                    &mut output.stderr
                                },
                                "[oversized output line omitted]",
                            );
                        } else {
                            capture_line(
                                &mut output,
                                &line,
                                stdout,
                                &nonce,
                                address,
                                &token,
                            );
                        }
                        line.clear();
                        oversized = false;
                    } else if line.len() < LINE_BYTES && !oversized {
                        line.push(byte);
                    } else {
                        oversized = true;
                        line.clear();
                    }
                }
            }
            let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
            if oversized {
                append_tail(
                    if stdout {
                        &mut output.stdout
                    } else {
                        &mut output.stderr
                    },
                    "[oversized output line omitted]",
                );
            } else if !line.is_empty() {
                capture_line(&mut output, &line, stdout, &nonce, address, &token);
            }
        })?;
    Ok(())
}

pub struct FastDaemon {
    child: Child,
    output: Arc<Mutex<CapturedOutput>>,
    cancelled: Arc<AtomicBool>,
    private_token: String,
    exit_status: Option<std::process::ExitStatus>,
    _state: tempfile::TempDir,
    pub directory: PathBuf,
    pub address: SocketAddr,
    stopped: bool,
    #[cfg(unix)]
    processes: process_owner::ProcessOwner,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}
// The Windows job handle is exclusively owned and only closed during teardown.
#[cfg(windows)]
unsafe impl Send for FastDaemon {}
impl FastDaemon {
    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        #[cfg(unix)]
        self.processes.refresh();
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.exit_status = status;
        }
        Ok(status)
    }
    /// Bounded, redacted tails only; never command/env dumps or token URLs.
    pub fn diagnostics(&self) -> String {
        let output = self.output.lock().unwrap_or_else(|e| e.into_inner());
        format!(
            "Fast daemon status: {}\nreadiness: {}\nstdout tail:\n{}\nstderr tail:\n{}",
            self.exit_status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "running or not yet polled".into()),
            output.ack_error.unwrap_or(if output.ack.is_some() {
                "acknowledged"
            } else {
                "pending"
            }),
            output.stdout,
            output.stderr
        )
    }
    pub fn startup_failure(&mut self, stage: &str) -> String {
        let _ = self.try_wait();
        let stage = if stage.len() > LINE_BYTES {
            "[oversized stage omitted]"
        } else {
            stage
        };
        format!(
            "Fast daemon startup failed at {}\n{}",
            safe_output(stage, &self.private_token),
            self.diagnostics()
        )
    }
    /// Parent uses this for root-service/prebind/chat-association HTTP failures:
    /// retain the HTTP status and bounded response body, not the request URL.
    pub fn startup_http_failure(
        &mut self,
        stage: &str,
        status: u16,
        body: &str,
    ) -> String {
        let body = if body.len() > LINE_BYTES {
            "[oversized response body omitted]"
        } else {
            body
        };
        format!(
            "{}\nHTTP {status}; response: {}",
            self.startup_failure(stage),
            safe_output(body, &self.private_token)
        )
    }
    /// Parent-owned deadlines should poll this without blocking the UI.
    pub fn try_startup_ack(&mut self) -> io::Result<bool> {
        let exited = self.try_wait()?.is_some();
        let output = self.output.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = output.ack_error {
            return Err(io::Error::other(error));
        }
        if exited {
            return Err(io::Error::other(
                "Fast daemon exited before readiness could be accepted",
            ));
        }
        Ok(output.ack.is_some())
    }
    pub fn startup_ack(&self) -> Option<FastStartupAcknowledgement> {
        self.output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ack
            .clone()
    }
    /// Blocking convenience for non-UI callers; desktop should use its own deadline.
    pub fn wait_readiness(
        &mut self,
        timeout: Duration,
    ) -> io::Result<FastStartupAcknowledgement> {
        let start = Instant::now();
        loop {
            match self.try_startup_ack() {
                Ok(true) => return Ok(self.startup_ack().expect("ack is retained")),
                Err(_) => {
                    return Err(io::Error::other(
                        self.startup_failure("readiness acknowledgement"),
                    ))
                }
                Ok(false) => {}
            }
            if start.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    self.startup_failure("readiness acknowledgement timeout"),
                ));
            }
            std::thread::sleep(
                Duration::from_millis(10).min(timeout.saturating_sub(start.elapsed())),
            );
        }
    }
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.cancelled.store(true, Ordering::Relaxed);
        #[cfg(unix)]
        self.processes.kill();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
        let _ = self.child.kill();
        if let Ok(status) = self.child.wait() {
            self.exit_status = Some(status);
        }
    }
}
impl Drop for FastDaemon {
    fn drop(&mut self) {
        self.stop();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

pub fn validate_fast_directory(directory: &Path) -> io::Result<PathBuf> {
    #[cfg(windows)]
    let root = dunce::canonicalize(directory)?;
    #[cfg(not(windows))]
    let root = std::fs::canonicalize(directory)?;
    if !root.is_dir() || root.parent().is_none() {
        return Err(io::Error::other(
            "Choose a project directory, not a filesystem root, for Fast Host",
        ));
    }
    Ok(root)
}
fn original_config_directory() -> Option<PathBuf> {
    if let Some(directory) = std::env::var_os("NEOISM_CONFIG_HOME") {
        return Some(directory.into());
    }
    #[cfg(windows)]
    {
        return dirs::home_dir().map(|home| home.join("AppData/Local/neoism"));
    }
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir().map(|home| home.join(".config/neoism"));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
            .map(|dir| dir.join("neoism"))
    }
}

fn credential_source_paths(parent_directory: &Path) -> (PathBuf, PathBuf) {
    // Match LocalProviderCredentialStore/LocalMcpCredentialStore, BEFORE the
    // child changes cwd. In particular Windows/HOME-unset and relative explicit
    // paths must still refer to the host's real stores, not the chosen project.
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        });
    let provider = std::env::var_os("NEOISM_AGENT_AUTH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            state_home
                .clone()
                .unwrap_or_else(|| ".state".into())
                .join("neoism-agent/auth.json")
        });
    let mcp = std::env::var_os("NEOISM_AGENT_MCP_AUTH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("NEOISM_AGENT_STATE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    state_home
                        .map(|base| base.join("neoism"))
                        .unwrap_or_else(|| ".neoism/state".into())
                })
                .join("mcp-auth.json")
        });
    (
        resolve_before_chdir(provider, parent_directory),
        resolve_before_chdir(mcp, parent_directory),
    )
}
fn resolve_before_chdir(path: PathBuf, parent_directory: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        parent_directory.join(path)
    }
}

fn free_loopback() -> io::Result<SocketAddr> {
    TcpListener::bind("127.0.0.1:0")?.local_addr()
}

/// Fresh state, loopback-only bind, mandatory daemon auth, exact declared root.
/// Unix descendants use a process group and a daemon-side parent death watcher;
/// Windows descendants use a kill-on-close job. No detached saved-server path.
pub fn spawn_fast_daemon(
    binary: &Path,
    directory: &Path,
    token: &str,
) -> io::Result<FastDaemon> {
    let agent_server = std::env::var("NEOISM_AGENT_SERVER")
        .or_else(|_| std::env::var("NEOISM_SERVER"))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Fast daemon requires an existing shared Agent endpoint; use spawn_fast_daemon_with_prefix after validating Agent health"))?;
    spawn_fast_daemon_with_prefix(
        binary,
        &[],
        directory,
        free_loopback()?,
        token,
        &agent_server,
    )
}

fn validate_shared_agent_endpoint(agent_server: &str) -> io::Result<()> {
    let invalid = || {
        io::Error::new(io::ErrorKind::InvalidInput,
        "Fast Host requires a direct local Agent HTTP root endpoint (127.0.0.1, localhost or [::1]); remote endpoints and proxy paths are not supported")
    };
    let endpoint = url::Url::parse(agent_server).map_err(|_| invalid())?;
    let local = match endpoint.host() {
        Some(url::Host::Domain(host)) => host == "localhost",
        Some(url::Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(ip)) => ip == std::net::Ipv6Addr::LOCALHOST,
        None => false,
    };
    // URL parsing normalizes dot segments and backslashes. Reject raw prefixes
    // too rather than accidentally accepting a proxy URL after normalization.
    let raw_authority_and_path = agent_server
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(invalid)?;
    let (authority, raw_path) = raw_authority_and_path
        .split_once('/')
        .unwrap_or((raw_authority_and_path, ""));
    if endpoint.scheme() != "http"
        || !local
        || endpoint.path() != "/"
        || !raw_path.is_empty()
        || authority.contains('@')
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.port() == Some(0)
        || agent_server.trim() != agent_server
        || agent_server.chars().any(char::is_control)
        || agent_server.contains('\\')
    {
        return Err(invalid());
    }
    Ok(())
}

/// Prefix arguments are passed verbatim before standalone daemon flags. Desktop
/// uses current_exe + ["--neoism-internal-fast-workspace-daemon"]. Empty prefix
/// retains the standalone sibling contract. No shell or argument re-parsing.
/// `agent_server` must name the existing shared Agent whose /v2/health the
/// parent has validated. Only direct loopback HTTP root endpoints are allowed;
/// remote Agents/proxy paths cannot execute this machine's physical root.
/// Never allocate an Agent port here: spawning another
/// Agent against the same chat database fails its exclusive WAL lock.
/// The shared Agent is not owned by this temporary daemon's process tree.
pub fn spawn_fast_daemon_with_prefix(
    binary: &Path,
    prefix: &[OsString],
    directory: &Path,
    address: SocketAddr,
    token: &str,
    agent_server: &str,
) -> io::Result<FastDaemon> {
    validate_shared_agent_endpoint(agent_server)?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Fast daemon requires a concrete loopback address",
        ));
    }
    let directory = validate_fast_directory(directory)?;
    // Keep absolute procfs paths intact: canonicalizing a running executable
    // after an update resolves to an unusable pathname ending in " (deleted)".
    let binary = if binary.is_absolute() {
        binary.to_path_buf()
    } else {
        std::fs::canonicalize(binary)?
    };
    let state = tempfile::Builder::new()
        .prefix("neoism-fast-daemon-")
        .tempdir()?;
    let nonce = uuid::Uuid::new_v4().to_string();
    // Snapshot only the product config, never the saved server/device registry
    // or daemon token. Preserve model/provider defaults without letting this
    // temporary daemon write the desktop's global config.
    let credentials = state.path().join("credentials");
    std::fs::create_dir(&credentials)?;
    let source_config = original_config_directory().map(|dir| dir.join("config.json"));
    if let Some(source) = source_config.filter(|path| path.is_file()) {
        std::fs::copy(source, credentials.join("config.json"))?;
    }
    let (provider_auth, mcp_auth) = credential_source_paths(&std::env::current_dir()?);
    let mut command = Command::new(binary);
    command
        .args(prefix)
        .env("NEOISM_FAST_READINESS_NONCE", &nonce)
        .args([
            "--addr",
            &address.to_string(),
            "--no-unix-socket",
            "--ephemeral",
            "--temporary-parent-pid",
            &std::process::id().to_string(),
            "--state-dir",
        ])
        .arg(state.path().join("state"))
        .arg("--workspace")
        .arg(&directory)
        .current_dir(&directory)
        .env("NEOISM_AGENT_AUTH_PATH", provider_auth)
        .env("NEOISM_AGENT_MCP_AUTH_PATH", mcp_auth)
        .env("NEOISM_FAST_SERVER", "1")
        .env("NEOISM_REQUIRE_AUTH", "1")
        .env("NEOISM_DAEMON_TOKEN", token)
        .env("NEOISM_DAEMON_DATA_DIR", state.path().join("auth"))
        .env("NEOISM_CONFIG_DIR", state.path().join("config"))
        .env("NEOISM_CONFIG_HOME", state.path().join("credentials"))
        .env("NEOISM_AGENT_SERVER", agent_server)
        .env("NEOISM_SERVER", agent_server)
        .env_remove("NEOISM_PROVISION_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    #[cfg(windows)]
    let job = {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if job.is_null()
                || SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0
            {
                let error = io::Error::last_os_error();
                let _ = child.kill();
                let _ = child.wait();
                if !job.is_null() {
                    CloseHandle(job);
                }
                return Err(error);
            }
            job
        }
    };
    #[cfg(not(windows))]
    let _ = &mut child;
    #[cfg(unix)]
    let processes = process_owner::ProcessOwner::new(child.id());
    let output = Arc::new(Mutex::new(CapturedOutput::default()));
    let mut daemon = FastDaemon {
        #[cfg(unix)]
        processes,
        output: output.clone(),
        cancelled: Arc::new(AtomicBool::new(false)),
        private_token: token.to_owned(),
        exit_status: None,
        child,
        _state: state,
        directory,
        address,
        stopped: false,
        #[cfg(windows)]
        job,
    };
    #[cfg(unix)]
    {
        nonblocking_pipe(daemon.child.stdout.as_ref().expect("piped stdout"))?;
        nonblocking_pipe(daemon.child.stderr.as_ref().expect("piped stderr"))?;
    }
    drain_output(
        daemon.child.stdout.take().expect("piped stdout"),
        output.clone(),
        daemon.cancelled.clone(),
        true,
        nonce.clone(),
        address,
        token.to_owned(),
    )?;
    drain_output(
        daemon.child.stderr.take().expect("piped stderr"),
        output,
        daemon.cancelled.clone(),
        false,
        nonce,
        address,
        token.to_owned(),
    )?;
    Ok(daemon)
}

/// Best-effort teardown of owned Unix descendants, including PTYs which
/// create their own sessions. This is lifecycle cleanup, not containment of
/// deliberately double-forked/detached programs started by trusted guests.
#[cfg(unix)]
pub fn terminate_fast_process_tree(pid: u32) {
    process_owner::ProcessOwner::new(pid).kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an installed nightly and a running local Agent"]
    fn installed_fast_daemon_readiness_probe() {
        let binary = PathBuf::from(
            std::env::var_os("NEOISM_FAST_REPRO_BINARY")
                .expect("set NEOISM_FAST_REPRO_BINARY"),
        );
        let directory = PathBuf::from(
            std::env::var_os("NEOISM_FAST_REPRO_DIRECTORY")
                .expect("set NEOISM_FAST_REPRO_DIRECTORY"),
        );
        let agent = std::env::var("NEOISM_FAST_REPRO_AGENT")
            .expect("set NEOISM_FAST_REPRO_AGENT");
        let mut daemon = spawn_fast_daemon_with_prefix(
            &binary,
            &[OsString::from("--neoism-internal-fast-workspace-daemon")],
            &directory,
            free_loopback().unwrap(),
            &uuid::Uuid::new_v4().to_string(),
            &agent,
        )
        .unwrap();
        if daemon.wait_readiness(Duration::from_secs(120)).is_err() {
            panic!("{}", daemon.startup_failure("isolated readiness probe"));
        }
    }

    #[test]
    fn shared_agent_endpoint_must_be_direct_local_http_root() {
        for endpoint in [
            "http://127.0.0.1:4096",
            "http://127.0.0.1:4096/",
            "http://localhost:4096",
            "http://[::1]:4096/",
        ] {
            validate_shared_agent_endpoint(endpoint).unwrap();
        }
        for endpoint in [
            "https://localhost:4096",
            "http://remote.example:4096",
            "http://192.168.1.2:4096",
            "http://127.0.0.2:4096",
            "http://[::2]:4096",
            "http://localhost:4096/agent",
            "http://localhost:4096/agent/..",
            "http://localhost:4096//",
            "http://localhost:4096/?token=secret",
            "http://localhost:4096/#secret",
            "http://user:secret@localhost:4096",
            "http://@localhost:4096",
            "http://localhost:0",
            " http://localhost:4096",
            "not a URL",
        ] {
            let error = validate_shared_agent_endpoint(endpoint).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(!error.to_string().contains(endpoint));
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[test]
    fn readiness_parser_checks_nonce_address_and_version() {
        let address = "127.0.0.1:12345".parse().unwrap();
        let mut output = CapturedOutput::default();
        let ack = serde_json::json!({"protocol": FAST_READINESS_PROTOCOL, "nonce": "expected", "address": address});
        capture_line(
            &mut output,
            ack.to_string().as_bytes(),
            true,
            "expected",
            address,
            "secret",
        );
        assert!(output.ack.is_some());
        assert!(output.stdout.is_empty());
        let old_key = serde_json::json!({"protocol": FAST_READINESS_PROTOCOL, "nonce": "expected", "addr": address});
        let mut rejected = CapturedOutput::default();
        capture_line(
            &mut rejected,
            old_key.to_string().as_bytes(),
            true,
            "expected",
            address,
            "secret",
        );
        assert!(rejected.ack_error.is_some());
        assert!(rejected.ack.is_none());
        for field in ["nonce", "address", "protocol"] {
            let mut invalid = ack.clone();
            invalid[field] = serde_json::json!("wrong");
            let mut output = CapturedOutput::default();
            capture_line(
                &mut output,
                invalid.to_string().as_bytes(),
                true,
                "expected",
                address,
                "secret",
            );
            assert!(output.ack_error.is_some());
            assert!(output.ack.is_none());
        }
    }

    #[test]
    fn diagnostic_redaction_is_conservative() {
        let safe = safe_output("plain private-master text\nAuthorization: Bearer abc\n{\"token\":\"xyz\"}\nconnect ws://localhost/ws?auth=abc\nNEOISM_DAEMON_TOKEN=abc\nPASSWORD=abc\nHTTP 409: workspace already associated", "private-master");
        for forbidden in ["private-master", "abc", "xyz", "ws://", "PASSWORD="] {
            assert!(!safe.contains(forbidden), "{safe}");
        }
        assert!(safe.contains("HTTP 409: workspace already associated"));
    }

    #[cfg(unix)]
    fn spawn_fake_daemon(
        binary: &Path,
        directory: &Path,
        token: &str,
    ) -> io::Result<FastDaemon> {
        // Offline fixture: scripts never contact this endpoint or open chat state.
        spawn_fast_daemon_with_prefix(
            binary,
            &[],
            directory,
            free_loopback()?,
            token,
            "http://127.0.0.1:49123",
        )
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn absolute_proc_executable_survives_unlinked_binary() {
        use std::io::{BufRead, BufReader};
        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let project = tempfile::tempdir().unwrap();
        let binary = project.path().join("running-shell");
        std::fs::copy("/bin/sh", &binary).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut owner = loop {
            let launched = Command::new(&binary)
                .args(["-c", "printf 'ready\\n'; read wait"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn();
            match launched {
                Ok(child) => break OwnedChild(child),
                // A concurrent test fork can briefly inherit the copy's write fd.
                Err(error)
                    if error.raw_os_error() == Some(libc::ETXTBSY)
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => panic!("could not start ELF fixture: {error}"),
            }
        };
        let mut ready = String::new();
        BufReader::new(owner.0.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        std::fs::remove_file(binary).unwrap();
        let executable = PathBuf::from(format!("/proc/{}/exe", owner.0.id()));
        assert!(std::fs::read_link(&executable)
            .unwrap()
            .to_string_lossy()
            .ends_with(" (deleted)"));
        let mut daemon = spawn_fast_daemon_with_prefix(
            &executable,
            &[OsString::from("-c"), OsString::from("exit 42")],
            project.path(),
            free_loopback().unwrap(),
            "private-master",
            "http://127.0.0.1:49123",
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = daemon.try_wait().unwrap() {
                assert_eq!(status.code(), Some(42));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn fake_child(project: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let binary = project.join("fake-output-daemon");
        std::fs::write(&binary, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        binary
    }

    #[cfg(unix)]
    #[test]
    fn explicit_shared_agent_endpoint_is_preserved_in_both_environment_keys() {
        let project = tempfile::tempdir().unwrap();
        let binary = fake_child(
            project.path(),
            r#"
            printf '%s\n%s\n' "$NEOISM_AGENT_SERVER" "$NEOISM_SERVER" > endpoint
        "#,
        );
        let endpoint = "http://127.0.0.1:49234";
        let mut daemon = spawn_fast_daemon_with_prefix(
            &binary,
            &[],
            project.path(),
            free_loopback().unwrap(),
            "private-master",
            endpoint,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while daemon.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(project.path().join("endpoint")).unwrap(),
            format!("{endpoint}\n{endpoint}\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn stopping_owned_daemon_does_not_kill_shared_sibling_process() {
        struct SharedFixture(Child);
        impl Drop for SharedFixture {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        // Represents the already-running home Agent: outside the owned process
        // group, with no real network, history database or association requests.
        let mut shared =
            SharedFixture(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let project = tempfile::tempdir().unwrap();
        let binary = fake_child(project.path(), "while :; do /bin/sleep 1; done");
        let mut daemon =
            spawn_fake_daemon(&binary, project.path(), "private-master").unwrap();
        daemon.stop();
        assert!(shared.0.try_wait().unwrap().is_none());
        assert!(daemon.try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn fake_child_drains_both_streams_and_retains_exit_status_and_safe_tails() {
        let project = tempfile::tempdir().unwrap();
        let binary = fake_child(
            project.path(),
            r#"
            printf '%20000s\n' x
            printf '%20000s\n' x >&2
            i=0
            while [ "$i" -lt 2000 ]; do
                printf 'out-%s\n' "$i"
                printf 'err-%s\n' "$i" >&2
                i=$((i + 1))
            done
            printf 'private-master\nAuthorization: Bearer credential\nws://localhost/ws?token=credential\n'
            printf 'stdout-final\n'
            printf 'stderr-final\n' >&2
            exit 23
        "#,
        );
        let mut daemon =
            spawn_fake_daemon(&binary, project.path(), "private-master").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let done = daemon.try_wait().unwrap().is_some();
            let diagnostics = daemon.diagnostics();
            if done
                && diagnostics.contains("stdout-final")
                && diagnostics.contains("stderr-final")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child/output capture did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(daemon.try_wait().unwrap().unwrap().code(), Some(23));
        let output = daemon.output.lock().unwrap();
        assert!(output.stdout.len() <= TAIL_BYTES && output.stderr.len() <= TAIL_BYTES);
        drop(output);
        let failure =
            daemon.startup_http_failure("chat association", 409, "already associated");
        assert!(
            failure.contains("23")
                && failure.contains("HTTP 409")
                && failure.contains("already associated")
        );
        for forbidden in ["private-master", "Bearer credential", "ws://"] {
            assert!(!failure.contains(forbidden));
        }
        let failure = daemon.startup_http_failure(
            "prebind",
            403,
            "{\"token\":\"private-master\",\"error\":\"forbidden\"}",
        );
        assert!(failure.contains("HTTP 403"));
        assert!(!failure.contains("private-master"));
    }

    #[cfg(unix)]
    #[test]
    fn prefix_arguments_are_verbatim_and_readiness_is_nonblocking() {
        let project = tempfile::tempdir().unwrap();
        let binary = fake_child(
            project.path(),
            r#"
            printf '%s\n' "$@" > argv
            shift 2
            addr="$2"
            printf '{"protocol":"neoism-fast-ready-v1","nonce":"%s","address":"%s"}\n' "$NEOISM_FAST_READINESS_NONCE" "$addr"
            while :; do /bin/sleep 1; done
        "#,
        );
        let prefix = [
            OsString::from("--neoism-internal-fast-workspace-daemon"),
            OsString::from("literal space ; $arg"),
        ];
        let address = free_loopback().unwrap();
        let mut daemon = spawn_fast_daemon_with_prefix(
            &binary,
            &prefix,
            project.path(),
            address,
            "private-master",
            "http://127.0.0.1:49123",
        )
        .unwrap();
        let ack = daemon.wait_readiness(Duration::from_secs(2)).unwrap();
        assert_eq!(ack.address, address);
        assert_eq!(ack.protocol, FAST_READINESS_PROTOCOL);
        assert!(daemon.try_startup_ack().unwrap());
        let argv = std::fs::read_to_string(project.path().join("argv")).unwrap();
        let args: Vec<_> = argv.lines().collect();
        assert_eq!(
            args,
            vec![
                "--neoism-internal-fast-workspace-daemon",
                "literal space ; $arg",
                "--addr",
                &address.to_string(),
                "--no-unix-socket",
                "--ephemeral",
                "--temporary-parent-pid",
                &std::process::id().to_string(),
                "--state-dir",
                daemon._state.path().join("state").to_str().unwrap(),
                "--workspace",
                project.path().to_str().unwrap()
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn signal_exit_is_not_flattened_to_an_exit_code() {
        use std::os::unix::process::ExitStatusExt;
        let project = tempfile::tempdir().unwrap();
        let binary = fake_child(
            project.path(),
            "printf 'before-signal\\n' >&2; kill -TERM $$",
        );
        let mut daemon =
            spawn_fake_daemon(&binary, project.path(), "private-master").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while daemon.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            daemon.try_wait().unwrap().unwrap().signal(),
            Some(libc::SIGTERM)
        );
        assert!(daemon.startup_failure("private child").contains("signal"));
    }

    #[cfg(unix)]
    #[test]
    fn oversized_lines_are_omitted_without_secret_fragments() {
        let project = tempfile::tempdir().unwrap();
        let binary = fake_child(project.path(), "printf 'private-master'; printf '%20000s' x; printf 'private-master\\n'; printf 'final\\n' >&2; exit 7");
        let daemon =
            spawn_fake_daemon(&binary, project.path(), "private-master").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !daemon
            .diagnostics()
            .contains("oversized output line omitted")
            || !daemon.diagnostics().contains("final")
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!daemon.diagnostics().contains("private-master"));
    }

    #[test]
    fn credential_paths_are_resolved_before_project_chdir() {
        let parent = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_before_chdir(".state/auth.json".into(), parent.path()),
            parent.path().join(".state/auth.json")
        );
        let absolute = parent.path().join("auth.json");
        assert_eq!(
            resolve_before_chdir(absolute.clone(), parent.path()),
            absolute
        );
        let (provider, mcp) = credential_source_paths(parent.path());
        assert!(provider.is_absolute());
        assert!(mcp.is_absolute());
    }
    #[cfg(unix)]
    #[test]
    fn child_uses_host_credential_stores_and_private_product_config() {
        use std::{os::unix::fs::PermissionsExt, time::Duration};
        let project = tempfile::tempdir().unwrap();
        let binary = project.path().join("env-daemon");
        std::fs::write(&binary, "#!/bin/sh\nprintf '%s\\n' \"$NEOISM_AGENT_AUTH_PATH\" \"$NEOISM_AGENT_MCP_AUTH_PATH\" \"$NEOISM_CONFIG_HOME\" > stores\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let expected = credential_source_paths(&std::env::current_dir().unwrap());
        let mut daemon =
            spawn_fake_daemon(&binary, project.path(), "private-master").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while daemon.try_wait().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let text = std::fs::read_to_string(project.path().join("stores")).unwrap();
        let paths: Vec<_> = text.lines().map(PathBuf::from).collect();
        assert_eq!(paths[0], expected.0);
        assert_eq!(paths[1], expected.1);
        assert!(paths[2].starts_with(daemon._state.path()));
    }

    #[test]
    fn root_and_missing_directory_are_rejected() {
        #[cfg(unix)]
        assert!(validate_fast_directory(Path::new("/")).is_err());
        assert!(
            validate_fast_directory(Path::new("/nonexistent-fast-project-1234")).is_err()
        );
        let project = tempfile::tempdir().unwrap();
        assert!(validate_fast_directory(project.path()).is_ok());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn stop_cleans_retained_pty_after_direct_daemon_crash() {
        use std::{
            os::unix::fs::PermissionsExt,
            time::{Duration, Instant},
        };
        let project = tempfile::tempdir().unwrap();
        let binary = project.path().join("pty-daemon");
        std::fs::write(&binary, "#!/bin/sh\n/usr/bin/setsid /bin/sleep 30 &\necho $! > pty.pid\n/bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let mut daemon =
            spawn_fake_daemon(&binary, project.path(), "private-master").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !project.path().join("pty.pid").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(daemon.try_wait().unwrap().is_none()); // live ownership poll
        let pty: u32 = std::fs::read_to_string(project.path().join("pty.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        daemon.child.kill().unwrap();
        daemon.child.wait().unwrap();
        assert!(daemon.try_wait().unwrap().is_some());
        daemon.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{pty}/stat"));
            if stat.is_err()
                || stat
                    .unwrap()
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .split_whitespace()
                    .next()
                    == Some("Z")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "retained PTY survived daemon stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn owned_daemon_is_reaped_and_project_files_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("fake-daemon");
        std::fs::write(&binary, "#!/bin/sh\nwhile :; do /bin/sleep 1; done\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let data = directory.path().join("keep.txt");
        std::fs::write(&data, "kept").unwrap();
        let mut daemon =
            spawn_fake_daemon(&binary, directory.path(), "private-master").unwrap();
        let state = daemon._state.path().to_owned();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(daemon.try_wait().unwrap().is_none());
        drop(daemon);
        assert!(!state.exists());
        assert_eq!(std::fs::read_to_string(&data).unwrap(), "kept");
    }
}
