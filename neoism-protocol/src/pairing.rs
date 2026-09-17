//! Pairing and device-identity wire types.
//!
//! Phase 10 introduces a small pairing protocol so that remote clients (a
//! phone, a browser, another laptop) can claim a long-lived `DeviceToken`
//! from a daemon by typing a short-lived pairing code on the host.
//!
//! This module only defines the message shapes. The on-host generation,
//! storage, redaction, and permission gating live in
//! `neoism-workspace-daemon`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Coarse-grained permissions that a `DeviceToken` may carry.
///
/// The daemon is deny-by-default: a token without an explicit permission may
/// not exercise the corresponding capability. The set is small and stable on
/// purpose — finer-grained scoping (per-path file access, etc.) can be
/// layered on later without breaking the wire format.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Permission {
    /// Read files inside the daemon's workspace root.
    ReadFiles,
    /// Write files inside the daemon's workspace root.
    WriteFiles,
    /// Mutating git operations (commit, branch, push, etc.).
    GitWrite,
    /// Spawn a new PTY session.
    PtyCreate,
    /// Manage other devices (list, revoke, issue new pairing codes).
    DeviceManage,
    /// Use Agent (chat/tools) inside a device-bound workspace. Not a
    /// daemon-wide file grant and not device administration.
    AgentUse,
}

/// Response to a request for a new short-lived pairing code.
///
/// `expires_at` is a unix timestamp (seconds). The code is single-use and
/// becomes invalid the moment it is claimed or expires, whichever comes
/// first. The raw code MUST NOT be logged by either side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingCodeResponse {
    pub code: String,
    pub expires_at: i64,
}

/// A remote device's request to redeem a pairing code for a `DeviceToken`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairClaimRequest {
    pub code: String,
    pub device_label: String,
    pub requested_permissions: BTreeSet<Permission>,
}

/// Outcome of a `PairClaimRequest`.
///
/// `Granted` is the only variant that carries the raw `device_token` string;
/// the token leaves the daemon in this single response and must be stored
/// securely by the client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PairClaimResponse {
    Granted {
        device_id: String,
        device_token: String,
        granted_permissions: BTreeSet<Permission>,
    },
    /// Operator approval has not yet been granted; the client should retry.
    Pending,
    Rejected {
        reason: String,
    },
}

/// One row in the response to `GET /sessions` — useful for surfacing an
/// "active remote devices" UI on the host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveSession {
    pub device_id: String,
    pub device_label: String,
    pub connected_at: i64,
    pub last_seen: i64,
    pub active_pty_count: u32,
    pub current_permissions: BTreeSet<Permission>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_roundtrip() {
        let mut perms = BTreeSet::new();
        perms.insert(Permission::ReadFiles);
        perms.insert(Permission::PtyCreate);
        let json = serde_json::to_string(&perms).unwrap();
        let back: BTreeSet<Permission> = serde_json::from_str(&json).unwrap();
        assert_eq!(perms, back);
    }
}
