//! Dedicated worker-local secret files. Never use a shared control-plane file or
//! the user's global auth files here: the controller supplies private paths in
//! the isolated runtime. Every operation checks the live single-workspace scope
//! and secret path. These are path-based guards, not no-follow secure-fd I/O:
//! controller-owned private directories must prevent concurrent same-user path
//! and temporary-file mutation. This does not establish OS isolation.
use crate::workspace_worker::validate_worker_secret_path;
use crate::*;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct WorkspaceWorkerProviderCredentialStore {
    binding: WorkspaceWorkerBinding,
    inner: LocalProviderCredentialStore,
    path: PathBuf,
}
#[derive(Clone)]
pub struct WorkspaceWorkerMcpCredentialStore {
    binding: WorkspaceWorkerBinding,
    inner: LocalMcpCredentialStore,
    path: PathBuf,
}
fn check(
    binding: &WorkspaceWorkerBinding,
    scope: &CredentialScope,
    path: &Path,
) -> Result<(), ServiceError> {
    validate_worker_secret_path(binding, path)?;
    if scope.tenant_id != binding.tenant_id()
        || scope.workspace_id.as_deref() != Some(binding.workspace_id())
    {
        return Err(ServiceError::new(
            "credential scope does not match workspace worker",
        ));
    }
    Ok(())
}
fn bound_scope(binding: &WorkspaceWorkerBinding) -> CredentialScope {
    CredentialScope {
        tenant_id: binding.tenant_id().into(),
        workspace_id: Some(binding.workspace_id().into()),
    }
}
impl WorkspaceWorkerProviderCredentialStore {
    /// `path` must be absolute, outside the workspace, and have an existing
    /// controller-owned private parent directory. Final symlinks are rejected;
    /// path admission is rechecked on every operation, but is not race-free I/O.
    pub fn new(
        binding: WorkspaceWorkerBinding,
        path: impl Into<PathBuf>,
    ) -> Result<Self, ServiceError> {
        let path = path.into();
        validate_worker_secret_path(&binding, &path)?;
        Ok(Self {
            binding,
            inner: LocalProviderCredentialStore::new(path.clone()),
            path,
        })
    }
}
impl WorkspaceWorkerMcpCredentialStore {
    /// `path` must be absolute, outside the workspace, and have an existing
    /// controller-owned private parent directory. Final symlinks are rejected;
    /// path admission is rechecked on every operation, but is not race-free I/O.
    pub fn new(
        binding: WorkspaceWorkerBinding,
        path: impl Into<PathBuf>,
    ) -> Result<Self, ServiceError> {
        let path = path.into();
        validate_worker_secret_path(&binding, &path)?;
        Ok(Self {
            binding,
            inner: LocalMcpCredentialStore::new(path.clone()),
            path,
        })
    }
    fn restore(&self, attempt: Option<McpOAuthAttempt>) -> Option<McpOAuthAttempt> {
        attempt.map(|mut a| {
            a.scope = bound_scope(&self.binding);
            a
        })
    }
}
impl ProviderCredentialStore for WorkspaceWorkerProviderCredentialStore {
    fn backend_name(&self) -> &'static str {
        "workspace-worker-local"
    }
    fn supports_hosted_scopes(&self) -> bool {
        validate_worker_secret_path(&self.binding, &self.path).is_ok()
    }
    fn list<'a>(
        &'a self,
        provider_id: Option<&'a str>,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<Vec<ProviderConnectionSummary>, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner.list(provider_id, scope).await
        })
    }
    fn get<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<Option<ProviderCredential>, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner.get(connection, scope).await
        })
    }
    fn resolve<'a>(
        &'a self,
        provider_id: &'a str,
        connection_id: Option<&'a str>,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<
        'a,
        Result<Option<(ProviderConnectionRef, ProviderCredential)>, ServiceError>,
    > {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner.resolve(provider_id, connection_id, scope).await
        })
    }
    fn create<'a>(
        &'a self,
        request: CreateProviderConnection,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, &request.scope, &self.path)?;
            self.inner.create(request).await
        })
    }
    fn rename<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
        label: &'a str,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner.rename(connection, scope, label).await
        })
    }
    fn delete<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<bool, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner.delete(connection, scope).await
        })
    }
    fn set_default<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
    ) -> ServiceFuture<'a, Result<ProviderConnectionSummary, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner.set_default(connection, scope).await
        })
    }
    fn update_credential<'a>(
        &'a self,
        connection: &'a ProviderConnectionRef,
        scope: &'a CredentialScope,
        credential: ProviderCredential,
    ) -> ServiceFuture<'a, Result<(), ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner
                .update_credential(connection, scope, credential)
                .await
        })
    }
}
impl McpCredentialStore for WorkspaceWorkerMcpCredentialStore {
    fn supports_hosted_scopes(&self) -> bool {
        validate_worker_secret_path(&self.binding, &self.path).is_ok()
    }
    fn get<'a>(
        &'a self,
        scope: &'a CredentialScope,
        connection: &'a McpConnectionRef,
    ) -> ServiceFuture<'a, Result<Option<McpCredential>, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner.get(&CredentialScope::local(), connection).await
        })
    }
    fn put<'a>(
        &'a self,
        scope: &'a CredentialScope,
        connection: &'a McpConnectionRef,
        credential: McpCredential,
    ) -> ServiceFuture<'a, Result<(), ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner
                .put(&CredentialScope::local(), connection, credential)
                .await
        })
    }
    fn delete<'a>(
        &'a self,
        scope: &'a CredentialScope,
        connection: &'a McpConnectionRef,
    ) -> ServiceFuture<'a, Result<bool, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            self.inner
                .delete(&CredentialScope::local(), connection)
                .await
        })
    }
    fn put_attempt<'a>(
        &'a self,
        mut attempt: McpOAuthAttempt,
    ) -> ServiceFuture<'a, Result<(), ServiceError>> {
        Box::pin(async move {
            check(&self.binding, &attempt.scope, &self.path)?;
            if !self.binding.admits_path(Path::new(&attempt.directory)) {
                return Err(ServiceError::new(
                    "MCP attempt directory outside worker root",
                ));
            }
            attempt.scope = CredentialScope::local();
            self.inner.put_attempt(attempt).await
        })
    }
    fn consume_attempt<'a>(
        &'a self,
        state: &'a str,
        scope: Option<&'a CredentialScope>,
    ) -> ServiceFuture<'a, Result<Option<McpOAuthAttempt>, ServiceError>> {
        Box::pin(async move {
            validate_worker_secret_path(&self.binding, &self.path)?;
            if let Some(scope) = scope {
                check(&self.binding, scope, &self.path)?;
            }
            let attempt = self
                .inner
                .consume_attempt(state, Some(&CredentialScope::local()))
                .await?;
            Ok(self
                .restore(attempt)
                .filter(|a| self.binding.admits_path(Path::new(&a.directory))))
        })
    }
    fn consume_connection_attempt<'a>(
        &'a self,
        scope: &'a CredentialScope,
        connection: &'a McpConnectionRef,
    ) -> ServiceFuture<'a, Result<Option<McpOAuthAttempt>, ServiceError>> {
        Box::pin(async move {
            check(&self.binding, scope, &self.path)?;
            let attempt = self
                .inner
                .consume_connection_attempt(&CredentialScope::local(), connection)
                .await?;
            Ok(self
                .restore(attempt)
                .filter(|a| self.binding.admits_path(Path::new(&a.directory))))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn worker_files_only_serve_the_single_bound_scope() {
        let root =
            std::env::temp_dir().join(format!("worker-secrets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let workspace = root.join("workspace");
        let private = root.join("private");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&private).unwrap();
        let binding = WorkspaceWorkerBinding::new(
            WorkspaceWorkerBootstrap {
                version: 1,
                tenant_id: "tenant-a".into(),
                workspace_id: "workspace-a".into(),
                runtime_id: "runtime-a".into(),
                runtime_generation: 1,
                root: workspace,
                expires_at: workspace_worker::unix_now().unwrap() + 600,
            },
            WorkspaceWorkerSigningKey::new([17u8; 32])
                .unwrap()
                .verification_key(),
        )
        .unwrap();
        let provider = WorkspaceWorkerProviderCredentialStore::new(
            binding.clone(),
            private.join("provider.json"),
        )
        .unwrap();
        let mcp = WorkspaceWorkerMcpCredentialStore::new(
            binding.clone(),
            private.join("mcp.json"),
        )
        .unwrap();
        let scope = bound_scope(&binding);
        assert!(provider.supports_hosted_scopes());
        assert!(mcp.supports_hosted_scopes());
        let summary = provider
            .create(CreateProviderConnection {
                provider_id: "example".into(),
                label: "worker".into(),
                scope: scope.clone(),
                credential: ProviderCredential::Api {
                    key: "private-worker-key".into(),
                    metadata: None,
                },
                set_default: true,
            })
            .await
            .unwrap();
        assert_eq!(summary.scope, scope);
        let connection = McpConnectionRef {
            connection_id: "worker-mcp".into(),
            server_url: "https://mcp.example/".into(),
        };
        assert!(mcp.get(&scope, &connection).await.unwrap().is_none());
        assert_eq!(provider.list(None, &scope).await.unwrap().len(), 1);
        for wrong in [
            CredentialScope::local(),
            CredentialScope {
                tenant_id: "tenant-b".into(),
                workspace_id: scope.workspace_id.clone(),
            },
            CredentialScope {
                tenant_id: scope.tenant_id.clone(),
                workspace_id: Some("other".into()),
            },
            CredentialScope {
                tenant_id: scope.tenant_id.clone(),
                workspace_id: None,
            },
        ] {
            assert!(provider.list(None, &wrong).await.is_err());
            assert!(provider.resolve("example", None, &wrong).await.is_err());
            assert!(mcp.get(&wrong, &connection).await.is_err());
            assert!(mcp.consume_attempt("nonce", Some(&wrong)).await.is_err());
        }
        assert!(WorkspaceWorkerProviderCredentialStore::new(
            binding.clone(),
            binding.root().join("provider.json")
        )
        .is_err());
        assert!(WorkspaceWorkerMcpCredentialStore::new(
            binding.clone(),
            PathBuf::from("relative.json")
        )
        .is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let workspace_secret = binding.root().join("workspace-secret.json");
            std::fs::write(&workspace_secret, "{}").unwrap();
            std::fs::remove_file(private.join("provider.json")).unwrap();
            symlink(&workspace_secret, private.join("provider.json")).unwrap();
            symlink(&workspace_secret, private.join("mcp.json")).unwrap();
            assert!(WorkspaceWorkerProviderCredentialStore::new(
                binding.clone(),
                private.join("provider.json")
            )
            .is_err());
            assert!(WorkspaceWorkerMcpCredentialStore::new(
                binding.clone(),
                private.join("mcp.json")
            )
            .is_err());
            // Stores registered before a file swap recheck before I/O.
            assert!(!provider.supports_hosted_scopes());
            assert!(!mcp.supports_hosted_scopes());
            assert!(provider.list(None, &scope).await.is_err());
            assert!(provider.resolve("example", None, &scope).await.is_err());
            assert!(mcp.get(&scope, &connection).await.is_err());
            assert!(mcp.delete(&scope, &connection).await.is_err());
            assert!(mcp.consume_attempt("nonce", None).await.is_err());
            assert_eq!(std::fs::read_to_string(&workspace_secret).unwrap(), "{}");
            std::fs::remove_file(private.join("provider.json")).unwrap();
            std::fs::remove_file(private.join("mcp.json")).unwrap();
            let original = root.join("private-original");
            std::fs::rename(&private, &original).unwrap();
            symlink(binding.root(), &private).unwrap();
            assert!(provider.list(None, &scope).await.is_err());
            assert!(mcp.get(&scope, &connection).await.is_err());
            assert!(WorkspaceWorkerProviderCredentialStore::new(
                binding.clone(),
                private.join("provider.json")
            )
            .is_err());
            assert!(WorkspaceWorkerMcpCredentialStore::new(
                binding.clone(),
                private.join("mcp.json")
            )
            .is_err());
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
