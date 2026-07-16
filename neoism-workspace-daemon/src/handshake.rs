//! Multi-machine pairing-token handshake.
//!
//! This module is the auth foundation for the "workplace spans multiple
//! laptops" story (think Tailscale). The protocol-level message shapes
//! (`Hello` / `HelloAck`) live in
//! [`neoism_protocol::workspace`]; what lives here is the daemon-side
//! state and policy:
//!
//!   * a short-lived **pairing token** the daemon mints on startup,
//!     prints to stdout, and (best-effort) persists to
//!     `$XDG_CONFIG_HOME/neoism/pairing-tokens` (or
//!     `~/.config/neoism/pairing-tokens`) so an operator can rediscover
//!     it without restarting the daemon;
//!   * a verifier that constant-time-compares an inbound token against
//!     the active set;
//!   * the [`HandshakeOutcome`] computed for a given inbound
//!     `Hello { token }` plus the `NEOISM_REQUIRE_AUTH` environment
//!     gate;
//!   * a best-effort `tailscale whois <ip>` shell-out so a paired
//!     surface (laptop chrome) can render "connected to
//!     laptop-A (you@tailnet)" without making its own tailscale calls.
//!     Identity resolution is **logging-only** — it never gates the
//!     handshake decision; the gate is the token check alone.
//!
//! Design choices worth flagging up front:
//!
//!   * **No tailscale dependency at runtime.** We shell out to the
//!     `tailscale` binary opportunistically and discard the result on
//!     any failure (missing binary, non-zero exit, parse miss). The
//!     daemon stays useful on hosts that don't have tailscale
//!     installed — multi-machine just degrades to "trust local" until
//!     a real tunnel exists.
//!
//!   * **Tokens never authorise capabilities.** Authorisation lives
//!     in `auth::DeviceRecord::granted_permissions`. The pairing
//!     handshake here is gate-only: did the connecting peer present a
//!     plausible bootstrap secret, and if not, do we reject? The
//!     long-lived device tokens still flow through `auth::AuthService`
//!     once the operator approves a `pair_claim`.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use neoism_protocol::auth::constant_time_eq;
use neoism_protocol::workspace::PairingSummary;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Environment variable that flips the daemon from "trust local" to
/// "reject connections without a valid pairing token". A connection
/// that omits `Hello` entirely (older clients) is also rejected when
/// this is set, mirroring the `Authorization: Bearer` enforcement on
/// the HTTP routes.
pub const ENV_REQUIRE_AUTH: &str = "NEOISM_REQUIRE_AUTH";

/// Filename the daemon writes the active pairing tokens to. One token
/// per line. The file is created with `0o600` on unix.
pub const PAIRING_TOKENS_FILENAME: &str = "pairing-tokens";

/// Length of a generated pairing token in bytes. 24 bytes -> 48 hex
/// chars, comfortably above the brute-force budget for a 60-day rolling
/// secret. We keep it short enough to be paste-able into a phone.
const TOKEN_BYTES: usize = 24;

/// Number of hex chars surfaced in `PairingSummary::fingerprint_prefix`.
/// Twelve hex chars over a SHA-256 prefix gives 48 bits of entropy —
/// plenty to disambiguate the handful of tokens an operator holds, and
/// short enough to render in a `revoke <prefix>` CLI column or a
/// settings-panel row without overflow.
pub const FINGERPRINT_PREFIX_LEN: usize = 12;

/// Cloneable handle to the daemon's set of accepted pairing tokens.
///
/// Reads (verify, list) take a shared lock; writes (mint, persist) take
/// an exclusive lock. The set is tiny (typically 1, occasionally 2 or 3
/// during rotation) so the cost of a linear constant-time scan is
/// negligible.
#[derive(Clone)]
pub struct PairingTokenStore {
    inner: Arc<RwLock<TokenInner>>,
}

struct TokenInner {
    /// Path to the persisted tokens file. `None` when the store is
    /// in-memory only (tests).
    path: Option<PathBuf>,
    /// Active pairing entries keyed by their raw token string. We hold
    /// the raw token (not just a hash) on purpose: the pairing-token
    /// file is already a secret that lives in the operator's `$HOME`
    /// with `0o600`, and storing the raw value lets us re-emit it on
    /// stdout/CLI without prompting the operator to re-mint.
    ///
    /// The `PairingTokenEntry` value adds the metadata
    /// (device label, last-seen, created-at, fingerprint prefix) the
    /// revocation UI needs to render an actionable row without ever
    /// exposing the raw secret.
    tokens: HashMap<String, PairingTokenEntry>,
}

/// Persisted-and-in-memory shape of a single accepted pairing token.
///
/// The struct is serialized one-per-line as JSON in the
/// `pairing-tokens` file. Legacy single-line plain-text tokens (from
/// the pre-F2 file format) load with `device_label = None` and
/// `last_seen = None` so an operator can still see + revoke them.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingTokenEntry {
    /// Raw token string. Never surfaced through the public API — only
    /// the fingerprint prefix and (optional) device label leave the
    /// daemon.
    token: String,
    /// Human-facing label, typically the `client_name` the device sent
    /// in its first successful `Hello` frame. `None` for legacy tokens
    /// loaded from the pre-F2 plain-text file format.
    #[serde(default)]
    device_label: Option<String>,
    /// Unix-seconds timestamp when this token was minted. Zero for
    /// legacy entries with no record on disk.
    #[serde(default)]
    created_at: i64,
    /// Unix-seconds timestamp of the most recent accepted `Hello` that
    /// presented this token. `None` until the first successful
    /// handshake.
    #[serde(default)]
    last_seen: Option<i64>,
}

impl PairingTokenEntry {
    fn new(token: String) -> Self {
        Self {
            token,
            device_label: None,
            created_at: unix_now(),
            last_seen: None,
        }
    }

    fn fingerprint_prefix(&self) -> String {
        fingerprint_prefix_for(&self.token)
    }

    fn to_summary(&self) -> PairingSummary {
        PairingSummary {
            device_label: self.device_label.clone(),
            last_seen: self.last_seen,
            fingerprint_prefix: self.fingerprint_prefix(),
            created_at: self.created_at,
        }
    }
}

/// Compute the 12-hex-char SHA-256 fingerprint prefix for a raw token.
///
/// Exposed at the module level so the dispatcher (and tests) can map a
/// revocation request's prefix back to a token without going through the
/// store API.
pub fn fingerprint_prefix_for(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(FINGERPRINT_PREFIX_LEN);
    for b in &digest[..(FINGERPRINT_PREFIX_LEN + 1) / 2] {
        out.push_str(&format!("{:02x}", b));
    }
    out.truncate(FINGERPRINT_PREFIX_LEN);
    out
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl PairingTokenStore {
    /// Load tokens from `config_dir/pairing-tokens` (if it exists) or
    /// initialise an empty store backed by that path.
    pub fn load(config_dir: &Path) -> std::io::Result<Self> {
        ensure_config_dir(config_dir)?;
        let path = config_dir.join(PAIRING_TOKENS_FILENAME);
        let tokens = if path.exists() {
            // We deliberately do NOT enforce strict permissions on the
            // tokens file the way `auth::devices.json` does — the
            // pairing token is a bootstrap secret, not a long-lived
            // capability, and operators frequently copy it across
            // machines using `scp` (which can preserve group/other
            // bits). We still write it `0o600`, but accept a wider
            // mode on read.
            let body = fs::read_to_string(&path)?;
            parse_tokens(&body)
        } else {
            HashMap::new()
        };
        Ok(Self {
            inner: Arc::new(RwLock::new(TokenInner {
                path: Some(path),
                tokens,
            })),
        })
    }

    /// In-memory store with no persistence. Used by tests.
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(RwLock::new(TokenInner {
                path: None,
                tokens: HashMap::new(),
            })),
        }
    }

    /// Mint a fresh pairing token, add it to the accepted set, and
    /// persist the updated set to disk (best-effort — a persist failure
    /// logs a warning but does not reject the mint). Returns the raw
    /// token; callers are expected to print it on stdout or surface it
    /// in a CLI subcommand.
    pub fn mint(&self) -> String {
        let token = generate_token();
        let entry = PairingTokenEntry::new(token.clone());
        {
            let mut guard = self.write();
            guard.tokens.insert(token.clone(), entry);
            if let Some(path) = guard.path.clone() {
                let snapshot: Vec<PairingTokenEntry> =
                    guard.tokens.values().cloned().collect();
                drop(guard);
                if let Err(err) = persist_tokens(&path, &snapshot) {
                    tracing::warn!(
                        error = %err,
                        path = %path.display(),
                        "could not persist pairing tokens; minted token kept in memory only"
                    );
                }
            }
        }
        token
    }

    /// Add an externally-supplied token (e.g. provisioned by an
    /// operator who already paired this host out-of-band). Returns
    /// `true` if the token was new.
    pub fn insert(&self, token: String) -> bool {
        let mut guard = self.write();
        if guard.tokens.contains_key(&token) {
            return false;
        }
        let entry = PairingTokenEntry::new(token.clone());
        guard.tokens.insert(token, entry);
        if let Some(path) = guard.path.clone() {
            let snapshot: Vec<PairingTokenEntry> =
                guard.tokens.values().cloned().collect();
            drop(guard);
            if let Err(err) = persist_tokens(&path, &snapshot) {
                tracing::warn!(error = %err, "could not persist pairing tokens");
            }
        }
        true
    }

    /// Constant-time check whether `candidate` matches any of the
    /// accepted tokens. Empty candidates always fail.
    pub fn verify(&self, candidate: &str) -> bool {
        if candidate.is_empty() {
            return false;
        }
        let guard = self.read();
        // Linear scan over a tiny set; `constant_time_eq` ensures
        // per-token comparisons don't leak which one matched via
        // timing.
        let mut hit = false;
        for known in guard.tokens.keys() {
            if constant_time_eq(known.as_bytes(), candidate.as_bytes()) {
                hit = true;
            }
        }
        hit
    }

    /// Stamp the most recent successful handshake for `candidate`. Also
    /// records the optional `device_label` from the same `Hello` frame
    /// when the entry doesn't yet have one (we keep the first label an
    /// operator confirms so an attacker can't silently rename the row
    /// after the fact by reconnecting with a different `client_name`).
    ///
    /// Best-effort persistence: a write failure logs a warning, the
    /// in-memory bookkeeping still applies. No-op when `candidate`
    /// doesn't match any active token (caller is expected to gate this
    /// on a previous `verify`).
    pub fn touch(&self, candidate: &str, device_label: Option<&str>) {
        if candidate.is_empty() {
            return;
        }
        let mut guard = self.write();
        let mut matched: Option<String> = None;
        for known in guard.tokens.keys() {
            if constant_time_eq(known.as_bytes(), candidate.as_bytes()) {
                matched = Some(known.clone());
            }
        }
        let Some(key) = matched else {
            return;
        };
        let now = unix_now();
        let path = guard.path.clone();
        let snapshot = {
            let entry = guard
                .tokens
                .get_mut(&key)
                .expect("just looked up matched key");
            entry.last_seen = Some(now);
            if entry.device_label.is_none() {
                if let Some(label) = device_label.map(str::trim).filter(|s| !s.is_empty())
                {
                    entry.device_label = Some(label.to_string());
                }
            }
            if entry.created_at == 0 {
                entry.created_at = now;
            }
            path.as_ref()
                .map(|_| guard.tokens.values().cloned().collect::<Vec<_>>())
        };
        if let (Some(path), Some(snapshot)) = (path, snapshot) {
            drop(guard);
            if let Err(err) = persist_tokens(&path, &snapshot) {
                tracing::warn!(error = %err, "could not persist pairing tokens after touch");
            }
        }
    }

    /// Snapshot of every accepted token, surfaced as a renderable
    /// [`PairingSummary`] (device label, last-seen, short fingerprint
    /// prefix). The raw token is never included.
    ///
    /// Sorted by `created_at` ascending, then by `fingerprint_prefix`,
    /// so the settings UI shows the oldest device first and revocation
    /// tests have a stable order.
    pub fn list(&self) -> Vec<PairingSummary> {
        let guard = self.read();
        let mut out: Vec<PairingSummary> = guard
            .tokens
            .values()
            .map(PairingTokenEntry::to_summary)
            .collect();
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.fingerprint_prefix.cmp(&b.fingerprint_prefix))
        });
        out
    }

    /// Revoke the token whose SHA-256 fingerprint matches the supplied
    /// `prefix` (matched against [`FINGERPRINT_PREFIX_LEN`] hex chars).
    ///
    /// Returns `true` if a token was removed. An empty / unknown prefix
    /// is a no-op. The store is re-persisted on success.
    pub fn revoke(&self, prefix: &str) -> bool {
        let trimmed = prefix.trim();
        if trimmed.is_empty() {
            return false;
        }
        let mut guard = self.write();
        let mut victim: Option<String> = None;
        for entry in guard.tokens.values() {
            if entry.fingerprint_prefix().starts_with(trimmed)
                || trimmed.starts_with(&entry.fingerprint_prefix())
            {
                victim = Some(entry.token.clone());
                break;
            }
        }
        let Some(key) = victim else {
            return false;
        };
        guard.tokens.remove(&key);
        if let Some(path) = guard.path.clone() {
            let snapshot: Vec<PairingTokenEntry> =
                guard.tokens.values().cloned().collect();
            drop(guard);
            if let Err(err) = persist_tokens(&path, &snapshot) {
                tracing::warn!(error = %err, "could not persist pairing tokens after revoke");
            }
        }
        true
    }

    /// Number of accepted tokens. Handy for tests and the boot log.
    pub fn len(&self) -> usize {
        self.read().tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, TokenInner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, TokenInner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

impl std::fmt::Debug for PairingTokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never expose tokens — even in Debug output we only surface
        // the cardinality.
        f.debug_struct("PairingTokenStore")
            .field("token_count", &self.len())
            .finish()
    }
}

/// Decision returned by [`evaluate_hello`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeOutcome {
    /// The connection passed the gate. `reason` is a short
    /// human-readable explanation suitable for the `HelloAck`
    /// (`"valid pairing token"`, `"trust-local (auth not required)"`,
    /// etc.).
    Accepted { reason: &'static str },
    /// The connection failed the gate. The caller should send
    /// `HelloAck { accepted: false, reason }` and then close the
    /// websocket.
    Rejected { reason: &'static str },
}

/// Evaluate a `Hello { token }` against the active pairing-token set.
///
/// * If `NEOISM_REQUIRE_AUTH=1` is set:
///   - a present, matching token  → `Accepted { reason: "valid pairing token" }`;
///   - everything else            → `Rejected { ... }`.
/// * If the gate env var is unset (or anything other than `"1"`):
///   the handshake degrades to "trust local" and is always accepted,
///   regardless of token presence. A future hardening step might
///   distinguish loopback peers from tailnet peers here; for now the
///   gate is purely opt-in.
pub fn evaluate_hello(
    token: Option<&str>,
    store: &PairingTokenStore,
) -> HandshakeOutcome {
    let required = require_auth_enabled();
    match (required, token) {
        (true, Some(t)) if store.verify(t) => HandshakeOutcome::Accepted {
            reason: "valid pairing token",
        },
        // The operator's static NEOISM_DAEMON_TOKEN is a first-class
        // Hello credential, not just the legacy `?token=` query check —
        // the desktop client refuses tokens in URLs (credentials never
        // enter the server registry), so the token FIELD it sends in
        // Hello was impossible to satisfy on daemons with an empty
        // pairing store (docker self-host with only a token env).
        (true, Some(t)) if daemon_token_matches(t) => HandshakeOutcome::Accepted {
            reason: "valid daemon token",
        },
        (true, Some(_)) => HandshakeOutcome::Rejected {
            reason: "invalid pairing token",
        },
        (true, None) => HandshakeOutcome::Rejected {
            reason: "pairing token required",
        },
        (false, _) => HandshakeOutcome::Accepted {
            reason: "trust-local (auth not required)",
        },
    }
}

/// Constant-time match against the operator-configured
/// `NEOISM_DAEMON_TOKEN`, when set and non-empty.
fn daemon_token_matches(candidate: &str) -> bool {
    match std::env::var("NEOISM_DAEMON_TOKEN") {
        Ok(expected) if !expected.trim().is_empty() => {
            constant_time_eq(expected.trim().as_bytes(), candidate.as_bytes())
        }
        _ => false,
    }
}

/// True when the operator has opted into mandatory pairing-token auth
/// via `NEOISM_REQUIRE_AUTH=1`. Any other value (including unset, "0",
/// "false") leaves the daemon in trust-local mode.
pub fn require_auth_enabled() -> bool {
    matches!(std::env::var(ENV_REQUIRE_AUTH).as_deref(), Ok("1"))
}

/// Resolve an inbound peer IP to a tailscale identity, best-effort.
///
/// This is logging-only. The function shells out to `tailscale whois
/// --json <ip>` and parses the `UserProfile.LoginName` field if
/// present. Any failure (missing binary, non-zero exit, parse miss,
/// timeout) returns `None` and the daemon proceeds as if the lookup
/// never happened.
///
/// Callers should never block on this — it's invoked from the websocket
/// upgrade path via [`tokio::task::spawn_blocking`] so a slow
/// `tailscale` binary doesn't stall the reactor.
pub fn tailscale_whois_blocking(ip: &str) -> Option<String> {
    use std::process::Command;
    use std::time::Duration;

    // Reject obviously-bogus inputs before paying for a subprocess.
    if ip.is_empty() || ip.contains(char::is_whitespace) {
        return None;
    }
    let mut cmd = Command::new("tailscale");
    cmd.arg("whois").arg("--json").arg(ip);
    // No native `Command` timeout in std; we trust `tailscale whois` to
    // return quickly (it's a local CLI call). The daemon spawns this
    // on a blocking task so a stuck child still doesn't break the
    // reactor — worst case the task lingers until the binary returns.
    let _ = Duration::from_secs(2); // documentation hint for future readers
    let output = match cmd.output() {
        Ok(out) => out,
        Err(_) => return None,
    };
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_tailscale_whois_json(&stdout)
}

/// Extract `UserProfile.LoginName` (preferred) or `Node.Name` from a
/// `tailscale whois --json` payload. Returns `None` for anything we
/// can't recognise so the caller logs an anonymous peer instead of
/// guessing.
fn parse_tailscale_whois_json(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(login) = value
        .get("UserProfile")
        .and_then(|u| u.get("LoginName"))
        .and_then(|n| n.as_str())
    {
        return Some(login.to_string());
    }
    if let Some(node) = value
        .get("Node")
        .and_then(|n| n.get("Name"))
        .and_then(|n| n.as_str())
    {
        return Some(node.to_string());
    }
    None
}

/// Pick the daemon's config directory. Honours
/// `$NEOISM_CONFIG_DIR` first (tests, custom installs), then
/// `$XDG_CONFIG_HOME`, then `~/.config/neoism`.
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("NEOISM_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    if let Some(d) = dirs::config_dir() {
        return d.join("neoism");
    }
    PathBuf::from(".").join(".neoism")
}

fn parse_tokens(body: &str) -> HashMap<String, PairingTokenEntry> {
    // The pre-F2 file format was one raw token per line; the new format
    // (introduced for the revocation UI) is one JSON object per line so
    // we can persist `device_label`/`last_seen`/`created_at` alongside.
    // Either format may appear in the same file during the migration —
    // we sniff each line independently.
    let mut out: HashMap<String, PairingTokenEntry> = HashMap::new();
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('{') {
            match serde_json::from_str::<PairingTokenEntry>(line) {
                Ok(entry) => {
                    out.insert(entry.token.clone(), entry);
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "skipping malformed pairing-token line"
                    );
                }
            }
        } else {
            // Legacy single-line format: just the raw token. Surface it
            // with no metadata so the UI still shows + revokes it.
            let entry = PairingTokenEntry {
                token: line.to_string(),
                device_label: None,
                created_at: 0,
                last_seen: None,
            };
            out.insert(entry.token.clone(), entry);
        }
    }
    out
}

fn persist_tokens(path: &Path, tokens: &[PairingTokenEntry]) -> std::io::Result<()> {
    // Atomic-ish replace: write tmp + rename. The tmp file is opened
    // 0o600 on unix so an intermediate copy never gets group/world
    // read bits.
    let tmp = path.with_extension("tokens.tmp");
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        let mut f = opts.open(&tmp)?;
        writeln!(
            f,
            "# neoism pairing tokens — one JSON object per line. Treat as a secret."
        )?;
        for entry in tokens {
            let serialized = serde_json::to_string(entry)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            writeln!(f, "{serialized}")?;
        }
        f.flush()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn ensure_config_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(dir)?;
        let mut perms = meta.permissions();
        if perms.mode() & 0o077 != 0 {
            perms.set_mode(0o700);
            let _ = fs::set_permissions(dir, perms);
        }
    }
    Ok(())
}

fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    // Prefix with `pair-` so a token leaking into logs/screenshots is
    // immediately recognisable as a pairing secret (and an operator
    // can `grep -F pair- ~/.bash_history` to spot accidental pastes).
    let mut out = String::with_capacity(5 + TOKEN_BYTES * 2);
    out.push_str("pair-");
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // `NEOISM_REQUIRE_AUTH` is process-global; serialize tests that
    // mutate it so they don't race each other (or the daemon tests in
    // the integration suite).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct RequireAuthGuard<'a> {
        _g: std::sync::MutexGuard<'a, ()>,
        prev: Option<String>,
    }

    impl<'a> RequireAuthGuard<'a> {
        fn enable() -> Self {
            let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var(ENV_REQUIRE_AUTH).ok();
            std::env::set_var(ENV_REQUIRE_AUTH, "1");
            Self { _g: g, prev }
        }
        fn disable() -> Self {
            let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var(ENV_REQUIRE_AUTH).ok();
            std::env::remove_var(ENV_REQUIRE_AUTH);
            Self { _g: g, prev }
        }
    }

    impl Drop for RequireAuthGuard<'_> {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(ENV_REQUIRE_AUTH, v),
                None => std::env::remove_var(ENV_REQUIRE_AUTH),
            }
        }
    }

    #[test]
    fn mint_token_starts_with_prefix_and_verifies() {
        let store = PairingTokenStore::in_memory();
        let token = store.mint();
        assert!(
            token.starts_with("pair-"),
            "token should be prefixed: {token}"
        );
        assert!(store.verify(&token));
        assert!(!store.verify("pair-bogus"));
        assert!(!store.verify(""));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn mint_persists_to_disk_and_reloads() {
        let dir = TempDir::new().unwrap();
        let store = PairingTokenStore::load(dir.path()).unwrap();
        let token = store.mint();
        // Force a reload from disk.
        let reloaded = PairingTokenStore::load(dir.path()).unwrap();
        assert!(reloaded.verify(&token));
    }

    #[test]
    fn evaluate_hello_accepts_when_auth_off() {
        let _g = RequireAuthGuard::disable();
        let store = PairingTokenStore::in_memory();
        // No token, no env, no problem: trust-local degradation.
        assert!(matches!(
            evaluate_hello(None, &store),
            HandshakeOutcome::Accepted { .. }
        ));
        // A token sent on a trust-local daemon is silently ignored
        // (still accepted, no rejection).
        assert!(matches!(
            evaluate_hello(Some("anything"), &store),
            HandshakeOutcome::Accepted { .. }
        ));
    }

    #[test]
    fn evaluate_hello_rejects_missing_token_when_required() {
        let _g = RequireAuthGuard::enable();
        let store = PairingTokenStore::in_memory();
        store.mint(); // make sure the store *could* accept *something*
        match evaluate_hello(None, &store) {
            HandshakeOutcome::Rejected { reason } => {
                assert!(reason.to_lowercase().contains("required"));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_hello_rejects_invalid_token_when_required() {
        let _g = RequireAuthGuard::enable();
        let store = PairingTokenStore::in_memory();
        let _good = store.mint();
        match evaluate_hello(Some("pair-totally-wrong"), &store) {
            HandshakeOutcome::Rejected { reason } => {
                assert!(reason.to_lowercase().contains("invalid"));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_hello_accepts_valid_token_when_required() {
        let _g = RequireAuthGuard::enable();
        let store = PairingTokenStore::in_memory();
        let token = store.mint();
        assert!(matches!(
            evaluate_hello(Some(&token), &store),
            HandshakeOutcome::Accepted { .. }
        ));
    }

    #[test]
    fn parse_tokens_skips_blank_and_comment_lines() {
        let body =
            "# header comment\n\npair-abc\n  pair-def  \n# another comment\npair-abc\n";
        let parsed = parse_tokens(body);
        assert!(parsed.contains_key("pair-abc"));
        assert!(parsed.contains_key("pair-def"));
        assert_eq!(parsed.len(), 2);
        // Legacy format -> no metadata.
        assert!(parsed.get("pair-abc").unwrap().device_label.is_none());
        assert_eq!(parsed.get("pair-abc").unwrap().created_at, 0);
    }

    #[test]
    fn fingerprint_prefix_is_deterministic_and_short() {
        let prefix = fingerprint_prefix_for("pair-deadbeef");
        assert_eq!(prefix.len(), FINGERPRINT_PREFIX_LEN);
        assert!(prefix.chars().all(|c| c.is_ascii_hexdigit()));
        // Same input → same prefix; different input → different prefix.
        assert_eq!(prefix, fingerprint_prefix_for("pair-deadbeef"));
        assert_ne!(prefix, fingerprint_prefix_for("pair-deadbeee"));
    }

    #[test]
    fn list_returns_summaries_without_raw_tokens() {
        let store = PairingTokenStore::in_memory();
        let token_a = store.mint();
        let token_b = store.mint();
        store.touch(&token_a, Some("laptop-a"));

        let summaries = store.list();
        assert_eq!(summaries.len(), 2);
        // Never expose raw tokens, even by accident through the summary.
        let serialized = serde_json::to_string(&summaries).unwrap();
        assert!(
            !serialized.contains(&token_a) && !serialized.contains(&token_b),
            "PairingSummary leaked a raw token: {serialized}"
        );
        // Touched entry shows the device label + last_seen.
        let touched = summaries
            .iter()
            .find(|s| s.fingerprint_prefix == fingerprint_prefix_for(&token_a))
            .expect("token a summary");
        assert_eq!(touched.device_label.as_deref(), Some("laptop-a"));
        assert!(touched.last_seen.is_some());
        let untouched = summaries
            .iter()
            .find(|s| s.fingerprint_prefix == fingerprint_prefix_for(&token_b))
            .expect("token b summary");
        assert!(untouched.device_label.is_none());
        assert!(untouched.last_seen.is_none());
    }

    #[test]
    fn revoke_removes_token_and_subsequent_verify_fails() {
        let store = PairingTokenStore::in_memory();
        let token = store.mint();
        let other = store.mint();
        assert!(store.verify(&token));
        assert!(store.verify(&other));

        let prefix = fingerprint_prefix_for(&token);
        assert!(store.revoke(&prefix));
        assert!(!store.verify(&token), "revoked token must fail verify");
        assert!(store.verify(&other), "non-target token must remain valid");
        assert_eq!(store.len(), 1);

        // Idempotent — revoking again yields false (already gone).
        assert!(!store.revoke(&prefix));
        // Empty / unknown prefix is a no-op.
        assert!(!store.revoke(""));
        assert!(!store.revoke("0000deadbeef"));
    }

    #[test]
    fn revoke_persists_across_reload() {
        let dir = TempDir::new().unwrap();
        let store = PairingTokenStore::load(dir.path()).unwrap();
        let keep = store.mint();
        let drop_me = store.mint();
        store.touch(&keep, Some("laptop-a"));
        store.touch(&drop_me, Some("phone"));

        // Snapshot summaries before revoke for shape comparison.
        let before = store.list();
        assert_eq!(before.len(), 2);

        let drop_prefix = fingerprint_prefix_for(&drop_me);
        assert!(store.revoke(&drop_prefix));

        // Reload from disk: revoked token must be gone, the surviving
        // token's device_label + last_seen must round-trip.
        let reloaded = PairingTokenStore::load(dir.path()).unwrap();
        assert!(reloaded.verify(&keep));
        assert!(!reloaded.verify(&drop_me));
        let after = reloaded.list();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].device_label.as_deref(), Some("laptop-a"));
        assert!(after[0].last_seen.is_some());
        assert_eq!(after[0].fingerprint_prefix, fingerprint_prefix_for(&keep));
    }

    #[test]
    fn touch_records_label_only_on_first_handshake() {
        let store = PairingTokenStore::in_memory();
        let token = store.mint();
        store.touch(&token, Some("first-label"));
        store.touch(&token, Some("attacker-rename"));
        let summary = &store.list()[0];
        assert_eq!(summary.device_label.as_deref(), Some("first-label"));
        // last_seen still updates even when the label is locked in.
        assert!(summary.last_seen.is_some());
    }

    #[test]
    fn legacy_plain_text_file_loads_then_round_trips_as_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(PAIRING_TOKENS_FILENAME);
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(&path, "# legacy header\npair-legacy123\n").unwrap();

        let store = PairingTokenStore::load(dir.path()).unwrap();
        assert!(store.verify("pair-legacy123"));
        let summaries = store.list();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].device_label.is_none());

        // Touching upgrades the file to JSON-per-line; reload sees the
        // metadata.
        store.touch("pair-legacy123", Some("import"));
        let reloaded = PairingTokenStore::load(dir.path()).unwrap();
        let summaries = reloaded.list();
        assert_eq!(summaries[0].device_label.as_deref(), Some("import"));
        assert!(summaries[0].last_seen.is_some());
    }

    #[test]
    fn parse_tailscale_whois_extracts_login_name() {
        let body =
            r#"{"UserProfile":{"LoginName":"you@tailnet"},"Node":{"Name":"laptop-a"}}"#;
        assert_eq!(
            parse_tailscale_whois_json(body),
            Some("you@tailnet".to_string())
        );
    }

    #[test]
    fn parse_tailscale_whois_falls_back_to_node_name() {
        let body = r#"{"Node":{"Name":"laptop-a"}}"#;
        assert_eq!(
            parse_tailscale_whois_json(body),
            Some("laptop-a".to_string())
        );
    }

    #[test]
    fn parse_tailscale_whois_returns_none_for_garbage() {
        assert_eq!(parse_tailscale_whois_json("not json"), None);
        assert_eq!(parse_tailscale_whois_json("{}"), None);
    }

    // ----------------------------------------------------------------
    // A2 integration-style coverage: drive the workspace dispatcher's
    // `Hello` arm end-to-end (env gate + store + reply shape +
    // disconnect signal). Lives in this module per the A2 spec; the
    // dispatcher itself is in `crate::workspace::handle`.
    // ----------------------------------------------------------------

    use crate::workspace::{
        self as workspace_handler, ConnectionWorkspace, WorkspaceManager,
    };
    use neoism_protocol::workspace::{WorkspaceClientMessage, WorkspaceServerMessage};

    fn dispatch_hello(
        store: &PairingTokenStore,
        token: Option<&str>,
    ) -> workspace_handler::DispatchOutcome {
        // `WorkspaceManager::bootstrap` reads/writes
        // `~/.local/share/neoism/workspaces.json`; the `Hello` arm
        // doesn't touch the manager so the side effects don't matter
        // here, but we still hand the dispatcher a real one to keep
        // the call signature stable.
        let manager = WorkspaceManager::bootstrap();
        let mut conn = ConnectionWorkspace::default();
        workspace_handler::handle(
            &manager,
            &mut conn,
            Some(store),
            None,
            WorkspaceClientMessage::Hello {
                token: token.map(str::to_string),
                client_name: Some("test-suite".to_string()),
                client_id: uuid::Uuid::nil(),
            },
        )
    }

    fn assert_hello_ack(
        outcome: &workspace_handler::DispatchOutcome,
        expect_accepted: bool,
    ) {
        assert_eq!(outcome.replies.len(), 1, "expected exactly one HelloAck");
        match &outcome.replies[0] {
            WorkspaceServerMessage::HelloAck {
                accepted, reason, ..
            } => {
                assert_eq!(*accepted, expect_accepted, "accepted flag mismatch");
                assert!(
                    reason.as_ref().map(|r| !r.is_empty()).unwrap_or(false),
                    "expected non-empty reason: {reason:?}"
                );
            }
            other => panic!("expected HelloAck, got {other:?}"),
        }
    }

    #[test]
    fn dispatcher_rejects_missing_token_when_auth_required() {
        let _g = RequireAuthGuard::enable();
        let store = PairingTokenStore::in_memory();
        let _ = store.mint();
        let outcome = dispatch_hello(&store, None);
        assert_hello_ack(&outcome, /*expect_accepted=*/ false);
        assert!(
            outcome.disconnect,
            "rejected handshake must signal disconnect"
        );
    }

    #[test]
    fn dispatcher_rejects_wrong_token_when_auth_required() {
        let _g = RequireAuthGuard::enable();
        let store = PairingTokenStore::in_memory();
        let _ = store.mint();
        let outcome = dispatch_hello(&store, Some("pair-totally-wrong"));
        assert_hello_ack(&outcome, /*expect_accepted=*/ false);
        assert!(outcome.disconnect);
    }

    #[test]
    fn dispatcher_accepts_valid_token_when_auth_required() {
        let _g = RequireAuthGuard::enable();
        let store = PairingTokenStore::in_memory();
        let token = store.mint();
        let outcome = dispatch_hello(&store, Some(&token));
        assert_hello_ack(&outcome, /*expect_accepted=*/ true);
        assert!(
            !outcome.disconnect,
            "accepted handshake must keep the connection alive"
        );
    }

    #[test]
    fn dispatcher_accepts_anything_in_legacy_mode() {
        // No env gate: trust-local degradation accepts every variant,
        // including the empty-token / wrong-token / no-token cases
        // that would otherwise be rejected. Mirrors how older clients
        // (which never learned to send `Hello`) keep working.
        let _g = RequireAuthGuard::disable();
        let store = PairingTokenStore::in_memory();
        for token in [None, Some(""), Some("pair-anything"), Some("garbage")] {
            let outcome = dispatch_hello(&store, token);
            assert_hello_ack(&outcome, /*expect_accepted=*/ true);
            assert!(
                !outcome.disconnect,
                "legacy mode must never disconnect on Hello (token={token:?})"
            );
        }
    }

    fn dispatch(
        store: Option<&PairingTokenStore>,
        msg: WorkspaceClientMessage,
    ) -> workspace_handler::DispatchOutcome {
        let manager = WorkspaceManager::bootstrap();
        let mut conn = ConnectionWorkspace::default();
        workspace_handler::handle(&manager, &mut conn, store, None, msg)
    }

    #[test]
    fn dispatcher_list_pairings_returns_summaries() {
        let store = PairingTokenStore::in_memory();
        let token = store.mint();
        store.touch(&token, Some("phone"));

        let outcome = dispatch(Some(&store), WorkspaceClientMessage::ListPairings);
        assert_eq!(outcome.replies.len(), 1);
        match &outcome.replies[0] {
            WorkspaceServerMessage::PairingList { pairings } => {
                assert_eq!(pairings.len(), 1);
                assert_eq!(pairings[0].device_label.as_deref(), Some("phone"));
                assert_eq!(
                    pairings[0].fingerprint_prefix,
                    fingerprint_prefix_for(&token)
                );
            }
            other => panic!("expected PairingList, got {other:?}"),
        }
        assert!(!outcome.disconnect);
    }

    #[test]
    fn dispatcher_revoke_pairing_drops_token_and_echoes_removed_true() {
        let store = PairingTokenStore::in_memory();
        let token = store.mint();
        let prefix = fingerprint_prefix_for(&token);

        let outcome = dispatch(
            Some(&store),
            WorkspaceClientMessage::RevokePairing {
                fingerprint_prefix: prefix.clone(),
            },
        );
        assert_eq!(outcome.replies.len(), 1);
        match &outcome.replies[0] {
            WorkspaceServerMessage::PairingRevoked {
                fingerprint_prefix,
                removed,
            } => {
                assert_eq!(fingerprint_prefix, &prefix);
                assert!(*removed, "expected removed=true for known prefix");
            }
            other => panic!("expected PairingRevoked, got {other:?}"),
        }
        assert!(!store.verify(&token), "token must be invalid after revoke");
    }

    #[test]
    fn dispatcher_revoke_pairing_unknown_prefix_reports_not_removed() {
        let store = PairingTokenStore::in_memory();
        let _ = store.mint();
        let outcome = dispatch(
            Some(&store),
            WorkspaceClientMessage::RevokePairing {
                fingerprint_prefix: "ffff00001111".into(),
            },
        );
        match &outcome.replies[0] {
            WorkspaceServerMessage::PairingRevoked { removed, .. } => {
                assert!(!*removed, "unknown prefix must report removed=false");
            }
            other => panic!("expected PairingRevoked, got {other:?}"),
        }
        assert_eq!(store.len(), 1, "store must be untouched");
    }

    #[test]
    fn dispatcher_list_pairings_without_store_returns_empty_list() {
        // In-process / legacy callers that pre-resolve the handshake
        // pass `pairing_tokens = None`. The settings panel still needs
        // a successful empty reply so it can render "no devices"
        // instead of erroring out.
        let outcome = dispatch(None, WorkspaceClientMessage::ListPairings);
        match &outcome.replies[0] {
            WorkspaceServerMessage::PairingList { pairings } => {
                assert!(pairings.is_empty());
            }
            other => panic!("expected PairingList, got {other:?}"),
        }
    }

    #[test]
    fn dispatcher_hello_touches_matched_token_so_list_shows_label() {
        let _g = RequireAuthGuard::enable();
        let store = PairingTokenStore::in_memory();
        let token = store.mint();

        // Accepted handshake; touch should populate device_label +
        // last_seen on the entry.
        let outcome = dispatch_hello(&store, Some(&token));
        assert_hello_ack(&outcome, /*expect_accepted=*/ true);

        let summaries = store.list();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].device_label.as_deref(), Some("test-suite"));
        assert!(summaries[0].last_seen.is_some());
    }
}
