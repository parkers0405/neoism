use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";
const DEFAULT_HOSTNAME: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 4096;
const HEALTH_PATH: &str = "/global/health";

static SERVER_THREAD: OnceLock<()> = OnceLock::new();

pub(crate) fn ensure_started() {
    ensure_started_inner(false);
}

pub(crate) fn ensure_started_for_request() {
    ensure_started_inner(true);
}

fn ensure_started_inner(wait_for_health: bool) {
    let server = configured_server();
    std::env::set_var("NEOISM_SERVER", &server);

    if is_healthy(&server) {
        tracing::info!(target: "neoism::agent_server", server, "using existing Neoism Agent server");
        return;
    }

    let Some((hostname, port)) = local_bind_target(&server) else {
        tracing::warn!(
            target: "neoism::agent_server",
            server,
            "Neoism Agent server is not healthy; not auto-starting non-local configured server"
        );
        return;
    };

    if SERVER_THREAD.get().is_some() {
        if wait_for_health && !wait_until_healthy(&server, Duration::from_millis(1500)) {
            tracing::warn!(
                target: "neoism::agent_server",
                server,
                "Neoism Agent server did not become healthy before request"
            );
        }
        return;
    }

    // Import provider API keys (OPENROUTER_API_KEY, XAI_API_KEY, …) from the
    // user's login shell before the embedded server reads the environment.
    // The GUI is usually launched from a desktop session that does not inherit
    // interactive-shell exports, so without this an env-authed provider is
    // reported "not connected" and hidden from the `/model` picker.
    hydrate_provider_env_from_login_shell();

    tracing::info!(
        target: "neoism::agent_server",
        server = format_args!("http://{hostname}:{port}"),
        "starting embedded Neoism Agent server"
    );

    match std::thread::Builder::new()
        .name("neoism-agent-server".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("neoism-agent-server-runtime")
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(
                        target: "neoism::agent_server",
                        %error,
                        "failed to create Neoism Agent runtime"
                    );
                    return;
                }
            };

            runtime.block_on(async move {
                let options = neoism_agent_server::ServerOptions {
                    hostname,
                    port,
                    cors: Vec::new(),
                };
                if let Err(error) = neoism_agent_server::listen(options).await {
                    tracing::warn!(
                        target: "neoism::agent_server",
                        %error,
                        "embedded Neoism Agent server exited"
                    );
                }
            });
        }) {
        Ok(_handle) => {
            let _ = SERVER_THREAD.set(());
            if wait_for_health
                && !wait_until_healthy(&server, Duration::from_millis(1500))
            {
                tracing::warn!(
                    target: "neoism::agent_server",
                    server,
                    "Neoism Agent server did not become healthy before request"
                );
            }
        }
        Err(error) => {
            tracing::error!(
                target: "neoism::agent_server",
                %error,
                "failed to spawn Neoism Agent server thread"
            );
        }
    }
}

fn configured_server() -> String {
    std::env::var("NEOISM_SERVER")
        .ok()
        .map(|server| server.trim().trim_end_matches('/').to_string())
        .filter(|server| !server.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

fn is_healthy(server: &str) -> bool {
    let Ok(response) = http_get(server, HEALTH_PATH, Duration::from_millis(250)) else {
        return false;
    };
    response.starts_with("HTTP/1.1 200 ") || response.starts_with("HTTP/1.0 200 ")
}

fn wait_until_healthy(server: &str, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if is_healthy(server) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    is_healthy(server)
}

fn http_get(server: &str, path: &str, timeout: Duration) -> Result<String, String> {
    let (host, port, base_path) = parse_http_server(server)?;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("failed to resolve Neoism Agent server: {error}"))?
        .next()
        .ok_or_else(|| "failed to resolve Neoism Agent server".to_string())?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|error| format!("Neoism Agent is not reachable at {server}: {error}"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let request_path = request_path(&base_path, path);
    let request = format!(
        "GET {request_path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).map_err(|error| {
        format!("failed to write Neoism Agent health request: {error}")
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        format!("failed to read Neoism Agent health response: {error}")
    })?;
    Ok(response)
}

fn local_bind_target(server: &str) -> Option<(String, u16)> {
    let (host, port, base_path) = parse_http_server(server).ok()?;
    if !base_path.is_empty() {
        return None;
    }
    if matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1") {
        return Some((DEFAULT_HOSTNAME.to_string(), port));
    }
    None
}

fn parse_http_server(server: &str) -> Result<(String, u16, String), String> {
    let rest = server.strip_prefix("http://").ok_or_else(|| {
        format!("unsupported Neoism Agent server '{server}'; expected http://")
    })?;
    let (host_port, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = host_port
        .rsplit_once(':')
        .map(|(host, port)| {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("invalid Neoism Agent port '{port}'"))?;
            Ok::<_, String>((host.to_string(), port))
        })
        .transpose()?
        .unwrap_or_else(|| (host_port.to_string(), DEFAULT_PORT));
    if host.is_empty() {
        return Err("Neoism Agent server host is empty".to_string());
    }
    Ok((host, port, base_path.trim_end_matches('/').to_string()))
}

fn request_path(base_path: &str, path: &str) -> String {
    if base_path.is_empty() {
        return path.to_string();
    }
    format!(
        "/{}/{}",
        base_path.trim_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Import provider credential env vars from the user's login/interactive shell
/// into this process, once, so the embedded agent server can detect providers
/// the user authed via a shell export.
///
/// Desktop launchers (systemd/uwsm/Hyprland/dock) start the app with a minimal
/// environment that lacks the exports from `~/.zshrc` / `~/.bashrc` /
/// `~/.profile`. The agent server decides which providers are "connected" by
/// reading `std::env::var` in its own process, so a key like
/// `OPENROUTER_API_KEY` that only exists in the interactive shell is invisible
/// and the provider never appears in `/model`. We resolve the shell environment
/// best-effort (bounded by a short timeout) and set any provider-looking keys
/// that aren't already present. Existing values always win — we never override
/// a key already in the process environment.
fn hydrate_provider_env_from_login_shell() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for (key, value) in resolve_login_shell_env() {
            if !is_provider_credential_key(&key) {
                continue;
            }
            if std::env::var_os(&key).is_some() || value.trim().is_empty() {
                continue;
            }
            tracing::info!(
                target: "neoism::agent_server",
                key = %key,
                "imported provider credential from login shell"
            );
            std::env::set_var(&key, value);
        }
    });
}

/// A conservative filter for env var names that look like provider credentials,
/// so we never import unrelated shell state (PATH, HOME, secrets for other
/// tools) into the process. Covers the overwhelmingly common conventions:
/// `*_API_KEY`, `*_API_TOKEN`, `*_AUTH_TOKEN` (e.g. `OPENROUTER_API_KEY`,
/// `XAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`).
fn is_provider_credential_key(key: &str) -> bool {
    key.ends_with("_API_KEY")
        || key.ends_with("_API_TOKEN")
        || key.ends_with("_AUTH_TOKEN")
}

/// Run the user's login+interactive shell to dump its environment. Returns the
/// parsed `KEY=VALUE` pairs, or an empty vec if the shell can't be resolved in
/// time. Bounded by a short timeout so a slow or misbehaving rc file can't stall
/// startup, and stdin is detached so an interactive shell can't block on a tty.
fn resolve_login_shell_env() -> Vec<(String, String)> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());
    // Prefer a login+interactive shell (sources both profile and rc files where
    // exports usually live); fall back to login-only if the interactive run
    // fails (some shells refuse `-i` without a controlling tty).
    for args in [
        ["-l", "-i", "-c", "env"].as_slice(),
        ["-l", "-c", "env"].as_slice(),
    ] {
        if let Some(output) = run_env_capture(&shell, args, Duration::from_secs(3)) {
            let parsed = parse_env_dump(&output);
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    Vec::new()
}

fn run_env_capture(shell: &str, args: &[&str], timeout: Duration) -> Option<String> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let mut child = Command::new(shell)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stdout.read_to_string(&mut buffer);
        let _ = tx.send(buffer);
    });
    let result = rx.recv_timeout(timeout).ok();
    // The shell has printed and exited (or we timed out); make sure it's reaped.
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn parse_env_dump(dump: &str) -> Vec<(String, String)> {
    dump.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            if key.is_empty() || key.contains(char::is_whitespace) {
                return None;
            }
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}
