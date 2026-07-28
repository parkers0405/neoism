//! Standalone-daemon token bootstrap (Wave 1B / Phase 0).
//!
//! The desktop's embedded daemon mints/loads `NEOISM_DAEMON_TOKEN` at
//! spawn time (see `neoism-frontend/desktop/src/embedded_daemon.rs`).
//! The **standalone** binary never did, so `auth::verify` had no token
//! to check: a non-loopback bind with `NEOISM_REQUIRE_AUTH=1` could not
//! be reached because the legacy `?token=` escape hatch had nothing to
//! compare against, and a release build with no token rejects every
//! upgrade outright (`auth::verify` → `AuthError::Missing`).
//!
//! This module mirrors the embedded logic so the two daemons share the
//! same on-disk token file:
//!
//!   * Path: Unix uses `$XDG_RUNTIME_DIR/neoism/daemon-token`
//!     (or `/tmp/neoism-$UID/daemon-token` when `XDG_RUNTIME_DIR` is
//!     unset). Windows uses the user's data directory under
//!     `neoism-daemon/daemon-token`.
//!     Unix parent dirs/files are tightened to `0o700`/`0o600`.
//!   * Env var: `NEOISM_DAEMON_TOKEN` — set in-process if absent so the
//!     legacy `?token=` upgrade path (`auth::verify`) has something to
//!     compare against.
//!
//! We only ever log the **path**, never the token value, so the secret
//! never leaks into logs/journald. Operators read the token from the
//! `0o600` file themselves.
//!
//! Sharing the same file with the embedded daemon is intentional: a
//! desktop client on the same host that already minted a token can
//! reach a later-started standalone daemon with the value it already
//! has, and vice-versa.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// Environment variable the legacy `?token=` upgrade path verifies
/// against (see [`crate::auth::verify`]).
pub const DAEMON_TOKEN_ENV: &str = "NEOISM_DAEMON_TOKEN";

/// Ensure `NEOISM_DAEMON_TOKEN` is set for the rest of the process.
///
/// If the env var is already present (operator-provisioned, or inherited
/// from a parent that ran the embedded daemon) we leave it untouched.
/// Otherwise we load the persisted token from
/// `$XDG_RUNTIME_DIR/neoism/daemon-token` or mint + persist a fresh one,
/// then export it in-process.
///
/// Returns the path the token lives at (for logging). On any disk
/// failure we still set a process-local token so the daemon comes up
/// reachable — we just can't persist it across restarts in that case.
pub fn ensure_daemon_token() -> PathBuf {
    let path = daemon_token_path();
    if std::env::var_os(DAEMON_TOKEN_ENV).is_some() {
        // Already provisioned by the operator or a parent process; do
        // not clobber it. Still report the canonical path so the log
        // line is informative either way.
        return path;
    }

    let token = match load_or_create_daemon_token(&path) {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(
                error = %error,
                path = %path.display(),
                "could not load/persist daemon token file; using a process-local token (not persisted across restarts)",
            );
            generate_daemon_token()
        }
    };
    std::env::set_var(DAEMON_TOKEN_ENV, token);
    path
}

/// Load the token from `path` (trimming trailing newline) or mint a new
/// one and persist it `0o600`. Races (two daemons starting at once) are
/// resolved by re-reading the winner's file on `AlreadyExists`.
fn load_or_create_daemon_token(path: &PathBuf) -> io::Result<String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        // Tighten the runtime sub-dir to 0o700 — best-effort; the file
        // itself is the real gate at 0o600.
        #[cfg(unix)]
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        // Same tightening on Windows: owner+SYSTEM-only DACL. Warn-and-go on
        // failure — ACL-less filesystems must not block token bootstrap.
        #[cfg(windows)]
        if let Err(error) = crate::windows_acl::harden_owner_only(parent) {
            tracing::warn!(
                error = %error,
                path = %parent.display(),
                "could not tighten daemon token dir ACL; continuing",
            );
        }
    }

    match fs::read_to_string(path) {
        Ok(existing) => {
            let existing = existing.trim().to_string();
            if !existing.is_empty() {
                return Ok(existing);
            }
            // Empty file — fall through to mint a fresh token below.
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let token = generate_daemon_token();
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            #[cfg(unix)]
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            #[cfg(windows)]
            if let Err(error) = crate::windows_acl::harden_owner_only(path) {
                tracing::warn!(
                    error = %error,
                    path = %path.display(),
                    "could not tighten daemon token file ACL; continuing",
                );
            }
            file.write_all(token.as_bytes())?;
            file.write_all(b"\n")?;
            Ok(token)
        }
        // Lost a startup race: another daemon created the file between
        // our read and our create. Adopt whatever it wrote.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read_to_string(path)?;
            let existing = existing.trim().to_string();
            if existing.is_empty() {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("daemon token file is empty: {}", path.display()),
                ))
            } else {
                Ok(existing)
            }
        }
        Err(error) => Err(error),
    }
}

/// Two concatenated UUIDs → a 64-hex-char opaque secret. Matches the
/// embedded daemon's generator so a token minted by either side is
/// indistinguishable on the wire.
fn generate_daemon_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Resolve the on-disk token path.
pub fn daemon_token_path() -> PathBuf {
    #[cfg(unix)]
    {
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

    #[cfg(windows)]
    {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("neoism-daemon")
            .join("daemon-token")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    // `XDG_RUNTIME_DIR` + `NEOISM_DAEMON_TOKEN` are process-global; the
    // tests that mutate them must not race each other.
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    struct EnvScope {
        prev_runtime: Option<std::ffi::OsString>,
        prev_token: Option<std::ffi::OsString>,
    }

    impl EnvScope {
        fn capture() -> Self {
            Self {
                prev_runtime: std::env::var_os("XDG_RUNTIME_DIR"),
                prev_token: std::env::var_os(DAEMON_TOKEN_ENV),
            }
        }
    }

    impl Drop for EnvScope {
        fn drop(&mut self) {
            match &self.prev_runtime {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
            match &self.prev_token {
                Some(v) => std::env::set_var(DAEMON_TOKEN_ENV, v),
                None => std::env::remove_var(DAEMON_TOKEN_ENV),
            }
        }
    }

    #[test]
    fn token_path_uses_xdg_runtime_dir_when_set() {
        let _guard = env_lock();
        let _scope = EnvScope::capture();
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/test");
        assert_eq!(
            daemon_token_path(),
            PathBuf::from("/run/user/test/neoism/daemon-token")
        );
    }

    #[test]
    fn token_path_falls_back_to_tmp_when_unset() {
        let _guard = env_lock();
        let _scope = EnvScope::capture();
        std::env::remove_var("XDG_RUNTIME_DIR");
        let path = daemon_token_path();
        let s = path.to_string_lossy();
        assert!(
            s.contains("neoism-") && s.ends_with("daemon-token"),
            "unexpected fallback token path: {s}"
        );
    }

    #[test]
    fn ensure_mints_persists_and_reloads_same_token() {
        let _guard = env_lock();
        let _scope = EnvScope::capture();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        std::env::remove_var(DAEMON_TOKEN_ENV);

        // First call mints + persists + exports.
        let path = ensure_daemon_token();
        assert!(path.exists(), "token file should be written: {path:?}");
        let minted = std::env::var(DAEMON_TOKEN_ENV).expect("env exported");
        assert!(!minted.is_empty());

        // File is 0o600.
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "token file must be 0o600, got {mode:o}");

        // The on-disk value must match the exported env (trailing
        // newline trimmed).
        let on_disk = fs::read_to_string(&path).unwrap().trim().to_string();
        assert_eq!(on_disk, minted);

        // A fresh process (env cleared) loads the same persisted token.
        std::env::remove_var(DAEMON_TOKEN_ENV);
        let path2 = ensure_daemon_token();
        assert_eq!(path2, path);
        assert_eq!(std::env::var(DAEMON_TOKEN_ENV).unwrap(), minted);
    }

    #[test]
    fn ensure_does_not_clobber_preexisting_env_token() {
        let _guard = env_lock();
        let _scope = EnvScope::capture();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        std::env::set_var(DAEMON_TOKEN_ENV, "operator-provisioned");

        let _path = ensure_daemon_token();
        assert_eq!(
            std::env::var(DAEMON_TOKEN_ENV).unwrap(),
            "operator-provisioned",
            "pre-set token must not be overwritten",
        );
    }
}
