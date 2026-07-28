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
        let Ok(guard) = state().lock() else {
            return None;
        };
        if guard.probed_at.is_some() || guard.in_flight {
            // A probe already ran (or is running); don't shell out
            // again — the freshest value is (or will shortly be) in
            // the cache.
            return guard.ip;
        }
    }
    let ip = probe_ipv4();
    if let Ok(mut guard) = state().lock() {
        guard.ip = ip;
        guard.probed_at = Some(Instant::now());
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
        let mut command = crate::background_process::command(cli);
        let output = command.args(["ip", "-4"]).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .and_then(|line| line.parse().ok())
    })
}
