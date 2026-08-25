#![recursion_limit = "512"]

#[cfg(test)]
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::Arc;

mod agent_tool_registry;
mod artifact_routes;
mod app_router;
pub mod auth_cli;
mod audit_routes;
mod background_job;
mod caller;
mod command_routes;
mod config;
mod custom_tool;
#[cfg(test)]
mod edit_smoke_tests;
mod error;
mod executable;
mod external_acp;
mod external_agent;
mod global_routes;
mod instruction;
mod interaction;
pub mod language_server;
mod lsp;
mod lsp_routes;
mod mcp;
mod mcp_auth;
mod mcp_routes;
mod message_model;
mod message_part_mutation;
mod model_selection;
mod openapi;
pub use openapi::canonical_openapi;
mod perf;
mod permission;
mod permission_runtime;
mod platform_shell;
mod plugin;
mod plugin_adapters;
mod plugins;
mod project;
mod project_routes;
mod provider_stream_message;
mod provider_stream_processor;

mod provider {
    pub(crate) use neoism_agent_builtins::provider::{estimate_tokens, ProviderEventStream, ProviderStream};
}
mod provider_error {
    pub(crate) use neoism_agent_builtins::provider_error::ProviderError;
}
mod pty;
mod pty_routes;
mod route_query;
mod context_epoch;
mod semantic;
mod server_util;
mod session_actions;
mod session_context;
mod session_coordinator;
mod session_export_route;
mod session_helpers;
mod session_import_route;
mod session_loop;
mod session_message_routes;
mod session_prompt;
mod session_prompt_routes;
mod session_queue;
mod session_retry;
mod session_routes;
mod session_run;
mod session_transfer;
mod session_undo;
mod skill;
mod snapshot;
mod state;
mod sync;
mod tool;
mod tool_routes;
mod tool_runtime;
mod tool_selection;
mod utility_runtime;
mod v2_routes;
mod vcs;
pub(crate) mod windows_process;
mod workflow;
mod workspace_runtime;

pub(crate) use agent_tool_registry::{
    available_tools_for_directory, execute_mcp_gateway,
    provider_tools_for_agent,
};
use anyhow::Context;
pub use app_router::app;
#[cfg(test)]
use command_routes::{command_arguments, expand_command_template};
#[cfg(test)]
use message_part_mutation::{
    append_text_delta, append_tool_input_delta, finish_text_part,
    mark_interrupted_tool_parts, set_tool_completed, set_tool_running,
};
pub(crate) use model_selection::{
    model_ref_from_config, model_ref_from_config_with_variant,
    model_ref_from_user_model, user_model_from_model_ref,
};
#[cfg(test)]
use neoism_agent_core::event_type;
#[cfg(test)]
use neoism_agent_core::{
    AssistantMessage, AssistantPath, CompletedTime, PartTime, PermissionAction,
    PermissionRequestInfo, ProviderStreamEvent, QuestionRequestInfo, ReasoningPart,
    SessionQueueStatus, SessionStatus, TimeInfo, TodoInfo, TokenUsage, ToolListItem,
    ToolPart, ToolState,
};
#[cfg(test)]
use neoism_agent_core::{
    CompactionPart, CreatedTime, EventPayload, Id, IdKind, MessageInfo, MessageWithParts,
    Page, Part, PermissionRule, PromptPart, PromptRequest, ProviderRole,
    SessionInfo, TextPart, UserMessage, UserModel,
};
pub(crate) use permission_runtime::{
    ask_permission_for_tool, parse_permission_required_error, permission_grants,
    permission_request_allowed,
};
pub use route_query::{InstanceQuery, VcsDiffQuery};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
pub(crate) use server_util::{
    default_state_dir, now_millis,
    resolve_directory, slug,
};
#[cfg(test)]
use session_context::build_session_summary;
#[cfg(test)]
use session_context::provider_messages_for_session;
use session_context::{compact_session_context, title_from_parts};
pub(crate) use session_helpers::{
    ensure_session, filter_sessions, message_id_of, part_id_of,
};
#[cfg(test)]
use session_loop::{next_provider_stream_event, ProviderEventPoll};
pub(crate) use session_prompt::append_prompt;
#[cfg(test)]
use session_queue::queued_prompt_count;
pub(crate) use session_routes::SessionListQuery;
#[cfg(test)]
use session_run::busy_status;
#[cfg(test)]
use session_run::finish_session_run;
use session_run::publish_idle_if_no_run;
pub use session_transfer::{
    export_session, export_sessions_under_workspace_root, import_session, SessionBundle,
    SESSION_BUNDLE_VERSION,
};
pub use state::AppState;
#[cfg(test)]
use state::SessionRun;
use tokio::net::TcpListener;
use tool_runtime::ensure_tool_permission;
#[cfg(test)]
use tool_runtime::execute_tool_call;
#[cfg(test)]
use tool_runtime::execute_tool_call_with_permission_wait;
#[cfg(test)]
use tool_selection::normalize_provider_tool_name;
use tool_selection::{tool_allowed_for_model, use_apply_patch_for_model};

pub fn services_with_workspace_search(
    workspace_search: std::sync::Arc<dyn neoism_agent_service_api::WorkspaceSearchService>,
) -> neoism_agent_service_api::AgentServices {
    neoism_agent_service_api::AgentServices::new(
        std::sync::Arc::new(neoism_agent_service_api::StandardExecutableService),
        workspace_search,
    )
}

pub fn standard_workspace_search(
) -> std::sync::Arc<dyn neoism_agent_service_api::WorkspaceSearchService> {
    std::sync::Arc::new(UnavailableWorkspaceSearch)
}

/// Minimal services for server-internal helpers which do not perform workspace
/// search. Standalone binaries explicitly inject their chosen search adapter.
pub fn standard_services() -> neoism_agent_service_api::AgentServices {
    services_with_workspace_search(standard_workspace_search())
}

struct UnavailableWorkspaceSearch;

impl neoism_agent_service_api::WorkspaceSearchService for UnavailableWorkspaceSearch {
    fn warm(&self, _root: &std::path::Path) -> Result<(), neoism_agent_service_api::ServiceError> { Ok(()) }
    fn pin_root(&self, _root: &std::path::Path) -> Result<std::sync::Arc<dyn neoism_agent_service_api::WorkspaceSearchRootPin>, neoism_agent_service_api::ServiceError> {
        Err(neoism_agent_service_api::ServiceError::new("workspace search service was not injected"))
    }
    fn find_files(&self, _request: &neoism_agent_service_api::FindFilesRequest) -> Result<neoism_agent_service_api::FindFilesResult, neoism_agent_service_api::ServiceError> {
        Err(neoism_agent_service_api::ServiceError::new("workspace search service was not injected"))
    }
    fn grep(&self, _request: &neoism_agent_service_api::GrepWorkspaceRequest) -> Result<neoism_agent_service_api::GrepWorkspaceResult, neoism_agent_service_api::ServiceError> {
        Err(neoism_agent_service_api::ServiceError::new("workspace search service was not injected"))
    }
    fn search_directories(&self, _request: &neoism_agent_service_api::DirectorySearchRequest) -> Result<neoism_agent_service_api::DirectorySearchResult, neoism_agent_service_api::ServiceError> {
        Err(neoism_agent_service_api::ServiceError::new("workspace search service was not injected"))
    }
}

#[derive(Clone)]
pub struct ServerOptions {
    pub hostname: String,
    pub port: u16,
    pub cors: Vec<String>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            hostname: "127.0.0.1".to_string(),
            port: 4096,
            cors: Vec::new(),
        }
    }
}

pub async fn listen(
    options: ServerOptions,
    services: neoism_agent_service_api::AgentServices,
) -> anyhow::Result<SocketAddr> {
    let started = crate::perf::now();
    let address: SocketAddr = format!("{}:{}", options.hostname, options.port)
        .parse()
        .with_context(|| {
            format!(
                "invalid listen address {}:{}",
                options.hostname, options.port
            )
        })?;
    if !address.ip().is_loopback()
        && std::env::var_os("NEOISM_AGENT_TOKEN").is_none()
        && std::env::var_os("NEOISM_AGENT_AUTH_CONFIG").is_none()
        && std::env::var("NEOISM_AGENT_ALLOW_UNAUTHENTICATED_REMOTE").as_deref() != Ok("1")
    {
        anyhow::bail!(
            "refusing unauthenticated non-loopback agent server; set NEOISM_AGENT_TOKEN or explicitly set NEOISM_AGENT_ALLOW_UNAUTHENTICATED_REMOTE=1"
        );
    }
    tracing::info!(
        target: "neoism_agent::perf",
        host = %options.hostname,
        port = options.port,
        perf_enabled = crate::perf::enabled(),
        "server listen starting"
    );
    let bind_started = crate::perf::now();
    let listener = TcpListener::bind(address).await?;
    let actual = listener.local_addr()?;
    tracing::info!(
        target: "neoism_agent::perf",
        listen_addr = %actual,
        bind_ms = crate::perf::elapsed_ms(bind_started),
        "server socket bound"
    );
    let state_started = crate::perf::now();
    let state = AppState::open_default(services).await?;
    tracing::info!(
        target: "neoism_agent::perf",
        listen_addr = %actual,
        state_open_ms = crate::perf::elapsed_ms(state_started),
        total_start_ms = crate::perf::elapsed_ms(started),
        "server state opened"
    );
    let result = axum::serve(listener, app_router::app_with_cors(state.clone(), &options.cors)).await;
    state.shutdown().await;
    tracing::warn!(
        target: "neoism_agent::perf",
        listen_addr = %actual,
        total_ms = crate::perf::elapsed_ms(started),
        error = result.as_ref().err().map(|error| error.to_string()),
        "server serve loop exited"
    );
    result?;
    Ok(actual)
}

#[cfg(test)]
mod tests;
