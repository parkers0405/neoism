use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::app_routes::{agent_get, agent_list, plugin_status, skill_list};
use crate::artifact_routes::{
    artifact_content, artifact_create, artifact_delete, artifact_get, artifact_list,
};
use crate::audit_routes::audit_list;
use crate::command_routes::command_list;
use crate::compat_routes::{
    empty_array, experimental_console_get, experimental_console_orgs,
    experimental_console_switch, sync_history, sync_replay, sync_start, sync_steal,
};
use crate::event_routes::{event_stream, global_event};
use crate::experimental_routes::{experimental_session_list, resource_list};
use crate::file_routes::{file_list, file_read, file_status};
use crate::global_routes::{
    config_get, config_update, config_validate, global_dispose, global_health,
    global_upgrade, instance_dispose, path_get,
};
use crate::goal_routes::{
    session_goal_clear, session_goal_get, session_goal_research, session_goal_set,
};
use crate::interaction::{
    permission_list, permission_reply, question_list, question_reject, question_reply,
};
use crate::lsp_routes::{
    lsp_code_actions, lsp_definition, lsp_diagnostics, lsp_document_highlights,
    lsp_document_symbols, lsp_formatting, lsp_hover, lsp_implementation,
    lsp_incoming_calls, lsp_inlay_hints, lsp_outgoing_calls, lsp_prepare_call_hierarchy,
    lsp_references, lsp_shutdown, lsp_signature_help, lsp_status, lsp_touch,
};
use crate::mcp_routes::{
    mcp_add, mcp_auth_authenticate, mcp_auth_callback, mcp_auth_callback_get,
    mcp_auth_remove, mcp_auth_start, mcp_catalog, mcp_config_patch, mcp_connect,
    mcp_disconnect, mcp_prompts, mcp_resources, mcp_status, mcp_tool_call, mcp_tools,
};
use crate::openapi::{canonical_openapi_doc, openapi_doc};
use crate::permission_runtime::session_permission_respond;
use crate::project_routes::{
    project_current, project_get, project_init_git, project_list, project_update,
};
use crate::provider_routes::{
    auth_get, auth_remove, auth_set, config_providers, provider_auth_methods,
    provider_list, provider_oauth_authorize, provider_oauth_callback,
};
use crate::pty_routes::{
    pty_connect, pty_connect_token, pty_create, pty_get, pty_list, pty_remove,
    pty_shells, pty_update,
};
use crate::search_routes::{find_file, find_symbol, find_text};
use crate::session_actions::{session_command, session_shell};
use crate::session_export_route::sessions_export;
use crate::session_import_route::session_import;
use crate::session_message_routes::{
    message_delete, message_get, message_list, part_delete, part_update,
};
use crate::session_prompt_routes::{
    prompt, session_abort, session_init, session_summarize,
};
use crate::session_queue::{
    prompt_async, session_queue, session_queue_clear, session_queue_pop,
};
use crate::session_routes::{
    session_children, session_create, session_delete, session_diff,
    session_directory_options, session_fork, session_get, session_list, session_set_pin,
    session_share, session_status, session_todo_list, session_unshare, session_update,
};
use crate::session_undo::{
    session_redo, session_revert, session_undo, session_undo_tree, session_unrevert,
};
use crate::state::AppState;
use crate::tool_routes::{tool_execute, tool_ids, tool_list};
use crate::v2_routes::{
    v2_capabilities, v2_compact, v2_context, v2_events, v2_message_list, v2_meta,
    v2_plugin, v2_plugins, v2_prompt, v2_prompt_async, v2_session_children, v2_session_list,
    v2_wait,
};
use crate::vcs_routes::{vcs_apply, vcs_diff, vcs_diff_raw, vcs_get, vcs_status};
use crate::workflow::{
    workflow_activate, workflow_get, workflow_history, workflow_list, workflow_pause,
    workflow_preview, workflow_run_now,
};
use crate::worktree_routes::{
    worktree_create, worktree_list, worktree_remove, worktree_reset,
};

pub fn app(state: AppState) -> Router {
    app_with_cors(state, &[])
}

pub(crate) fn app_with_cors(state: AppState, allowed_origins: &[String]) -> Router {
    let middleware_state = state.clone();
    let router = Router::new()
        .route("/global/health", get(global_health))
        .route("/global/event", get(global_event))
        .route("/global/config", get(config_get).patch(config_update))
        .route("/global/config/validate", get(config_validate))
        .route("/global/dispose", post(global_dispose))
        .route("/global/upgrade", post(global_upgrade))
        .route("/event", get(event_stream))
        .route("/doc", get(openapi_doc))
        .route("/v2/meta", get(v2_meta))
        .route("/v2/openapi.json", get(canonical_openapi_doc))
        .route("/v2/audit", get(audit_list))
        .route("/v2/capabilities", get(v2_capabilities))
        .route("/v2/plugins", get(v2_plugins))
        .route("/v2/plugins/:plugin_id", get(v2_plugin))
        .route(
            "/v2/plugins/dev.neoism.subagents/sessions/:session_id/tasks",
            get(crate::plugins::subagents::list_tasks),
        )
        .route(
            "/v2/plugins/dev.neoism.subagents/sessions/:session_id/stop",
            post(crate::plugins::subagents::stop_tasks),
        )
        .route("/v2/events", get(v2_events))
        .route("/v2/artifacts", get(artifact_list).post(artifact_create))
        .route(
            "/v2/artifacts/:artifact_id",
            get(artifact_get).delete(artifact_delete),
        )
        .route("/v2/artifacts/:artifact_id/content", get(artifact_content))
        .route("/v2/interactions/permissions", get(permission_list))
        .route(
            "/v2/interactions/permissions/:request_id/reply",
            post(permission_reply),
        )
        .route("/v2/interactions/questions", get(question_list))
        .route(
            "/v2/interactions/questions/:request_id/reply",
            post(question_reply),
        )
        .route(
            "/v2/interactions/questions/:request_id/reject",
            post(question_reject),
        )
        .route("/v2/agents", get(agent_list))
        .route("/v2/agents/:name", get(agent_get))
        .route("/v2/commands", get(command_list))
        .route("/v2/providers", get(provider_list))
        .route("/v2/providers/configured", get(config_providers))
        .route("/v2/providers/auth-methods", get(provider_auth_methods))
        .route(
            "/v2/providers/:provider_id/auth",
            get(auth_get).put(auth_set).delete(auth_remove),
        )
        .route(
            "/v2/providers/:provider_id/oauth/authorize",
            post(provider_oauth_authorize),
        )
        .route(
            "/v2/providers/:provider_id/oauth/callback",
            post(provider_oauth_callback),
        )
        .route("/v2/skills", get(skill_list))
        .route("/v2/tools", get(tool_list))
        .route(
            "/v2/sessions/:session_id/jobs/:job_id",
            delete(crate::background_job::stop_background_task),
        )
        .route("/v2/sessions", get(v2_session_list).post(session_create))
        .route("/v2/sessions/status", get(session_status))
        .route(
            "/v2/sessions/:session_id",
            get(session_get).patch(session_update).delete(session_delete),
        )
        .route("/v2/sessions/:session_id/messages", get(v2_message_list))
        .route("/v2/sessions/:session_id/children", get(v2_session_children))
        .route("/v2/sessions/:session_id/prompt", post(v2_prompt))
        .route("/v2/sessions/:session_id/prompt-async", post(v2_prompt_async))
        .route("/v2/sessions/:session_id/abort", post(session_abort))
        .route("/v2/sessions/:session_id/compact", post(v2_compact))
        .route("/v2/sessions/:session_id/wait", post(v2_wait))
        .route("/v2/sessions/:session_id/context", get(v2_context))
        .route(
            "/v2/sessions/:session_id/queue",
            get(session_queue).delete(session_queue_clear),
        )
        .route("/v2/sessions/:session_id/queue/pop", post(session_queue_pop))
        .route("/v2/sessions/:session_id/commands", post(session_command))
        .route("/v2/sessions/:session_id/undo", post(session_undo))
        .route("/v2/sessions/:session_id/redo", post(session_redo))
        .route("/v2/sessions/:session_id/summarize", post(session_summarize))
        .route("/v2/sessions/:session_id/pin", post(session_set_pin))
        .route("/path", get(path_get))
        .route("/instance/dispose", post(instance_dispose))
        .route("/vcs", get(vcs_get))
        .route("/vcs/diff", get(vcs_diff))
        .route("/vcs/status", get(vcs_status))
        .route("/vcs/diff/raw", get(vcs_diff_raw))
        .route("/vcs/apply", post(vcs_apply))
        .route("/v2/plugins/dev.neoism.vcs", get(vcs_get))
        .route("/v2/plugins/dev.neoism.vcs/diff", get(vcs_diff))
        .route("/v2/plugins/dev.neoism.vcs/status", get(vcs_status))
        .route("/v2/plugins/dev.neoism.vcs/diff/raw", get(vcs_diff_raw))
        .route("/v2/plugins/dev.neoism.vcs/apply", post(vcs_apply))
        .route("/command", get(command_list))
        .route("/agent", get(agent_list))
        .route("/agent/:name", get(agent_get))
        .route("/skill", get(skill_list))
        .route("/workflow", get(workflow_list))
        .route("/workflow/:workflow_id", get(workflow_get))
        .route("/workflow/:workflow_id/activate", post(workflow_activate))
        .route("/workflow/:workflow_id/pause", post(workflow_pause))
        .route("/workflow/:workflow_id/run", post(workflow_run_now))
        .route("/workflow/:workflow_id/preview", get(workflow_preview))
        .route("/workflow/:workflow_id/runs", get(workflow_history))
        .route("/v2/plugins/dev.neoism.workflows", get(workflow_list))
        .route("/v2/plugins/dev.neoism.workflows/:workflow_id", get(workflow_get))
        .route("/v2/plugins/dev.neoism.workflows/:workflow_id/activate", post(workflow_activate))
        .route("/v2/plugins/dev.neoism.workflows/:workflow_id/pause", post(workflow_pause))
        .route("/v2/plugins/dev.neoism.workflows/:workflow_id/run", post(workflow_run_now))
        .route("/v2/plugins/dev.neoism.workflows/:workflow_id/preview", get(workflow_preview))
        .route("/v2/plugins/dev.neoism.workflows/:workflow_id/runs", get(workflow_history))
        .route("/plugin", get(plugin_status))
        .route("/lsp", get(lsp_status))
        .route("/lsp/hover", get(lsp_hover))
        .route("/lsp/signature-help", get(lsp_signature_help))
        .route("/lsp/inlay-hints", get(lsp_inlay_hints))
        .route("/lsp/document-highlights", get(lsp_document_highlights))
        .route("/lsp/definition", get(lsp_definition))
        .route("/lsp/references", get(lsp_references))
        .route("/lsp/implementation", get(lsp_implementation))
        .route(
            "/lsp/prepare-call-hierarchy",
            get(lsp_prepare_call_hierarchy),
        )
        .route("/lsp/incoming-calls", get(lsp_incoming_calls))
        .route("/lsp/outgoing-calls", get(lsp_outgoing_calls))
        .route("/lsp/diagnostics", get(lsp_diagnostics))
        .route("/lsp/document-symbols", get(lsp_document_symbols))
        .route("/lsp/formatting", get(lsp_formatting))
        .route("/lsp/code-actions", get(lsp_code_actions))
        .route("/lsp/touch", post(lsp_touch))
        .route("/lsp/shutdown", post(lsp_shutdown))
        .route("/v2/plugins/dev.neoism.lsp", get(lsp_status))
        .route("/v2/plugins/dev.neoism.lsp/hover", get(lsp_hover))
        .route("/v2/plugins/dev.neoism.lsp/signature-help", get(lsp_signature_help))
        .route("/v2/plugins/dev.neoism.lsp/inlay-hints", get(lsp_inlay_hints))
        .route("/v2/plugins/dev.neoism.lsp/document-highlights", get(lsp_document_highlights))
        .route("/v2/plugins/dev.neoism.lsp/definition", get(lsp_definition))
        .route("/v2/plugins/dev.neoism.lsp/references", get(lsp_references))
        .route("/v2/plugins/dev.neoism.lsp/implementation", get(lsp_implementation))
        .route("/v2/plugins/dev.neoism.lsp/prepare-call-hierarchy", get(lsp_prepare_call_hierarchy))
        .route("/v2/plugins/dev.neoism.lsp/incoming-calls", get(lsp_incoming_calls))
        .route("/v2/plugins/dev.neoism.lsp/outgoing-calls", get(lsp_outgoing_calls))
        .route("/v2/plugins/dev.neoism.lsp/diagnostics", get(lsp_diagnostics))
        .route("/v2/plugins/dev.neoism.lsp/document-symbols", get(lsp_document_symbols))
        .route("/v2/plugins/dev.neoism.lsp/formatting", get(lsp_formatting))
        .route("/v2/plugins/dev.neoism.lsp/code-actions", get(lsp_code_actions))
        .route("/v2/plugins/dev.neoism.lsp/touch", post(lsp_touch))
        .route("/v2/plugins/dev.neoism.lsp/shutdown", post(lsp_shutdown))
        .route("/formatter", get(empty_array))
        .route("/find", get(find_text))
        .route("/find/file", get(find_file))
        .route("/find/symbol", get(find_symbol))
        .route(
            "/search/semantic",
            get(crate::semantic::semantic_search_route),
        )
        .route(
            "/v2/plugins/dev.neoism.semantic/search",
            get(crate::semantic::semantic_search_route),
        )
        .route("/file", get(file_list))
        .route("/file/content", get(file_read))
        .route("/file/status", get(file_status))
        .route("/project", get(project_list))
        .route("/project/current", get(project_current))
        .route("/project/git/init", post(project_init_git))
        .route(
            "/project/:project_id",
            get(project_get).patch(project_update),
        )
        .route("/config", get(config_get).patch(config_update))
        .route("/config/validate", get(config_validate))
        .route("/config/providers", get(config_providers))
        .route("/provider", get(provider_list))
        .route("/provider/auth", get(provider_auth_methods))
        .route(
            "/auth/:provider_id",
            get(auth_get).put(auth_set).delete(auth_remove),
        )
        .route(
            "/provider/:provider_id/oauth/authorize",
            post(provider_oauth_authorize),
        )
        .route(
            "/provider/:provider_id/oauth/callback",
            post(provider_oauth_callback),
        )
        .route("/permission", get(permission_list))
        .route("/permission/:request_id/reply", post(permission_reply))
        .route("/question", get(question_list))
        .route("/question/:request_id/reply", post(question_reply))
        .route("/question/:request_id/reject", post(question_reject))
        .route("/pty/shells", get(pty_shells))
        .route("/pty", get(pty_list).post(pty_create))
        .route(
            "/pty/:pty_id",
            get(pty_get).put(pty_update).delete(pty_remove),
        )
        .route("/pty/:pty_id/connect-token", post(pty_connect_token))
        .route("/pty/:pty_id/connect", get(pty_connect))
        .route(
            "/v2/plugins/dev.neoism.pty/shells",
            get(pty_shells),
        )
        .route(
            "/v2/plugins/dev.neoism.pty",
            get(pty_list).post(pty_create),
        )
        .route(
            "/v2/plugins/dev.neoism.pty/:pty_id",
            get(pty_get).put(pty_update).delete(pty_remove),
        )
        .route(
            "/v2/plugins/dev.neoism.pty/:pty_id/connect-token",
            post(pty_connect_token),
        )
        .route(
            "/v2/plugins/dev.neoism.pty/:pty_id/connect",
            get(pty_connect),
        )
        .route("/sync/start", post(sync_start))
        .route("/sync/replay", post(sync_replay))
        .route("/sync/steal", post(sync_steal))
        .route("/sync/history", post(sync_history))
        .route("/experimental/console", get(experimental_console_get))
        .route("/experimental/console/orgs", get(experimental_console_orgs))
        .route(
            "/experimental/console/switch",
            post(experimental_console_switch),
        )
        .route("/experimental/tool/ids", get(tool_ids))
        .route("/experimental/tool", get(tool_list))
        .route("/experimental/tool/:tool_id/execute", post(tool_execute))
        .route(
            "/experimental/worktree",
            get(worktree_list)
                .post(worktree_create)
                .delete(worktree_remove),
        )
        .route("/experimental/worktree/reset", post(worktree_reset))
        .route("/experimental/session", get(experimental_session_list))
        .route("/experimental/resource", get(resource_list))
        .route("/api/session", get(v2_session_list))
        .route(
            "/api/session/:session_id",
            get(session_get)
                .delete(session_delete)
                .patch(session_update),
        )
        .route(
            "/api/session/:session_id/children",
            get(v2_session_children),
        )
        .route("/api/session/:session_id/todo", get(session_todo_list))
        .route("/api/session/:session_id/fork", post(session_fork))
        .route("/api/session/:session_id/diff", get(session_diff))
        .route(
            "/api/session/:session_id/goal",
            get(session_goal_get)
                .post(session_goal_set)
                .delete(session_goal_clear),
        )
        .route(
            "/api/session/:session_id/goal/research",
            post(session_goal_research),
        )
        .route("/api/session/:session_id/pin", post(session_set_pin))
        .route("/api/session/:session_id/undo", get(session_undo_tree))
        .route("/api/session/:session_id/undo/tree", get(session_undo_tree))
        .route(
            "/api/session/:session_id/summarize",
            post(session_summarize),
        )
        .route("/api/session/:session_id/message", get(v2_message_list))
        .route(
            "/api/session/:session_id/message/:message_id",
            get(message_get).delete(message_delete),
        )
        .route(
            "/api/session/:session_id/message/:message_id/part/:part_id",
            delete(part_delete).patch(part_update),
        )
        .route("/api/session/:session_id/prompt", post(v2_prompt))
        .route(
            "/api/session/:session_id/prompt_async",
            post(v2_prompt_async),
        )
        .route("/api/session/:session_id/abort", post(session_abort))
        .route("/api/session/:session_id/command", post(session_command))
        .route("/api/session/:session_id/shell", post(session_shell))
        .route(
            "/api/session/:session_id/queue",
            get(session_queue).delete(session_queue_clear),
        )
        .route(
            "/api/session/:session_id/queue/pop",
            post(session_queue_pop),
        )
        .route("/api/session/:session_id/revert", post(session_revert))
        .route("/api/session/:session_id/unrevert", post(session_unrevert))
        .route("/api/session/:session_id/undo", post(session_undo))
        .route("/api/session/:session_id/redo", post(session_redo))
        .route("/api/session/:session_id/compact", post(v2_compact))
        .route("/api/session/:session_id/wait", post(v2_wait))
        .route("/api/session/:session_id/context", get(v2_context))
        .route("/session", get(session_list).post(session_create))
        .route("/sessions/import", post(session_import))
        .route("/sessions/export", post(sessions_export))
        .route("/session/status", get(session_status))
        .route("/session/:session_id/children", get(session_children))
        .route("/session/:session_id/todo", get(session_todo_list))
        .route("/session/:session_id/init", post(session_init))
        .route("/session/:session_id/fork", post(session_fork))
        .route(
            "/session/:session_id/share",
            post(session_share).delete(session_unshare),
        )
        .route("/session/:session_id/diff", get(session_diff))
        .route(
            "/session/:session_id/goal",
            get(session_goal_get)
                .post(session_goal_set)
                .delete(session_goal_clear),
        )
        .route(
            "/session/:session_id/goal/research",
            post(session_goal_research),
        )
        .route(
            "/v2/plugins/dev.neoism.goals/:session_id",
            get(session_goal_get)
                .post(session_goal_set)
                .delete(session_goal_clear),
        )
        .route(
            "/v2/plugins/dev.neoism.goals/:session_id/research",
            post(session_goal_research),
        )
        .route("/session/:session_id/pin", post(session_set_pin))
        .route("/session/:session_id/undo", get(session_undo_tree))
        .route("/session/:session_id/undo/tree", get(session_undo_tree))
        .route("/session/:session_id/summarize", post(session_summarize))
        .route(
            "/session/:session_id/directory",
            get(session_directory_options),
        )
        .route(
            "/session/:session_id",
            get(session_get)
                .delete(session_delete)
                .patch(session_update),
        )
        .route(
            "/session/:session_id/message",
            get(message_list).post(prompt),
        )
        .route(
            "/session/:session_id/message/:message_id",
            get(message_get).delete(message_delete),
        )
        .route(
            "/session/:session_id/message/:message_id/part/:part_id",
            delete(part_delete).patch(part_update),
        )
        .route(
            "/session/:session_id/queue",
            get(session_queue).delete(session_queue_clear),
        )
        .route("/session/:session_id/queue/pop", post(session_queue_pop))
        .route("/session/:session_id/prompt_async", post(prompt_async))
        .route("/session/:session_id/abort", post(session_abort))
        .route(
            "/session/:session_id/background-task/:job_id",
            delete(crate::background_job::stop_background_task),
        )
        .route("/session/:session_id/command", post(session_command))
        .route("/session/:session_id/shell", post(session_shell))
        .route("/session/:session_id/revert", post(session_revert))
        .route("/session/:session_id/unrevert", post(session_unrevert))
        .route("/session/:session_id/undo", post(session_undo))
        .route("/session/:session_id/redo", post(session_redo))
        .route(
            "/session/:session_id/permissions/:permission_id",
            post(session_permission_respond),
        )
        .route("/mcp", get(mcp_status).post(mcp_add))
        .route("/mcp/catalog", get(mcp_catalog))
        .route(
            "/mcp/:name/auth",
            post(mcp_auth_start).delete(mcp_auth_remove),
        )
        .route(
            "/mcp/:name/auth/callback",
            get(mcp_auth_callback_get).post(mcp_auth_callback),
        )
        .route("/mcp/:name/auth/authenticate", post(mcp_auth_authenticate))
        .route("/mcp/:name/connect", post(mcp_connect))
        .route("/mcp/:name/disconnect", post(mcp_disconnect))
        .route("/mcp/:name/config", patch(mcp_config_patch))
        .route("/mcp/:name/tools", get(mcp_tools))
        .route("/mcp/:name/tools/:tool_name", post(mcp_tool_call))
        .route("/mcp/:name/resources", get(mcp_resources))
        .route("/mcp/:name/prompts", get(mcp_prompts))
        .route(
            "/v2/plugins/dev.neoism.mcp",
            get(mcp_status).post(mcp_add),
        )
        .route("/v2/plugins/dev.neoism.mcp/catalog", get(mcp_catalog))
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/auth",
            post(mcp_auth_start).delete(mcp_auth_remove),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/auth/callback",
            get(mcp_auth_callback_get).post(mcp_auth_callback),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/auth/authenticate",
            post(mcp_auth_authenticate),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/connect",
            post(mcp_connect),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/disconnect",
            post(mcp_disconnect),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/config",
            patch(mcp_config_patch),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/tools",
            get(mcp_tools),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/tools/:tool_name",
            post(mcp_tool_call),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/resources",
            get(mcp_resources),
        )
        .route(
            "/v2/plugins/dev.neoism.mcp/:name/prompts",
            get(mcp_prompts),
        )
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::info_span!(
                        target: "neoism_agent::perf",
                        "http_request",
                        method = %request.method(),
                        uri = %request.uri(),
                    )
                })
                .on_request(DefaultOnRequest::new().level(Level::TRACE))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(middleware::from_fn_with_state(
            middleware_state,
            authenticate_request,
        ));
    if allowed_origins.is_empty() {
        router.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
    } else {
        let origins = allowed_origins
            .iter()
            .filter_map(|origin| origin.parse::<HeaderValue>().ok())
            .collect::<Vec<_>>();
        router.layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods(Any)
                .allow_headers(Any),
        )
    }
}

async fn authenticate_request(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS || request.uri().path() == "/global/health" {
        return next.run(request).await;
    }
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let claims = match crate::caller::authenticate(supplied) {
        Ok(claims) => claims,
        Err(message) => return auth_error(StatusCode::UNAUTHORIZED, "auth.invalid_token", &message),
    };
    let mut request_guard = None;
    let mut audit_tenant = None;
    if let Some(claims) = claims {
        request_guard = match crate::caller::begin_request(&claims) {
            Ok(guard) => Some(guard),
            Err(message) => {
                return auth_error(StatusCode::TOO_MANY_REQUESTS, "quota.exceeded", message)
            }
        };
        audit_tenant = Some(claims.tenant_id.clone());
        let requested_directory = request_directory(&request);
        if let Some(directory) = requested_directory.as_deref() {
            if !crate::caller::allows_directory(&claims, &directory) {
                return auth_error(
                    StatusCode::FORBIDDEN,
                    "auth.directory_forbidden",
                    "The caller is not authorized for this directory",
                );
            }
        }
        if claims.hosted && hosted_restricted_path(request.uri().path()) {
            return auth_error(
                StatusCode::FORBIDDEN,
                "auth.hosted_route_forbidden",
                "This global credential or configuration route is unavailable in hosted mode",
            );
        }
        if claims.hosted
            && !claims.directory_prefixes.is_empty()
            && requested_directory.is_none()
            && requires_directory_scope(request.uri().path())
        {
            return auth_error(
                StatusCode::BAD_REQUEST,
                "auth.directory_scope_required",
                "This hosted route requires a directory scope",
            );
        }
        let query_session_id = request_session_id(request.uri());
        if claims.hosted
            && matches!(request.uri().path(), "/v2/events" | "/event")
            && query_session_id.is_none()
        {
            return auth_error(
                StatusCode::BAD_REQUEST,
                "auth.session_scope_required",
                "Hosted event streams require sessionId",
            );
        }
        let mut owned_session = session_id_from_path(request.uri().path())
            .map(str::to_string)
            .or(query_session_id);
        if owned_session.is_none() {
            if let Some(request_id) = interaction_id_from_path(request.uri().path()) {
                match state.inner.store.interaction_session_id(request_id).await {
                    Ok(session_id) => owned_session = session_id,
                    Err(error) => {
                        tracing::warn!(%error, "failed to authorize interaction owner");
                        return auth_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "auth.lookup_failed",
                            "Failed to authorize the interaction",
                        );
                    }
                }
            }
        }
        if let Some(session_id) = owned_session.as_deref() {
            match state.inner.store.get_session(session_id).await {
                Ok(Some(session)) if crate::caller::session_tenant(&session) != claims.tenant_id => {
                    return auth_error(
                        StatusCode::FORBIDDEN,
                        "auth.tenant_forbidden",
                        "The caller does not own this session",
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to authorize session owner");
                    return auth_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "auth.lookup_failed",
                        "Failed to authorize the session",
                    );
                }
                _ => {}
            }
        }
        request.extensions_mut().insert(claims);
    }
    if let Some(plugin_id) = route_plugin(request.uri().path()) {
        let directory = request_directory(&request).unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        if !crate::plugins::enabled(&directory, plugin_id) {
            return auth_error(
                StatusCode::NOT_FOUND,
                "plugin.disabled",
                "This plugin is disabled for the workspace",
            );
        }
    }
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if let Some(tenant_id) = audit_tenant {
        let entry = neoism_agent_core::AuditEntry {
            id: neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Audit).to_string(),
            tenant_id,
            method,
            path,
            status: response.status().as_u16(),
            created: crate::now_millis(),
        };
        if let Err(error) = state.inner.store.append_audit(&entry).await {
            tracing::warn!(%error, "failed to append hosted audit entry");
        }
    }
    drop(request_guard);
    response
}

fn route_plugin(path: &str) -> Option<&'static str> {
    if path == "/search/semantic" || path == "/v2/plugins/dev.neoism.semantic/search" {
        Some("dev.neoism.semantic")
    } else if path == "/workflow"
        || path.starts_with("/workflow/")
        || path == "/v2/plugins/dev.neoism.workflows"
        || path.starts_with("/v2/plugins/dev.neoism.workflows/")
    {
        Some("dev.neoism.workflows")
    } else if path == "/lsp"
        || path.starts_with("/lsp/")
        || path == "/v2/plugins/dev.neoism.lsp"
        || path.starts_with("/v2/plugins/dev.neoism.lsp/")
    {
        Some("dev.neoism.lsp")
    } else if path == "/mcp"
        || path.starts_with("/mcp/")
        || path == "/v2/plugins/dev.neoism.mcp"
        || path.starts_with("/v2/plugins/dev.neoism.mcp/")
    {
        Some("dev.neoism.mcp")
    } else if path == "/vcs"
        || path.starts_with("/vcs/")
        || path == "/v2/plugins/dev.neoism.vcs"
        || path.starts_with("/v2/plugins/dev.neoism.vcs/")
    {
        Some("dev.neoism.vcs")
    } else if path == "/pty"
        || path.starts_with("/pty/")
        || path == "/v2/plugins/dev.neoism.pty"
        || path.starts_with("/v2/plugins/dev.neoism.pty/")
    {
        Some("dev.neoism.pty")
    } else {
        None
    }
}

fn auth_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "code": code,
            "message": message,
            "retryable": false,
            "details": {}
        })),
    )
        .into_response()
}

fn request_directory(request: &Request<Body>) -> Option<String> {
    request
        .headers()
        .get("x-neoism-directory")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            url::form_urlencoded::parse(request.uri().query()?.as_bytes())
                .find(|(key, _)| key == "directory")
                .map(|(_, value)| value.into_owned())
        })
}

fn request_session_id(uri: &axum::http::Uri) -> Option<String> {
    url::form_urlencoded::parse(uri.query()?.as_bytes())
        .find(|(key, _)| key == "sessionId" || key == "sessionID")
        .map(|(_, value)| value.into_owned())
}

fn session_id_from_path(path: &str) -> Option<&str> {
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let index = parts
        .iter()
        .position(|part| *part == "sessions" || *part == "session")?;
    let id = *parts.get(index + 1)?;
    (!matches!(id, "status" | "workspace" | "project")).then_some(id)
}

fn interaction_id_from_path(path: &str) -> Option<&str> {
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let index = parts
        .iter()
        .position(|part| *part == "permissions" || *part == "questions")?;
    parts.get(index + 1).copied()
}

fn hosted_restricted_path(path: &str) -> bool {
    matches!(
        path,
        "/global/config" | "/global/config/validate" | "/global/event" | "/config" | "/plugin"
    ) || path.starts_with("/auth/")
        || (path.starts_with("/provider/") && path.contains("/oauth/"))
        || (path.starts_with("/v2/providers/")
            && (path.ends_with("/auth") || path.contains("/oauth/")))
}

fn requires_directory_scope(path: &str) -> bool {
    !path.starts_with("/v2/sessions")
        && !path.starts_with("/session")
        && !path.starts_with("/v2/interactions")
        && !path.starts_with("/permission")
        && !path.starts_with("/question")
        && !path.starts_with("/v2/artifacts")
        && !path.starts_with("/v2/events")
        && !path.starts_with("/event")
        && !path.starts_with("/v2/audit")
        && !path.starts_with("/v2/meta")
        && !path.starts_with("/v2/openapi")
        && !path.starts_with("/v2/capabilities")
        && !path.starts_with("/v2/plugins")
        && path != "/global/health"
}
