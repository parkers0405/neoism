use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const TENANT_EXTRA_KEY: &str = "neoismTenantId";

#[derive(Clone, Debug)]
pub(crate) struct CallerClaims {
    /// Stable authenticated actor. Unlike `tenant_id`, this remains distinct
    /// for every host/guest sharing a workspace namespace.
    pub(crate) subject: String,
    pub(crate) workspace_id: Option<String>,
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

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostedAuthConfig {
    tokens: Vec<HostedToken>,
}

#[derive(Clone, Deserialize)]
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

#[derive(Clone)]
pub(crate) struct CallerPolicy {
    daemon_key: Option<Arc<[u8]>>,
    hosted_config: Result<Option<Arc<HostedAuthConfig>>, String>,
    local_token: Option<String>,
    usage: Arc<UsageTracker>,
}

impl CallerPolicy {
    pub(crate) fn from_env() -> Self {
        let hosted_config = std::env::var("NEOISM_AGENT_AUTH_CONFIG")
            .ok()
            .map(|raw| {
                serde_json::from_str(&raw)
                    .map(Arc::new)
                    .map_err(|error| format!("invalid NEOISM_AGENT_AUTH_CONFIG: {error}"))
            })
            .transpose();
        Self {
            daemon_key: std::env::var("NEOISM_DAEMON_TOKEN")
                .ok()
                .map(|key| Arc::<[u8]>::from(key.into_bytes())),
            hosted_config,
            local_token: std::env::var("NEOISM_AGENT_TOKEN").ok(),
            usage: Arc::new(UsageTracker::default()),
        }
    }

    pub(crate) fn authenticate(
        &self,
        supplied: Option<&str>,
    ) -> Result<Option<CallerClaims>, String> {
        if supplied.is_some_and(|token| {
            token.starts_with(neoism_agent_service_api::daemon_credential::PREFIX)
        }) {
            let token = supplied.expect("checked above");
            let key = self.daemon_key.as_deref().ok_or_else(|| {
                "daemon credential verifier is not configured".to_string()
            })?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "system clock is before Unix epoch".to_string())?
                .as_secs() as i64;
            let claims =
                neoism_agent_service_api::daemon_credential::verify(token, key, now)
                    .map_err(str::to_string)?;
            return Ok(Some(CallerClaims {
                subject: claims.subject,
                workspace_id: Some(claims.workspace_id),
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
        if let Some(config) = self.hosted_config.as_ref().map_err(Clone::clone)?.as_ref()
        {
            let supplied = supplied.ok_or_else(|| "missing bearer token".to_string())?;
            let token = config
                .tokens
                .iter()
                .find(|candidate| {
                    constant_time_eq(supplied.as_bytes(), candidate.token.as_bytes())
                })
                .ok_or_else(|| "invalid bearer token".to_string())?;
            if token.tenant_id.trim().is_empty() {
                return Err("hosted token tenantId is empty".to_string());
            }
            return Ok(Some(CallerClaims {
                subject: format!("hosted:{}", token.tenant_id),
                workspace_id: None,
                tenant_id: token.tenant_id.clone(),
                directory_prefixes: token.directory_prefixes.clone(),
                hosted: true,
                max_sessions: token.max_sessions,
                max_artifacts: token.max_artifacts,
                max_artifact_bytes: token.max_artifact_bytes,
                artifact_retention_days: token.artifact_retention_days,
                requests_per_minute: token.requests_per_minute,
                max_in_flight: token.max_in_flight,
            }));
        }
        let Some(expected) = self.local_token.as_deref() else {
            return Ok(None);
        };
        let supplied = supplied.ok_or_else(|| "missing bearer token".to_string())?;
        constant_time_eq(supplied.as_bytes(), expected.as_bytes())
            .then_some(Some(CallerClaims {
                subject: "local-operator".to_string(),
                workspace_id: None,
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

    pub(crate) fn begin_request(
        &self,
        claims: &CallerClaims,
    ) -> Result<RequestGuard, &'static str> {
        self.usage.begin_request(claims)
    }
}

struct Usage {
    window: Option<Instant>,
    requests: u32,
    in_flight: u32,
    last_seen: Instant,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            window: None,
            requests: 0,
            in_flight: 0,
            last_seen: Instant::now(),
        }
    }
}

const USAGE_RETENTION: Duration = Duration::from_secs(5 * 60);
const MAX_USAGE_TENANTS: usize = 4096;

#[derive(Default)]
struct UsageTracker {
    entries: Mutex<HashMap<String, Usage>>,
}

pub(crate) struct RequestGuard {
    tenant_id: String,
    usage: Arc<UsageTracker>,
}

impl UsageTracker {
    fn begin_request(
        self: &Arc<Self>,
        claims: &CallerClaims,
    ) -> Result<RequestGuard, &'static str> {
        let now = Instant::now();
        let mut usage = self.entries.lock().expect("caller usage lock poisoned");
        usage.retain(|_, entry| {
            entry.in_flight > 0 || now.duration_since(entry.last_seen) < USAGE_RETENTION
        });
        if !usage.contains_key(&claims.tenant_id) && usage.len() >= MAX_USAGE_TENANTS {
            let oldest_inactive = usage
                .iter()
                .filter(|(_, entry)| entry.in_flight == 0)
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(tenant, _)| tenant.clone());
            if let Some(tenant) = oldest_inactive {
                usage.remove(&tenant);
            } else {
                return Err("request quota tracker capacity exceeded");
            }
        }
        let entry = usage.entry(claims.tenant_id.clone()).or_default();
        if entry
            .window
            .is_none_or(|window| window.elapsed() >= Duration::from_secs(60))
        {
            entry.window = Some(now);
            entry.requests = 0;
        }
        entry.last_seen = now;
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
            usage: self.clone(),
        })
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.entries
            .lock()
            .expect("caller usage lock poisoned")
            .len()
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if let Ok(mut usage) = self.usage.entries.lock() {
            if let Some(entry) = usage.get_mut(&self.tenant_id) {
                entry.in_flight = entry.in_flight.saturating_sub(1);
                entry.last_seen = Instant::now();
            }
        }
    }
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
            subject: format!("subject:{tenant_id}"),
            workspace_id: None,
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
        let policy = CallerPolicy::from_env();
        let mut concurrency = claims(format!(
            "concurrency-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit)
        ));
        concurrency.max_in_flight = Some(1);
        let guard = policy.begin_request(&concurrency).unwrap();
        assert!(policy.begin_request(&concurrency).is_err());
        drop(guard);
        assert!(policy.begin_request(&concurrency).is_ok());

        let mut rate = claims(format!(
            "rate-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit)
        ));
        rate.requests_per_minute = Some(1);
        drop(policy.begin_request(&rate).unwrap());
        assert!(policy.begin_request(&rate).is_err());
    }

    #[test]
    fn quota_tracker_is_bounded() {
        let policy = CallerPolicy::from_env();
        for index in 0..(MAX_USAGE_TENANTS + 128) {
            drop(
                policy
                    .begin_request(&claims(format!("tenant-{index}")))
                    .unwrap(),
            );
        }
        assert_eq!(policy.usage.entry_count(), MAX_USAGE_TENANTS);
    }

    #[tokio::test]
    async fn two_app_states_isolate_utility_and_quota_runtime() {
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-utility-isolation-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let first = crate::state::AppState::open_database(root.join("first.db"))
            .await
            .unwrap();
        let second = crate::state::AppState::open_database(root.join("second.db"))
            .await
            .unwrap();

        assert!(!Arc::ptr_eq(
            &first.inner.utilities,
            &second.inner.utilities
        ));
        let mut limited = claims("shared-tenant".into());
        limited.max_in_flight = Some(1);
        let first_guard = first.inner.caller_policy.begin_request(&limited).unwrap();
        let second_guard = second.inner.caller_policy.begin_request(&limited).unwrap();
        assert!(first.inner.caller_policy.begin_request(&limited).is_err());
        assert!(second.inner.caller_policy.begin_request(&limited).is_err());

        let file = root.join("shared.txt");
        let file_guard = first.inner.utilities.file_locks.lock_file(&file).await;
        let independent_guard = tokio::time::timeout(
            Duration::from_millis(100),
            second.inner.utilities.file_locks.lock_file(&file),
        )
        .await
        .expect("separate AppState file-lock registries must not block each other");

        drop((first_guard, second_guard, file_guard, independent_guard));
        let _ = std::fs::remove_dir_all(root);
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
