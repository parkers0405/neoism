//! Neoism workspace daemon binary entrypoint.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use neoism_workspace_daemon::{
    auth, handshake, nvim::NvimSessionRegistry, persistence, server,
    workspace::WorkspaceManager,
};

#[derive(Debug, Parser)]
#[command(name = "neoism-workspace-daemon")]
#[command(about = "Neoism workspace daemon")]
struct Cli {
    /// Address for the daemon HTTP/WebSocket server.
    #[arg(long, env = "NEOISM_DAEMON_ADDR", default_value = "127.0.0.1:7878")]
    addr: SocketAddr,
    /// Detach and continue running in the background.
    #[arg(long)]
    background: bool,
    /// Write the daemon process id to this file and remove it on shutdown.
    #[arg(long)]
    pidfile: Option<PathBuf>,
    /// Skip snapshot load/save entirely.
    #[arg(long)]
    ephemeral: bool,
    /// Directory for daemon state.json snapshots.
    #[arg(long, value_name = "DIR")]
    state_dir: Option<PathBuf>,
    /// Also serve on this unix socket. Defaults to the desktop's
    /// attach path (`$XDG_RUNTIME_DIR/neoism.sock`) so a plain
    /// `neoism` launch probes and joins THIS daemon instead of
    /// spawning its own embedded one — otherwise desktop and web end
    /// up in two separate workspace trees that never see each other.
    #[arg(long, value_name = "PATH", env = "NEOISM_DAEMON_SOCKET")]
    unix_socket: Option<PathBuf>,
    /// Disable the unix-socket listener (TCP only).
    #[arg(long)]
    no_unix_socket: bool,
    /// Declare this directory as a workspace at startup (repeatable).
    /// The headless-host primitive: `neoism-workspace-daemon --workspace
    /// /work/repo` serves that dir as a joinable workspace with no client
    /// having to create it first. Idempotent — a dir already declared by
    /// a previous run (restored from the state snapshot) is not duplicated.
    #[arg(long, value_name = "DIR")]
    workspace: Vec<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if cli.background {
        daemonize_background()?;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,neoism_workspace_daemon=debug")),
        )
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    // Phase 0: make sure the daemon has a `NEOISM_DAEMON_TOKEN` before
    // anything binds. We log only the path, never the token value.
    let token_path = neoism_workspace_daemon::daemon_token::ensure_daemon_token();
    tracing::info!(
        token_path = %token_path.display(),
        require_auth = handshake::require_auth_enabled(),
        "daemon token ready (read the secret from this file; never logged)",
    );

    let _pidfile_guard = match cli.pidfile {
        Some(path) => {
            let guard = persistence::PidfileGuard::create(path)?;
            tracing::info!(path = %guard.path().display(), "pidfile written");
            Some(guard)
        }
        None => None,
    };

    let state_dir = persistence::resolve_state_dir(cli.state_dir.as_deref());
    let data_dir = auth::data_dir();
    tracing::info!(?data_dir, "loading auth state");
    let auth_service = auth::AuthService::bootstrap(&data_dir)?;
    let workspaces =
        WorkspaceManager::bootstrap_with_state_dir(state_dir.clone(), cli.ephemeral);
    tracing::info!(
        state_dir = %state_dir.display(),
        ephemeral = cli.ephemeral,
        persistent = workspaces.snapshot_writer().is_persistent(),
        "workspace persistence ready",
    );
    // Pairing-token store for the `Hello` handshake. Persists under
    // `$XDG_CONFIG_HOME/neoism/pairing-tokens` so operators can
    // re-pair a phone/web client across daemon restarts without
    // re-minting. Load failures degrade to an in-memory store — the
    // env-gate (`NEOISM_REQUIRE_AUTH=1`) is opt-in, so a broken
    // pairing-tokens file shouldn't lock the daemon shut.
    let pairing_config_dir = handshake::config_dir();
    let pairing_tokens = handshake::PairingTokenStore::load(&pairing_config_dir)
        .unwrap_or_else(|err| {
            tracing::warn!(
                error = %err,
                path = %pairing_config_dir.display(),
                "could not load pairing-tokens file; falling back to in-memory store",
            );
            handshake::PairingTokenStore::in_memory()
        });
    tracing::info!(
        token_count = pairing_tokens.len(),
        require_auth = handshake::require_auth_enabled(),
        "pairing-token store ready",
    );
    // Paired-host store for cross-host workspace promote. A load
    // failure degrades to memory-only — pairing still works for the
    // daemon's lifetime, it just won't survive a restart.
    let paired_hosts = neoism_workspace_daemon::hosts::PairedHostStore::load(&data_dir)
        .unwrap_or_else(|err| {
            tracing::warn!(
                error = %err,
                "could not load paired-hosts file; falling back to in-memory store",
            );
            neoism_workspace_daemon::hosts::PairedHostStore::in_memory()
        });
    for dir in &cli.workspace {
        let workspace = workspaces.declare_startup_workspace(dir);
        tracing::info!(
            workspace_id = %workspace.id,
            root = ?workspace.root_dir,
            title = %workspace.title,
            "startup workspace ready",
        );
    }
    // GC any clipboard image temp files older than the 24h TTL before
    // we start serving (and trim the LRU cap as a side effect). The
    // opportunistic per-paste sweep keeps the cap enforced afterwards.
    neoism_workspace_daemon::workspace::sweep_clipboard_dir_on_startup();
    let app = server::router(server::AppState {
        auth: auth_service,
        sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
        workspaces: workspaces.clone(),
        pairing_tokens,
        nvim_sessions: NvimSessionRegistry::new(),
        crdt: neoism_workspace_daemon::crdt::sync::CrdtSyncHub::default(),
        paired_hosts,
    });

    tracing::info!(addr = %cli.addr, "neoism-workspace-daemon listening");

    // Serve the same router on the desktop's default unix socket so a
    // plain `neoism` launch (which probes that socket before spawning
    // an embedded daemon) lands on THIS daemon and shares one
    // workspace tree with web/TCP clients.
    #[cfg(unix)]
    let unix_socket_guard = if cli.no_unix_socket {
        None
    } else {
        let path = cli
            .unix_socket
            .clone()
            .unwrap_or_else(default_unix_socket_path);
        serve_unix_socket(app.clone(), path).await
    };

    let shutdown_writer = workspaces.snapshot_writer().clone();
    let listener = tokio::net::TcpListener::bind(cli.addr).await?;
    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    );
    // NO graceful shutdown: axum's graceful path waits for every open
    // connection — and this daemon holds long-lived client websockets
    // that never close on their own. A SIGTERM'd daemon then became a
    // drain-zombie (TCP listener released, process alive forever, unix
    // socket held) that looked exactly like a sync bug to clients. On
    // signal we persist the snapshot (inside `shutdown_signal`) and
    // exit NOW; clients see a clean disconnect and reconnect.
    tokio::select! {
        result = serve => { result?; }
        _ = shutdown_signal(shutdown_writer) => {
            tracing::info!(
                "shutdown: snapshot persisted; aborting open connections and exiting"
            );
        }
    }

    workspaces.snapshot_writer().shutdown().await;
    #[cfg(unix)]
    if let Some(path) = unix_socket_guard {
        let _ = std::fs::remove_file(&path);
    }

    Ok(())
}

/// The desktop's default daemon socket — MUST mirror
/// `neoism::embedded_daemon::default_socket_path` so the desktop's
/// no-flags probe finds us.
#[cfg(unix)]
fn default_unix_socket_path() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime);
        if !path.as_os_str().is_empty() {
            return path.join("neoism.sock");
        }
    }
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/tmp/neoism-{uid}.sock"))
}

/// Bind `path` and serve `app` over it with hyper-util's auto builder
/// (the same accept loop the desktop's embedded daemon runs — axum's
/// `serve` insists on `ConnectInfo<SocketAddr>`, which unix streams
/// can't provide; handlers already take `Option<ConnectInfo>` for this
/// reason). Returns the bound path (for cleanup) or `None` when
/// another live daemon already owns the socket / the bind fails —
/// both degrade to TCP-only with a warning rather than aborting.
#[cfg(unix)]
async fn serve_unix_socket(app: axum::Router, path: PathBuf) -> Option<PathBuf> {
    use hyper::body::Incoming;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder as HyperServerBuilder;
    use tower::Service as _;

    if path.exists() {
        // A live daemon (e.g. a desktop-embedded one) may own this
        // socket — probe before stealing it. Only unlink stale files.
        match tokio::net::UnixStream::connect(&path).await {
            Ok(_) => {
                tracing::warn!(
                    socket = %path.display(),
                    "another daemon is live on the default socket; skipping unix bind (TCP only). \
                     Stop it (or the desktop that spawned it) and restart this daemon to unify.",
                );
                return None;
            }
            Err(_) => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(
                %error,
                socket = %path.display(),
                "could not bind unix socket; continuing TCP only",
            );
            return None;
        }
    };
    tracing::info!(socket = %path.display(), "neoism-workspace-daemon listening on unix socket");

    tokio::spawn(async move {
        loop {
            let (stream, _addr) = match listener.accept().await {
                Ok(pair) => pair,
                Err(error) => {
                    tracing::warn!(%error, "unix socket accept failed");
                    continue;
                }
            };
            let tower_service = app.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let hyper_service =
                    hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                        let mut svc = tower_service.clone();
                        async move { svc.call(req).await }
                    });
                if let Err(error) = HyperServerBuilder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(io, hyper_service)
                    .await
                {
                    tracing::debug!(%error, "unix socket connection ended with error");
                }
            });
        }
    });

    Some(path)
}

async fn shutdown_signal(writer: persistence::SnapshotWriter) {
    #[cfg(unix)]
    {
        let mut sigterm = match tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        ) {
            Ok(sigterm) => sigterm,
            Err(err) => {
                tracing::warn!(error = %err, "failed to install SIGTERM handler");
                let _ = tokio::signal::ctrl_c().await;
                writer.shutdown().await;
                return;
            }
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received Ctrl-C; shutting down");
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM; shutting down");
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("received Ctrl-C; shutting down");
    }

    writer.shutdown().await;
}

#[cfg(unix)]
fn daemonize_background() -> std::io::Result<()> {
    unsafe {
        fork_parent_exits()?;
        if libc::setsid() < 0 {
            return Err(std::io::Error::last_os_error());
        }
        fork_parent_exits()?;
        redirect_stdio_to_devnull()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn daemonize_background() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "--background is only supported on Unix",
    ))
}

#[cfg(unix)]
unsafe fn fork_parent_exits() -> std::io::Result<()> {
    match libc::fork() {
        -1 => Err(std::io::Error::last_os_error()),
        0 => Ok(()),
        _pid => std::process::exit(0),
    }
}

#[cfg(unix)]
unsafe fn redirect_stdio_to_devnull() -> std::io::Result<()> {
    let path = std::ffi::CString::new("/dev/null").expect("literal has no nul");
    let fd = libc::open(path.as_ptr(), libc::O_RDWR);
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        if libc::dup2(fd, target) < 0 {
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }
    }
    if fd > libc::STDERR_FILENO {
        libc::close(fd);
    }
    Ok(())
}
