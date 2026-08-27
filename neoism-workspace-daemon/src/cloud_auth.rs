//! Minimal cloud-facing auth helpers.
//!
//! This is intentionally smaller than OIDC. Cloud deployments can gate
//! provisioning with either:
//!
//! * `Authorization: Bearer $NEOISM_CLOUD_PROVISION_TOKEN`
//! * an existing paired device bearer token with `GitWrite` or
//!   `DeviceManage`
//! * the legacy `NEOISM_DAEMON_TOKEN`, when explicitly configured
//!
//! WebSocket `Hello` still flows through the workspace dispatcher; the
//! server marks already-authenticated upgrades as preauthenticated so
//! existing bearer/daemon-token paths do not get rejected by the pairing
//! token gate.

use axum::http::{header, HeaderMap, StatusCode};
use neoism_protocol::auth::constant_time_eq;
use neoism_protocol::pairing::Permission;

use crate::auth::AuthService;

pub const ENV_CLOUD_PROVISION_TOKEN: &str = "NEOISM_CLOUD_PROVISION_TOKEN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudPrincipal {
    pub subject: String,
    pub method: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAuthError {
    pub status: StatusCode,
    pub message: &'static str,
}

pub fn authorize_provision(
    headers: &HeaderMap,
    auth: &AuthService,
) -> Result<CloudPrincipal, CloudAuthError> {
    let bearer = extract_bearer(headers).ok_or(CloudAuthError {
        status: StatusCode::UNAUTHORIZED,
        message: "missing bearer token",
    })?;

    if provision_token_matches(&bearer) {
        return Ok(CloudPrincipal {
            subject: "cloud-provision-token".into(),
            method: "cloud-provision-token",
        });
    }

    if legacy_daemon_token_matches(&bearer) {
        return Ok(CloudPrincipal {
            subject: "daemon-token".into(),
            method: "daemon-token",
        });
    }

    match auth.authenticate_bearer(&bearer) {
        Ok(record)
            if record.granted_permissions.contains(&Permission::GitWrite)
                || record
                    .granted_permissions
                    .contains(&Permission::DeviceManage) =>
        {
            Ok(CloudPrincipal {
                subject: record.device_id,
                method: "device-bearer",
            })
        }
        Ok(_) => Err(CloudAuthError {
            status: StatusCode::FORBIDDEN,
            message: "device token lacks GitWrite or DeviceManage",
        }),
        Err(_) => Err(CloudAuthError {
            status: StatusCode::UNAUTHORIZED,
            message: "invalid bearer token",
        }),
    }
}

pub fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let prefix = "Bearer ";
    if !raw.starts_with(prefix) {
        return None;
    }
    let token = raw[prefix.len()..].trim();
    (!token.is_empty()).then(|| token.to_string())
}

pub fn provision_token_configured() -> bool {
    std::env::var(ENV_CLOUD_PROVISION_TOKEN)
        .map(|token| !token.is_empty())
        .unwrap_or(false)
}

fn provision_token_matches(candidate: &str) -> bool {
    match std::env::var(ENV_CLOUD_PROVISION_TOKEN) {
        Ok(expected) if !expected.is_empty() => {
            constant_time_eq(candidate.as_bytes(), expected.as_bytes())
        }
        _ => false,
    }
}

pub fn legacy_daemon_token_configured() -> bool {
    std::env::var("NEOISM_DAEMON_TOKEN")
        .map(|token| !token.is_empty())
        .unwrap_or(false)
}

pub fn legacy_daemon_token_matches(candidate: &str) -> bool {
    crate::daemon_token::accepted_daemon_tokens()
        .iter()
        .any(|expected| constant_time_eq(candidate.as_bytes(), expected.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        prev_cloud: Option<String>,
        prev_daemon: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(cloud: Option<&str>, daemon: Option<&str>) -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev_cloud = std::env::var(ENV_CLOUD_PROVISION_TOKEN).ok();
            let prev_daemon = std::env::var("NEOISM_DAEMON_TOKEN").ok();
            match cloud {
                Some(value) => std::env::set_var(ENV_CLOUD_PROVISION_TOKEN, value),
                None => std::env::remove_var(ENV_CLOUD_PROVISION_TOKEN),
            }
            match daemon {
                Some(value) => std::env::set_var("NEOISM_DAEMON_TOKEN", value),
                None => std::env::remove_var("NEOISM_DAEMON_TOKEN"),
            }
            Self {
                prev_cloud,
                prev_daemon,
                _guard: guard,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev_cloud {
                Some(value) => std::env::set_var(ENV_CLOUD_PROVISION_TOKEN, value),
                None => std::env::remove_var(ENV_CLOUD_PROVISION_TOKEN),
            }
            match &self.prev_daemon {
                Some(value) => std::env::set_var("NEOISM_DAEMON_TOKEN", value),
                None => std::env::remove_var("NEOISM_DAEMON_TOKEN"),
            }
        }
    }

    #[test]
    fn explicit_cloud_token_is_the_only_cloud_match() {
        let _env = EnvGuard::set(Some("provision-secret"), None);
        assert!(provision_token_configured());
        assert!(provision_token_matches("provision-secret"));
        assert!(!provision_token_matches("wrong"));
    }

    #[test]
    fn daemon_token_match_requires_configured_token() {
        let _env = EnvGuard::set(None, None);
        assert!(!legacy_daemon_token_configured());
        assert!(!legacy_daemon_token_matches("anything"));
    }
}
