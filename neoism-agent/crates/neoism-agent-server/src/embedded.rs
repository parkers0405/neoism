//! Owned, listener-free access to the same router used by the native server.

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context;
use axum::{body::Body, http::Request, response::Response, Router};
use neoism_agent_service_api::{
    AgentServices, ConfigDiscoveryScope, ConfigLayer, ConfigSnapshot,
    ConfigSnapshotRequest, ConfigSourceService, ConfigUpdateRequest,
    ConfigWritableTarget, ServiceError, ServiceFuture,
};
use tokio::sync::{watch, Mutex, RwLock};
use tower::ServiceExt;

use crate::{AppState, ManagementPolicy};

pub struct EmbeddedOptions {
    pub database: PathBuf,
    pub services: AgentServices,
    pub management: ManagementPolicy,
    pub shutdown_timeout: Duration,
}

impl EmbeddedOptions {
    /// Native services may use local configuration and credentials. For a shared
    /// installation supply validated hosted services, not local defaults.
    pub fn new(database: impl Into<PathBuf>, services: AgentServices) -> Self {
        Self {
            database: database.into(),
            services,
            management: ManagementPolicy::disabled(),
            shutdown_timeout: Duration::from_secs(30),
        }
    }

    /// Replace config discovery with an immutable host-owned document. This
    /// does not sandbox native tools or replace the credential services.
    pub fn with_config(mut self, document: serde_json::Value) -> anyhow::Result<Self> {
        serde_json::from_value::<neoism_agent_core::AgentConfigDocument>(document.clone())
            .context("invalid embedded Agent configuration")?;
        self.services.config = Arc::new(EmbeddedConfig(document));
        Ok(self)
    }
}

struct EmbeddedConfig(serde_json::Value);

impl ConfigSourceService for EmbeddedConfig {
    fn snapshot(&self, request: &ConfigSnapshotRequest) -> Result<ConfigSnapshot, ServiceError> {
        Ok(ConfigSnapshot {
            identity: format!("embedded:{}", self.0),
            workspace: request.workspace.clone(),
            layers: vec![ConfigLayer {
                source_id: "embedded".into(),
                scope: ConfigDiscoveryScope::Installation,
                document: self.0.clone(),
                writable: false,
            }],
            discovery_roots: vec![],
            writable_target: ConfigWritableTarget {
                source_id: "embedded".into(),
                label: "host-owned configuration".into(),
            },
        })
    }

    fn update<'a>(&'a self, _: &'a ConfigUpdateRequest) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
        Box::pin(async { Err(ServiceError::new("embedded configuration is host-owned and immutable")) })
    }
}

/// Clones share one runtime and lifecycle. `close` retires every clone.
///
/// This opens no socket and does not bypass route authentication. HTTP responses,
/// including SSE bodies, are returned directly without buffering. WebSocket
/// upgrades require a real server transport and are not provided by this host.
/// Call `close().await` before dropping the last handle or reopening its database.
#[derive(Clone)]
pub struct EmbeddedAgent {
    inner: Arc<Inner>,
}

struct Inner {
    state: AppState,
    router: Router,
    stopping: watch::Sender<bool>,
    admission: Arc<RwLock<()>>,
    close: Mutex<Close>,
    timeout: Duration,
}

enum Close {
    Open,
    Running(tokio::task::JoinHandle<Result<(), String>>),
    Finished(Result<(), String>),
}

impl EmbeddedAgent {
    pub async fn open(options: EmbeddedOptions) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !options.database.as_os_str().is_empty(),
            "database path is required"
        );
        anyhow::ensure!(
            !options.shutdown_timeout.is_zero(),
            "shutdown timeout must be positive"
        );
        options
            .services
            .validate()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let state = AppState::open_database_with_services_and_management(
            options.database,
            options.services,
            options.management,
        )
        .await?;
        let router = crate::app(state.clone());
        let (stopping, _) = watch::channel(false);
        Ok(Self {
            inner: Arc::new(Inner {
                state,
                router,
                stopping,
                admission: Arc::new(RwLock::new(())),
                close: Mutex::new(Close::Open),
                timeout: options.shutdown_timeout,
            }),
        })
    }

    /// Dispatch through the existing API middleware, handlers, and plugin routes.
    /// Preserve Authorization headers: an in-memory caller is not automatically
    /// a privileged or tenant-authenticated caller.
    pub async fn request(&self, request: Request<Body>) -> anyhow::Result<Response> {
        let _admission = self.inner.admission.read().await;
        let mut stopping = self.inner.stopping.subscribe();
        anyhow::ensure!(!*stopping.borrow(), "embedded agent is closing or closed");
        tokio::select! {
            biased;
            _ = stopping.changed() => anyhow::bail!("embedded agent is closing or closed"),
            response = self.inner.router.clone().oneshot(request) => {
                Ok(response.expect("Axum Router service is infallible"))
            }
        }
    }

    /// Stop admission, cancel pending request futures, and retire the same native
    /// state used by the server. Existing SSE streams terminate during retirement.
    /// Shutdown continues if this caller drops its future; another caller can
    /// await the same result. A failure is sticky: do not reopen that database in
    /// this process after incomplete retirement.
    pub async fn close(&self) -> anyhow::Result<()> {
        let mut close = self.inner.close.lock().await;
        if matches!(*close, Close::Open) {
            self.inner.state.begin_shutdown();
            self.inner.stopping.send_replace(true);
            let state = self.inner.state.clone();
            let admission = self.inner.admission.clone();
            let timeout = self.inner.timeout;
            *close = Close::Running(tokio::spawn(async move {
                tokio::time::timeout(timeout, async {
                    let _retirement = admission.write().await;
                    state.shutdown().await.map_err(|error| error.to_string())
                })
                .await
                .map_err(|_| "embedded agent shutdown exceeded its deadline".to_owned())?
            }));
        }
        if let Close::Running(task) = &mut *close {
            let result = task.await.unwrap_or_else(|error| {
                Err(format!("embedded shutdown task failed: {error}"))
            });
            *close = Close::Finished(result);
        }
        match &*close {
            Close::Finished(Ok(())) => Ok(()),
            Close::Finished(Err(error)) => {
                Err(anyhow::anyhow!(error.clone())).context(crate::AgentShutdownError)
            }
            _ => unreachable!("shutdown result is recorded before returning"),
        }
    }
}

#[cfg(test)]
mod tests;
