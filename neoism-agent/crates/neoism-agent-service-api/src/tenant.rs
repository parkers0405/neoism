use serde::{Deserialize, Serialize};

use crate::{ServiceError, ServiceFuture};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActorType {
    Human,
    ServiceAccount,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantQuotas {
    pub max_sessions: Option<usize>,
    pub max_artifacts: Option<usize>,
    pub max_artifact_bytes: Option<usize>,
    pub artifact_retention_days: Option<u64>,
    pub requests_per_minute: Option<u32>,
    pub max_in_flight: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum ExecutionPolicy {
    Disabled,
    NativeLocal,
    Sandboxed {
        provider: String,
        idle_ttl_seconds: u64,
        max_lifetime_seconds: u64,
    },
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self::Disabled
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTenant {
    pub tenant_id: String,
    pub subject: String,
    pub actor_type: ActorType,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub directory_prefixes: Vec<String>,
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub quotas: TenantQuotas,
    #[serde(default)]
    pub execution: ExecutionPolicy,
}

/// Host-owned identity boundary. Implementations validate a bearer token and
/// return immutable tenant and actor claims; model-controlled input never
/// participates in this resolution.
pub trait TenantResolver: Send + Sync {
    fn backend_name(&self) -> &'static str {
        "injected"
    }

    fn resolve<'a>(
        &'a self,
        bearer: &'a str,
    ) -> ServiceFuture<'a, Result<Option<ResolvedTenant>, ServiceError>>;
}