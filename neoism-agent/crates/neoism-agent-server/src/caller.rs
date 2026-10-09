use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const HOST_LOCAL_ACCESS_KEY: &str = "neoismHostLocalAccess";
pub(crate) const TENANT_EXTRA_KEY: &str = "neoismTenantId";
pub(crate) const EXECUTION_POLICY_EXTRA_KEY: &str = "neoismExecutionPolicy";
pub(crate) const CREATED_BY_EXTRA_KEY: &str = "neoismCreatedBy";
pub(crate) const QUOTAS_EXTRA_KEY: &str = "neoismTenantQuotas";
pub(crate) const DIRECTORY_PREFIXES_EXTRA_KEY: &str = "neoismDirectoryPrefixes";

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
    pub(crate) resolved: Option<neoism_agent_service_api::ResolvedTenant>,
    pub(crate) worker: Option<Arc<neoism_agent_service_api::WorkspaceWorkerBinding>>,
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
    tenant_resolver: Option<Arc<dyn neoism_agent_service_api::TenantResolver>>,
    strict_hosted: bool,
    worker: Option<Arc<neoism_agent_service_api::WorkspaceWorkerBinding>>,
}

/// The canonical daemon-token file shared by every Neoism process on this
/// machine: `$XDG_RUNTIME_DIR/neoism/daemon-token` on unix, the per-user
/// temp-dir variant elsewhere. Mirrors `daemon_token::daemon_token_path` /
/// the desktop's `embedded_daemon_token_path`.
fn canonical_daemon_token_from_disk() -> Option<String> {
    let path = {
        #[cfg(unix)]
        {
            match std::env::var_os("XDG_RUNTIME_DIR") {
                Some(runtime) if !runtime.is_empty() => std::path::PathBuf::from(runtime)
                    .join("neoism")
                    .join("daemon-token"),
                _ => {
                    let uid = unsafe { libc::geteuid() };
                    std::env::temp_dir()
                        .join(format!("neoism-{uid}"))
                        .join("daemon-token")
                }
            }
        }
        #[cfg(not(unix))]
        {
            // Mirror the desktop's `per_user_suffix`: sanitized USERNAME.
            let user = std::env::var("USERNAME")
                .ok()
                .filter(|name| !name.is_empty())
                .map(|name| {
                    name.chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect::<String>()
                })
                .unwrap_or_else(|| "default".to_string());
            std::env::temp_dir()
                .join(format!("neoism-{user}"))
                .join("daemon-token")
        }
    };
    let token = std::fs::read_to_string(path).ok()?;
    let token = token.trim().to_string();
    (!token.is_empty()).then_some(token)
}

impl CallerPolicy {
    pub(crate) fn from_env() -> Self {
        Self::from_env_with_resolver(None)
    }

    pub(crate) fn for_hosted(
        tenant_resolver: Option<Arc<dyn neoism_agent_service_api::TenantResolver>>,
    ) -> Self {
        let mut policy = Self::from_env_with_resolver(tenant_resolver);
        policy.strict_hosted = true;
        policy
    }

    pub(crate) fn for_workspace_worker(
        tenant_resolver: Option<Arc<dyn neoism_agent_service_api::TenantResolver>>,
        binding: Arc<neoism_agent_service_api::WorkspaceWorkerBinding>,
    ) -> Self {
        let mut policy = Self::for_hosted(tenant_resolver);
        policy.worker = Some(binding);
        policy
    }

    pub(crate) fn from_env_with_resolver(
        tenant_resolver: Option<Arc<dyn neoism_agent_service_api::TenantResolver>>,
    ) -> Self {
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
            tenant_resolver,
            strict_hosted: false,
            worker: None,
        }
    }

    pub(crate) async fn authenticate_request(
        &self,
        supplied: Option<&str>,
    ) -> Result<Option<CallerClaims>, String> {
        if !self.strict_hosted
            && supplied.is_some_and(|token| {
                token.starts_with(neoism_agent_service_api::daemon_credential::PREFIX)
            })
        {
            return self.authenticate(supplied);
        }
        let worker_claims = self
            .worker
            .as_ref()
            .map(|worker| {
                worker
                    .verify(
                        supplied
                            .ok_or_else(|| "missing worker bearer token".to_string())?,
                    )
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        if let (Some(resolver), Some(token)) = (&self.tenant_resolver, supplied) {
            if let Some(resolved) = resolver
                .resolve(token)
                .await
                .map_err(|error| error.to_string())?
            {
                if resolved.tenant_id.trim().is_empty()
                    || resolved.subject.trim().is_empty()
                {
                    return Err(
                        "tenant resolver returned an empty tenant or subject".into()
                    );
                }
                if let (Some(worker), Some(signed)) = (&self.worker, &worker_claims) {
                    worker
                        .validate_resolved(signed, &resolved)
                        .map_err(|e| e.to_string())?;
                }
                let quotas = resolved.quotas.clone();
                return Ok(Some(CallerClaims {
                    subject: resolved.subject.clone(),
                    workspace_id: resolved.workspace_id.clone(),
                    tenant_id: resolved.tenant_id.clone(),
                    directory_prefixes: resolved.directory_prefixes.clone(),
                    hosted: true,
                    max_sessions: quotas.max_sessions,
                    max_artifacts: quotas.max_artifacts,
                    max_artifact_bytes: quotas.max_artifact_bytes,
                    artifact_retention_days: quotas.artifact_retention_days,
                    requests_per_minute: quotas.requests_per_minute,
                    max_in_flight: quotas.max_in_flight,
                    resolved: Some(resolved),
                    worker: self.worker.clone(),
                }));
            }
        }
        if self.strict_hosted {
            return Err("invalid hosted bearer token".to_string());
        }
        self.authenticate(supplied)
    }

    pub(crate) fn authenticate(
        &self,
        supplied: Option<&str>,
    ) -> Result<Option<CallerClaims>, String> {
        if self.strict_hosted {
            return Err("hosted authentication requires the injected resolver".into());
        }
        if supplied.is_some_and(|token| {
            token.starts_with(neoism_agent_service_api::daemon_credential::PREFIX)
        }) {
            let token = supplied.expect("checked above");
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "system clock is before Unix epoch".to_string())?
                .as_secs() as i64;
            // The signing daemon and this verifier can drift: the env var is
            // captured at process start, but the canonical token file rotates
            // when the runtime dir is recreated, and dev profiles keep their
            // token in a different file than a standalone daemon. On a
            // signature mismatch (or missing env key), retry against the
            // canonical on-disk token before rejecting — both processes on
            // one machine share that trust root.
            let env_key = self.daemon_key.as_deref();
            let verified = match env_key {
                Some(key) => {
                    neoism_agent_service_api::daemon_credential::verify(token, key, now)
                }
                None => Err("daemon credential verifier is not configured"),
            };
            let claims = match verified {
                Ok(claims) => claims,
                Err(env_error) => {
                    let file_key = canonical_daemon_token_from_disk()
                        .filter(|file_key| Some(file_key.as_bytes()) != env_key);
                    match file_key {
                        Some(file_key) => {
                            neoism_agent_service_api::daemon_credential::verify(
                                token,
                                file_key.as_bytes(),
                                now,
                            )
                            .map_err(|_| env_error.to_string())?
                        }
                        None => return Err(env_error.to_string()),
                    }
                }
            };
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
                resolved: None,
                worker: None,
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
                resolved: None,
                worker: None,
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
                resolved: None,
                worker: None,
            }))
            .ok_or_else(|| "invalid bearer token".to_string())
    }

    pub(crate) fn begin_request(
        &self,
        claims: &CallerClaims,
    ) -> Result<RequestGuard, &'static str> {
        if let Some(worker) = &self.worker {
            if worker.validate().is_err()
                || !claims.resolved.as_ref().is_some_and(|r| {
                    r.expires_at.is_some_and(|expiry| {
                        neoism_agent_service_api::workspace_worker::unix_now()
                            .is_ok_and(|now| expiry > now)
                    })
                })
            {
                return Err("worker identity or credential has expired");
            }
        }
        self.usage.begin_request(claims)
    }
}

impl CallerClaims {
    pub(crate) fn execution_policy(&self) -> neoism_agent_service_api::ExecutionPolicy {
        use neoism_agent_service_api::ExecutionPolicy;
        if self.hosted {
            return if self.worker.as_ref().is_some_and(|w| w.validate().is_ok())
                && self.resolved.as_ref().is_some_and(|r| {
                    r.expires_at.is_some_and(|expiry| {
                        neoism_agent_service_api::workspace_worker::unix_now()
                            .is_ok_and(|now| expiry > now)
                    })
                }) {
                ExecutionPolicy::NativeLocal
            } else {
                ExecutionPolicy::Disabled
            };
        }
        ExecutionPolicy::NativeLocal
    }

    pub(crate) fn actor_type_label(&self) -> &'static str {
        match self.resolved.as_ref().map(|resolved| &resolved.actor_type) {
            Some(neoism_agent_service_api::ActorType::ServiceAccount) => {
                "service-account"
            }
            _ => "human",
        }
    }

    pub(crate) fn quotas(&self) -> neoism_agent_service_api::TenantQuotas {
        neoism_agent_service_api::TenantQuotas {
            max_sessions: self.max_sessions,
            max_artifacts: self.max_artifacts,
            max_artifact_bytes: self.max_artifact_bytes,
            artifact_retention_days: self.artifact_retention_days,
            requests_per_minute: self.requests_per_minute,
            max_in_flight: self.max_in_flight,
        }
    }
}

/// Native tools belong to the deployment, not the session creator. A local
/// daemon-bound workspace (including guests) shares the host's capabilities;
/// a hosted control plane never derives trust from a tenant name.
pub(crate) fn local_collaboration_session(
    hosted: bool,
    session: &neoism_agent_core::SessionInfo,
) -> bool {
    if hosted {
        return false;
    }
    let tenant = session_tenant(session);
    tenant == "local"
        || session
            .workspace_id
            .as_ref()
            .is_some_and(|workspace_id| tenant == format!("workspace:{workspace_id}"))
}

pub(crate) fn allows_session_path(
    hosted: bool,
    session: &neoism_agent_core::SessionInfo,
    path: &std::path::Path,
) -> bool {
    if local_collaboration_session(hosted, session) {
        return true;
    }
    let prefixes = session
        .extra
        .get(DIRECTORY_PREFIXES_EXTRA_KEY)
        .and_then(serde_json::Value::as_array);
    if let Some(prefixes) = prefixes.filter(|prefixes| !prefixes.is_empty()) {
        return prefixes.iter().any(|prefix| {
            prefix
                .as_str()
                .map(std::path::Path::new)
                .is_some_and(|prefix| prefix.is_absolute() && path.starts_with(prefix))
        });
    }
    path.starts_with(std::path::Path::new(&session.directory))
}

/// The explicit serialized policy is parsed before any local collaboration
/// override. Unknown/removed modes and malformed values always fail closed.
pub(crate) fn session_execution_policy(
    services: &neoism_agent_service_api::AgentServices,
    session: &neoism_agent_core::SessionInfo,
) -> neoism_agent_service_api::ExecutionPolicy {
    use neoism_agent_service_api::ExecutionPolicy;
    let policy = match session.extra.get(EXECUTION_POLICY_EXTRA_KEY) {
        Some(value) => match serde_json::from_value::<ExecutionPolicy>(value.clone()) {
            Ok(policy) => Some(policy),
            Err(_) => return ExecutionPolicy::Disabled,
        },
        None => None,
    };
    if services.shared_control_plane() {
        return ExecutionPolicy::Disabled;
    }
    if let Some(worker) = &services.workspace_worker {
        return if services.hosted
            && policy == Some(ExecutionPolicy::NativeLocal)
            && worker_session_admitted(worker, session)
        {
            ExecutionPolicy::NativeLocal
        } else {
            ExecutionPolicy::Disabled
        };
    }
    if local_collaboration_session(services.hosted, session) {
        ExecutionPolicy::NativeLocal
    } else {
        policy.unwrap_or(ExecutionPolicy::Disabled)
    }
}

pub(crate) fn worker_session_admitted(
    worker: &neoism_agent_service_api::WorkspaceWorkerBinding,
    session: &neoism_agent_core::SessionInfo,
) -> bool {
    let workspace = session.workspace_id.as_ref().map(ToString::to_string);
    worker.admits_session(
        session_tenant(session),
        workspace.as_deref(),
        std::path::Path::new(&session.directory),
    )
}

/// Worker-aware path gate for parent route/tool integration. Do not use the
/// legacy local-collaboration path gate directly in worker runtimes.
pub(crate) fn services_allow_session_path(
    services: &neoism_agent_service_api::AgentServices,
    session: &neoism_agent_core::SessionInfo,
    path: &std::path::Path,
) -> bool {
    if let Some(worker) = &services.workspace_worker {
        if !services.hosted || !worker_session_admitted(worker, session) {
            return false;
        }
        if let Some(prefixes) = session
            .extra
            .get(DIRECTORY_PREFIXES_EXTRA_KEY)
            .and_then(serde_json::Value::as_array)
            .filter(|p| !p.is_empty())
        {
            return prefixes
                .iter()
                .filter_map(serde_json::Value::as_str)
                .any(|prefix| {
                    worker.admits_scoped_path(std::path::Path::new(prefix), path)
                });
        }
        return worker.admits_scoped_path(std::path::Path::new(&session.directory), path);
    }
    allows_session_path(services.hosted, session, path)
}

pub(crate) fn native_execution_allowed(
    policy: &neoism_agent_service_api::ExecutionPolicy,
) -> bool {
    matches!(
        policy,
        neoism_agent_service_api::ExecutionPolicy::NativeLocal
    )
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
    if let Some(worker) = &claims.worker {
        if !worker.admits_path(std::path::Path::new(directory))
            || !claims.resolved.as_ref().is_some_and(|r| {
                r.expires_at.is_some_and(|expiry| {
                    neoism_agent_service_api::workspace_worker::unix_now()
                        .is_ok_and(|now| expiry > now)
                })
            })
        {
            return false;
        }
    }
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
    if let Some(worker) = &claims.worker {
        if !worker_session_admitted(worker, session) {
            return false;
        }
    }
    if !claims.hosted
        && claims.workspace_id.is_none()
        && claims.tenant_id == "local"
        && session
            .extra
            .get(HOST_LOCAL_ACCESS_KEY)
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    {
        return allows_directory(claims, &session.directory);
    }
    claims.workspace_id.as_ref().is_none_or(|workspace_id| {
        session
            .workspace_id
            .as_ref()
            .is_some_and(|session_workspace| {
                session_workspace.to_string() == *workspace_id
            })
    }) && allows_session_scope(claims, session_tenant(session), &session.directory)
}

fn allows_session_scope(claims: &CallerClaims, tenant_id: &str, directory: &str) -> bool {
    tenant_id == claims.tenant_id && allows_directory(claims, directory)
}

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
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

    fn test_services(hosted: bool) -> neoism_agent_service_api::AgentServices {
        let mut services = crate::standard_services();
        services.hosted = hosted;
        services
    }

    #[test]
    fn workspace_name_cannot_elevate_a_disabled_session() {
        let session: neoism_agent_core::SessionInfo =
            serde_json::from_value(serde_json::json!({
                "id": "ses_test",
                "slug": "test",
                "projectId": "project",
                "workspaceId": "workspace-a",
                "directory": "/tmp",
                "title": "test",
                "version": "test",
                "time": { "created": 1, "updated": 1 },
                "neoismTenantId": "workspace:workspace-a",
                "neoismExecutionPolicy": {"mode":"disabled"}
            }))
            .unwrap();
        assert_eq!(
            session_execution_policy(&test_services(true), &session),
            neoism_agent_service_api::ExecutionPolicy::Disabled
        );
    }

    #[test]
    fn joined_guest_in_local_deployment_has_native_tools_and_external_paths() {
        let session: neoism_agent_core::SessionInfo =
            serde_json::from_value(serde_json::json!({
                "id": "ses_guest", "slug": "guest", "projectId": "project",
                "workspaceId": "workspace-a", "directory": "/tmp", "title": "guest",
                "version": "test", "time": { "created": 1, "updated": 1 },
                "neoismTenantId": "workspace:workspace-a",
                "neoismCreatedBy": "device:guest", "neoismExecutionPolicy": {"mode":"disabled"},
                "neoismDirectoryPrefixes": ["/tmp"]
            }))
            .unwrap();
        assert!(local_collaboration_session(false, &session));
        assert_eq!(
            session_execution_policy(&test_services(false), &session),
            neoism_agent_service_api::ExecutionPolicy::NativeLocal
        );
        assert!(allows_session_path(
            false,
            &session,
            std::path::Path::new("/external")
        ));
        assert!(!local_collaboration_session(true, &session));
        assert_eq!(
            session_execution_policy(&test_services(true), &session),
            neoism_agent_service_api::ExecutionPolicy::Disabled
        );
        assert!(!allows_session_path(
            true,
            &session,
            std::path::Path::new("/external")
        ));
    }

    fn worker_fixture() -> (
        neoism_agent_service_api::WorkspaceWorkerBinding,
        neoism_agent_service_api::WorkspaceWorkerSigningKey,
        neoism_agent_service_api::WorkspaceWorkerCredentialClaims,
        neoism_agent_core::SessionInfo,
    ) {
        use neoism_agent_service_api::*;
        let root = std::env::temp_dir().join(format!(
            "caller-worker-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let now = workspace_worker::unix_now().unwrap();
        let key = WorkspaceWorkerSigningKey::new([9u8; 32]).unwrap();
        let binding = WorkspaceWorkerBinding::new(
            WorkspaceWorkerBootstrap {
                version: 1,
                tenant_id: "tenant-a".into(),
                workspace_id: "ws-a".into(),
                runtime_id: "runtime-new".into(),
                runtime_generation: 5,
                root: root.clone(),
                expires_at: now + 600,
            },
            key.verification_key(),
        )
        .unwrap();
        let signed = WorkspaceWorkerCredentialClaims {
            version: 1,
            tenant_id: "tenant-a".into(),
            workspace_id: "ws-a".into(),
            runtime_id: "runtime-new".into(),
            runtime_generation: 5,
            subject: "actor-a".into(),
            actor_type: ActorType::Human,
            directory_prefix: root.clone(),
            scopes: vec![],
            quotas: TenantQuotas::default(),
            issued_at: now,
            expires_at: now + 120,
        };
        let session = serde_json::from_value(serde_json::json!({
            "id":"ses_worker", "slug":"worker", "projectId":"project", "workspaceId":"ws-a",
            "directory":root, "title":"worker", "version":"test", "time":{"created":1,"updated":1},
            "neoismTenantId":"tenant-a", "neoismExecutionPolicy":{"mode":"native-local"}
        })).unwrap();
        (binding, key, signed, session)
    }

    #[tokio::test]
    async fn worker_credentials_reject_wrong_tenant_workspace_generation_and_expiry() {
        use neoism_agent_service_api::*;
        let (binding, key, signed, session) = worker_fixture();
        let policy = CallerPolicy::for_workspace_worker(
            Some(Arc::new(WorkspaceWorkerTenantResolver::new(
                binding.clone(),
            ))),
            Arc::new(binding.clone()),
        );
        let claims = policy
            .authenticate_request(Some(&key.issue(&signed).unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert!(allows_session(&claims, &session));
        assert_eq!(claims.execution_policy(), ExecutionPolicy::NativeLocal);
        for kind in 0..4 {
            let mut wrong = signed.clone();
            match kind {
                0 => wrong.tenant_id = "tenant-b".into(),
                1 => wrong.workspace_id = "ws-b".into(),
                2 => wrong.runtime_generation = 4,
                _ => {
                    wrong.issued_at -= 200;
                    wrong.expires_at = signed.issued_at - 1;
                }
            }
            assert!(policy
                .authenticate_request(Some(&key.issue(&wrong).unwrap()))
                .await
                .is_err());
        }
        assert!(policy.authenticate_request(None).await.is_err());
        assert!(policy.authenticate(Some("local-token")).is_err());
        let mut expired = claims;
        expired.resolved.as_mut().unwrap().expires_at = Some(signed.issued_at - 1);
        assert!(policy.begin_request(&expired).is_err());
        assert!(!allows_session(&expired, &session));
        let _ = std::fs::remove_dir_all(binding.root());
    }

    struct WorkerMismatchResolver(neoism_agent_service_api::ResolvedTenant);
    impl neoism_agent_service_api::TenantResolver for WorkerMismatchResolver {
        fn resolve<'a>(
            &'a self,
            _: &'a str,
        ) -> neoism_agent_service_api::ServiceFuture<
            'a,
            Result<
                Option<neoism_agent_service_api::ResolvedTenant>,
                neoism_agent_service_api::ServiceError,
            >,
        > {
            Box::pin(async move { Ok(Some(self.0.clone())) })
        }
    }
    #[tokio::test]
    async fn custom_resolver_cannot_bypass_worker_signature_or_actor_claims() {
        let (binding, key, signed, _) = worker_fixture();
        let mut wrong = signed.resolved_tenant();
        wrong.subject = "other-actor".into();
        let policy = CallerPolicy::for_workspace_worker(
            Some(Arc::new(WorkerMismatchResolver(wrong))),
            Arc::new(binding.clone()),
        );
        assert!(policy
            .authenticate_request(Some(&key.issue(&signed).unwrap()))
            .await
            .is_err());
        assert!(policy
            .authenticate_request(Some("unsigned-token"))
            .await
            .is_err());
        let _ = std::fs::remove_dir_all(binding.root());
    }

    #[test]
    fn logical_worker_sessions_survive_runtime_replacement_but_shared_cannot_elevate() {
        use neoism_agent_service_api::ExecutionPolicy;
        let (binding, _, _, mut session) = worker_fixture();
        let worker_services = test_services(false).for_workspace_worker(binding.clone());
        // No runtime identity in durable session extras. Even stale machine-only
        // metadata is irrelevant: live credentials, not sessions, bind a runtime.
        session
            .extra
            .insert("neoismRuntimeGeneration".into(), serde_json::json!(1));
        assert_eq!(
            session_execution_policy(&worker_services, &session),
            ExecutionPolicy::NativeLocal
        );
        assert_eq!(
            session_execution_policy(&test_services(true), &session),
            ExecutionPolicy::Disabled
        );
        assert!(services_allow_session_path(
            &worker_services,
            &session,
            &binding.root().join("new.txt")
        ));
        assert!(!services_allow_session_path(
            &worker_services,
            &session,
            binding.root().parent().unwrap()
        ));
        session
            .extra
            .insert(TENANT_EXTRA_KEY.into(), serde_json::json!("other"));
        assert_eq!(
            session_execution_policy(&worker_services, &session),
            ExecutionPolicy::Disabled
        );
        session
            .extra
            .insert(TENANT_EXTRA_KEY.into(), serde_json::json!("tenant-a"));
        session.workspace_id = Some("other".into());
        assert_eq!(
            session_execution_policy(&worker_services, &session),
            ExecutionPolicy::Disabled
        );
        session.workspace_id = Some("ws-a".into());
        session.extra.remove(EXECUTION_POLICY_EXTRA_KEY);
        assert_eq!(
            session_execution_policy(&worker_services, &session),
            ExecutionPolicy::Disabled
        );
        session
            .extra
            .insert(TENANT_EXTRA_KEY.into(), serde_json::json!("local"));
        assert_eq!(
            session_execution_policy(&test_services(false), &session),
            ExecutionPolicy::NativeLocal
        );
        for invalid in [
            serde_json::json!("native-local"),
            serde_json::json!({"mode":"sandboxed"}),
            serde_json::json!({"mode":"unknown"}),
        ] {
            session
                .extra
                .insert(EXECUTION_POLICY_EXTRA_KEY.into(), invalid);
            assert_eq!(
                session_execution_policy(&test_services(false), &session),
                ExecutionPolicy::Disabled
            );
        }
        let _ = std::fs::remove_dir_all(binding.root());
    }

    #[test]
    fn signature_mismatch_falls_back_to_canonical_token_file() {
        let runtime = std::env::temp_dir()
            .join(format!("neoism-caller-fallback-{}", std::process::id()));
        let dir = runtime.join("neoism");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("daemon-token"), "file-key\n").unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims =
            neoism_agent_service_api::daemon_credential::DaemonCredentialClaims::new(
                "guest".to_string(),
                "ws-1",
                "workspace:ws-1".to_string(),
                vec!["/tmp".to_string()],
                true,
                now,
                60,
            )
            .unwrap();
        let credential =
            neoism_agent_service_api::daemon_credential::issue(&claims, b"file-key")
                .unwrap();

        // Env-captured key is STALE (the drift scenario); the canonical file
        // holds the signer's key.
        let policy = CallerPolicy {
            daemon_key: Some(Arc::<[u8]>::from(b"stale-env-key".as_slice())),
            hosted_config: Ok(None),
            local_token: None,
            usage: Arc::new(UsageTracker::default()),
            tenant_resolver: None,
            strict_hosted: false,
            worker: None,
        };
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        let verified = policy.authenticate(Some(&credential));
        std::env::remove_var("XDG_RUNTIME_DIR");
        let claims = verified.expect("file-key fallback").expect("claims");
        assert_eq!(claims.subject, "guest");
        assert_eq!(claims.workspace_id.as_deref(), Some("ws-1"));

        // A credential signed with a key matching NEITHER still fails.
        let bogus = neoism_agent_service_api::daemon_credential::issue(
            &claims_for_bogus(now),
            b"other",
        )
        .unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        let rejected = policy.authenticate(Some(&bogus));
        std::env::remove_var("XDG_RUNTIME_DIR");
        assert!(rejected.is_err());
        let _ = std::fs::remove_dir_all(&runtime);
    }

    fn claims_for_bogus(
        now: i64,
    ) -> neoism_agent_service_api::daemon_credential::DaemonCredentialClaims {
        neoism_agent_service_api::daemon_credential::DaemonCredentialClaims::new(
            "intruder".to_string(),
            "ws-2",
            "workspace:ws-2".to_string(),
            vec![],
            true,
            now,
            60,
        )
        .unwrap()
    }

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
            resolved: None,
            worker: None,
        }
    }

    #[test]
    fn hosted_workspace_without_isolating_provider_has_execution_disabled() {
        let mut guest = claims("workspace:workspace-a".into());
        guest.workspace_id = Some("workspace-a".into());
        assert_eq!(
            guest.execution_policy(),
            neoism_agent_service_api::ExecutionPolicy::Disabled
        );
    }

    struct TestTenantResolver;

    impl neoism_agent_service_api::TenantResolver for TestTenantResolver {
        fn backend_name(&self) -> &'static str {
            "test"
        }

        fn resolve<'a>(
            &'a self,
            bearer: &'a str,
        ) -> neoism_agent_service_api::ServiceFuture<
            'a,
            Result<
                Option<neoism_agent_service_api::ResolvedTenant>,
                neoism_agent_service_api::ServiceError,
            >,
        > {
            Box::pin(async move {
                Ok((bearer == "synapse-token").then(|| {
                    neoism_agent_service_api::ResolvedTenant {
                        tenant_id: "company-a".into(),
                        subject: "user-a".into(),
                        actor_type: neoism_agent_service_api::ActorType::Human,
                        scopes: vec!["sessions.prompt".into()],
                        directory_prefixes: vec!["/workspace/company-a".into()],
                        workspace_id: None,
                        quotas: neoism_agent_service_api::TenantQuotas {
                            max_sessions: Some(12),
                            ..Default::default()
                        },
                        execution: neoism_agent_service_api::ExecutionPolicy::NativeLocal,
                        runtime_id: None,
                        runtime_generation: None,
                        expires_at: None,
                    }
                }))
            })
        }
    }

    #[tokio::test]
    async fn injected_tenant_resolver_supplies_actor_quota_and_execution_policy() {
        let policy =
            CallerPolicy::from_env_with_resolver(Some(Arc::new(TestTenantResolver)));
        let claims = policy
            .authenticate_request(Some("synapse-token"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claims.tenant_id, "company-a");
        assert_eq!(claims.subject, "user-a");
        assert_eq!(claims.max_sessions, Some(12));
        assert_eq!(
            claims.execution_policy(),
            neoism_agent_service_api::ExecutionPolicy::Disabled
        );
    }

    #[tokio::test]
    async fn missing_hosted_resolver_denies_all_authentication_without_panicking() {
        let mut policy = CallerPolicy::for_hosted(None);
        policy.local_token = Some("local-secret".into());
        policy.daemon_key = Some(Arc::from(b"daemon-secret".as_slice()));
        assert!(policy.authenticate_request(None).await.is_err());
        assert!(policy
            .authenticate_request(Some("local-secret"))
            .await
            .is_err());
        assert!(policy
            .authenticate_request(Some("daemon-secret"))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn hosted_requests_never_fall_back_to_local_or_daemon_auth() {
        let mut policy = CallerPolicy::for_hosted(Some(Arc::new(TestTenantResolver)));
        policy.local_token = Some("local-secret".into());
        assert!(policy.authenticate_request(None).await.is_err());
        assert!(policy
            .authenticate_request(Some("local-secret"))
            .await
            .is_err());
        assert!(policy
            .authenticate_request(Some("invalid-hosted-token"))
            .await
            .is_err());
        let resolved = policy
            .authenticate_request(Some("synapse-token"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.tenant_id, "company-a");
        assert!(resolved.resolved.is_some());
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
