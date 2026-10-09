use crate::{
    ActorType, HostApprovedAccess, HostError, HostStatus, Result, TenantQuotas,
    Verification, WorkspaceHost,
};
use neoism_cloud_runtime::{Binding, MachineState, WorkerConnection};
use serde::Deserialize;
use std::time::Duration;

/// Checked on the exact provider endpoint BEFORE any credential is sent. The
/// implementation is trusted host code (not a client/provider supplied allowlist).
pub trait TrustedEndpointPolicy: Send + Sync {
    fn allows(&self, connection: &WorkerConnection) -> bool;
}
/// Exact scheme/host/port origin allowlist. HTTPS cert validation remains enabled.
pub struct AllowedOrigins(Vec<String>);
impl AllowedOrigins {
    pub fn new(origins: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut out = Vec::new();
        for origin in origins {
            let url = url::Url::parse(&origin).map_err(|_| HostError::Invalid)?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/"
            {
                return Err(HostError::Invalid);
            }
            out.push(url.origin().ascii_serialization());
        }
        Ok(Self(out))
    }
}
impl TrustedEndpointPolicy for AllowedOrigins {
    fn allows(&self, connection: &WorkerConnection) -> bool {
        url::Url::parse(connection.agent_api_base_url())
            .ok()
            .is_some_and(|url| self.0.contains(&url.origin().ascii_serialization()))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInfo {
    deployment: String,
    execution_available: bool,
    worker: WorkerInfo,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerInfo {
    version: u32,
    tenant_id: String,
    workspace_id: String,
    runtime_id: String,
    runtime_generation: u64,
    root: String,
    expires_at: i64,
}
#[derive(Deserialize)]
struct HealthInfo {
    healthy: bool,
    version: String,
}
impl WorkspaceHost {
    pub(crate) async fn wait_ready(
        &self,
        expected: Binding,
        deadline: tokio::time::Instant,
    ) -> Result<HostStatus> {
        let mut delay = Duration::from_millis(50);
        loop {
            let attempt = async {
                let b = self.runtime.reconcile(&expected.allocation.owner).await?;
                if b.allocation != expected.allocation
                    || expected.status.as_ref().is_some_and(|s| {
                        b.status.as_ref().map(|current| &current.handle)
                            != Some(&s.handle)
                    })
                {
                    return Err(HostError::Stale);
                }
                if b.retired
                    || b.status.as_ref().is_some_and(|s| {
                        matches!(
                            s.state,
                            MachineState::Failed
                                | MachineState::Stopped
                                | MachineState::Destroyed
                        )
                    })
                {
                    return Err(HostError::Unready);
                }
                self.verify(b).await
            };
            match tokio::time::timeout_at(deadline, attempt).await {
                Err(_) => return Err(HostError::Timeout),
                Ok(Ok(v)) => return Ok(v),
                Ok(Err(HostError::Unready)) => {}
                Ok(Err(e)) => return Err(e),
            }
            if tokio::time::Instant::now() + delay >= deadline {
                return Err(HostError::Timeout);
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(2));
        }
    }
    pub(crate) async fn verify(&self, b: Binding) -> Result<HostStatus> {
        if b.retired || b.pending.is_some() || b.last_error.is_some() {
            return Err(HostError::Unready);
        }
        let s = b.status.as_ref().ok_or(HostError::Unready)?;
        if s.state != MachineState::Running {
            return Err(HostError::Unready);
        }
        s.validate(&b.allocation, Some(&s.handle))
            .map_err(|_| HostError::Unready)?;
        let c = s.connection.as_ref().ok_or(HostError::Unready)?;
        if !self.endpoints.allows(c) {
            return Err(HostError::Denied);
        }
        let d = b.allocation.launch.as_ref().ok_or(HostError::Unready)?;
        let probe = HostApprovedAccess {
            subject: "host-readiness".into(),
            actor_type: ActorType::ServiceAccount,
            directory_prefix: d.root().into(),
            scopes: vec!["agent:read".into()],
            quotas: TenantQuotas::default(),
            ttl_seconds: 30,
        };
        let (token, exp) = self.issue(&b, &probe)?;
        let info: RuntimeInfo = self.get_json(c, "v2/runtime", &token).await?;
        let w = info.worker;
        if info.deployment != "workspace-worker"
            || !info.execution_available
            || w.version != 1
            || w.tenant_id != b.allocation.owner.tenant
            || w.workspace_id != b.allocation.owner.workspace
            || w.runtime_id != d.runtime_id()
            || w.runtime_generation != b.allocation.generation
            || !neoism_agent_service_api::worker_vm_path_contains(
                std::path::Path::new(d.root()),
                std::path::Path::new(&w.root),
            )
            || !neoism_agent_service_api::worker_vm_path_contains(
                std::path::Path::new(&w.root),
                std::path::Path::new(d.root()),
            )
            || w.expires_at != d.expires_at()
            || w.expires_at <= crate::now()?
        {
            return Err(HostError::Unready);
        }
        if let Some(expected) = &self.launch.expected_image_version {
            let health: HealthInfo = self.get_json(c, "v2/health", &token).await?;
            if !health.healthy || &health.version != expected {
                return Err(HostError::Unready);
            }
        }
        self.recheck(&b)?;
        let verification = Verification {
            at: crate::now()?,
            expires_at: exp,
            handle: s.handle.clone(),
            descriptor: d.clone(),
            revision: b.revision,
        };
        Ok(HostStatus {
            binding: b,
            verification: Some(verification),
        })
    }
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        c: &WorkerConnection,
        path: &str,
        token: &str,
    ) -> Result<T> {
        // Whole operation bounded, including body streamed by a slow peer.
        let op = async {
            let base = url::Url::parse(c.agent_api_base_url())
                .map_err(|_| HostError::Unready)?;
            let target = base.join(path).map_err(|_| HostError::Unready)?;
            let mut auth =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| HostError::Unready)?;
            auth.set_sensitive(true);
            let mut response = self
                .client
                .get(target)
                .header(reqwest::header::AUTHORIZATION, auth)
                .send()
                .await
                .map_err(|_| HostError::Unready)?;
            if !response.status().is_success()
                || !response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.split(';').next() == Some("application/json"))
            {
                return Err(HostError::Unready);
            }
            if response.content_length().is_some_and(|len| len > 16_384) {
                return Err(HostError::Unready);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) =
                response.chunk().await.map_err(|_| HostError::Unready)?
            {
                if bytes.len() + chunk.len() > 16_384 {
                    return Err(HostError::Unready);
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| HostError::Unready)
        };
        tokio::time::timeout(self.launch.probe_timeout, op)
            .await
            .map_err(|_| HostError::Unready)?
    }
}
