//! Roundtrip every auth/pairing wire variant through serde_json and exercise
//! the constant-time compare helper.

use std::collections::BTreeSet;

use neoism_protocol::auth::{constant_time_eq, AuthToken};
use neoism_protocol::pairing::{
    ActiveSession, PairClaimRequest, PairClaimResponse, PairingCodeResponse, Permission,
};

fn rt_token(t: &AuthToken) {
    let json = serde_json::to_string(t).expect("serialize");
    let back: AuthToken = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(t.as_str(), back.as_str());
}

#[test]
fn auth_token_roundtrip() {
    rt_token(&AuthToken::new("some-token"));
    rt_token(&AuthToken::new(""));
}

#[test]
fn auth_token_debug_is_redacted() {
    let t = AuthToken::new("super-secret");
    let debug = format!("{:?}", t);
    assert!(
        !debug.contains("super-secret"),
        "raw token leaked into Debug: {debug}"
    );
    assert!(debug.contains("redacted"));
}

#[test]
fn pairing_code_response_roundtrip() {
    let msg = PairingCodeResponse {
        code: "ABC234".into(),
        expires_at: 1_700_000_000,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: PairingCodeResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn pair_claim_request_roundtrip() {
    let mut perms = BTreeSet::new();
    perms.insert(Permission::ReadFiles);
    perms.insert(Permission::PtyCreate);
    let msg = PairClaimRequest {
        code: "ABC234".into(),
        device_label: "Parker's iPhone".into(),
        requested_permissions: perms,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: PairClaimRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn pair_claim_response_granted_roundtrip() {
    let mut perms = BTreeSet::new();
    perms.insert(Permission::ReadFiles);
    let msg = PairClaimResponse::Granted {
        device_id: "dev-1".into(),
        device_token: "raw-token".into(),
        granted_permissions: perms,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: PairClaimResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
    assert!(
        json.contains("\"status\":\"granted\""),
        "tagged variant: {json}"
    );
}

#[test]
fn pair_claim_response_pending_roundtrip() {
    let msg = PairClaimResponse::Pending;
    let json = serde_json::to_string(&msg).unwrap();
    let back: PairClaimResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
    assert!(json.contains("\"status\":\"pending\""));
}

#[test]
fn pair_claim_response_rejected_roundtrip() {
    let msg = PairClaimResponse::Rejected {
        reason: "code expired".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: PairClaimResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
    assert!(json.contains("\"status\":\"rejected\""));
}

#[test]
fn active_session_roundtrip() {
    let mut perms = BTreeSet::new();
    perms.insert(Permission::ReadFiles);
    perms.insert(Permission::WriteFiles);
    let msg = ActiveSession {
        device_id: "dev-1".into(),
        device_label: "Parker's iPhone".into(),
        connected_at: 1_700_000_000,
        last_seen: 1_700_000_100,
        active_pty_count: 2,
        current_permissions: perms,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: ActiveSession = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn permission_variants_serialize_stably() {
    let variants = [
        Permission::ReadFiles,
        Permission::WriteFiles,
        Permission::GitWrite,
        Permission::PtyCreate,
        Permission::DeviceManage,
        Permission::AgentUse,
    ];
    for v in variants {
        let json = serde_json::to_string(&v).unwrap();
        let back: Permission = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }
}

#[test]
fn constant_time_eq_equal() {
    assert!(constant_time_eq(b"hello", b"hello"));
    assert!(constant_time_eq(b"", b""));
    assert!(constant_time_eq(&[0u8; 64], &[0u8; 64]));
}

#[test]
fn constant_time_eq_unequal() {
    assert!(!constant_time_eq(b"hello", b"world"));
    assert!(!constant_time_eq(b"hello", b"helloo"));
    assert!(!constant_time_eq(b"abc", b""));
}
