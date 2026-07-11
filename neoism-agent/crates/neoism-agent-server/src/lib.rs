#![recursion_limit = "512"]

#[cfg(test)]
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::Arc;

mod agent;
mod agent_tool_registry;
mod app_router;
mod app_routes;
mod auth_store;
mod background_job;
mod command_routes;
mod compat_routes;
mod config;
mod custom_tool;
#[cfg(test)]
mod edit_smoke_tests;
mod error;
mod event_routes;
mod experimental_routes;
mod external_acp;
mod external_agent;
mod file_routes;
mod firecrawl;
mod global_routes;
mod goal_routes;
mod instruction;
mod interaction;
mod lsp;
mod lsp_routes;
mod managed_lsp_path;
mod mcp;
mod mcp_auth;
mod mcp_memory;
mod mcp_notes;
mod mcp_routes;
mod message_model;
mod message_part_mutation;
mod model_selection;
mod openapi;
mod perf;
mod permission;
mod permission_runtime;
mod plugin;
mod project;
mod project_routes;
mod provider;
mod provider_auth;
mod provider_auth_browser;
mod provider_catalog;
mod provider_error;
mod provider_responses;
mod provider_routes;
mod provider_stream_message;
mod provider_stream_processor;
mod provider_transform;
mod pty;
mod pty_routes;
mod route_query;
pub mod rust_lsp;
mod search_routes;
mod semantic;
mod server_util;
mod session_actions;
mod session_context;
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
mod v2_routes;
mod vcs;
mod vcs_routes;
mod worktree;
mod worktree_routes;

#[cfg(test)]
use agent::AgentCatalog;
pub(crate) use agent_tool_registry::{
    available_tools_for_directory, configured_mcp_tools_with_state,
    execute_mcp_tool_by_runtime_id, provider_tools_for_agent,
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
    model_from_body, model_ref_from_config, model_ref_from_config_with_variant,
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
    CreatedTime, EventPayload, Id, IdKind, MessageInfo, MessageWithParts, Page, Part,
    PermissionRule, PromptPart, PromptRequest, ProviderMessage, ProviderRole,
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
    default_cache_dir, default_config_dir, default_state_dir, now_millis,
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
use state::AppState;
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

pub async fn listen(options: ServerOptions) -> anyhow::Result<SocketAddr> {
    let started = crate::perf::now();
    let address: SocketAddr = format!("{}:{}", options.hostname, options.port)
        .parse()
        .with_context(|| {
            format!(
                "invalid listen address {}:{}",
                options.hostname, options.port
            )
        })?;
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
    let state = AppState::open_default().await?;
    crate::semantic::spawn_indexer(state.clone());
    tracing::info!(
        target: "neoism_agent::perf",
        listen_addr = %actual,
        state_open_ms = crate::perf::elapsed_ms(state_started),
        total_start_ms = crate::perf::elapsed_ms(started),
        "server state opened"
    );
    let result = axum::serve(listener, app(state)).await;
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
