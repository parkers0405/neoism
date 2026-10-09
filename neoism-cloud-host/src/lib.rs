//! Provider-neutral whole-workspace host and credential broker.
pub mod docker;
mod http;
mod manager;
mod readiness;
mod signing;
pub use http::*;
pub use manager::*;
pub use neoism_agent_service_api::{ActorType, TenantQuotas};
pub use neoism_cloud_runtime as runtime;
pub use readiness::{AllowedOrigins, TrustedEndpointPolicy};
pub use signing::*;

pub type Result<T> = std::result::Result<T, HostError>;
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("runtime operation failed")]
    Runtime(#[from] neoism_cloud_runtime::Error),
    #[error("private signing store unavailable")]
    Signing,
    #[error("invalid host configuration or access")]
    Invalid,
    #[error("workspace access denied")]
    Denied,
    #[error("worker is not verified and available")]
    Unready,
    #[error("worker binding changed")]
    Stale,
    #[error("worker admission expired")]
    Expired,
    #[error("worker verification deadline exceeded")]
    Timeout,
}
pub(crate) fn now() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|v| i64::try_from(v.as_secs()).ok())
        .ok_or(HostError::Invalid)
}
