//! SSH-host resolution + loopback tunnel layer for remote-daemon attach.
//!
//! "Work from anywhere" Wave 2A. The desktop already knows how to attach to a
//! daemon over a websocket URL (`DaemonEndpoint::parse` accepts `ws://...` and
//! defaults the path to `/session`). This module produces such a URL by
//! reaching a *remote* daemon the same way Codex does:
//!
//! 1. Pick a free local port `<port>`.
//! 2. Spawn `ssh -N -L <port>:127.0.0.1:17878 <alias>` — a port-forward only,
//!    no remote shell.
//! 3. Hand back `ws://127.0.0.1:<port>/session`, which the existing daemon
//!    plumbing dials as if the daemon were local.
//!
//! ## Security model (loopback-only forward)
//!
//! The forward binds the *local* end to `127.0.0.1:<port>` (ssh's default for
//! `-L` without an explicit bind address) and targets `127.0.0.1:17878` on the
//! *remote* host. Both ends are loopback:
//!   - Locally, only this machine can reach `<port>`; nothing is exposed to the
//!     LAN.
//!   - Remotely, the daemon only needs to listen on its own loopback; it is
//!     never bound to a public interface. Traffic between the two loopbacks
//!     rides inside the authenticated, encrypted SSH channel.
//! This is the minimal-trust reach: no daemon port is ever world-reachable.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Dedicated loopback port for the persistent Quick-SSH daemon.
///
/// The desktop's embedded daemon owns 7878. Reusing that port meant an older
/// Neoism GUI on the SSH host silently became Quick SSH's backend, bypassing
/// the standalone daemon bootstrap and coupling the remote workspace to the
/// GUI process's lifetime/version. Keep Quick SSH on its own service port.
const REMOTE_DAEMON_PORT: u16 = 17878;
/// Stable daemon workspace used for Quick SSH. It is scoped by the remote
/// daemon/user, so reconnecting through a different local tunnel still finds
/// the same remote home workspace and PTYs.
pub const QUICK_SSH_WORKSPACE_ID: &str = "neoism-quick-ssh-home";

/// Whether an adopted daemon workspace is Neoism's private Quick-SSH home.
/// Quick SSH deliberately behaves like a local terminal (including cwd-driven
/// Explorer re-rooting), unlike a shared hosted workspace whose declared root
/// remains stable for every participant.
pub(crate) fn is_quick_ssh_workspace_id(workspace_id: &str) -> bool {
    workspace_id == QUICK_SSH_WORKSPACE_ID
        || workspace_id
            .strip_prefix(QUICK_SSH_WORKSPACE_ID)
            .is_some_and(|suffix| suffix.starts_with('-'))
}

/// Stable per-target workspace id. The remote daemon may serve several Quick
/// SSH aliases, and one desktop may keep several SSH hosts in the same window;
/// including the target prevents their otherwise-identical "home" ids from
/// colliding in the local workspace strip.
fn quick_ssh_workspace_id_with_args(target: &str, ssh_args: &[String]) -> String {
    // Deterministic FNV-1a avoids a new hashing dependency and keeps the id
    // stable across processes/platforms.
    let mut hash = 0xcbf29ce484222325_u64;
    for part in std::iter::once(target).chain(safe_connection_args(ssh_args)) {
        for byte in part.as_bytes().iter().copied().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("{QUICK_SSH_WORKSPACE_ID}-{hash:016x}")
}
/// How long to wait for the local forwarded port to start accepting
/// connections before giving up. Kept short so we never block startup.
const FORWARD_READY_TIMEOUT: Duration = Duration::from_secs(8);
/// Poll interval while waiting for the forward to come up.
const FORWARD_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A single `Host` block parsed out of `~/.ssh/config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHostAlias {
    /// The alias from the `Host` line (e.g. `home-server`).
    pub alias: String,
    /// `HostName` override, if the config specified one.
    pub hostname: Option<String>,
    /// `User` override, if present.
    pub user: Option<String>,
    /// `Port` override, if present.
    pub port: Option<u16>,
}

impl SshHostAlias {
    fn new(alias: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            hostname: None,
            user: None,
            port: None,
        }
    }

    /// A short, human-readable summary for UI/logging: `alias (user@hostname)`.
    /// Consumed by the Wave 2A host-switcher UI.
    #[allow(dead_code)]
    pub fn describe(&self) -> String {
        match (&self.user, &self.hostname) {
            (Some(user), Some(host)) => format!("{} ({user}@{host})", self.alias),
            (None, Some(host)) => format!("{} ({host})", self.alias),
            _ => self.alias.clone(),
        }
    }
}

/// Errors from attaching to a remote daemon over SSH. We never panic — a
/// missing `ssh` binary, a missing config, or a tunnel that won't come up all
/// degrade into one of these so the caller can fall back to local mode.
#[derive(Debug, thiserror::Error)]
pub enum SshAttachError {
    #[error("ssh host alias `{0}` not found in ~/.ssh/config")]
    UnknownAlias(String),
    #[error("could not allocate a local port: {0}")]
    PortAllocation(io::Error),
    #[error("failed to spawn `ssh` (is it installed and on PATH?): {0}")]
    SpawnFailed(io::Error),
    #[error("ssh exited before the forward came up: {0}")]
    SshExited(String),
    #[error("could not read the remote Neoism daemon credential: {0}")]
    CredentialUnavailable(String),
    #[error("ssh forward on 127.0.0.1:{port} did not come up within {timeout:?}")]
    ForwardTimeout { port: u16, timeout: Duration },
}

/// A live remote attachment. Holds the `ssh` child so the forward stays open
/// for as long as this guard is alive; dropping it tears the tunnel down.
pub struct DaemonAttach {
    /// `ws://127.0.0.1:<port>/session` — feed this straight into the existing
    /// daemon-url plumbing.
    pub daemon_url: String,
    /// The local port the forward is bound to.
    pub local_port: u16,
    /// The alias this attachment was opened for.
    pub alias: String,
    /// Stable identity for this exact host + transport/auth route.
    pub workspace_id: String,
    /// Original transport/auth options, retained so a dropped tunnel can be
    /// recreated without asking the user to type the SSH command again.
    pub ssh_args: Vec<String>,
    /// Remote daemon bearer read through the authenticated SSH channel. Kept
    /// in memory only and deliberately omitted from `Debug`.
    pub credential: String,
    /// Held only for its `Drop` guard — keeps the ssh forward alive for as
    /// long as this `DaemonAttach` lives, then kills it.
    #[allow(dead_code)]
    child: SshTunnel,
}

impl std::fmt::Debug for DaemonAttach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonAttach")
            .field("daemon_url", &self.daemon_url)
            .field("local_port", &self.local_port)
            .field("alias", &self.alias)
            .field("workspace_id", &self.workspace_id)
            .field("ssh_args", &self.ssh_args)
            .finish()
    }
}

impl DaemonAttach {
    /// Non-blocking liveness check for the SSH control process. The daemon is
    /// probed during bootstrap and websocket dialing; tab switching must not
    /// pause the UI for a network health timeout.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.child.try_wait(), Ok(None))
    }
}

/// RAII wrapper around the `ssh -N -L ...` child: kill on drop so a tunnel
/// never outlives the desktop process.
struct SshTunnel {
    child: Child,
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        // Best-effort teardown. The child may already be gone.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Options controlling how `attach_over_ssh` brings up the tunnel.
#[derive(Debug, Clone)]
pub struct AttachOptions {
    /// When set, also run the remote daemon via
    /// `ssh <alias> neoism-workspace-daemon --addr 127.0.0.1:<port>` so it is
    /// listening before we forward. The minimal path (default) assumes the
    /// daemon is already up on the remote loopback.
    pub launch_remote_daemon: bool,
    /// Remote port the daemon listens on (loopback). Defaults to the dedicated
    /// Quick-SSH daemon port, 17878.
    pub remote_port: u16,
    /// Connection options parsed from the user's interactive `ssh` command
    /// (`-p`, `-i`, `-F`, `-J`, ...). They are passed as distinct argv items
    /// before the host, never through a shell.
    pub ssh_args: Vec<String>,
}

impl Default for AttachOptions {
    fn default() -> Self {
        Self {
            launch_remote_daemon: false,
            remote_port: REMOTE_DAEMON_PORT,
            ssh_args: Vec::new(),
        }
    }
}

/// Quick-SSH entry point used by the command composer. It bootstraps (or
/// reuses) a persistent daemon on the SSH host, then returns the same local
/// loopback endpoint used by ordinary hosted workspaces.
pub fn attach_workspace_over_ssh(
    alias: &str,
    ssh_args: Vec<String>,
) -> Result<DaemonAttach, SshAttachError> {
    attach_over_ssh_with(
        alias,
        &AttachOptions {
            launch_remote_daemon: true,
            ssh_args,
            ..AttachOptions::default()
        },
    )
}

/// Path to the user's SSH config (`~/.ssh/config`).
fn ssh_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".ssh").join("config"))
}

/// Read and parse `~/.ssh/config`, best-effort. Returns an empty vec if the
/// file is missing or unreadable — never errors.
pub fn available_hosts() -> Vec<SshHostAlias> {
    let Some(path) = ssh_config_path() else {
        return Vec::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_ssh_config(&text),
        Err(_) => Vec::new(),
    }
}

/// Look up a single alias in `~/.ssh/config`.
pub fn find_host(alias: &str) -> Option<SshHostAlias> {
    available_hosts()
        .into_iter()
        .find(|host| host.alias == alias)
}

/// Pure parser for `~/.ssh/config` content. Extracts each `Host` block's alias
/// plus `HostName` / `User` / `Port` if present.
///
/// Deliberately small and tolerant:
///   - keys are matched case-insensitively (`HostName` == `hostname`).
///   - a `Host` line may list multiple aliases; we emit one entry per alias and
///     apply any following `HostName`/`User`/`Port` to all of them.
///   - wildcard-only patterns (`*`, `?`, `!`) are skipped — they're match
///     rules, not connectable hosts.
///   - `key=value` and `key value` are both accepted.
///   - blank lines and `#` comments are ignored.
pub fn parse_ssh_config(text: &str) -> Vec<SshHostAlias> {
    let mut hosts: Vec<SshHostAlias> = Vec::new();
    // Indices into `hosts` for the aliases declared by the current `Host` line.
    let mut current: Vec<usize> = Vec::new();

    for raw_line in text.lines() {
        // Strip inline comments and surrounding whitespace.
        let line = match raw_line.find('#') {
            Some(idx) => &raw_line[..idx],
            None => raw_line,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let (key, value) = split_key_value(line);
        let key_lower = key.to_ascii_lowercase();

        match key_lower.as_str() {
            "host" => {
                current.clear();
                for pattern in value.split_whitespace() {
                    // Skip negations and wildcard-only patterns; they don't
                    // name a concrete connectable host.
                    if pattern.starts_with('!') || is_wildcard(pattern) {
                        continue;
                    }
                    let idx = hosts.len();
                    hosts.push(SshHostAlias::new(pattern));
                    current.push(idx);
                }
            }
            "hostname" => {
                let value = value.trim();
                if !value.is_empty() {
                    for &idx in &current {
                        hosts[idx].hostname = Some(value.to_string());
                    }
                }
            }
            "user" => {
                let value = value.trim();
                if !value.is_empty() {
                    for &idx in &current {
                        hosts[idx].user = Some(value.to_string());
                    }
                }
            }
            "port" => {
                if let Ok(port) = value.trim().parse::<u16>() {
                    for &idx in &current {
                        hosts[idx].port = Some(port);
                    }
                }
            }
            // Match==Host-scoped block reset; anything inside a `Match` block
            // we don't attribute to a connectable alias.
            "match" => {
                current.clear();
            }
            _ => {}
        }
    }

    hosts
}

/// Split a config line into `(key, value)`, accepting both `key value` and
/// `key=value` forms.
fn split_key_value(line: &str) -> (&str, &str) {
    if let Some((key, value)) = line.split_once('=') {
        return (key.trim(), value.trim());
    }
    match line.split_once(char::is_whitespace) {
        Some((key, value)) => (key.trim(), value.trim()),
        None => (line, ""),
    }
}

/// True if the pattern is purely wildcard/match syntax (contains `*` or `?`).
fn is_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// Bind a transient `TcpListener` on `127.0.0.1:0`, read back the OS-assigned
/// port, and drop the listener. There is a small race window before `ssh`
/// re-binds the port, but in practice ssh grabs it immediately and the worst
/// case surfaces as a clean `SshExited`/`ForwardTimeout` we can retry.
fn pick_free_local_port() -> Result<u16, io::Error> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    Ok(port)
}

/// Attach to a remote daemon over SSH using default options (port-forward only,
/// assume the remote daemon is already listening on 127.0.0.1:17878).
pub fn attach_over_ssh(alias: &str) -> Result<DaemonAttach, SshAttachError> {
    attach_over_ssh_with(alias, &AttachOptions::default())
}

/// Attach to a remote daemon over SSH with explicit options.
pub fn attach_over_ssh_with(
    alias: &str,
    options: &AttachOptions,
) -> Result<DaemonAttach, SshAttachError> {
    // Resolve the alias against ~/.ssh/config when possible. We don't *require*
    // it to be present — ssh itself may resolve hosts via system config or
    // direct `user@host` — but a hit lets us log/describe it and lets the UI
    // enumerate. Only hard-fail if a config exists and clearly lacks the alias
    // AND the alias doesn't look like a bare host.
    if find_host(alias).is_none() && !looks_like_direct_host(alias) {
        let hosts = available_hosts();
        if !hosts.is_empty() {
            return Err(SshAttachError::UnknownAlias(alias.to_string()));
        }
        // No config at all: fall through and let ssh try to resolve it.
    }

    let local_port = pick_free_local_port().map_err(SshAttachError::PortAllocation)?;
    let forward_spec = format!(
        "{local_port}:127.0.0.1:{remote}",
        remote = options.remote_port
    );

    let mut command = Command::new("ssh");
    command
        .arg("-L")
        .arg(&forward_spec)
        // Fail fast on host-key / auth prompts rather than hanging forever.
        .arg("-o")
        .arg("BatchMode=yes")
        // Match modern OpenSSH's safe non-interactive first-connect policy:
        // record a brand-new host key, but still reject a changed key (which
        // can indicate a MITM or a rebuilt host that needs explicit cleanup).
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("-o")
        .arg("ConnectTimeout=8")
        // Keepalive so a dead tunnel surfaces instead of silently wedging.
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg("-o")
        .arg("ServerAliveCountMax=3");

    // Preserve the transport/auth choices from the command the user typed.
    // Forwarding options are deliberately excluded: Neoism owns this one
    // loopback forward, and replaying a user `-L/-R/-D/-W` here could bind a
    // second listener or turn the control connection into a proxy.
    for arg in safe_connection_args(&options.ssh_args) {
        command.arg(arg);
    }

    if options.launch_remote_daemon {
        // Start one persistent per-user remote engine when the well-known
        // loopback port is not already occupied, then keep this SSH control
        // connection alive for the forward. Reusing one daemon preserves PTYs,
        // file caches, Agent conversations, and workspace state across
        // reconnects instead of recreating a slow per-request ssh/cat process.
        command.arg(alias);
        // Use the remote login shell so ~/.local/bin and the user's real shell
        // are available to daemon-created PTYs. The bash/nc probe prevents a
        // second daemon from overwriting the live daemon's pidfile merely to
        // fail its bind. The daemon itself is detached; closing Neoism only
        // closes the tunnel, so returning later resumes the same remote state.
        command.arg(format!(
            "exec /bin/sh -lc \
             'PATH=\"$HOME/.local/bin:$PATH\"; export PATH; \
              command -v neoism-workspace-daemon >/dev/null 2>&1 || \
                {{ echo \"neoism-workspace-daemon is not installed on the SSH host\" >&2; exit 127; }}; \
              daemon_up=0; \
              if command -v bash >/dev/null 2>&1 && \
                 bash -c \"exec 3<>/dev/tcp/127.0.0.1/{port}\" 2>/dev/null; then daemon_up=1; \
              elif command -v nc >/dev/null 2>&1 && nc -z 127.0.0.1 {port} >/dev/null 2>&1; then daemon_up=1; fi; \
              if [ \"$daemon_up\" -eq 0 ]; then \
                cd \"$HOME\" && \
                unset NEOISM_DAEMON_TOKEN && \
                neoism-workspace-daemon --background --no-unix-socket \
                  --addr 127.0.0.1:{port} \
                  --state-dir \"$HOME/.local/state/neoism/ssh-daemon\" \
                  --pidfile \"$HOME/.local/state/neoism/ssh-daemon/daemon.pid\"; \
              fi; \
              exec sleep 2147483647'",
            port = options.remote_port,
        ));
    } else {
        // No remote command; `-N` holds the forward open and nothing else.
        command.arg("-N");
        command.arg(alias);
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let child = command.spawn().map_err(SshAttachError::SpawnFailed)?;
    let mut tunnel = SshTunnel { child };

    // Wait for the local forwarded port to start accepting connections, or for
    // ssh to die, or for the timeout to elapse. Never block forever.
    let deadline = Instant::now() + FORWARD_READY_TIMEOUT;
    loop {
        // If ssh has already exited, surface its status instead of waiting.
        match tunnel.child.try_wait() {
            Ok(Some(status)) => {
                return Err(SshAttachError::SshExited(format!(
                    "ssh exited with {status}"
                )));
            }
            Ok(None) => {}
            Err(err) => {
                return Err(SshAttachError::SshExited(err.to_string()));
            }
        }

        if daemon_health_is_ready(local_port) {
            let credential = read_remote_daemon_credential(alias, &options.ssh_args)?;
            let daemon_url = format!("ws://127.0.0.1:{local_port}/session");
            tracing::info!(
                alias,
                local_port,
                daemon = %daemon_url,
                "ssh -L forward up; attaching to remote daemon"
            );
            return Ok(DaemonAttach {
                daemon_url,
                local_port,
                alias: alias.to_string(),
                workspace_id: quick_ssh_workspace_id_with_args(alias, &options.ssh_args),
                ssh_args: options.ssh_args.clone(),
                credential,
                child: tunnel,
            });
        }

        if Instant::now() >= deadline {
            return Err(SshAttachError::ForwardTimeout {
                port: local_port,
                timeout: FORWARD_READY_TIMEOUT,
            });
        }

        std::thread::sleep(FORWARD_POLL_INTERVAL);
    }
    // `tunnel` is dropped here on every error path, killing the ssh child.
}

/// A bare `user@host` or `host` argument that ssh can resolve directly even
/// without a config entry.
fn looks_like_direct_host(alias: &str) -> bool {
    !alias.is_empty() && !alias.contains(char::is_whitespace) && !is_wildcard(alias)
}

/// Keep only SSH options that describe how to reach/authenticate to the host.
/// The parser already emits flag/value pairs; this second boundary makes the
/// tunnel safe even if another caller supplies `AttachOptions` directly.
fn safe_connection_args(args: &[String]) -> Vec<&str> {
    const SAFE_VALUE_FLAGS: &[&str] =
        &["-p", "-i", "-o", "-l", "-F", "-J", "-b", "-c", "-I"];
    let mut safe = Vec::new();
    let mut index = 0;
    while index + 1 < args.len() {
        if SAFE_VALUE_FLAGS.contains(&args[index].as_str()) {
            safe.push(args[index].as_str());
            safe.push(args[index + 1].as_str());
        }
        index += 2;
    }
    safe
}

/// Read the daemon bearer over a second short-lived SSH exec channel. The
/// long-lived forwarding channel never exposes it on argv or in logs, and the
/// desktop keeps the returned value in memory only. Both Unix token locations
/// are checked because a GUI-launched daemon and an sshd login shell can have
/// different `XDG_RUNTIME_DIR` environments.
fn read_remote_daemon_credential(
    alias: &str,
    ssh_args: &[String],
) -> Result<String, SshAttachError> {
    let mut command = Command::new("ssh");
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("ConnectTimeout=8");
    for arg in safe_connection_args(ssh_args) {
        command.arg(arg);
    }
    command.arg(alias).arg(
        "runtime_token=; \
         if [ -n \"${XDG_RUNTIME_DIR:-}\" ] && [ -r \"$XDG_RUNTIME_DIR/neoism/daemon-token\" ]; then \
           runtime_token=\"$XDG_RUNTIME_DIR/neoism/daemon-token\"; \
         else \
           fallback=\"/tmp/neoism-$(id -u)/daemon-token\"; \
           [ -r \"$fallback\" ] && runtime_token=\"$fallback\"; \
         fi; \
         [ -n \"$runtime_token\" ] || exit 1; \
         IFS= read -r token < \"$runtime_token\"; \
         [ -n \"$token\" ] || exit 1; \
         printf '%s\\n' \"$token\"",
    );
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| SshAttachError::CredentialUnavailable(error.to_string()))?;
    if !output.status.success() {
        return Err(SshAttachError::CredentialUnavailable(format!(
            "ssh exited with {}",
            output.status
        )));
    }
    let credential = String::from_utf8(output.stdout).map_err(|_| {
        SshAttachError::CredentialUnavailable("token was not UTF-8".into())
    })?;
    let credential = credential.trim().to_string();
    if credential.is_empty() || credential.contains('\r') || credential.contains('\n') {
        return Err(SshAttachError::CredentialUnavailable(
            "token file was empty or malformed".into(),
        ));
    }
    Ok(credential)
}

/// Probe the daemon itself rather than merely the SSH listener. An `ssh -L`
/// socket starts accepting locally even while nothing is listening on the
/// remote side; treating that as ready caused a race where the websocket dial
/// failed immediately after a successful-looking attach.
fn daemon_health_is_ready(port: u16) -> bool {
    let address = std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) =
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(250))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(350)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(350)));
    if stream
        .write_all(
            b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .is_err()
    {
        return false;
    }
    let mut response = [0_u8; 64];
    let Ok(read) = stream.read(&mut response) else {
        return false;
    };
    response[..read].starts_with(b"HTTP/1.1 200")
        || response[..read].starts_with(b"HTTP/1.0 200")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_host_block() {
        let cfg = "\
Host home-server
    HostName 192.168.1.20
    User parker
    Port 2222
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(
            hosts[0],
            SshHostAlias {
                alias: "home-server".into(),
                hostname: Some("192.168.1.20".into()),
                user: Some("parker".into()),
                port: Some(2222),
            }
        );
    }

    #[test]
    fn parses_multiple_blocks() {
        let cfg = "\
Host alpha
    HostName alpha.example.com

Host beta
    HostName 10.0.0.5
    User root
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "alpha");
        assert_eq!(hosts[0].hostname.as_deref(), Some("alpha.example.com"));
        assert_eq!(hosts[0].user, None);
        assert_eq!(hosts[1].alias, "beta");
        assert_eq!(hosts[1].hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(hosts[1].user.as_deref(), Some("root"));
    }

    #[test]
    fn host_with_no_extra_keys() {
        let cfg = "Host bare\n";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0], SshHostAlias::new("bare"));
    }

    #[test]
    fn skips_wildcard_and_negation_patterns() {
        let cfg = "\
Host *
    ForwardAgent yes

Host !secret real-host
    HostName real.example.com

Host *.internal
    User svc
";
        let hosts = parse_ssh_config(cfg);
        // Only `real-host` is a connectable alias; `*`, `!secret`, `*.internal`
        // are all skipped.
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "real-host");
        assert_eq!(hosts[0].hostname.as_deref(), Some("real.example.com"));
    }

    #[test]
    fn multiple_aliases_on_one_host_line_share_settings() {
        let cfg = "\
Host work work-vpn
    HostName work.example.com
    User dev
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 2);
        for host in &hosts {
            assert_eq!(host.hostname.as_deref(), Some("work.example.com"));
            assert_eq!(host.user.as_deref(), Some("dev"));
        }
        assert_eq!(hosts[0].alias, "work");
        assert_eq!(hosts[1].alias, "work-vpn");
    }

    #[test]
    fn accepts_equals_and_case_insensitive_keys() {
        let cfg = "\
host=myhost
  hostname=myhost.example.com
  PORT=22
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "myhost");
        assert_eq!(hosts[0].hostname.as_deref(), Some("myhost.example.com"));
        assert_eq!(hosts[0].port, Some(22));
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let cfg = "\
# top comment

Host gamma   # inline comment
    HostName gamma.example.com  # another
    # standalone comment line
    User g
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "gamma");
        assert_eq!(hosts[0].hostname.as_deref(), Some("gamma.example.com"));
        assert_eq!(hosts[0].user.as_deref(), Some("g"));
    }

    #[test]
    fn invalid_port_is_ignored() {
        let cfg = "\
Host h
    Port notaport
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].port, None);
    }

    #[test]
    fn empty_config_yields_no_hosts() {
        assert!(parse_ssh_config("").is_empty());
        assert!(parse_ssh_config("# just a comment\n\n").is_empty());
    }

    #[test]
    fn keys_before_any_host_are_dropped() {
        // Global options before the first Host line shouldn't attach anywhere.
        let cfg = "\
HostName orphan.example.com
User nobody

Host real
    HostName real.example.com
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "real");
        assert_eq!(hosts[0].hostname.as_deref(), Some("real.example.com"));
    }

    #[test]
    fn describe_formats_user_host() {
        let host = SshHostAlias {
            alias: "box".into(),
            hostname: Some("box.example.com".into()),
            user: Some("me".into()),
            port: None,
        };
        assert_eq!(host.describe(), "box (me@box.example.com)");
        assert_eq!(SshHostAlias::new("plain").describe(), "plain");
    }

    #[test]
    fn direct_host_detection() {
        assert!(looks_like_direct_host("user@host"));
        assert!(looks_like_direct_host("home-server"));
        assert!(!looks_like_direct_host("has space"));
        assert!(!looks_like_direct_host("wild*card"));
        assert!(!looks_like_direct_host(""));
    }

    #[test]
    fn quick_ssh_uses_a_dedicated_remote_daemon_port() {
        let options = AttachOptions::default();
        assert_eq!(options.remote_port, 17878);
        assert_ne!(options.remote_port, 7878);
    }

    #[test]
    fn tunnel_replays_only_connection_options() {
        let args = vec![
            "-p".into(),
            "2222".into(),
            "-i".into(),
            "/tmp/key".into(),
            "-L".into(),
            "9000:localhost:9".into(),
            "-W".into(),
            "bad-proxy".into(),
        ];
        assert_eq!(
            safe_connection_args(&args),
            vec!["-p", "2222", "-i", "/tmp/key"]
        );
    }

    #[test]
    fn quick_ssh_workspace_ids_are_stable_and_target_scoped() {
        assert_eq!(
            quick_ssh_workspace_id_with_args("user@host", &[]),
            quick_ssh_workspace_id_with_args("user@host", &[])
        );
        assert_ne!(
            quick_ssh_workspace_id_with_args("user@host", &[]),
            quick_ssh_workspace_id_with_args("other@host", &[])
        );
        assert_ne!(
            quick_ssh_workspace_id_with_args("host", &[]),
            quick_ssh_workspace_id_with_args("host", &["-p".into(), "2222".into()])
        );
        assert!(is_quick_ssh_workspace_id(QUICK_SSH_WORKSPACE_ID));
        assert!(is_quick_ssh_workspace_id(
            &quick_ssh_workspace_id_with_args("host", &[])
        ));
        assert!(!is_quick_ssh_workspace_id("neoism-quick-ssh-homebrew"));
    }
}
