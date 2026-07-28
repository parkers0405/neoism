//! Token-based authentication for daemon connections.
//!
//! Two layers live here:
//!
//! 1. The legacy single-token verifier ([`verify`]) introduced in Phase 7,
//!    which compares an inbound `?token=` query against `NEOISM_DAEMON_TOKEN`.
//!    The websocket upgrade in `server.rs` still uses this — it's the
//!    "operator-on-this-machine" escape hatch.
//!
//! 2. The Phase 10 device registry: every paired remote device gets a
//!    long-lived `DeviceToken`. Tokens are 256-bit random values; only the
//!    sha256 hash hits disk. On verification we hash the inbound bearer and
//!    constant-time-compare against the stored hash.
//!
//! All on-disk artefacts live under [`data_dir`] — XDG data home if set,
//! otherwise `~/.local/share/neoism-daemon`. The directory is created with
//! `0o700` permissions on unix; the registry file is `0o600`. We refuse to
//! load a registry that is group/world writable.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use neoism_protocol::auth::{constant_time_eq, AuthError};
use neoism_protocol::pairing::Permission;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

/// One paired device. The raw token never lives on disk — only `token_hash`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub device_id: String,
    pub device_label: String,
    /// Hex-encoded sha256 of the raw device token.
    pub token_hash: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub granted_permissions: BTreeSet<Permission>,
}

impl DeviceRecord {
    /// Public, log-safe identifier for the token — first 8 chars of hash.
    /// Useful for cross-referencing audit entries without ever touching
    /// the raw bearer.
    pub fn token_id(&self) -> &str {
        let n = self.token_hash.len().min(8);
        &self.token_hash[..n]
    }
}

/// On-disk shape of the device registry.
#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    /// Schema version — bump on incompatible changes.
    #[serde(default = "default_version")]
    version: u32,
    devices: Vec<DeviceRecord>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("registry io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("registry parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("registry path is writable by group/world; refusing to load: {0}")]
    PermissionsTooOpen(PathBuf),
}

/// Cloneable handle to the device registry.
#[derive(Clone)]
pub struct DeviceRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

struct RegistryInner {
    path: PathBuf,
    records: Vec<DeviceRecord>,
}

impl DeviceRegistry {
    /// Load (or initialise) the registry at `<data_dir>/devices.json`.
    pub fn load(data_dir: &Path) -> Result<Self, RegistryError> {
        ensure_data_dir(data_dir)?;
        let path = data_dir.join("devices.json");
        let records = if path.exists() {
            check_file_permissions(&path)?;
            let bytes = fs::read(&path)?;
            if bytes.is_empty() {
                Vec::new()
            } else {
                let parsed: RegistryFile = serde_json::from_slice(&bytes)?;
                parsed.devices
            }
        } else {
            Vec::new()
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(RegistryInner { path, records })),
        })
    }

    /// Issue a fresh device token, write it to disk, and return the raw
    /// token *once* (caller is responsible for handing it to the client).
    pub fn issue(
        &self,
        device_label: &str,
        granted_permissions: BTreeSet<Permission>,
    ) -> Result<IssuedDevice, RegistryError> {
        let raw_token = generate_token();
        let token_hash = hash_token(&raw_token);
        let device_id = uuid::Uuid::new_v4().to_string();
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let record = DeviceRecord {
            device_id: device_id.clone(),
            device_label: device_label.to_string(),
            token_hash: token_hash.clone(),
            created_at: now,
            last_seen: now,
            granted_permissions: granted_permissions.clone(),
        };
        {
            let mut guard = self.lock();
            guard.records.push(record);
            persist(&guard.path, &guard.records)?;
        }
        tracing::info!(
            %device_id,
            token_id = %token_hash[..8.min(token_hash.len())],
            "issued new device token"
        );
        Ok(IssuedDevice {
            device_id,
            raw_token,
            granted_permissions,
        })
    }

    /// Verify an inbound `Authorization: Bearer <token>` header. Returns the
    /// device record on success and updates its `last_seen` timestamp.
    pub fn verify_bearer(&self, raw_token: &str) -> Result<DeviceRecord, AuthError> {
        if raw_token.is_empty() {
            return Err(AuthError::Missing);
        }
        let inbound_hash = hash_token(raw_token);
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let mut guard = self.lock();
        // Iterate so we can do a constant-time compare per record. We
        // deliberately don't short-circuit the loop on a hit; total cost
        // scales linearly with the registry size, which is fine for the
        // expected handful of paired devices.
        let mut hit_idx: Option<usize> = None;
        for (idx, rec) in guard.records.iter().enumerate() {
            if constant_time_eq(rec.token_hash.as_bytes(), inbound_hash.as_bytes())
                && hit_idx.is_none()
            {
                hit_idx = Some(idx);
            }
        }
        let Some(idx) = hit_idx else {
            return Err(AuthError::Invalid);
        };
        guard.records[idx].last_seen = now;
        // Best-effort persist; verification still succeeds even if disk write
        // fails (we already authenticated).
        if let Err(err) = persist(&guard.path, &guard.records) {
            tracing::warn!(error = %err, "could not persist updated last_seen");
        }
        Ok(guard.records[idx].clone())
    }

    /// Remove the device identified by `device_id` from the registry.
    /// Returns `true` if a record was removed.
    pub fn revoke(&self, device_id: &str) -> Result<bool, RegistryError> {
        let mut guard = self.lock();
        let before = guard.records.len();
        guard.records.retain(|r| r.device_id != device_id);
        let removed = guard.records.len() < before;
        if removed {
            persist(&guard.path, &guard.records)?;
            tracing::info!(%device_id, "revoked device");
        }
        Ok(removed)
    }

    /// Snapshot of every paired device (cloned). Suitable for `GET /devices`.
    pub fn list(&self) -> Vec<DeviceRecord> {
        let guard = self.lock();
        guard.records.clone()
    }

    /// Number of paired devices — handy in tests.
    pub fn len(&self) -> usize {
        self.lock().records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryInner> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Returned exactly once from `DeviceRegistry::issue` — the only place the
/// raw token is exposed.
#[derive(Debug)]
pub struct IssuedDevice {
    pub device_id: String,
    pub raw_token: String,
    pub granted_permissions: BTreeSet<Permission>,
}

/// Generate a 256-bit random token, base64-url-encoded for transport.
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    // Plain hex avoids pulling in a base64 dep here — we already have one
    // in the workspace, but `neoism-workspace-daemon`'s deps don't include
    // it. Hex is fine: 64 chars, opaque, transportable as HTTP header.
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// sha256(token) → lowercase hex. Hex (not base64) so equality comparisons
/// remain byte-comparable.
fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    let digest = h.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn persist(path: &Path, records: &[DeviceRecord]) -> Result<(), RegistryError> {
    let payload = RegistryFile {
        version: 1,
        devices: records.to_vec(),
    };
    let body = serde_json::to_vec_pretty(&payload)?;

    // Atomic-ish replace: write to tmp + rename. Mode 0o600 on the tmp file.
    let tmp = path.with_extension("json.tmp");
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        let mut f: File = opts.open(&tmp)?;
        // Windows counterpart of the 0o600 open mode: owner+SYSTEM-only DACL
        // on the tmp file before the secrets land in it; the rename below
        // carries the descriptor to the final path. Warn-and-go on failure —
        // ACL-less filesystems must not fail the write.
        #[cfg(windows)]
        if let Err(error) = crate::windows_acl::harden_owner_only(&tmp) {
            tracing::warn!(
                error = %error,
                path = %tmp.display(),
                "could not tighten device registry ACL; continuing",
            );
        }
        f.write_all(&body)?;
        f.flush()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn ensure_data_dir(dir: &Path) -> Result<(), RegistryError> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(dir)?.permissions();
        if perms.mode() & 0o077 != 0 {
            // Tighten — best-effort.
            let mut p = perms.clone();
            p.set_mode(0o700);
            let _ = fs::set_permissions(dir, p);
        }
    }
    #[cfg(windows)]
    if let Err(error) = crate::windows_acl::harden_owner_only(dir) {
        tracing::warn!(
            error = %error,
            path = %dir.display(),
            "could not tighten daemon data dir ACL; continuing",
        );
    }
    Ok(())
}

#[cfg(unix)]
fn check_file_permissions(path: &Path) -> Result<(), RegistryError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path)?;
    let mode = meta.permissions().mode();
    // Reject if group or other has any bits set.
    if mode & 0o077 != 0 {
        return Err(RegistryError::PermissionsTooOpen(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_file_permissions(path: &Path) -> Result<(), RegistryError> {
    // Mirror the unix semantics on Windows: a DACL that lets anyone beyond
    // the owner, SYSTEM, or Administrators read the file is rejected exactly
    // like group/other mode bits above. A failure to *read* the ACL only
    // warns — FAT32/ExFAT cannot express one, and erroring there would brick
    // startup on such filesystems.
    #[cfg(windows)]
    match crate::windows_acl::check_owner_only(path) {
        Ok(true) => {}
        Ok(false) => return Err(RegistryError::PermissionsTooOpen(path.to_path_buf())),
        Err(error) => {
            tracing::warn!(
                error = %error,
                path = %path.display(),
                "could not read device registry ACL; skipping permission check",
            );
        }
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

/// Pick the data directory for daemon state. Honours `$NEOISM_DAEMON_DATA_DIR`
/// first (tests), then `$XDG_DATA_HOME`, then `~/.local/share/...`.
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("NEOISM_DAEMON_DATA_DIR") {
        return PathBuf::from(p);
    }
    if let Some(d) = dirs::data_dir() {
        return d.join("neoism-daemon");
    }
    PathBuf::from(".").join(".neoism-daemon")
}

// ---------------------------------------------------------------------------
// Legacy `?token=` verifier — left intact so the existing websocket upgrade
// keeps working. New routes use `verify_bearer` instead.
// ---------------------------------------------------------------------------

/// Verify a token presented via `?token=` against `NEOISM_DAEMON_TOKEN`.
pub fn verify(provided: Option<&str>) -> Result<(), AuthError> {
    match std::env::var("NEOISM_DAEMON_TOKEN") {
        Ok(expected) if !expected.is_empty() => {
            let presented = provided.ok_or(AuthError::Missing)?;
            if constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
                Ok(())
            } else {
                Err(AuthError::Invalid)
            }
        }
        _ => {
            if cfg!(debug_assertions) {
                tracing::warn!(
                    "NEOISM_DAEMON_TOKEN not set; allowing connection (debug build only)"
                );
                Ok(())
            } else {
                tracing::error!(
                    "NEOISM_DAEMON_TOKEN not set in release build; rejecting connection"
                );
                Err(AuthError::Missing)
            }
        }
    }
}

/// Convenience: ensure the verified device has every permission in `needed`.
pub fn require_permissions(
    rec: &DeviceRecord,
    needed: &[Permission],
) -> Result<(), AuthError> {
    for p in needed {
        if !rec.granted_permissions.contains(p) {
            return Err(AuthError::PermissionDenied(perm_name(*p)));
        }
    }
    Ok(())
}

fn perm_name(p: Permission) -> &'static str {
    match p {
        Permission::ReadFiles => "ReadFiles",
        Permission::WriteFiles => "WriteFiles",
        Permission::GitWrite => "GitWrite",
        Permission::PtyCreate => "PtyCreate",
        Permission::DeviceManage => "DeviceManage",
    }
}

// ---------------------------------------------------------------------------
// AuthService: orchestrator that wires registry + pairing + audit + perms.
// ---------------------------------------------------------------------------

use neoism_protocol::pairing::{
    PairClaimRequest, PairClaimResponse, PairingCodeResponse,
};

use crate::audit::{AuditLog, AuditResult};
use crate::pairing::{ClaimOutcome, PairingCodeStore};
use crate::permissions::{evaluate as evaluate_approval, ApprovalDecision};

/// One-stop daemon-side auth surface. Cheap to clone; everything inside is
/// arc-shared.
#[derive(Clone)]
pub struct AuthService {
    pub registry: DeviceRegistry,
    pub pairing: PairingCodeStore,
    pub audit: AuditLog,
}

impl AuthService {
    /// Stand up the service rooted at `data_dir`. Creates the dir, opens
    /// the audit log, loads the registry. Fails fast on permission issues.
    pub fn bootstrap(data_dir: &Path) -> Result<Self, RegistryError> {
        let registry = DeviceRegistry::load(data_dir)?;
        let audit = AuditLog::open(data_dir).map_err(RegistryError::Io)?;
        Ok(Self {
            registry,
            pairing: PairingCodeStore::new(),
            audit,
        })
    }

    /// Mint a pairing code. The caller (typically the local operator via the
    /// `pair` subcommand or localhost-bound HTTP route) decides which
    /// permissions the eventual claim should be allowed to request.
    pub fn mint_pairing_code(
        &self,
        requested: BTreeSet<Permission>,
    ) -> PairingCodeResponse {
        let resp = self.pairing.mint(requested);
        let _ = self.audit.record_now(
            None,
            "pair_mint",
            &format!("code_id={}", crate::pairing::code_id(&resp.code)),
            AuditResult::Success,
        );
        resp
    }

    /// Redeem a pairing code for a device token. Drives the
    /// operator-approval gate; in auto-approve mode this is one-shot.
    pub fn claim_pairing(&self, req: PairClaimRequest) -> PairClaimResponse {
        let code_id = crate::pairing::code_id(&req.code);
        let outcome = self.pairing.claim(&req.code);
        let requested = match outcome {
            ClaimOutcome::Ok {
                requested_permissions,
            } => {
                // Merge what the mint side declared with what the claim is
                // asking for. The eventual grant is the *intersection* with
                // the operator's decision below.
                let mut merged = requested_permissions;
                merged.extend(req.requested_permissions.iter().copied());
                merged
            }
            ClaimOutcome::Expired => {
                let _ = self.audit.record_now(
                    None,
                    "pair_claim",
                    &format!("code_id={code_id}; reason=expired"),
                    AuditResult::Denied,
                );
                return PairClaimResponse::Rejected {
                    reason: "code expired".into(),
                };
            }
            ClaimOutcome::Unknown => {
                let _ = self.audit.record_now(
                    None,
                    "pair_claim",
                    &format!("code_id={code_id}; reason=unknown_or_used"),
                    AuditResult::Denied,
                );
                return PairClaimResponse::Rejected {
                    reason: "code already used".into(),
                };
            }
        };

        match evaluate_approval(&requested) {
            ApprovalDecision::Granted(granted) => {
                match self.registry.issue(&req.device_label, granted.clone()) {
                    Ok(issued) => {
                        let _ = self.audit.record_now(
                            Some(&issued.device_id),
                            "pair_claim",
                            &format!(
                                "code_id={code_id}; label={}; perms={}",
                                redact_label(&req.device_label),
                                granted.len()
                            ),
                            AuditResult::Success,
                        );
                        PairClaimResponse::Granted {
                            device_id: issued.device_id,
                            device_token: issued.raw_token,
                            granted_permissions: issued.granted_permissions,
                        }
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "failed to persist new device");
                        let _ = self.audit.record_now(
                            None,
                            "pair_claim",
                            &format!("code_id={code_id}; reason=persist_failed"),
                            AuditResult::Error {
                                reason: err.to_string(),
                            },
                        );
                        PairClaimResponse::Rejected {
                            reason: "internal error".into(),
                        }
                    }
                }
            }
            ApprovalDecision::Pending => {
                let _ = self.audit.record_now(
                    None,
                    "pair_claim",
                    &format!("code_id={code_id}; reason=awaiting_operator"),
                    AuditResult::Denied,
                );
                PairClaimResponse::Pending
            }
            ApprovalDecision::Rejected(reason) => {
                let _ = self.audit.record_now(
                    None,
                    "pair_claim",
                    &format!("code_id={code_id}; reason=operator_denied"),
                    AuditResult::Denied,
                );
                PairClaimResponse::Rejected { reason }
            }
        }
    }

    /// Authenticate an inbound bearer token. Logs the attempt either way.
    pub fn authenticate_bearer(
        &self,
        raw_token: &str,
    ) -> Result<DeviceRecord, AuthError> {
        match self.registry.verify_bearer(raw_token) {
            Ok(rec) => {
                let _ = self.audit.record_now(
                    Some(&rec.device_id),
                    "auth_bearer",
                    &format!("token_id={}", rec.token_id()),
                    AuditResult::Success,
                );
                Ok(rec)
            }
            Err(err) => {
                let _ = self.audit.record_now(
                    None,
                    "auth_bearer",
                    "token_id=<unknown>",
                    AuditResult::Denied,
                );
                Err(err)
            }
        }
    }

    /// Revoke a device. Requires the caller to already have proved they hold
    /// the `DeviceManage` permission — enforce that at the route layer.
    pub fn revoke_device(
        &self,
        acting_device_id: Option<&str>,
        target_device_id: &str,
    ) -> Result<bool, RegistryError> {
        let removed = self.registry.revoke(target_device_id)?;
        let result = if removed {
            AuditResult::Success
        } else {
            AuditResult::Denied
        };
        let _ = self.audit.record_now(
            acting_device_id,
            "device_revoke",
            &format!("target={target_device_id}"),
            result,
        );
        Ok(removed)
    }
}

/// Replace a free-form label with a short summary suitable for audit. We
/// keep the first ~32 chars but never the full string, in case the client
/// stuffed sensitive info in there.
fn redact_label(label: &str) -> String {
    const MAX: usize = 32;
    if label.len() <= MAX {
        label.to_string()
    } else {
        format!("{}...", &label[..MAX])
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // AuthService tests touch `NEOISM_AUTO_APPROVE`; serialize them so they
    // don't race other env-touching tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct AutoApproveGuard<'a> {
        _g: std::sync::MutexGuard<'a, ()>,
        prev: Option<String>,
    }
    impl<'a> AutoApproveGuard<'a> {
        fn enable() -> Self {
            let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("NEOISM_AUTO_APPROVE").ok();
            std::env::set_var("NEOISM_AUTO_APPROVE", "true");
            Self { _g: g, prev }
        }
        fn disable() -> Self {
            let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("NEOISM_AUTO_APPROVE").ok();
            std::env::remove_var("NEOISM_AUTO_APPROVE");
            Self { _g: g, prev }
        }
    }
    impl Drop for AutoApproveGuard<'_> {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("NEOISM_AUTO_APPROVE", v),
                None => std::env::remove_var("NEOISM_AUTO_APPROVE"),
            }
        }
    }

    #[test]
    fn end_to_end_pairing_flow_with_auto_approve() {
        let _g = AutoApproveGuard::enable();
        let dir = TempDir::new().unwrap();
        let svc = AuthService::bootstrap(dir.path()).unwrap();
        let code = svc.mint_pairing_code(BTreeSet::from([Permission::ReadFiles]));
        let mut requested = BTreeSet::new();
        requested.insert(Permission::ReadFiles);
        let resp = svc.claim_pairing(PairClaimRequest {
            code: code.code.clone(),
            device_label: "phone".into(),
            requested_permissions: requested,
        });
        let token = match resp {
            PairClaimResponse::Granted { device_token, .. } => device_token,
            other => panic!("expected Granted, got {other:?}"),
        };
        let rec = svc.authenticate_bearer(&token).expect("verifies");
        assert!(rec.granted_permissions.contains(&Permission::ReadFiles));
    }

    #[test]
    fn pending_when_auto_approve_off() {
        let _g = AutoApproveGuard::disable();
        let dir = TempDir::new().unwrap();
        let svc = AuthService::bootstrap(dir.path()).unwrap();
        let code = svc.mint_pairing_code(BTreeSet::new());
        let resp = svc.claim_pairing(PairClaimRequest {
            code: code.code,
            device_label: "phone".into(),
            requested_permissions: BTreeSet::from([Permission::ReadFiles]),
        });
        assert_eq!(resp, PairClaimResponse::Pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_registry() -> (TempDir, DeviceRegistry) {
        let dir = TempDir::new().unwrap();
        let reg = DeviceRegistry::load(dir.path()).unwrap();
        (dir, reg)
    }

    #[test]
    fn issue_then_verify_roundtrip() {
        let (_dir, reg) = fresh_registry();
        let issued = reg
            .issue("Test Device", BTreeSet::from([Permission::ReadFiles]))
            .unwrap();
        let rec = reg.verify_bearer(&issued.raw_token).expect("verifies");
        assert_eq!(rec.device_id, issued.device_id);
        assert!(rec.granted_permissions.contains(&Permission::ReadFiles));
    }

    #[test]
    fn verify_rejects_unknown_token() {
        let (_dir, reg) = fresh_registry();
        let err = reg.verify_bearer("totally-bogus").unwrap_err();
        assert!(matches!(err, AuthError::Invalid));
    }

    #[test]
    fn verify_rejects_empty_token() {
        let (_dir, reg) = fresh_registry();
        let err = reg.verify_bearer("").unwrap_err();
        assert!(matches!(err, AuthError::Missing));
    }

    #[test]
    fn revoke_blocks_subsequent_verify() {
        let (_dir, reg) = fresh_registry();
        let issued = reg
            .issue("Test", BTreeSet::from([Permission::ReadFiles]))
            .unwrap();
        assert!(reg.verify_bearer(&issued.raw_token).is_ok());
        let removed = reg.revoke(&issued.device_id).unwrap();
        assert!(removed);
        let err = reg.verify_bearer(&issued.raw_token).unwrap_err();
        assert!(matches!(err, AuthError::Invalid));
    }

    #[test]
    fn registry_round_trips_through_disk() {
        let dir = TempDir::new().unwrap();
        let reg = DeviceRegistry::load(dir.path()).unwrap();
        let issued = reg
            .issue("Test", BTreeSet::from([Permission::PtyCreate]))
            .unwrap();
        drop(reg);
        let reg2 = DeviceRegistry::load(dir.path()).unwrap();
        let rec = reg2
            .verify_bearer(&issued.raw_token)
            .expect("persisted token verifies");
        assert_eq!(rec.device_id, issued.device_id);
    }

    #[test]
    fn require_permissions_enforces_set() {
        let (_dir, reg) = fresh_registry();
        let issued = reg
            .issue("Test", BTreeSet::from([Permission::ReadFiles]))
            .unwrap();
        let rec = reg.verify_bearer(&issued.raw_token).unwrap();
        assert!(require_permissions(&rec, &[Permission::ReadFiles]).is_ok());
        let err = require_permissions(&rec, &[Permission::WriteFiles]).unwrap_err();
        assert!(matches!(err, AuthError::PermissionDenied(_)));
    }

    #[test]
    fn raw_token_does_not_appear_on_disk() {
        let dir = TempDir::new().unwrap();
        let reg = DeviceRegistry::load(dir.path()).unwrap();
        let issued = reg
            .issue("Test", BTreeSet::from([Permission::ReadFiles]))
            .unwrap();
        let bytes = std::fs::read(dir.path().join("devices.json")).unwrap();
        let body = String::from_utf8(bytes).unwrap();
        assert!(
            !body.contains(&issued.raw_token),
            "raw token leaked into devices.json"
        );
        // The hash should be there though.
        let expected_hash = hash_token(&issued.raw_token);
        assert!(body.contains(&expected_hash));
    }
}
