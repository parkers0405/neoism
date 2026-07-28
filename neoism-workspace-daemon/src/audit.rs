//! Append-only audit log for the daemon.
//!
//! Every authenticated action (session open, PTY create, file write, git op,
//! device claim, device revoke, ...) is funneled through [`AuditLog::record`]
//! which serialises one JSON object per line to `audit.log` inside the data
//! directory.
//!
//! Hard rules:
//! * Raw tokens and pairing codes **never** appear in the log. Callers pass a
//!   `device_id` (a public, non-sensitive uuid); only the token's id prefix
//!   (first 8 chars of its sha256 hash) is ever persisted by the auth layer.
//! * `args_summary` is also free-form, but call sites scrub file contents
//!   and token strings before invoking this module.
//! * Writes are serialised through a `Mutex<File>` so concurrent tasks
//!   produce well-formed JSONL.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Result of a recorded action — mirrors the rough taxonomy that the audit
/// reader cares about (UI, alerting, etc.). Free-form `Error` carries a
/// short reason string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditResult {
    Success,
    Denied,
    Error { reason: String },
}

/// One row of the audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix timestamp seconds.
    pub ts: i64,
    /// `None` for unauthenticated actions (e.g. an inbound pair-code request
    /// before any device exists).
    pub device_id: Option<String>,
    pub action: String,
    pub args_summary: String,
    pub result: AuditResult,
}

/// Handle to an audit log file. Cheap to clone — internally `Arc<Mutex<...>>`.
#[derive(Clone)]
pub struct AuditLog {
    inner: Arc<AuditLogInner>,
}

struct AuditLogInner {
    path: PathBuf,
    file: Mutex<File>,
}

impl AuditLog {
    /// Open (or create) the audit log at `<data_dir>/audit.log`.
    pub fn open(data_dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("audit.log");
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(&path)?;
        // Windows counterpart of the 0o600 open mode above. Warn-and-go —
        // an ACL-less filesystem must not disable auditing entirely.
        #[cfg(windows)]
        if let Err(error) = crate::windows_acl::harden_owner_only(&path) {
            tracing::warn!(
                error = %error,
                path = %path.display(),
                "could not tighten audit log ACL; continuing",
            );
        }
        Ok(Self {
            inner: Arc::new(AuditLogInner {
                path,
                file: Mutex::new(file),
            }),
        })
    }

    /// Path the log is being written to (useful for diagnostics / tests).
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Record one entry. Returns an io error only if disk write fails; the
    /// caller should usually log-and-continue rather than abort.
    pub fn record(&self, entry: AuditEntry) -> std::io::Result<()> {
        let line = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
        let mut file = match self.inner.file.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }

    /// Convenience: stamp `ts` to now and record.
    pub fn record_now(
        &self,
        device_id: Option<&str>,
        action: &str,
        args_summary: &str,
        result: AuditResult,
    ) -> std::io::Result<()> {
        self.record(AuditEntry {
            ts: OffsetDateTime::now_utc().unix_timestamp(),
            device_id: device_id.map(|s| s.to_string()),
            action: action.to_string(),
            args_summary: args_summary.to_string(),
            result,
        })
    }

    /// Read back all rows. Used by tests and (future) "Active sessions" UI.
    pub fn read_all(&self) -> std::io::Result<Vec<AuditEntry>> {
        let bytes = std::fs::read(&self.inner.path)?;
        let mut out = Vec::new();
        for line in bytes.split(|b| *b == b'\n') {
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<AuditEntry>(line) {
                Ok(e) => out.push(e),
                Err(err) => {
                    tracing::warn!(error = %err, "skipping malformed audit row");
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_and_read_roundtrip() {
        let dir = TempDir::new().unwrap();
        let log = AuditLog::open(dir.path()).unwrap();
        log.record_now(Some("dev-1"), "list_sessions", "{}", AuditResult::Success)
            .unwrap();
        log.record_now(None, "pair_claim", "code_id=abcd1234", AuditResult::Denied)
            .unwrap();
        let rows = log.read_all().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].action, "list_sessions");
        assert_eq!(rows[0].device_id.as_deref(), Some("dev-1"));
        assert_eq!(rows[1].action, "pair_claim");
        assert_eq!(rows[1].result, AuditResult::Denied);
    }
}
