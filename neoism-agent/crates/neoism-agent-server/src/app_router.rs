use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::app_routes::{agent_get, agent_list, plugin_status, skill_list};
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
    lsp_code_actions, lsp_definition, lsp_diagnostics, lsp_document_symbols,
    lsp_formatting, lsp_hover, lsp_implementation, lsp_incoming_calls,
    lsp_outgoing_calls, lsp_prepare_call_hierarchy, lsp_references, lsp_shutdown,
    lsp_status, lsp_touch,
};
use crate::mcp_routes::{
    mcp_add, mcp_auth_authenticate, mcp_auth_callback, mcp_auth_remove, mcp_auth_start,
    mcp_connect, mcp_disconnect, mcp_prompts, mcp_resources, mcp_status, mcp_tool_call,
    mcp_tools,
};
use crate::openapi::openapi_doc;
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
    session_children, session_create, session_delete, session_diff, session_fork,
    session_get, session_list, session_set_pin, session_share, session_status,
    session_todo_list, session_unshare, session_update,
};
use crate::session_undo::{
    session_redo, session_revert, session_undo, session_undo_tree, session_unrevert,
};
use crate::state::AppState;
use crate::tool_routes::{tool_execute, tool_ids, tool_list};
use crate::v2_routes::{
    v2_compact, v2_context, v2_message_list, v2_prompt, v2_prompt_async,
    v2_session_children, v2_session_list, v2_wait,
};
use crate::vcs_routes::{vcs_apply, vcs_diff, vcs_diff_raw, vcs_get, vcs_status};
use crate::worktree_routes::{
    worktree_create, worktree_list, worktree_remove, worktree_reset,
};

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/global/health", get(global_health))
        .route("/global/event", get(global_event))
        .route("/global/config", get(config_get).patch(config_update))
        .route("/global/config/validate", get(config_validate))
        .route("/global/dispose", post(global_dispose))
        .route("/global/upgrade", post(global_upgrade))
        .route("/event", get(event_stream))
        .route("/doc", get(openapi_doc))
        .route("/path", get(path_get))
        .route("/instance/dispose", post(instance_dispose))
        .route("/vcs", get(vcs_get))
        .route("/vcs/diff", get(vcs_diff))
        .route("/vcs/status", get(vcs_status))
        .route("/vcs/diff/raw", get(vcs_diff_raw))
        .route("/vcs/apply", post(vcs_apply))
        .route("/command", get(command_list))
        .route("/agent", get(agent_list))
        .route("/agent/:name", get(agent_get))
        .route("/skill", get(skill_list))
        .route("/plugin", get(plugin_status))
        .route("/lsp", get(lsp_status))
        .route("/lsp/hover", get(lsp_hover))
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
        .route("/formatter", get(empty_array))
        .route("/find", get(find_text))
        .route("/find/file", get(find_file))
        .route("/find/symbol", get(find_symbol))
        .route(
            "/search/semantic",
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
        .route("/session/:session_id/pin", post(session_set_pin))
        .route("/session/:session_id/undo", get(session_undo_tree))
        .route("/session/:session_id/undo/tree", get(session_undo_tree))
        .route("/session/:session_id/summarize", post(session_summarize))
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
        .route(
            "/mcp/:name/auth",
            post(mcp_auth_start).delete(mcp_auth_remove),
        )
        .route("/mcp/:name/auth/callback", post(mcp_auth_callback))
        .route("/mcp/:name/auth/authenticate", post(mcp_auth_authenticate))
        .route("/mcp/:name/connect", post(mcp_connect))
        .route("/mcp/:name/disconnect", post(mcp_disconnect))
        .route("/mcp/:name/tools", get(mcp_tools))
        .route("/mcp/:name/tools/:tool_name", post(mcp_tool_call))
        .route("/mcp/:name/resources", get(mcp_resources))
        .route("/mcp/:name/prompts", get(mcp_prompts))
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
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}
