//! Cached `tailscale ip -4` probe.
//!
//! The tailscale CLI blocks for ~2s when tailscaled is not running —
//! it retries the localapi socket dial before giving up. macOS hit
//! this on EVERY daemon workspace sync (`desktop_daemon_url`), which
//! runs on each file open / tab switch / workspace switch, freezing
//! the main thread ~2s per action. Probe on a background thread and
//! serve callers from a cache.
//!
//! The cache re-probes (in the background) once `TTL` has passed, so
//! a tailnet that comes up AFTER the app launched is picked up within
//! a couple of minutes without a restart — starting tailscaled used
//! to leave the workspaces flow blind until neoism was relaunched.

use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(60);

#[derive(Default)]
struct ProbeState {
    ip: Option<IpAddr>,
    probed_at: Option<Instant>,
    in_flight: bool,
}

fn state() -> &'static Mutex<ProbeState> {
    static STATE: OnceLock<Mutex<ProbeState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ProbeState::default()))
}

/// Non-blocking accessor for hot paths (main thread): returns the
/// last probed value immediately and, when the cache is stale, kicks
/// a refresh on a background thread. Returns `None` until the first
/// probe lands; callers that re-sync periodically (daemon workspace
/// sync) pick up the real value on a later pass.
pub fn cached_ipv4() -> Option<IpAddr> {
    let Ok(mut guard) = state().lock() else {
        return None;
    };
    let stale = guard
        .probed_at
        .is_none_or(|probed_at| probed_at.elapsed() >= TTL);
    if stale && !guard.in_flight {
        guard.in_flight = true;
        let spawned = std::thread::Builder::new()
            .name("neoism-tailscale-probe".into())
            .spawn(|| {
                let ip = probe_ipv4();
                if let Ok(mut guard) = state().lock() {
                    guard.ip = ip;
                    guard.probed_at = Some(Instant::now());
                    guard.in_flight = false;
                }
            })
            .is_ok();
        if !spawned {
            guard.in_flight = false;
        }
    }
    guard.ip
}

/// Blocking accessor for startup paths that need the answer now
/// (embedded daemon bind addresses). Probes inline when the cache has
/// never been filled; primes the cache so the non-blocking accessor
/// starts out warm.
pub fn blocking_ipv4() -> Option<IpAddr> {
    {
        let Ok(mut guard) = state().lock() else {
            return None;
        };
        if guard.probed_at.is_some() || guard.in_flight {
            return guard.ip;
        }
        guard.in_flight = true;
    }
    let ip = probe_ipv4();
    if let Ok(mut guard) = state().lock() {
        guard.ip = ip;
        guard.probed_at = Some(Instant::now());
        guard.in_flight = false;
    }
    ip
}

/// CLI candidates, most specific last: the bare PATH lookup covers
/// linux and open-source mac installs; the app-bundle binary covers
/// macs running the GUI Tailscale.app (network-extension variant),
/// which ships no PATH-visible `tailscale` and whose daemon the
/// open-source CLI cannot reach.
fn cli_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &[
            "tailscale",
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        &["tailscale"]
    }
}

fn probe_ipv4() -> Option<IpAddr> {
    cli_candidates().iter().find_map(|cli| {
        // Bound each candidate separately: on macOS a stalled PATH CLI must
        // not consume the app-bundle fallback's opportunity to answer.
        let deadline = Instant::now() + Duration::from_secs(2);
        // Direct hidden child, not the GUI trampoline: on timeout we must
        // terminate/reap the actual CLI rather than orphan it behind a wrapper.
        let mut command = std::process::Command::new(cli);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
            );
        }
        command.args(["ip", "-4"]);
        let stdout = bounded_output(&mut command, deadline).ok()?;
        String::from_utf8_lossy(&stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .and_then(|line| line.parse().ok())
    })
}

/// File-backed bounded capture avoids pipe-full deadlocks and a reader thread
/// stuck forever on a pipe inherited by a descendant. Nothing from CLI output
/// (or environment/arguments) is written to logs.
fn bounded_output(
    command: &mut std::process::Command,
    deadline: Instant,
) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    use std::process::Stdio;
    if Instant::now() >= deadline {
        return Err(std::io::ErrorKind::TimedOut.into());
    }
    let mut output = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(output.try_clone()?);
    let mut child = command.spawn()?;
    loop {
        let result = child.try_wait();
        match result {
            Ok(Some(status)) if status.success() => {
                output.seek(SeekFrom::Start(0))?;
                let mut bytes = Vec::new();
                output.take(4097).read_to_end(&mut bytes)?;
                return if bytes.len() <= 4096 {
                    Ok(bytes)
                } else {
                    Err(std::io::ErrorKind::InvalidData.into())
                };
            }
            Ok(Some(_)) => return Err(std::io::ErrorKind::Other.into()),
            Ok(None)
                if Instant::now() < deadline
                    && output.metadata().is_ok_and(|m| m.len() <= 4096) => {}
            result => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::debug!("Tailscale discovery timed out or failed; continuing without tailnet discovery");
                return Err(result
                    .err()
                    .unwrap_or_else(|| std::io::ErrorKind::TimedOut.into()));
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn hung_discovery_is_killed_and_reaped() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let start = Instant::now();
        assert!(bounded_output(&mut command, start + Duration::from_millis(80)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[cfg(unix)]
    #[test]
    fn slow_discovery_can_succeed_within_budget() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 0.05; printf '100.64.0.1\\n'"]);
        assert_eq!(
            bounded_output(&mut command, Instant::now() + Duration::from_secs(1))
                .unwrap(),
            b"100.64.0.1\n"
        );
    }
    #[test]
    fn expired_budget_does_not_spawn() {
        let mut command =
            std::process::Command::new("not-a-real-neoism-discovery-program");
        assert_eq!(
            bounded_output(&mut command, Instant::now())
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
    }
}
