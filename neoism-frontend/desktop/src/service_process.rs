//! One desktop-owned workspace service, monitored off the UI thread. The stdin
//! pipe is a parent-death leash; stdout is a versioned startup protocol, never a
//! log stream. A bound port alone is neither readiness nor daemon identity.
use std::io::{self, BufRead, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

const DAEMON_FLAG: &str = "--neoism-internal-workspace-daemon";
const READY: &str = "NEOISM_WORKSPACE_SERVICE/1 READY";
// Not the GUI's patience budget. Cold hosted association/bootstrap can take
// 96 seconds; do not kill a healthy boot simply because first-frame proceeds.
const BOOT_TIMEOUT: Duration = Duration::from_secs(120);
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub(crate) struct ServiceProcess(Arc<Owner>);
struct Owner {
    stop: mpsc::Sender<()>,
    state: Arc<Mutex<ServiceState>>,
}
#[derive(Clone, Copy)]
enum ServiceState {
    Starting,
    Ready,
    Failed(&'static str),
}
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

impl ServiceProcess {
    /// Startup-only, bounded. Timing out leaves the owner and its retries alive.
    pub(crate) fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            match *self.0.state.lock().unwrap_or_else(|e| e.into_inner()) {
                ServiceState::Ready => return Ok(()),
                ServiceState::Failed(reason) => return Err(reason.into()),
                ServiceState::Starting => {}
            }
            if Instant::now() >= deadline {
                return Err(
                    "workspace service is still starting in the background".into()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub(crate) fn spawn_daemon() -> io::Result<ServiceProcess> {
    static OWNER: OnceLock<Mutex<Weak<Owner>>> = OnceLock::new();
    let mut slot = OWNER
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(owner) = slot.upgrade() {
        return Ok(ServiceProcess(owner));
    }
    let (stop, stopping) = mpsc::channel();
    let state = Arc::new(Mutex::new(ServiceState::Starting));
    let monitor_state = state.clone();
    std::thread::Builder::new()
        .name("neoism-service-owner".into())
        .spawn(move || monitor(stopping, monitor_state))?;
    let owner = Arc::new(Owner { stop, state });
    *slot = Arc::downgrade(&owner);
    Ok(ServiceProcess(owner))
}

struct RunningChild {
    child: Child,
    startup: mpsc::Receiver<bool>,
    started: Instant,
    acknowledged: bool,
}
impl Drop for RunningChild {
    fn drop(&mut self) {
        // Kill only our own service. Never terminate an unrelated port owner.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn spawn_child() -> io::Result<RunningChild> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(DAEMON_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
        );
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped service stdout");
    let (tx, startup) = mpsc::channel();
    if let Err(error) = std::thread::Builder::new()
        .name("neoism-service-ready".into())
        .spawn(move || {
            let mut line = String::new();
            // Malformed/oversize data and EOF are explicit startup failures.
            let ok = io::BufReader::new(stdout)
                .take(1024)
                .read_line(&mut line)
                .is_ok()
                && line.trim_end() == READY;
            let _ = tx.send(ok);
        })
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(RunningChild {
        child,
        startup,
        started: Instant::now(),
        acknowledged: false,
    })
}

fn set_state(state: &Mutex<ServiceState>, value: ServiceState) {
    *state.lock().unwrap_or_else(|e| e.into_inner()) = value;
}
fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs((1u64 << failures.min(5)).min(30))
}
fn monitor(stop: mpsc::Receiver<()>, state: Arc<Mutex<ServiceState>>) {
    monitor_service(stop, state, probe_default_daemon, spawn_child);
}

fn monitor_service(
    stop: mpsc::Receiver<()>,
    state: Arc<Mutex<ServiceState>>,
    mut probe_daemon: impl FnMut() -> DaemonProbe,
    mut launch: impl FnMut() -> io::Result<RunningChild>,
) {
    let mut child: Option<RunningChild> = None;
    let mut next_attempt = Instant::now();
    let mut failures = 0u32;
    let mut unhealthy_since = None;
    let mut was_ready = false;
    loop {
        match stop.recv_timeout(Duration::from_millis(100)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let mut failed = None;
        if let Some(running) = child.as_mut() {
            match running.child.try_wait() {
                Ok(Some(status)) => {
                    tracing::warn!(
                        ?status,
                        "workspace service child exited; scheduling recovery"
                    );
                    failed = Some("workspace service exited before becoming available");
                }
                Err(_) => failed = Some("could not monitor workspace service"),
                Ok(None) => {}
            }
            if !running.acknowledged && failed.is_none() {
                match running.startup.try_recv() {
                    Ok(true) => running.acknowledged = true,
                    Ok(false) | Err(mpsc::TryRecvError::Disconnected) =>
                        failed = Some("workspace service reported a startup error; see log/workspace-service.log"),
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                if !running.acknowledged && running.started.elapsed() >= BOOT_TIMEOUT {
                    failed =
                        Some("workspace service startup exceeded 120 seconds; retrying");
                }
            }
        }
        if let Some(reason) = failed {
            tracing::warn!(reason, "workspace service unavailable");
            set_state(&state, ServiceState::Failed(reason));
            child = None;
            was_ready = false;
            unhealthy_since = None;
            next_attempt = Instant::now() + retry_delay(failures);
            failures = failures.saturating_add(1);
        }
        let probe = probe_daemon();
        let acknowledged = child.as_ref().is_none_or(|c| c.acknowledged);
        if probe == DaemonProbe::Ready && acknowledged {
            if !was_ready {
                tracing::info!("workspace service ready (validated daemon health)");
            }
            was_ready = true;
            unhealthy_since = None;
            set_state(&state, ServiceState::Ready);
            if child
                .as_ref()
                .is_none_or(|c| c.started.elapsed() > Duration::from_secs(60))
            {
                failures = 0;
            }
        } else {
            if was_ready {
                set_state(
                    &state,
                    ServiceState::Failed("workspace service lost readiness; recovering"),
                );
                was_ready = false;
            }
            if let Some(running) = child.as_ref() {
                if running.acknowledged {
                    let since = unhealthy_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(30) {
                        tracing::warn!("workspace service stopped responding for 30 seconds; restarting owned child");
                        child = None;
                        unhealthy_since = None;
                        next_attempt = Instant::now() + retry_delay(failures);
                        failures = failures.saturating_add(1);
                    }
                }
            } else if Instant::now() >= next_attempt {
                if probe == DaemonProbe::Missing {
                    set_state(&state, ServiceState::Starting);
                    match launch() {
                        Ok(running) => {
                            tracing::info!(
                                pid = running.child.id(),
                                "started workspace service child"
                            );
                            child = Some(running);
                        }
                        Err(error) => {
                            tracing::warn!(kind = ?error.kind(), "could not launch workspace service; retrying");
                            set_state(
                                &state,
                                ServiceState::Failed(
                                    "could not launch workspace service; retrying",
                                ),
                            );
                            next_attempt = Instant::now() + retry_delay(failures);
                            failures = failures.saturating_add(1);
                        }
                    }
                } else {
                    // Occupied is NOT attached/ready. Don't spawn into a port
                    // collision or kill its owner; retry validation quietly.
                    tracing::warn!("local daemon endpoint is occupied but has not returned Neoism daemon health; waiting");
                    set_state(
                        &state,
                        ServiceState::Failed(
                            "local endpoint is occupied but not a ready Neoism daemon",
                        ),
                    );
                    next_attempt = Instant::now() + Duration::from_secs(30);
                }
            }
        }
        // Steady-state checks are cheap and off-thread, not a hot polling loop.
        if was_ready {
            match stop.recv_timeout(Duration::from_secs(1)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
enum DaemonProbe {
    Missing,
    NotReady,
    Ready,
}
fn probe_default_daemon() -> DaemonProbe {
    #[cfg(unix)]
    {
        match std::os::unix::net::UnixStream::connect(
            crate::embedded_daemon::default_socket_path(),
        ) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
                let _ = stream.set_write_timeout(Some(PROBE_TIMEOUT));
                if daemon_health(&mut stream) {
                    DaemonProbe::Ready
                } else {
                    DaemonProbe::NotReady
                }
            }
            Err(_) => DaemonProbe::Missing,
        }
    }
    #[cfg(not(unix))]
    {
        probe_tcp(crate::embedded_daemon::default_tcp_port())
    }
}
#[cfg(any(not(unix), test))]
fn probe_tcp(port: u16) -> DaemonProbe {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    match std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
        Ok(mut stream) => {
            let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
            let _ = stream.set_write_timeout(Some(PROBE_TIMEOUT));
            if daemon_health(&mut stream) {
                DaemonProbe::Ready
            } else {
                DaemonProbe::NotReady
            }
        }
        Err(error) => {
            // Windows can exhaust a short connect deadline before reporting a
            // refused loopback connection. Only an established connection can
            // prove the endpoint is occupied; the child's bind remains final.
            tracing::debug!(kind = ?error.kind(), os_error = error.raw_os_error(), "local daemon endpoint could not be reached");
            DaemonProbe::Missing
        }
    }
}
fn daemon_health(stream: &mut (impl Read + Write)) -> bool {
    let deadline = Instant::now() + PROBE_TIMEOUT;
    if stream
        .write_all(
            b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .is_err()
    {
        return false;
    }
    let mut response = Vec::new();
    let mut chunk = [0; 1024];
    while Instant::now() < deadline && response.len() <= 8192 {
        match stream.read(&mut chunk) {
            Ok(0) => return valid_daemon_response(&response),
            Ok(n) => response.extend_from_slice(&chunk[..n]),
            Err(_) => return false,
        }
    }
    false
}
fn valid_daemon_response(response: &[u8]) -> bool {
    let Ok(response) = std::str::from_utf8(response) else {
        return false;
    };
    let Some((headers, body)) = response.split_once("\r\n\r\n") else {
        return false;
    };
    (headers.starts_with("HTTP/1.1 200 ") || headers.starts_with("HTTP/1.0 200 "))
        && body == "neoism-daemon"
}

pub(crate) fn maybe_run_internal_service(
) -> Option<Result<(), Box<dyn std::error::Error>>> {
    if std::env::args().nth(1).as_deref() != Some(DAEMON_FLAG) {
        return None;
    }
    // Dispatch precedes GUI logging. Keep logs separate (no truncation of the
    // desktop log) and stdout reserved exclusively for the readiness protocol.
    init_service_logging();
    exit_when_parent_closes_stdin();
    Some(run_daemon())
}
fn init_service_logging() {
    let dir = neoism_backend::config::config_dir_path().join("log");
    let writer = std::fs::create_dir_all(&dir).and_then(|_| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("workspace-service.log"))
    });
    match writer {
        Ok(file) => {
            let _ = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_max_level(tracing::Level::INFO)
                .with_writer(Mutex::new(file))
                .try_init();
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_max_level(tracing::Level::INFO)
                .with_writer(io::stderr)
                .try_init();
        }
    }
}
fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    let daemon = match crate::embedded_daemon::EmbeddedDaemonHandle::spawn() {
        Ok(daemon) => daemon,
        Err(error) => {
            tracing::error!(kind = ?error.kind(), "workspace service startup failed");
            println!("NEOISM_WORKSPACE_SERVICE/1 ERROR {:?}", error.kind());
            let _ = io::stdout().flush();
            return Err(error.into());
        }
    };
    println!("{READY}");
    io::stdout().flush()?;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if daemon.is_finished() {
            tracing::error!("workspace service runtime exited");
            return Err("workspace service runtime exited".into());
        }
    }
}
fn exit_when_parent_closes_stdin() {
    std::thread::Builder::new()
        .name("neoism-service-parent-watch".into())
        .spawn(|| {
            let mut input = io::stdin().lock();
            let mut buffer = [0_u8; 64];
            loop {
                match input.read(&mut buffer) {
                    Ok(0) | Err(_) => std::process::exit(0),
                    Ok(_) => {}
                }
            }
        })
        .expect("failed to start service parent watcher");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daemon_identity_is_not_http_success() {
        assert!(!valid_daemon_response(b"HTTP/1.1 200 OK\r\n\r\nOK"));
        assert!(!valid_daemon_response(
            b"HTTP/1.1 503 unavailable\r\n\r\nneoism-daemon"
        ));
        assert!(valid_daemon_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nneoism-daemon"
        ));
    }
    #[test]
    fn closed_daemon_port_allows_startup() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert_eq!(probe_tcp(port), DaemonProbe::Missing);
    }

    #[test]
    fn listening_but_not_serving_is_not_ready() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let started = Instant::now();
        assert_eq!(
            probe_tcp(listener.local_addr().unwrap().port()),
            DaemonProbe::NotReady
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[cfg(unix)]
    #[test]
    fn early_child_exit_is_retried_by_the_same_owner() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let launches = Arc::new(AtomicUsize::new(0));
        let count = launches.clone();
        let (stop, stopping) = mpsc::channel();
        let state = Arc::new(Mutex::new(ServiceState::Starting));
        let worker = std::thread::spawn(move || {
            monitor_service(
                stopping,
                state,
                || DaemonProbe::Missing,
                || {
                    count.fetch_add(1, Ordering::SeqCst);
                    let child = Command::new("sh")
                        .args(["-c", "exit 17"])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()?;
                    let (_tx, startup) = mpsc::channel();
                    Ok(RunningChild {
                        child,
                        startup,
                        started: Instant::now(),
                        acknowledged: false,
                    })
                },
            );
        });
        let started = Instant::now();
        while launches.load(Ordering::SeqCst) < 2
            && started.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(launches.load(Ordering::SeqCst), 2);
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn child_ack_is_required_even_when_health_is_ready() {
        let (stop, stopping) = mpsc::channel();
        let state = Arc::new(Mutex::new(ServiceState::Starting));
        let observed = state.clone();
        let worker = std::thread::spawn(move || {
            let mut probes = 0;
            let mut pending_senders = Vec::new();
            monitor_service(
                stopping,
                state,
                || {
                    probes += 1;
                    if probes == 1 {
                        DaemonProbe::Missing
                    } else {
                        DaemonProbe::Ready
                    }
                },
                || {
                    let child = Command::new("sh")
                        .args(["-c", "exec sleep 30"])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()?;
                    let (tx, startup) = mpsc::channel();
                    pending_senders.push(tx);
                    Ok(RunningChild {
                        child,
                        startup,
                        started: Instant::now(),
                        acknowledged: false,
                    })
                },
            );
        });
        let owner = ServiceProcess(Arc::new(Owner {
            stop,
            state: observed,
        }));
        assert!(owner.wait_ready(Duration::from_millis(450)).is_err());
        assert!(matches!(
            *owner.0.state.lock().unwrap(),
            ServiceState::Starting
        ));
        drop(owner);
        worker.join().unwrap();
    }

    #[test]
    fn retry_is_backed_off_and_capped() {
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(4));
        assert_eq!(retry_delay(1000), Duration::from_secs(30));
    }
}
