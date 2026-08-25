//! Short-lived credentials minted by an authenticated Neoism workspace daemon.
//!
//! The daemon and Agent server already share the daemon authentication key in
//! `NEOISM_DAEMON_TOKEN`; this format deliberately derives from that key rather
//! than introducing another persisted full-access secret.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const ISSUER: &str = "neoism-workspace-daemon";
pub const AUDIENCE: &str = "neoism-agent-server";
pub const PREFIX: &str = "neoism-daemon-v1";
pub const MAX_LIFETIME_SECS: i64 = 120;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonCredentialClaims {
    pub issuer: String,
    pub audience: String,
    pub subject: String,
    pub tenant_id: String,
    pub directory_prefixes: Vec<String>,
    pub hosted: bool,
    pub issued_at: i64,
    pub expires_at: i64,
}


impl DaemonCredentialClaims {
    pub fn new(
        subject: impl Into<String>,
        tenant_id: impl Into<String>,
        directory_prefixes: Vec<String>,
        hosted: bool,
        now: i64,
        lifetime_secs: i64,
    ) -> Result<Self, &'static str> {
        if !(1..=MAX_LIFETIME_SECS).contains(&lifetime_secs) {
            return Err("daemon credential lifetime is out of bounds");
        }
        Ok(Self {
            issuer: ISSUER.into(),
            audience: AUDIENCE.into(),
            subject: subject.into(),
            tenant_id: tenant_id.into(),
            directory_prefixes,
            hosted,
            issued_at: now,
            expires_at: now.saturating_add(lifetime_secs),
        })
    }
}

pub fn issue(
    claims: &DaemonCredentialClaims,
    daemon_key: &[u8],
) -> Result<String, &'static str> {
    validate_shape(claims)?;
    if daemon_key.is_empty() {
        return Err("daemon credential signing key is empty");
    }
    let payload =
        serde_json::to_vec(claims).map_err(|_| "cannot encode daemon credential")?;
    let payload = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{PREFIX}.{payload}");
    let signature = hmac_sha256(daemon_key, signing_input.as_bytes());
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

pub fn verify(
    token: &str,
    daemon_key: &[u8],
    now: i64,
) -> Result<DaemonCredentialClaims, &'static str> {
    let mut parts = token.split('.');
    let prefix = parts.next().ok_or("malformed daemon credential")?;
    let payload = parts.next().ok_or("malformed daemon credential")?;
    let signature = parts.next().ok_or("malformed daemon credential")?;
    if parts.next().is_some() || prefix != PREFIX || daemon_key.is_empty() {
        return Err("malformed daemon credential");
    }
    let supplied = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "malformed daemon credential signature")?;
    let signing_input = format!("{PREFIX}.{payload}");
    let expected = hmac_sha256(daemon_key, signing_input.as_bytes());
    if supplied.len() != expected.len()
        || supplied.ct_eq(expected.as_slice()).unwrap_u8() != 1
    {
        return Err("invalid daemon credential signature");
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| "malformed daemon credential payload")?;
    let claims: DaemonCredentialClaims = serde_json::from_slice(&payload)
        .map_err(|_| "malformed daemon credential claims")?;
    validate_shape(&claims)?;
    if claims.issued_at > now.saturating_add(5) {
        return Err("daemon credential is not yet valid");
    }
    if claims.expires_at <= now {
        return Err("daemon credential has expired");
    }
    if claims.expires_at.saturating_sub(claims.issued_at) > MAX_LIFETIME_SECS {
        return Err("daemon credential lifetime is out of bounds");
    }
    Ok(claims)
}

fn validate_shape(claims: &DaemonCredentialClaims) -> Result<(), &'static str> {
    if claims.issuer != ISSUER {
        return Err("invalid daemon credential issuer");
    }
    if claims.audience != AUDIENCE {
        return Err("invalid daemon credential audience");
    }
    if claims.subject.trim().is_empty() || claims.tenant_id.trim().is_empty() {
        return Err("daemon credential identity is empty");
    }
    if claims.expires_at <= claims.issued_at {
        return Err("daemon credential expiry is invalid");
    }
    Ok(())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut normalized = [0u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner);
    outer.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_forged_expired_and_wrong_audience_credentials() {
        let key = b"daemon-key";
        let valid = DaemonCredentialClaims::new(
            "device:a",
            "tenant:a",
            vec!["/repo".into()],
            true,
            100,
            60,
        )
        .unwrap();
        let token = issue(&valid, key).unwrap();
        assert_eq!(verify(&token, key, 120).unwrap(), valid);
        assert!(verify(&token, b"wrong-key", 120).is_err());
        assert!(verify(&token, key, 161).is_err());

        let mut wrong_audience = valid;
        wrong_audience.audience = "other-service".into();
        // Sign the altered claims directly to prove audience validation is
        // independent of signature validation.
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&wrong_audience).unwrap());
        let input = format!("{PREFIX}.{payload}");
        let token = format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(hmac_sha256(key, input.as_bytes()))
        );
        assert_eq!(
            verify(&token, key, 120),
            Err("invalid daemon credential audience")
        );
    }
}