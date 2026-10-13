//! Shared standalone daemon CLI and process runner (never the desktop embedded service).

use std::ffi::OsString;
use std::io::Write;

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::{auth, handshake, persistence, server, workspace::WorkspaceManager};

#[derive(Debug, Parser)]
#[command(name = "neoism-workspace-daemon", version)]
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
    /// Owned temporary host: terminate descendants when its desktop parent exits.
    #[arg(long)]
    temporary_parent_pid: Option<u32>,
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

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn namespace_help_and_version_do_not_boot() {
        for (flag, kind) in [
            ("--help", clap::error::ErrorKind::DisplayHelp),
            ("--version", clap::error::ErrorKind::DisplayVersion),
        ] {
            let error = parse_from(["neoism", FAST_WORKSPACE_DAEMON_FLAG, flag], None)
                .unwrap_err();
            assert_eq!(error.kind(), kind);
        }
    }

    #[test]
    fn only_the_leading_fast_namespace_is_stripped() {
        let error =
            parse_from(["neoism", "--ephemeral", FAST_WORKSPACE_DAEMON_FLAG], None)
                .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        let error = parse_from(["neoism", "--neoism-internal-workspace-daemon"], None)
            .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    fn fast_args(root: &std::path::Path) -> Vec<OsString> {
        vec![
            "neoism".into(),
            FAST_WORKSPACE_DAEMON_FLAG.into(),
            "--addr".into(),
            "127.0.0.1:0".into(),
            "--no-unix-socket".into(),
            "--ephemeral".into(),
            "--workspace".into(),
            root.as_os_str().to_owned(),
            "--temporary-parent-pid".into(),
            "123".into(),
        ]
    }

    #[test]
    fn fast_parser_preserves_standalone_flags_and_validates_nonce() {
        let root = tempfile::tempdir().unwrap();
        let args = fast_args(root.path());
        let (cli, nonce) = parse_from(args.clone(), Some("request-1".into())).unwrap();
        assert_eq!(cli.addr, "127.0.0.1:0".parse().unwrap());
        assert_eq!(cli.workspace, vec![root.path().to_path_buf()]);
        assert!(cli.ephemeral && cli.no_unix_socket);
        assert_eq!(cli.temporary_parent_pid, Some(123));
        assert_eq!(nonce.as_deref(), Some("request-1"));
        for nonce in [
            None,
            Some(String::new()),
            Some("bad\nnonce".into()),
            Some("x".repeat(257)),
        ] {
            assert!(parse_from(args.clone(), nonce).is_err());
        }
        // Sibling standalone executables use exactly the same acknowledgement.
        let mut sibling = args;
        sibling.remove(1);
        assert!(parse_from(sibling, Some("request-1".into())).is_ok());
    }

    #[test]
    fn fast_parser_rejects_unsafe_or_incomplete_arguments() {
        let root = tempfile::tempdir().unwrap();
        let args = fast_args(root.path());
        for extra in [
            vec!["--background"],
            vec!["--unix-socket", "/tmp/other.sock"],
            vec!["--addr", "0.0.0.0:0"],
            vec!["--temporary-parent-pid", "0"],
            vec!["--unknown-fast-option"],
        ] {
            let mut invalid = args.clone();
            invalid.extend(extra.into_iter().map(OsString::from));
            assert!(parse_from(invalid, Some("request-1".into())).is_err());
        }
        let mut missing_socket_opt = args.clone();
        missing_socket_opt.retain(|arg| arg != "--no-unix-socket");
        assert!(parse_from(missing_socket_opt, Some("request-1".into())).is_err());
        assert!(parse_from(
            ["neoism", FAST_WORKSPACE_DAEMON_FLAG, "--no-unix-socket"],
            Some("request-1".into())
        )
        .is_err());
    }

    #[test]
    fn only_normal_standalone_owns_agent_server() {
        assert!(owns_agent_server(None));
        assert!(!owns_agent_server(Some("fast-startup-nonce")));
    }

    #[test]
    fn normal_cli_does_not_request_readiness() {
        let (cli, nonce) =
            parse_from(["neoism-workspace-daemon", "--ephemeral"], None).unwrap();
        assert!(nonce.is_none());
        assert!(!cli.no_unix_socket);
    }

    #[test]
    fn readiness_is_one_versioned_json_line_and_is_flushed() {
        #[derive(Default)]
        struct Writer {
            bytes: Vec<u8>,
            flushed: bool,
        }
        impl Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushed = true;
                Ok(())
            }
        }
        let acknowledgement = StartupAcknowledgement {
            protocol: FAST_READINESS_PROTOCOL.into(),
            nonce: "request-1".into(),
            address: "127.0.0.1:43210".parse().unwrap(),
        };
        let mut writer = Writer::default();
        write_readiness(&mut writer, &acknowledgement).unwrap();
        assert!(writer.flushed);
        assert_eq!(writer.bytes.last(), Some(&b'\n'));
        assert_eq!(writer.bytes.iter().filter(|&&b| b == b'\n').count(), 1);
        let decoded: StartupAcknowledgement =
            serde_json::from_slice(&writer.bytes).unwrap();
        assert_eq!(decoded, acknowledgement);
        assert_eq!(decoded.protocol, "neoism-fast-ready-v1");
        assert_eq!(
            String::from_utf8(writer.bytes).unwrap(),
            "{\"protocol\":\"neoism-fast-ready-v1\",\"nonce\":\"request-1\",\"address\":\"127.0.0.1:43210\"}\n"
        );
    }

    #[test]
    fn version_is_reported_without_starting_services() {
        let version =
            Cli::try_parse_from(["neoism-workspace-daemon", "--version"]).unwrap_err();
        assert_eq!(version.kind(), clap::error::ErrorKind::DisplayVersion);
        assert_eq!(
            version.to_string().trim(),
            concat!("neoism-workspace-daemon ", env!("CARGO_PKG_VERSION"))
        );
    }
}

pub const FAST_WORKSPACE_DAEMON_FLAG: &str = "--neoism-internal-fast-workspace-daemon";
pub const FAST_READINESS_PROTOCOL: &str = "neoism-fast-ready-v1";
pub const FAST_READINESS_NONCE_ENV: &str = "NEOISM_FAST_READINESS_NONCE";
pub type StandaloneResult = Result<(), Box<dyn std::error::Error>>;

/// stdout's single-line startup acknowledgement. No secrets are included.
#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct StartupAcknowledgement {
    pub protocol: String,
    pub nonce: String,
    pub address: SocketAddr,
}

fn parse_from<I, T>(
    args: I,
    nonce: Option<String>,
) -> Result<(Cli, Option<String>), clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let fast = args
        .get(1)
        .is_some_and(|arg| arg == FAST_WORKSPACE_DAEMON_FLAG);
    if fast {
        args.remove(1);
    }
    // --no-unix-socket also disables an inherited NEOISM_DAEMON_SOCKET.
    // Reject explicit Unix-listener requests, not a harmless ambient default.
    let explicit_unix_socket = args.iter().skip(1).any(|arg| {
        arg == "--unix-socket"
            || arg
                .to_str()
                .is_some_and(|arg| arg.starts_with("--unix-socket="))
    });
    let cli = Cli::try_parse_from(args)?;
    if fast || nonce.is_some() {
        let invalid =
            |message| clap::Error::raw(clap::error::ErrorKind::ArgumentConflict, message);
        if nonce.as_deref().is_none_or(|n| {
            n.is_empty() || n.len() > 256 || n.chars().any(char::is_control)
        }) {
            return Err(invalid("Fast startup requires a nonempty NEOISM_FAST_READINESS_NONCE (maximum 256 characters, no control characters)"));
        }
        if !cli.addr.ip().is_loopback()
            || !cli.no_unix_socket
            || explicit_unix_socket
            || cli.background
        {
            return Err(invalid("Fast startup requires loopback TCP, --no-unix-socket, no --unix-socket and no --background"));
        }
        if cli.workspace.is_empty() {
            return Err(invalid("Fast startup requires at least one --workspace"));
        }
        for root in &cli.workspace {
            if !root.is_absolute() || !root.is_dir() {
                return Err(invalid(
                    "Fast --workspace must be an existing absolute directory",
                ));
            }
        }
        if cli.temporary_parent_pid == Some(0) {
            return Err(invalid("--temporary-parent-pid must be nonzero"));
        }
    }
    Ok((cli, nonce))
}

/// Run the standalone CLI, owning its runtime and bounded shutdown. `args`
/// includes argv[0]. The sole optional namespace prefix is the Fast flag at
/// argv[1]; all remaining arguments are parsed by the original standalone CLI.
/// Help/version return successfully before logging, watchers, or runtime startup.
pub fn run_standalone_from<I, T>(args: I) -> StandaloneResult
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let nonce = std::env::var(FAST_READINESS_NONCE_ENV).ok();
    let (cli, nonce) = match parse_from(args, nonce) {
        Ok(parsed) => parsed,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.print()?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    if let Some(parent) = cli.temporary_parent_pid {
        crate::temporary::watch_parent(parent)?;
    }

    if cli.background {
        daemonize_background()?;
    }

    let logging =
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("info,neoism_workspace_daemon=debug")
            }));
    if nonce.is_some() {
        let _ = logging
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .try_init();
    } else {
        // Preserve the standalone daemon's ordinary console logging behavior.
        let _ = logging.try_init();
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(run(cli, nonce));
    // Runtime::drop waits forever for spawn_blocking workers. Long-lived
    // agent/tool workers can outlive the accept loops, leaving a listenerless
    // daemon holding the shared agent database's exclusive lock. Persistence
    // is flushed by run() before this point. Bound the final runtime teardown
    // so this dedicated process can exit and release its file locks.
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}

fn owns_agent_server(readiness_nonce: Option<&str>) -> bool {
    readiness_nonce.is_none()
}

async fn run(cli: Cli, nonce: Option<String>) -> StandaloneResult {
    // Phase 0: make sure the daemon has a `NEOISM_DAEMON_TOKEN` before
    // anything binds. We log only the path, never the token value.
    let token_path = crate::daemon_token::ensure_daemon_token();
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
    // Only ordinary standalone daemons own Agent startup/supervision. A Fast
    // child borrows the home owner's existing local Agent and shared history;
    // if that owner disappears, association retries/fails without taking over.
    if owns_agent_server(nonce.as_deref()) {
        crate::agent::ensure_agent_server_started(workspaces.clone());
    }
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
    let paired_hosts =
        crate::hosts::PairedHostStore::load(&data_dir).unwrap_or_else(|err| {
            tracing::warn!(
                error = %err,
                "could not load paired-hosts file; falling back to in-memory store",
            );
            crate::hosts::PairedHostStore::in_memory()
        });
    // Reserve the TCP endpoint before slow agent association. Nothing serves
    // requests and no readiness is emitted until the declared namespace is ready.
    let early_listener = if nonce.is_some() {
        Some(tokio::net::TcpListener::bind(cli.addr).await?)
    } else {
        None
    };
    for dir in &cli.workspace {
        let workspace = workspaces.declare_startup_workspace(dir);
        if let Some(root) = workspace.root_dir.as_deref() {
            if let Err(error) = crate::agent_hosting::associate(&workspace.id, root).await
            {
                tracing::error!(
                    workspace_id = %workspace.id,
                    root = %root.display(),
                    error = %error,
                    "startup workspace association failed; readiness withheld",
                );
                return Err(error.into());
            }
        }
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
    crate::workspace::sweep_clipboard_dir_on_startup();
    let app = server::router(server::AppState {
        lsp_runtime: neoism_agent_server::language_server::LspRuntime::new(
            neoism_agent_neoism_adapter::neoism_services(),
        ),
        auth: auth_service,
        sessions: crate::sessions::SessionRegistry::shared(),
        workspaces: workspaces.clone(),
        pairing_tokens,
        crdt: crate::crdt::sync::CrdtSyncHub::default(),
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
    let listener = match early_listener {
        Some(listener) => listener,
        None => tokio::net::TcpListener::bind(cli.addr).await?,
    };
    let actual_addr = listener.local_addr()?;
    if let Some(nonce) = nonce {
        let acknowledgement = StartupAcknowledgement {
            protocol: FAST_READINESS_PROTOCOL.into(),
            nonce,
            address: actual_addr,
        };
        write_readiness(&mut std::io::stdout().lock(), &acknowledgement)?;
    }
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
            // Farewell frame FIRST: tell every connected guest the host
            // is ending the session so they detach their adopted
            // workspace and re-dial home instead of reconnecting forever
            // against a daemon that is about to vanish. The per-socket
            // pumps live on this same runtime, so give them a brief
            // window to flush the frame before we fall through and the
            // runtime drop aborts their tasks.
            workspaces.broadcast_host_ended("The host ended the session");
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            tracing::info!(
                "shutdown: HostEnded broadcast; snapshot persisted; aborting open connections and exiting"
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

fn write_readiness(
    writer: &mut impl Write,
    acknowledgement: &StartupAcknowledgement,
) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, acknowledgement)?;
    writer.write_all(b"\n")?;
    writer.flush()
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

    // Windows has no SIGTERM: console close and system shutdown arrive as
    // console control events instead, and missing them would skip the
    // snapshot flush below exactly when it matters most.
    #[cfg(windows)]
    {
        match (
            tokio::signal::windows::ctrl_close(),
            tokio::signal::windows::ctrl_shutdown(),
        ) {
            (Ok(mut ctrl_close), Ok(mut ctrl_shutdown)) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        tracing::info!("received Ctrl-C; shutting down");
                    }
                    _ = ctrl_close.recv() => {
                        tracing::info!("console closing; shutting down");
                    }
                    _ = ctrl_shutdown.recv() => {
                        tracing::info!("system shutdown; shutting down");
                    }
                }
            }
            _ => {
                tracing::warn!("failed to install console control handlers");
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("received Ctrl-C; shutting down");
            }
        }
    }

    #[cfg(all(not(unix), not(windows)))]
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

/// Windows stand-in for the double-fork: re-spawn ourselves detached
/// (minus `--background`, so the child doesn't recurse) and exit the
/// parent. The child re-parses the remaining args, so pidfile/state
/// flags behave as on unix.
#[cfg(windows)]
fn daemonize_background() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os()
        .skip(1)
        .filter(|arg| arg != "--background")
        .collect();
    let mut command = std::process::Command::new(exe);
    command.args(args);
    crate::detach_std_command(&mut command);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::process::exit(0);
}

#[cfg(all(not(unix), not(windows)))]
fn daemonize_background() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "--background is only supported on Unix and Windows",
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
