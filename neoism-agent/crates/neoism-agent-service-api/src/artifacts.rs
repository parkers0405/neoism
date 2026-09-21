use crate::{ServiceError, ServiceFuture};

/// Host-injected durable artifact bytes. Metadata and authorization remain in
/// the Agent store; this backend makes the payload available to every replica.
pub trait ArtifactBlobStore: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn shared(&self) -> bool;
    fn put<'a>(
        &'a self,
        tenant_id: &'a str,
        artifact_id: &'a str,
        bytes: &'a [u8],
    ) -> ServiceFuture<'a, Result<(), ServiceError>>;
    fn get<'a>(
        &'a self,
        tenant_id: &'a str,
        artifact_id: &'a str,
    ) -> ServiceFuture<'a, Result<Option<Vec<u8>>, ServiceError>>;
    fn delete<'a>(
        &'a self,
        tenant_id: &'a str,
        artifact_id: &'a str,
    ) -> ServiceFuture<'a, Result<(), ServiceError>>;
}