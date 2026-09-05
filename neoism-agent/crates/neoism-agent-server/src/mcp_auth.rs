use std::sync::Arc;

use anyhow::anyhow;
#[cfg(test)]
use neoism_agent_service_api::LocalMcpCredentialStore;
use neoism_agent_service_api::{
    CredentialScope, McpConnectionRef, McpCredential, McpCredentialStore,
    McpOAuthAttempt, McpOAuthClientRegistration, McpOAuthTokens,
};

pub(crate) type McpAuthTokens = McpOAuthTokens;
pub(crate) type McpAuthClientInfo = McpOAuthClientRegistration;
pub(crate) type McpAuthEntry = McpCredential;

/// Request/session-scoped facade over the host-injected async store.
#[derive(Clone)]
pub(crate) struct McpAuthStore {
    store: Arc<dyn McpCredentialStore>,
    scope: CredentialScope,
}

impl McpAuthStore {
    pub(crate) fn from_services(
        services: &neoism_agent_service_api::AgentServices,
        scope: CredentialScope,
        hosted: bool,
    ) -> anyhow::Result<Self> {
        if hosted && !services.mcp_credentials.supports_hosted_scopes() {
            return Err(anyhow!("hosted MCP credentials require an injected tenant-isolated McpCredentialStore"));
        }
        Ok(Self {
            store: services.mcp_credentials.clone(),
            scope,
        })
    }

    pub(crate) fn local(services: &neoism_agent_service_api::AgentServices) -> Self {
        Self {
            store: services.mcp_credentials.clone(),
            scope: CredentialScope::local(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            store: Arc::new(LocalMcpCredentialStore::new(path)),
            scope: CredentialScope::local(),
        }
    }

    pub(crate) fn connection(name: &str, server_url: &str) -> McpConnectionRef {
        McpConnectionRef {
            connection_id: name.to_string(),
            server_url: server_url.to_string(),
        }
    }

    pub(crate) async fn get_for_url(
        &self,
        name: &str,
        server_url: &str,
    ) -> anyhow::Result<Option<McpAuthEntry>> {
        Ok(self
            .store
            .get(&self.scope, &Self::connection(name, server_url))
            .await?)
    }

    pub(crate) async fn set_for_url(
        &self,
        name: &str,
        server_url: &str,
        entry: McpAuthEntry,
    ) -> anyhow::Result<()> {
        Ok(self
            .store
            .put(&self.scope, &Self::connection(name, server_url), entry)
            .await?)
    }

    pub(crate) async fn remove_for_url(
        &self,
        name: &str,
        server_url: &str,
    ) -> anyhow::Result<bool> {
        Ok(self
            .store
            .delete(&self.scope, &Self::connection(name, server_url))
            .await?)
    }

    pub(crate) async fn clear_tokens(
        &self,
        name: &str,
        server_url: &str,
    ) -> anyhow::Result<bool> {
        let Some(mut entry) = self.get_for_url(name, server_url).await? else {
            return Ok(false);
        };
        let removed = entry.tokens.take().is_some();
        self.set_for_url(name, server_url, entry).await?;
        Ok(removed)
    }

    #[cfg(test)]
    pub(crate) async fn update_tokens(
        &self,
        name: &str,
        server_url: &str,
        tokens: McpAuthTokens,
    ) -> anyhow::Result<()> {
        let mut entry = self
            .get_for_url(name, server_url)
            .await?
            .unwrap_or_default();
        entry.tokens = Some(tokens);
        self.set_for_url(name, server_url, entry).await
    }

    pub(crate) async fn put_attempt(
        &self,
        attempt: McpOAuthAttempt,
    ) -> anyhow::Result<()> {
        if attempt.scope != self.scope {
            return Err(anyhow!("MCP OAuth attempt scope mismatch"));
        }
        Ok(self.store.put_attempt(attempt).await?)
    }

    pub(crate) async fn consume_attempt(
        &self,
        state: &str,
        enforce_scope: bool,
    ) -> anyhow::Result<Option<McpOAuthAttempt>> {
        Ok(self
            .store
            .consume_attempt(state, enforce_scope.then_some(&self.scope))
            .await?)
    }

    pub(crate) async fn consume_unscoped_attempt(
        &self,
        state: &str,
    ) -> anyhow::Result<Option<McpOAuthAttempt>> {
        Ok(self.store.consume_attempt(state, None).await?)
    }

    pub(crate) fn for_attempt(&self, attempt: &McpOAuthAttempt) -> anyhow::Result<Self> {
        if attempt.scope != CredentialScope::local()
            && !self.store.supports_hosted_scopes()
        {
            return Err(anyhow!("hosted MCP credentials require an injected tenant-isolated McpCredentialStore"));
        }
        Ok(Self {
            store: self.store.clone(),
            scope: attempt.scope.clone(),
        })
    }

    pub(crate) async fn consume_connection_attempt(
        &self,
        name: &str,
        server_url: &str,
    ) -> anyhow::Result<Option<McpOAuthAttempt>> {
        Ok(self
            .store
            .consume_connection_attempt(&self.scope, &Self::connection(name, server_url))
            .await?)
    }

    pub(crate) fn scope(&self) -> &CredentialScope {
        &self.scope
    }
}
