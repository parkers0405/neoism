use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(crate) const TENANT_EXTRA_KEY: &str = "neoismTenantId";

#[derive(Clone, Debug)]
pub(crate) struct CallerClaims {
    pub(crate) tenant_id: String,
    pub(crate) directory_prefixes: Vec<String>,
    pub(crate) hosted: bool,
    pub(crate) max_sessions: Option<usize>,
    pub(crate) max_artifacts: Option<usize>,
    pub(crate) max_artifact_bytes: Option<usize>,
    pub(crate) artifact_retention_days: Option<u64>,
    pub(crate) requests_per_minute: Option<u32>,
    pub(crate) max_in_flight: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostedAuthConfig {
    tokens: Vec<HostedToken>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostedToken {
    token: String,
    tenant_id: String,
    #[serde(default)]
    directory_prefixes: Vec<String>,
    #[serde(default)]
    max_sessions: Option<usize>,
    #[serde(default)]
    max_artifacts: Option<usize>,
    #[serde(default)]
    max_artifact_bytes: Option<usize>,
    #[serde(default)]
    artifact_retention_days: Option<u64>,
    #[serde(default)]
    requests_per_minute: Option<u32>,
    #[serde(default)]
    max_in_flight: Option<u32>,
}

pub(crate) fn authenticate(supplied: Option<&str>) -> Result<Option<CallerClaims>, String> {
    if supplied.is_some_and(|token| {
        token.starts_with(neoism_agent_service_api::daemon_credential::PREFIX)
    }) {
        let token = supplied.expect("checked above");
        let key = std::env::var("NEOISM_DAEMON_TOKEN")
            .map_err(|_| "daemon credential verifier is not configured".to_string())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "system clock is before Unix epoch".to_string())?
            .as_secs() as i64;
        let claims = neoism_agent_service_api::daemon_credential::verify(
            token,
            key.as_bytes(),
            now,
        )
        .map_err(str::to_string)?;
        return Ok(Some(CallerClaims {
            tenant_id: claims.tenant_id,
            directory_prefixes: claims.directory_prefixes,
            hosted: claims.hosted,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
        }));
    }
    if let Ok(raw) = std::env::var("NEOISM_AGENT_AUTH_CONFIG") {
        let config: HostedAuthConfig = serde_json::from_str(&raw)
            .map_err(|error| format!("invalid NEOISM_AGENT_AUTH_CONFIG: {error}"))?;
        let supplied = supplied.ok_or_else(|| "missing bearer token".to_string())?;
        let token = config
            .tokens
            .into_iter()
            .find(|candidate| constant_time_eq(supplied.as_bytes(), candidate.token.as_bytes()))
            .ok_or_else(|| "invalid bearer token".to_string())?;
        if token.tenant_id.trim().is_empty() {
            return Err("hosted token tenantId is empty".to_string());
        }
        return Ok(Some(CallerClaims {
            tenant_id: token.tenant_id,
            directory_prefixes: token.directory_prefixes,
            hosted: true,
            max_sessions: token.max_sessions,
            max_artifacts: token.max_artifacts,
            max_artifact_bytes: token.max_artifact_bytes,
            artifact_retention_days: token.artifact_retention_days,
            requests_per_minute: token.requests_per_minute,
            max_in_flight: token.max_in_flight,
        }));
    }
    let Ok(expected) = std::env::var("NEOISM_AGENT_TOKEN") else {
        return Ok(None);
    };
    let supplied = supplied.ok_or_else(|| "missing bearer token".to_string())?;
    constant_time_eq(supplied.as_bytes(), expected.as_bytes())
        .then_some(Some(CallerClaims {
            tenant_id: "local".to_string(),
            directory_prefixes: Vec::new(),
            hosted: false,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
        }))
        .ok_or_else(|| "invalid bearer token".to_string())
}

#[derive(Default)]
struct Usage {
    window: Option<Instant>,
    requests: u32,
    in_flight: u32,
}

pub(crate) struct RequestGuard {
    tenant_id: String,
}

pub(crate) fn begin_request(claims: &CallerClaims) -> Result<RequestGuard, &'static str> {
    let mut usage = usage().lock().expect("caller usage lock poisoned");
    let entry = usage.entry(claims.tenant_id.clone()).or_default();
    if entry
        .window
        .is_none_or(|window| window.elapsed() >= Duration::from_secs(60))
    {
        entry.window = Some(Instant::now());
        entry.requests = 0;
    }
    if claims
        .requests_per_minute
        .is_some_and(|limit| entry.requests >= limit)
    {
        return Err("request rate quota exceeded");
    }
    if claims
        .max_in_flight
        .is_some_and(|limit| entry.in_flight >= limit)
    {
        return Err("concurrent request quota exceeded");
    }
    entry.requests += 1;
    entry.in_flight += 1;
    Ok(RequestGuard {
        tenant_id: claims.tenant_id.clone(),
    })
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if let Ok(mut usage) = usage().lock() {
            if let Some(entry) = usage.get_mut(&self.tenant_id) {
                entry.in_flight = entry.in_flight.saturating_sub(1);
            }
        }
    }
}

fn usage() -> &'static Mutex<HashMap<String, Usage>> {
    static USAGE: OnceLock<Mutex<HashMap<String, Usage>>> = OnceLock::new();
    USAGE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn allows_directory(claims: &CallerClaims, directory: &str) -> bool {
    if claims.directory_prefixes.is_empty() {
        return true;
    }
    let Ok(directory) = std::fs::canonicalize(directory) else {
        return false;
    };
    claims.directory_prefixes.iter().any(|prefix| {
        std::fs::canonicalize(prefix)
            .is_ok_and(|prefix| directory == prefix || directory.starts_with(prefix))
    })
}

pub(crate) fn session_tenant(session: &neoism_agent_core::SessionInfo) -> &str {
    session
        .extra
        .get(TENANT_EXTRA_KEY)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("local")
}

pub(crate) fn allows_session(
    claims: &CallerClaims,
    session: &neoism_agent_core::SessionInfo,
) -> bool {
    allows_session_scope(claims, session_tenant(session), &session.directory)
}

fn allows_session_scope(claims: &CallerClaims, tenant_id: &str, directory: &str) -> bool {
    tenant_id == claims.tenant_id && allows_directory(claims, directory)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(tenant_id: String) -> CallerClaims {
        CallerClaims {
            tenant_id,
            directory_prefixes: Vec::new(),
            hosted: true,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
        }
    }

    #[test]
    fn enforces_rate_and_concurrency_quotas() {
        let mut concurrency = claims(format!(
            "concurrency-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit)
        ));
        concurrency.max_in_flight = Some(1);
        let guard = begin_request(&concurrency).unwrap();
        assert!(begin_request(&concurrency).is_err());
        drop(guard);
        assert!(begin_request(&concurrency).is_ok());

        let mut rate = claims(format!(
            "rate-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit)
        ));
        rate.requests_per_minute = Some(1);
        drop(begin_request(&rate).unwrap());
        assert!(begin_request(&rate).is_err());
    }

    #[test]
    fn scoped_claims_deny_cross_directory_and_cross_tenant_sessions() {
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-caller-scope-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit)
        ));
        let allowed = root.join("allowed");
        let denied = root.join("denied");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&denied).unwrap();
        let mut scoped = claims("tenant-a".into());
        scoped.directory_prefixes = vec![allowed.to_string_lossy().into_owned()];

        assert!(!allows_session_scope(
            &scoped,
            "tenant-a",
            &denied.to_string_lossy(),
        ));
        assert!(!allows_session_scope(
            &scoped,
            "tenant-b",
            &allowed.to_string_lossy(),
        ));
        assert!(allows_session_scope(
            &scoped,
            "tenant-a",
            &allowed.to_string_lossy(),
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}