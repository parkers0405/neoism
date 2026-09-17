//! Short-lived pairing codes.
//!
//! `PairingCodeStore` is the in-memory bookkeeping for codes the operator
//! has minted but no remote device has claimed yet. Codes are:
//!
//! * 8 characters, alphabet limited to unambiguous chars (no `0`/`O`/`1`/`l`,
//!   etc.).
//! * Single-use — claiming or expiring removes the code immediately.
//! * Valid for 60 seconds by default.
//! * Never logged in raw form; we only ever log the first 4 chars of the
//!   sha256 of the code (`code_id`).
//!
//! The pairing code is held *only* in memory: a daemon restart invalidates
//! every outstanding code, which is the conservative behaviour.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use neoism_protocol::pairing::{PairingCodeResponse, Permission};
use rand::Rng;
use sha2::{Digest, Sha256};

/// Pairing-code TTL. Hard-coded for now; an operator override can be added
/// later if there's a real use-case (60s is intentionally tight).
pub const PAIRING_TTL: Duration = Duration::from_secs(60);

/// Length of a generated code. 8 chars over a 32-symbol alphabet gives 40
/// bits of entropy — plenty for a single-use code that expires in 60s.
const CODE_LEN: usize = 8;

/// Unambiguous alphabet — no 0/O/1/l/I, no lowercase to avoid mistakes when
/// the user reads the code aloud.
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// In-flight pairing code. The code is held in plaintext only here; the
/// hashed `code_id` lives in audit/log lines.
#[derive(Debug, Clone)]
pub struct PendingPairing {
    pub code: String,
    pub expires_at: SystemTime,
    pub requested_permissions: BTreeSet<Permission>,
    /// Shared workspace this phone-share code may join. Unscoped codes
    /// (operator mint) stay daemon-wide for existing pairing.
    pub workspace_id: Option<String>,
    /// Operator already confirmed this mint (phone-share QR). Claim may
    /// grant the requested set without a second approval prompt.
    pub preapproved: bool,
}

#[derive(Clone, Default)]
pub struct PairingCodeStore {
    inner: Arc<Mutex<HashMap<String, PendingPairing>>>,
}

impl PairingCodeStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a fresh pairing code valid for [`PAIRING_TTL`].
    ///
    /// `requested_permissions` is what a future claim *may* ask for; the
    /// operator-approval gate is still the final arbiter (see
    /// `permissions::evaluate`). Passing an empty set is fine — claims can
    /// still ask for permissions, they just won't have been pre-declared.
    pub fn mint(&self, requested: BTreeSet<Permission>) -> PairingCodeResponse {
        self.mint_with(requested, false, None)
    }

    /// Mint a code the operator already approved by starting phone-share.
    pub fn mint_preapproved(
        &self,
        requested: BTreeSet<Permission>,
        workspace_id: String,
    ) -> PairingCodeResponse {
        self.mint_with(requested, true, Some(workspace_id))
    }

    fn mint_with(
        &self,
        requested: BTreeSet<Permission>,
        preapproved: bool,
        workspace_id: Option<String>,
    ) -> PairingCodeResponse {
        let code = generate_code();
        let expires_at = SystemTime::now() + PAIRING_TTL;
        let entry = PendingPairing {
            code: code.clone(),
            expires_at,
            requested_permissions: requested,
            workspace_id,
            preapproved,
        };
        {
            let mut guard = self.lock();
            let now = SystemTime::now();
            guard.retain(|_, v| now <= v.expires_at);
            if guard.len() >= 32 {
                tracing::warn!("pairing code store full; refusing mint");
                return PairingCodeResponse {
                    code: String::new(),
                    expires_at: 0,
                };
            }
            guard.insert(code.clone(), entry);
        }
        let expires_unix = expires_at
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        tracing::info!(
            code_id = %code_id(&code),
            expires_at = expires_unix,
            "minted pairing code"
        );
        PairingCodeResponse {
            code,
            expires_at: expires_unix,
        }
    }

    /// Outcome of a claim attempt. The successful path returns the originally
    /// requested permission set so the caller can feed it into the operator-
    /// approval gate.
    pub fn claim(&self, code: &str) -> ClaimOutcome {
        // Sweep expired entries before lookup so single-use semantics hold
        // even if a code expired *just* before the claim arrived.
        self.purge_expired();
        let mut guard = self.lock();
        // Constant-time lookup over outstanding codes — we don't index by
        // the raw code as the map key in the search loop. (The HashMap key
        // is still the raw code, but we never `.get()` it — we iterate.)
        let mut hit: Option<String> = None;
        for k in guard.keys() {
            if neoism_protocol::auth::constant_time_eq(k.as_bytes(), code.as_bytes()) {
                hit = Some(k.clone());
                break;
            }
        }
        match hit {
            Some(key) => {
                let entry = guard.remove(&key).expect("just looked up");
                if SystemTime::now() > entry.expires_at {
                    tracing::info!(
                        code_id = %code_id(&entry.code),
                        "pairing code claimed after expiry"
                    );
                    ClaimOutcome::Expired
                } else {
                    tracing::info!(
                        code_id = %code_id(&entry.code),
                        "pairing code consumed"
                    );
                    ClaimOutcome::Ok {
                        requested_permissions: entry.requested_permissions,
                        workspace_id: entry.workspace_id,
                        preapproved: entry.preapproved,
                    }
                }
            }
            None => ClaimOutcome::Unknown,
        }
    }

    fn purge_expired(&self) {
        let now = SystemTime::now();
        let mut guard = self.lock();
        guard.retain(|_, v| now <= v.expires_at);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingPairing>> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    Ok {
        requested_permissions: BTreeSet<Permission>,
        workspace_id: Option<String>,
        preapproved: bool,
    },
    Expired,
    /// Either never minted or already claimed — we collapse these so we
    /// don't leak which case it was over the wire.
    Unknown,
}

fn generate_code() -> String {
    let mut rng = rand::thread_rng();
    let mut out = String::with_capacity(CODE_LEN);
    for _ in 0..CODE_LEN {
        let idx = rng.gen_range(0..ALPHABET.len());
        out.push(ALPHABET[idx] as char);
    }
    out
}

/// Stable identifier for a pairing code suitable for logs/audit. Returns the
/// first 4 chars of sha256(code) — enough to correlate a mint with a claim
/// in the audit log without revealing the code itself.
pub fn code_id(code: &str) -> String {
    let mut h = Sha256::new();
    h.update(code.as_bytes());
    let digest = h.finalize();
    let hex = digest
        .iter()
        .take(2)
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_then_claim_consumes_code() {
        let store = PairingCodeStore::new();
        let resp = store.mint(BTreeSet::new());
        let outcome = store.claim(&resp.code);
        assert!(matches!(outcome, ClaimOutcome::Ok { .. }));
        // Single-use: a second claim with the same code fails.
        let outcome2 = store.claim(&resp.code);
        assert_eq!(outcome2, ClaimOutcome::Unknown);
    }

    #[test]
    fn unknown_code_returns_unknown() {
        let store = PairingCodeStore::new();
        assert_eq!(store.claim("NOTACODE"), ClaimOutcome::Unknown);
    }

    #[test]
    fn generated_codes_use_unambiguous_alphabet() {
        for _ in 0..100 {
            let c = generate_code();
            assert_eq!(c.len(), CODE_LEN);
            for ch in c.chars() {
                assert!(
                    ALPHABET.contains(&(ch as u8)),
                    "code contained ambiguous char: {ch}"
                );
            }
        }
    }

    #[test]
    fn code_id_is_short_and_deterministic() {
        let a = code_id("ABC234");
        let b = code_id("ABC234");
        assert_eq!(a, b);
        assert_eq!(a.len(), 4);
        let c = code_id("DIFFER");
        assert_ne!(a, c);
    }
}
