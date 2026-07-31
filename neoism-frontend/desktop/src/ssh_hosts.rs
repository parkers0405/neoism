//! SSH-host resolution + loopback tunnel layer for remote-daemon attach.
//!
//! "Work from anywhere" Wave 2A. The desktop already knows how to attach to a
//! daemon over a websocket URL (`DaemonEndpoint::parse` accepts `ws://...` and
//! defaults the path to `/session`). This module produces such a URL by
//! reaching a *remote* daemon the same way Codex does:
//!
//! 1. Pick a free local port `<port>`.
//! 2. Spawn `ssh -N -L <port>:127.0.0.1:7878 <alias>` — a port-forward only,
//!    no remote shell.
//! 3. Hand back `ws://127.0.0.1:<port>/session`, which the existing daemon
//!    plumbing dials as if the daemon were local.
//!
//! ## Security model (loopback-only forward)
//!
//! The forward binds the *local* end to `127.0.0.1:<port>` (ssh's default for
//! `-L` without an explicit bind address) and targets `127.0.0.1:7878` on the
//! *remote* host. Both ends are loopback:
//!   - Locally, only this machine can reach `<port>`; nothing is exposed to the
//!     LAN.
//!   - Remotely, the daemon only needs to listen on its own loopback; it is
//!     never bound to a public interface. Traffic between the two loopbacks
//!     rides inside the authenticated, encrypted SSH channel.
//! This is the minimal-trust reach: no daemon port is ever world-reachable.

use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Default port the remote workspace-daemon listens on (loopback).
const REMOTE_DAEMON_PORT: u16 = 7878;
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
            .finish()
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
    /// Remote port the daemon listens on (loopback). Defaults to 7878.
    pub remote_port: u16,
}

impl Default for AttachOptions {
    fn default() -> Self {
        Self {
            launch_remote_daemon: false,
            remote_port: REMOTE_DAEMON_PORT,
        }
    }
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
/// assume the remote daemon is already listening on 127.0.0.1:7878).
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
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("-o")
        .arg("ConnectTimeout=8")
        // Keepalive so a dead tunnel surfaces instead of silently wedging.
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg("-o")
        .arg("ServerAliveCountMax=3");

    if options.launch_remote_daemon {
        // Optional: bring the remote daemon up ourselves. We run a remote
        // command (so no `-N`) that binds the daemon to its own loopback. The
        // daemon stays attached to this ssh session in the foreground, so the
        // forward and the daemon share a lifetime — killing the tunnel stops
        // both. `ExitOnForwardFailure` tears everything down on a bad bind.
        command.arg(alias);
        // Run the daemon through the remote user's *login* shell. A bare
        // `ssh host neoism-workspace-daemon` executes with a stripped,
        // non-login environment: no `~/.local/bin` on PATH (so the daemon
        // binary may not even be found) and, crucially, no usable `$SHELL`,
        // which makes the PTYs the daemon spawns fall back to `/bin/sh` with
        // no OSC 133 integration — so every command's block timer spins
        // forever. `-l` restores the interactive PATH/`$SHELL`; `exec` keeps
        // the daemon tied to this ssh session's lifetime.
        command.arg(format!(
            "exec \"${{SHELL:-/bin/bash}}\" -lc \
             'neoism-workspace-daemon --addr 127.0.0.1:{}'",
            options.remote_port
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

        if local_port_is_open(local_port) {
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

/// Probe whether the local forwarded port accepts TCP connections yet.
fn local_port_is_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(250),
    )
    .is_ok()
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
}
