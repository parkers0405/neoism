//! Embedded-daemon mode (G8).
//!
//! When the desktop binary launches with no `--daemon-url` and no daemon
//! is already listening on the default unix socket, we spawn a daemon
//! in-process: a `WorkspaceManager` + the axum router from
//! `neoism_workspace_daemon::server`, served over a unix socket inside
//! a dedicated tokio runtime thread. The (future) `DaemonClient` then
//! connects to that socket exactly the same way it would connect to an
//! external daemon, so local-only users get a zero-configuration boot.
//!
//! Socket path: `$NEOISM_DAEMON_SOCKET` when set, then
//! `$XDG_RUNTIME_DIR/neoism.sock` when `XDG_RUNTIME_DIR` is set (the
//! systemd-managed per-user runtime directory on most Linuxes), and
//! `/tmp/neoism-$UID.sock` otherwise. We stay on a unix socket, not TCP,
//! so the daemon has no network listener at all in local mode and the
//! file's owner/permissions act as the authz gate.
//!
//! Lifecycle:
//! * `EmbeddedDaemonHandle::spawn()` creates the socket, builds the
//!   axum router, spawns a single-thread tokio runtime on a worker
//!   thread, and starts an accept loop that hands each `UnixStream`
//!   off to `hyper-util`'s `auto::Builder` driving the axum router.
//! * Drop aborts the accept-loop task (via `CancellationToken`-style
//!   `Notify`) and unlinks the socket file.

#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::IpAddr;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperServerBuilder;
use tokio::net::UnixListener;
use tokio::sync::Notify;
use tower_service::Service;

use neoism_workspace_daemon::{auth, handshake, server, workspace::WorkspaceManager};

const EMBEDDED_DAEMON_TOKEN_ENV: &str = "NEOISM_DAEMON_TOKEN";
const EMBEDDED_DAEMON_SOCKET_ENV: &str = "NEOISM_DAEMON_SOCKET";

/// Handle to an embedded daemon. The daemon runs until this handle is
/// dropped (or the process exits). Dropping unlinks the socket and
/// signals the accept loop to stop.
pub struct EmbeddedDaemonHandle {
    socket_path: PathBuf,
    shutdown: Arc<Notify>,
    // We keep the runtime alive on a dedicated worker thread; dropping
    // the JoinHandle here is fine. When the shutdown notify fires the
    // runtime's accept loop returns, the runtime is dropped on that
    // thread, and the thread exits cleanly. We hold the JoinHandle in
    // an Option only so Drop can take it.
    runtime_thread: Option<std::thread::JoinHandle<()>>,
}

impl EmbeddedDaemonHandle {
    /// Spawn the embedded daemon. Binds a unix socket at the per-user
    /// default path (or returns an error if the bind fails; caller
    /// should fall back to a different socket path or report the
    /// failure clearly).
    pub fn spawn() -> io::Result<Self> {
        let socket_path = default_socket_path();
        Self::spawn_at(socket_path)
    }

    /// Spawn the embedded daemon bound to a specific socket path. This
    /// is mostly useful for tests; production code uses `spawn()`.
    pub fn spawn_at(socket_path: PathBuf) -> io::Result<Self> {
        ensure_embedded_daemon_token();
        // The embedded daemon runs IN-PROCESS and is only reachable over this
        // trusted per-user unix socket; its sole client is this desktop, which
        // connects without a pairing token. A stray `NEOISM_REQUIRE_AUTH=1`
        // (left in the shell from running the standalone daemon / `scripts/
        // neoism-daemon.sh`, or inherited from a parent) would make the
        // embedded daemon reject the desktop's own token-less `Hello` and then
        // silently drop every editor + workspace message — which surfaces as a
        // BLANK nvim editor and an empty workspace picker with no error.
        // `NEOISM_REQUIRE_AUTH` is meant for the network-exposed standalone
        // daemon (Tailscale), never this in-process one. Remote daemons enforce
        // their own auth in their own process, so clearing it here for the
        // desktop process is safe.
        std::env::remove_var("NEOISM_REQUIRE_AUTH");
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A stale socket file from a previous (crashed) run will make
        // `bind()` fail with `EADDRINUSE` even though nobody's
        // listening. Probe it first; if the connect fails the path is
        // dead and safe to remove.
        if socket_path.exists() {
            let metadata = std::fs::symlink_metadata(&socket_path)?;
            if !metadata.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!(
                        "embedded daemon socket path exists but is not a socket: {}",
                        socket_path.display()
                    ),
                ));
            }

            match std::os::unix::net::UnixStream::connect(&socket_path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!(
                            "another neoism process is already listening on {}",
                            socket_path.display()
                        ),
                    ));
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) =>
                {
                    std::fs::remove_file(&socket_path)?;
                }
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!(
                            "embedded daemon socket path exists but is not usable: {}: {error}",
                            socket_path.display()
                        ),
                    ));
                }
            }
        }

        let shutdown = Arc::new(Notify::new());
        let shutdown_for_task = Arc::clone(&shutdown);
        let socket_path_for_task = socket_path.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<io::Result<()>>(1);

        let runtime_thread = std::thread::Builder::new()
            .name("neoism-embedded-daemon".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .thread_name("neoism-embedded-daemon-rt")
                    .worker_threads(2)
                    .build()
                {
                    Ok(rt) => rt,
                    Err(error) => {
                        let _ = ready_tx.send(Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!(
                                "embedded daemon: failed to build tokio runtime: {error}"
                            ),
                        )));
                        return;
                    }
                };

                runtime.block_on(async move {
                    let listener = match UnixListener::bind(&socket_path_for_task) {
                        Ok(listener) => listener,
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                            return;
                        }
                    };

                    // COLD-START GATE ends HERE. The moment the unix socket
                    // is bound, signal readiness so the desktop's main
                    // thread unblocks (`resolve_daemon` in main.rs) and goes
                    // straight on to create + paint its window. Everything
                    // below — auth/workspace bootstrap, router build, the
                    // `tailscale ip -4` subprocess, and the TCP 7878 binds —
                    // now runs concurrently on THIS daemon thread instead of
                    // on the launch critical path. A bound socket is all the
                    // caller needs to avoid racing a second embedded daemon
                    // (the sole reason this readiness handshake exists);
                    // connections opened before the serve loop spins up just
                    // queue in the listener backlog (sub-millisecond).
                    let _ = ready_tx.send(Ok(()));

                    // Build the same AppState the binary's main()
                    // constructs. Auth + pairing degrade to in-memory
                    // for the embedded case. Local-mode users don't
                    // have an authn story yet, and we shouldn't be
                    // dropping pairing-tokens files onto disk silently.
                    let data_dir = auth::data_dir();
                    let auth_service = match auth::AuthService::bootstrap(&data_dir) {
                        Ok(service) => service,
                        Err(error) => {
                            // Readiness was already signalled, so we can no
                            // longer surface this to the caller. Log and let
                            // the daemon thread exit; the desktop just runs
                            // without daemon-backed features (same outcome as
                            // a spawn failure in `resolve_daemon`).
                            tracing::error!(
                                %error,
                                "embedded daemon: auth bootstrap failed; daemon unavailable"
                            );
                            return;
                        }
                    };
                    let workspaces = WorkspaceManager::bootstrap();
                    let pairing_tokens = handshake::PairingTokenStore::in_memory();
                    // Paired hosts DO persist (unlike pairing tokens):
                    // a cross-host pairing the user set up from the
                    // desktop must survive a restart or `promote` is
                    // back to manual URL+token plumbing. Degrades to
                    // memory-only when the data dir is unwritable.
                    let paired_hosts =
                        neoism_workspace_daemon::hosts::PairedHostStore::load(&data_dir)
                            .unwrap_or_else(|error| {
                                tracing::warn!(
                                    %error,
                                    "embedded daemon: paired-hosts load failed; using in-memory store",
                                );
                                neoism_workspace_daemon::hosts::PairedHostStore::in_memory()
                            });
                    let app = server::router(server::AppState {
                        auth: auth_service,
                        sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
                        workspaces,
                        pairing_tokens,
                        crdt: neoism_workspace_daemon::crdt::sync::CrdtSyncHub::default(),
                        paired_hosts,
                    });

                    tracing::info!(
                        socket = %socket_path_for_task.display(),
                        "embedded daemon listening on unix socket",
                    );

                    // ONE BRAIN: also serve the web's default TCP
                    // endpoint (loopback), best-effort. Without this, a
                    // desktop-first boot owned the unix socket while the
                    // browser's ws://127.0.0.1:7878 had nothing to dial — a
                    // SPLIT BRAIN where presence, carets, and text
                    // "mysteriously" never crossed between web and desktop.
                    //
                    // Share/pairing also needs the desktop-owned daemon to be
                    // reachable from the tailnet. Bind the machine's Tailscale
                    // IPv4 when present instead of requiring the user to stop
                    // desktop and launch the standalone daemon manually.
                    let mut tcp_listeners = Vec::new();
                    let mut bound_addrs = Vec::new();
                    for (label, addr) in embedded_tcp_bind_addrs() {
                        match tokio::net::TcpListener::bind((addr, 7878)).await {
                            Ok(listener) => {
                                tracing::info!(
                                    bind = %addr,
                                    kind = label,
                                    "embedded daemon also listening on TCP 7878"
                                );
                                tcp_listeners.push(listener);
                                bound_addrs.push(addr);
                            }
                            Err(error) => {
                                tracing::info!(
                                    %error,
                                    bind = %addr,
                                    kind = label,
                                    "embedded daemon: TCP 7878 bind unavailable"
                                );
                            }
                        }
                    }

                    async fn accept_tcp(
                        listeners: &[tokio::net::TcpListener],
                    ) -> io::Result<tokio::net::TcpStream> {
                        if listeners.is_empty() {
                            return std::future::pending().await;
                        }
                        let mut accepts = listeners
                            .iter()
                            .map(|listener| listener.accept())
                            .collect::<futures::stream::FuturesUnordered<_>>();
                        accepts
                            .next()
                            .await
                            .expect("tcp accept future set is non-empty")
                            .map(|(stream, _)| stream)
                    }

                    // Serve one accepted stream (unix or TCP — both are
                    // tokio AsyncRead+AsyncWrite) through the shared
                    // router. axum::Router implements
                    // tower::Service<Request<Incoming>>; hyper-util's
                    // auto-builder wants a hyper Service, hence the shim.
                    fn spawn_connection<S>(app: neoism_workspace_daemon::server::AppRouter, stream: S)
                    where
                        S: tokio::io::AsyncRead
                            + tokio::io::AsyncWrite
                            + Unpin
                            + Send
                            + 'static,
                    {
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);
                            let hyper_service = hyper::service::service_fn(
                                move |req: hyper::Request<Incoming>| {
                                    let mut svc = app.clone();
                                    async move { svc.call(req).await }
                                },
                            );
                            if let Err(error) = HyperServerBuilder::new(TokioExecutor::new())
                                .serve_connection_with_upgrades(io, hyper_service)
                                .await
                            {
                                tracing::debug!(%error, "embedded daemon connection ended with error");
                            }
                        });
                    }

                    // LATE TAILNET BIND: the loop above only sees the
                    // tailscale IP that existed at daemon startup. When
                    // tailscaled comes up (or logs in) AFTER neoism, the
                    // daemon used to stay unreachable from the tailnet
                    // until a full app restart — peers could be
                    // discovered but never joined. Watch the cached
                    // probe and bind the IP the moment it appears; each
                    // late listener gets its own accept loop feeding the
                    // same router. Tasks die with this runtime on
                    // shutdown.
                    {
                        let app = app.clone();
                        tokio::spawn(async move {
                            let mut bound = bound_addrs;
                            loop {
                                tokio::time::sleep(std::time::Duration::from_secs(
                                    60,
                                ))
                                .await;
                                let Some(ip) = crate::tailscale::cached_ipv4()
                                else {
                                    continue;
                                };
                                if bound.contains(&ip) {
                                    continue;
                                }
                                match tokio::net::TcpListener::bind((ip, 7878)).await
                                {
                                    Ok(listener) => {
                                        tracing::info!(
                                            bind = %ip,
                                            kind = "tailscale",
                                            "embedded daemon: late tailnet bind on TCP 7878"
                                        );
                                        bound.push(ip);
                                        let app = app.clone();
                                        tokio::spawn(async move {
                                            loop {
                                                match listener.accept().await {
                                                    Ok((stream, _addr)) => {
                                                        spawn_connection(
                                                            app.clone(),
                                                            stream,
                                                        );
                                                    }
                                                    Err(error) => {
                                                        tracing::warn!(
                                                            %error,
                                                            "embedded daemon late-bind accept failed"
                                                        );
                                                        break;
                                                    }
                                                }
                                            }
                                        });
                                    }
                                    Err(error) => {
                                        tracing::debug!(
                                            %error,
                                            bind = %ip,
                                            "embedded daemon: late tailnet bind unavailable"
                                        );
                                    }
                                }
                            }
                        });
                    }

                    let shutdown = shutdown_for_task;
                    loop {
                        tokio::select! {
                            _ = shutdown.notified() => {
                                tracing::debug!("embedded daemon: shutdown signal received");
                                break;
                            }
                            accept = listener.accept() => {
                                match accept {
                                    Ok((stream, _addr)) => spawn_connection(app.clone(), stream),
                                    Err(error) => {
                                        tracing::warn!(%error, "embedded daemon accept failed");
                                    }
                                }
                            }
                            accept = accept_tcp(&tcp_listeners) => {
                                match accept {
                                    Ok(stream) => spawn_connection(app.clone(), stream),
                                    Err(error) => {
                                        tracing::warn!(%error, "embedded daemon tcp accept failed");
                                    }
                                }
                            }
                        }
                    }
                });
            })
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("embedded daemon: failed to spawn runtime thread: {error}"),
                )
            })?;

        // Wait for the runtime thread to either bind the socket
        // successfully or report a startup failure. Without this, the
        // caller could probe before the socket exists and decide to
        // spawn a *second* embedded daemon, racing into EADDRINUSE.
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                socket_path,
                shutdown,
                runtime_thread: Some(runtime_thread),
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "embedded daemon: runtime thread exited before reporting readiness",
            )),
        }
    }

    /// Path to the unix socket the embedded daemon is listening on.
    /// The (future) `DaemonClient` connects here.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

fn embedded_tcp_bind_addrs() -> Vec<(&'static str, IpAddr)> {
    let mut addrs = vec![("loopback", IpAddr::from([127, 0, 0, 1]))];
    // Blocking is fine here (daemon startup, off the UI hot path) and
    // primes the process-wide cache so the workspace-sync path never
    // shells out to the ~2s-when-down tailscale CLI itself.
    if let Some(tailscale_ip) = crate::tailscale::blocking_ipv4() {
        if !addrs.iter().any(|(_, addr)| *addr == tailscale_ip) {
            addrs.push(("tailscale", tailscale_ip));
        }
    }
    addrs
}

impl Drop for EmbeddedDaemonHandle {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        if let Some(handle) = self.runtime_thread.take() {
            // BOUNDED wind-down. A plain `handle.join()` here had NO
            // timeout despite the old comment claiming otherwise, so
            // "Cmd+W quit" blocked until the daemon's tokio runtime fully
            // drained — and after an editor/LSP/index session that drain
            // takes noticeably longer (the "slow only after nvim opened"
            // quit). Move the blocking join onto a detached helper and
            // wait on a channel with a short bound; if the runtime doesn't
            // settle in time, return anyway and let the OS reap the thread
            // at process exit. Per-delta buffer writes are already
            // persisted before close (daemon is the single writer on each
            // edit, not on shutdown), so cutting the wind-down short is
            // safe for durability.
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
            let _ = done_rx.recv_timeout(std::time::Duration::from_millis(150));
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

pub(crate) fn ensure_embedded_daemon_token() {
    if std::env::var_os(EMBEDDED_DAEMON_TOKEN_ENV).is_some() {
        return;
    }

    let token = match load_or_create_embedded_daemon_token() {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "could not load embedded daemon token file; falling back to process-local token",
            );
            generate_embedded_daemon_token()
        }
    };
    std::env::set_var(EMBEDDED_DAEMON_TOKEN_ENV, token);
}

fn load_or_create_embedded_daemon_token() -> io::Result<String> {
    let path = embedded_daemon_token_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }

    match fs::read_to_string(&path) {
        Ok(token) => {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let token = generate_embedded_daemon_token();
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            file.write_all(token.as_bytes())?;
            file.write_all(b"\n")?;
            Ok(token)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read_to_string(&path)?;
            let existing = existing.trim().to_string();
            if existing.is_empty() {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("embedded daemon token file is empty: {}", path.display()),
                ))
            } else {
                Ok(existing)
            }
        }
        Err(error) => Err(error),
    }
}

fn generate_embedded_daemon_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn embedded_daemon_token_path() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime);
        if !path.as_os_str().is_empty() {
            return path.join("neoism").join("daemon-token");
        }
    }

    let uid = unsafe { libc::geteuid() };
    std::env::temp_dir()
        .join(format!("neoism-{uid}"))
        .join("daemon-token")
}

/// Default per-user unix socket path. Allows `$NEOISM_DAEMON_SOCKET` for
/// isolated local dev instances without overriding `$XDG_RUNTIME_DIR` (Wayland
/// needs the real compositor runtime dir), then prefers `$XDG_RUNTIME_DIR` and
/// falls back to `/tmp/neoism-$UID.sock` otherwise.
pub fn default_socket_path() -> PathBuf {
    if let Some(socket) = std::env::var_os(EMBEDDED_DAEMON_SOCKET_ENV) {
        let path = PathBuf::from(socket);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime);
        if !path.as_os_str().is_empty() {
            return path.join("neoism.sock");
        }
    }
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/tmp/neoism-{uid}.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock poisoned")
    }

    /// Quick connect probe matching `main::probe_daemon_socket`. Lives
    /// in tests-only so we don't grow a second public copy.
    fn probe(path: &Path) -> bool {
        StdUnixStream::connect(path).is_ok()
    }

    /// Compatible probe that returns false if the path can't even be
    /// turned into a SocketAddr (i.e. doesn't exist as a file).
    fn try_probe(path: &Path) -> bool {
        if !path.exists() {
            return false;
        }
        probe(path)
    }

    #[test]
    fn spawn_then_drop_unlinks_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("neoism-test.sock");
        assert!(!try_probe(&socket_path), "socket should not exist yet");

        let handle = EmbeddedDaemonHandle::spawn_at(socket_path.clone()).expect("spawn");
        assert_eq!(handle.socket_path(), socket_path.as_path());
        assert!(socket_path.exists(), "socket file should exist after spawn");
        assert!(try_probe(&socket_path), "probe should succeed after spawn");

        drop(handle);

        // After drop the socket file should be unlinked, and a probe
        // should fail. Give the runtime a beat to actually exit before
        // we assert. Drop returns once `join()` completes, so this is
        // usually instant, but the kernel-level unlink can race with a
        // concurrent stat() on slow CI runners.
        for _ in 0..20 {
            if !socket_path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !socket_path.exists(),
            "socket file should be unlinked after drop"
        );
        assert!(!try_probe(&socket_path), "probe should fail after drop");
    }

    #[test]
    fn default_socket_path_uses_xdg_runtime_dir_when_set() {
        let _guard = env_lock();
        // Save + restore so we don't leak env state to other tests in
        // the same binary.
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/test");
        let path = default_socket_path();
        match prev {
            Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
        assert_eq!(path, PathBuf::from("/run/user/test/neoism.sock"));
    }

    #[test]
    fn default_socket_path_falls_back_to_tmp() {
        let _guard = env_lock();
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::remove_var("XDG_RUNTIME_DIR");
        let path = default_socket_path();
        match prev {
            Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
        let s = path.to_string_lossy();
        assert!(
            s.starts_with("/tmp/neoism-") && s.ends_with(".sock"),
            "unexpected fallback socket path: {s}"
        );
    }

    /// End-to-end reproduction of the DESKTOP editor path: spawn the
    /// in-process embedded daemon over a unix socket, connect a real
    /// `DaemonClient` to it (exactly as the live app does), then send
    /// `Resize` + `OpenBuffer` for a real file via `send_editor` and
    /// assert the file's text streams back as an editor redraw.
    ///
    /// The daemon-side path is already proven by the workspace-daemon
    /// integration test; this isolates the CLIENT/connection layer the
    /// app actually uses (unix socket + `DaemonClient` + `into_channels`).
    /// A blank editor in the app with this test PASSING means the bug is
    /// above the connection (routing into the pane); FAILING means it's
    /// the connection itself.
    #[test]
    fn desktop_daemonclient_streams_file_redraw_over_unix() {
        if std::process::Command::new("nvim")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("nvim not installed; skipping");
            return;
        }
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                "neoism=info,neoism_workspace_daemon=info,neoism_backend=info",
            )
            .with_test_writer()
            .try_init();
        let _guard = env_lock();
        std::env::remove_var("NEOISM_REQUIRE_AUTH");

        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("daemon.sock");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let file = work.join("hello.rs");
        std::fs::write(
            &file,
            "LINEALPHA one\nMARKERNVIMREPRO two\nLINEDELTA three\n",
        )
        .unwrap();

        let _daemon =
            EmbeddedDaemonHandle::spawn_at(socket_path.clone()).expect("spawn daemon");

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let saw_marker = rt.block_on(async {
            for _ in 0..200 {
                if socket_path.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let endpoint = format!("unix://{}", socket_path.display());
            let client = crate::daemon_client::DaemonClient::connect(endpoint)
                .await
                .expect("DaemonClient connect");
            let (handle, mut rx, _status) = client.into_channels();

            let surface = Some("s1".to_string());
            handle
                .send_editor(neoism_protocol::editor::EditorClientMessage::Resize {
                    width: 80,
                    height: 24,
                    surface_id: surface.clone(),
                })
                .await
                .expect("send resize");
            handle
                .send_editor_with_workspace_root(
                    neoism_protocol::editor::EditorClientMessage::OpenBuffer {
                        path: file.clone(),
                        line: None,
                        character: None,
                        surface_id: surface.clone(),
                    },
                    Some(work.clone()),
                )
                .await
                .expect("send open");

            let mut grid_text = String::new();
            let mut editor_msgs = 0usize;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                    Ok(Some(crate::daemon_client::DaemonServerMessage::Editor {
                        message,
                        ..
                    })) => {
                        editor_msgs += 1;
                        let s = serde_json::to_string(&message).unwrap_or_default();
                        let mut idx = 0;
                        while let Some(pos) = s[idx..].find("\"ch\":\"") {
                            let start = idx + pos + 6;
                            if let Some(end) = s[start..].find('"') {
                                grid_text.push_str(&s[start..start + end]);
                                idx = start + end;
                            } else {
                                break;
                            }
                        }
                        if grid_text.contains("MARKERNVIMREPRO") {
                            break;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {}
                }
            }
            eprintln!(
                "REPRO(client): editor_msgs={editor_msgs} grid_text_len={} saw_marker={}",
                grid_text.len(),
                grid_text.contains("MARKERNVIMREPRO")
            );
            grid_text.contains("MARKERNVIMREPRO")
        });

        assert!(
            saw_marker,
            "DESKTOP DaemonClient path: the file's text never streamed back over the unix daemon connection — the bug is in the client/connection layer"
        );
    }
}
