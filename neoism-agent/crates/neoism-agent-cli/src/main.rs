use clap::{Parser, Subcommand};
use neoism_agent_server::ServerOptions;
use tracing_subscriber::EnvFilter;

mod acp;
mod chat;
mod chat_blockers;
mod chat_command_handlers;
mod chat_commands;
mod chat_diff;
mod chat_header;
mod chat_input;
mod chat_markdown;
mod chat_picker;
mod chat_render;
mod chat_session;
mod chat_sse;
mod chat_status;
mod chat_stream;
mod chat_terminal;
mod chat_tool_render;
mod chat_ui;
mod cli_commands;
mod cli_direct_commands;
mod cli_http;
mod model_ref;
mod tui_launcher;

pub(crate) use cli_http::{
    get_and_print, normalize_server, post_and_print, print_json, redact_secrets,
    request_with_dir, response_json,
};
pub(crate) use model_ref::split_model_ref;

const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";
pub(crate) const RESET: &str = "\x1b[0m";
pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const DIM: &str = "\x1b[2m";
pub(crate) const ITALIC: &str = "\x1b[3m";
// OpenCode dark theme palette (truecolor)
pub(crate) const PURPLE: &str = "\x1b[38;2;157;124;216m"; // #9d7cd8 accent
pub(crate) const RED: &str = "\x1b[38;2;224;108;117m"; // #e06c75 error
pub(crate) const ORANGE: &str = "\x1b[38;2;245;167;66m"; // #f5a742 warning
pub(crate) const GREEN: &str = "\x1b[38;2;127;216;143m"; // #7fd88f success
pub(crate) const CYAN: &str = "\x1b[38;2;92;156;245m"; // #5c9cf5 secondary blue
pub(crate) const YELLOW: &str = "\x1b[38;2;229;192;123m"; // #e5c07b
pub(crate) const WHITE: &str = "\x1b[38;2;238;238;238m"; // #eeeeee text
pub(crate) const CLEAR_LINE: &str = "\x1b[2K";
pub(crate) const SPINNER_FRAMES: &[&str] = &["■", "▣", "□", "▣"];

pub(crate) fn provider_id(provider: &serde_json::Value) -> String {
    provider
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[derive(Debug, Parser)]
#[command(
    name = "neoism-agent",
    version,
    about = "Headless Neoism agent framework"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the canonical V2 OpenAPI document without starting a server.
    Openapi,
    Serve {
        #[arg(long, default_value = "4096")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        hostname: String,
        #[arg(long)]
        cors: Vec<String>,
    },
    Acp {
        #[arg(long, default_value = "4096")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        hostname: String,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
    },
    Run {
        message: Vec<String>,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        variant: Option<String>,
    },
    Chat {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        variant: Option<String>,
    },
    Tui {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        variant: Option<String>,
    },
    Doctor {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Providers {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
    Models {
        provider: Option<String>,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        verbose: bool,
    },
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Tool {
        #[command(subcommand)]
        command: ToolCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    Methods {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
    Status {
        provider: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
    SetApi {
        provider: String,
        key: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
    Remove {
        provider: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
    #[command(name = "login-codex", alias = "login-openai", alias = "login-chatgpt")]
    LoginCodex {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        browser: bool,
        #[arg(long)]
        no_wait: bool,
    },
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    Status {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Connect {
        name: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Disconnect {
        name: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Tools {
        name: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Resources {
        name: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Prompts {
        name: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    AuthStart {
        name: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    AuthFinish {
        name: String,
        code: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    List {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Get {
        id: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
    Messages {
        id: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    Undo {
        id: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
    Redo {
        id: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
    Abort {
        id: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
    },
}

#[derive(Debug, Subcommand)]
enum ToolCommand {
    List {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Ids {
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
    },
    Exec {
        id: String,
        #[arg(long, default_value = DEFAULT_SERVER)]
        server: String,
        #[arg(long)]
        dir: Option<String>,
        #[arg(long, default_value = "{}")]
        args: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let default_filter = if std::env::var_os("NEOISM_AGENT_PERF_LOG").is_some() {
        "neoism_agent=info,neoism_agent::perf=trace,tower_http=info"
    } else {
        "neoism_agent=info,tower_http=warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(default_filter)),
        )
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Openapi => {
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut output = stdout.lock();
            serde_json::to_writer_pretty(&mut output, &neoism_agent_server::canonical_openapi())?;
            writeln!(output)?;
        }
        Command::Serve {
            port,
            hostname,
            cors,
        } => {
            let options = ServerOptions {
                hostname: hostname.clone(),
                port,
                cors,
            };
            println!("neoism agent listening on http://{hostname}:{port}");
            neoism_agent_server::listen(
                options,
                neoism_agent_server::standard_services(),
            )
            .await?;
        }
        Command::Acp {
            port,
            hostname,
            server,
            cwd,
        } => acp::acp(hostname, port, server, cwd).await?,
        Command::Run {
            message,
            server,
            session,
            dir,
            provider,
            model,
            agent,
            variant,
        } => {
            cli_direct_commands::run(
                server,
                session,
                dir,
                provider,
                model,
                agent,
                variant,
                message.join(" "),
            )
            .await?
        }
        Command::Chat {
            server,
            session,
            dir,
            model,
            provider,
            agent,
            variant,
        } => chat::chat(server, session, dir, provider, model, agent, variant).await?,
        Command::Tui {
            server,
            session,
            dir,
            model,
            provider,
            agent,
            variant,
        } => {
            tui_launcher::run(tui_launcher::TuiOptions {
                server,
                session,
                dir,
                model,
                provider,
                agent,
                variant,
            })
            .await?
        }
        Command::Doctor { server, dir } => {
            cli_direct_commands::doctor(server, dir).await?
        }
        Command::Providers { server } => {
            get_and_print(server, "/v2/providers", None, false).await?
        }
        Command::Models {
            provider,
            server,
            verbose,
        } => cli_direct_commands::models(server, provider, verbose).await?,
        Command::Auth { command } => cli_commands::auth(command).await?,
        Command::Mcp { command } => cli_commands::mcp(command).await?,
        Command::Session { command } => cli_commands::session(command).await?,
        Command::Tool { command } => cli_commands::tool(command).await?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{
        event_type, CommandInfo, EventPayload, Id, IdKind, MessageWithParts, PromptPart,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex as StdMutex};

    #[test]
    fn acp_prompt_parts_accept_text_blocks_and_resources() {
        let parts = acp::prompt_parts_from_acp(&json!({
            "prompt": [
                { "type": "text", "text": "hello" },
                { "type": "resource", "resource": { "text": " world" } },
                { "type": "resource_link", "uri": "file:///tmp/example.txt", "mimeType": "text/plain", "name": "example.txt" },
                { "type": "image", "data": "abc", "mimeType": "image/png", "name": "shot.png" }
            ]
        }))
        .unwrap();
        assert_eq!(parts.len(), 4);
        assert!(matches!(&parts[0], PromptPart::Text { text } if text == "hello"));
        assert!(
            matches!(&parts[2], PromptPart::File { filename, mime, .. } if filename == "example.txt" && mime == "text/plain")
        );
        assert!(
            matches!(&parts[3], PromptPart::File { url, filename, mime } if url == "data:image/png;base64,abc" && filename == "shot.png" && mime == "image/png")
        );
    }

    #[test]
    fn acp_model_params_accept_provider_model_strings() {
        let model = acp::model_param(&json!({ "model": "openai/gpt-5.1" })).unwrap();
        assert_eq!(model.provider_id, "openai");
        assert_eq!(model.model_id, "gpt-5.1");
    }

    #[test]
    fn acp_message_updates_replay_text_and_completed_edit_tools() {
        let session_id = Id::ascending(IdKind::Session).to_string();
        let message_id = Id::ascending(IdKind::Message).to_string();
        let parent_id = Id::ascending(IdKind::Message).to_string();
        let text_part_id = Id::ascending(IdKind::Part).to_string();
        let tool_part_id = Id::ascending(IdKind::Part).to_string();
        let message: MessageWithParts = serde_json::from_value(json!({
            "info": {
                "role": "assistant",
                "id": message_id,
                "sessionId": session_id,
                "time": { "created": 1, "completed": 2 },
                "parentId": parent_id,
                "mode": "build",
                "agent": "build",
                "path": { "cwd": "/tmp/project", "root": "/tmp/project" },
                "cost": 0.0,
                "tokens": {
                    "input": 1,
                    "output": 2,
                    "reasoning": 0,
                    "cache": { "read": 0, "write": 0 }
                },
                "modelId": "gpt-5.5",
                "providerId": "openai",
                "finish": "stop"
            },
            "parts": [
                {
                    "type": "text",
                    "id": text_part_id,
                    "sessionId": session_id,
                    "messageId": message_id,
                    "text": "done"
                },
                {
                    "type": "tool",
                    "id": tool_part_id,
                    "sessionId": session_id,
                    "messageId": message_id,
                    "tool": "edit",
                    "callId": "call_edit",
                    "state": {
                        "status": "completed",
                        "input": {
                            "filePath": "TASK.md",
                            "oldString": "before",
                            "newString": "after"
                        },
                        "output": "Replaced 1 occurrence",
                        "metadata": {},
                        "title": "Edited TASK.md",
                        "time": { "start": 1, "end": 2 }
                    }
                }
            ]
        }))
        .unwrap();

        let updates = acp::acp_updates_for_message(&message);

        assert!(updates.iter().any(|update| {
            update["method"] == "session/update"
                && update["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
                && update["params"]["update"]["content"]["text"] == "done"
        }));
        assert!(updates.iter().any(|update| {
            let update = &update["params"]["update"];
            update["sessionUpdate"] == "tool_call_update"
                && update["toolCallId"] == "call_edit"
                && update["status"] == "completed"
                && update["kind"] == "edit"
                && update["content"][1]["type"] == "diff"
                && update["content"][1]["path"] == "TASK.md"
        }));
    }

    #[test]
    fn acp_config_options_include_model_and_mode_selects() {
        let payload = acp::build_config_options_payload(
            vec![json!({
                "modelId": "openai/gpt-5.5",
                "name": "OpenAI / GPT-5.5",
                "_meta": { "variants": ["default", "high"] }
            })],
            vec![json!({
                "id": "build",
                "name": "build",
                "description": "Build agent"
            })],
            Some("openai/gpt-5.5".to_string()),
            Some("build".to_string()),
        );

        assert_eq!(payload["models"]["currentModelId"], "openai/gpt-5.5");
        assert_eq!(payload["modes"]["currentModeId"], "build");
        assert_eq!(payload["configOptions"][0]["id"], "model");
        assert_eq!(payload["configOptions"][0]["type"], "select");
        assert_eq!(payload["configOptions"][1]["id"], "effort");
        assert_eq!(payload["configOptions"][1]["currentValue"], "default");
        assert_eq!(payload["configOptions"][2]["id"], "mode");
        assert_eq!(payload["configOptions"][2]["options"][0]["value"], "build");
    }

    #[test]
    fn acp_available_commands_update_includes_builtin_compact() {
        let update = acp::available_commands_notification(
            "ses_test",
            vec![CommandInfo {
                name: "init".to_string(),
                description: Some("Initialize".to_string()),
                template: None,
                agent: None,
                model: None,
                subtask: None,
            }],
        );

        assert_eq!(update["method"], "session/update");
        assert_eq!(
            update["params"]["update"]["sessionUpdate"],
            "available_commands_update"
        );
        let commands = update["params"]["update"]["availableCommands"]
            .as_array()
            .unwrap();
        assert!(commands.iter().any(|command| command["name"] == "init"));
        assert!(commands.iter().any(|command| command["name"] == "compact"));
    }

    #[test]
    fn acp_initialize_includes_terminal_auth_when_supported() {
        let payload = acp::initialize_response(json!({
            "clientCapabilities": {
                "_meta": {
                    "terminal-auth": true
                }
            }
        }));
        assert_eq!(
            payload["authMethods"][0]["_meta"]["terminal-auth"]["command"],
            "neoism-agent"
        );
        assert_eq!(
            payload["authMethods"][0]["_meta"]["terminal-auth"]["args"][0],
            "auth"
        );
        assert_eq!(
            payload["agentCapabilities"]["promptCapabilities"]["image"],
            true
        );
    }

    #[test]
    fn acp_live_events_map_deltas_tools_todos_and_permissions() {
        let session_id = Id::ascending(IdKind::Session).to_string();
        let message_id = Id::ascending(IdKind::Message).to_string();
        let text_part_id = Id::ascending(IdKind::Part).to_string();
        let reasoning_part_id = Id::ascending(IdKind::Part).to_string();
        let tool_part_id = Id::ascending(IdKind::Part).to_string();
        let mut live = acp::AcpLiveState::default();
        let pending_permissions = Arc::new(StdMutex::new(BTreeMap::new()));
        let pending_questions = Arc::new(StdMutex::new(BTreeMap::new()));
        let request_counter = Arc::new(AtomicU64::new(1));

        let reasoning_start = EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({
                "sessionID": session_id,
                "part": {
                    "type": "reasoning",
                    "id": reasoning_part_id,
                    "sessionId": session_id,
                    "messageId": message_id,
                    "text": "",
                    "time": { "start": 1, "end": null },
                    "metadata": null
                }
            }),
        );
        let _ = acp::acp_live_outputs_for_event(
            &reasoning_start,
            &session_id,
            &mut live,
            &pending_permissions,
            &pending_questions,
            &request_counter,
        );

        let thought_delta = EventPayload::new(
            event_type::MESSAGE_PART_DELTA,
            json!({
                "sessionID": session_id,
                "messageID": message_id,
                "partID": reasoning_part_id,
                "field": "text",
                "delta": "thinking"
            }),
        );
        let thought_updates = acp::acp_live_outputs_for_event(
            &thought_delta,
            &session_id,
            &mut live,
            &pending_permissions,
            &pending_questions,
            &request_counter,
        );
        assert_eq!(
            thought_updates[0]["params"]["update"]["sessionUpdate"],
            "agent_thought_chunk"
        );

        let text_delta = EventPayload::new(
            event_type::MESSAGE_PART_DELTA,
            json!({
                "sessionID": session_id,
                "messageID": message_id,
                "partID": text_part_id,
                "field": "text",
                "delta": "hello"
            }),
        );
        let text_updates = acp::acp_live_outputs_for_event(
            &text_delta,
            &session_id,
            &mut live,
            &pending_permissions,
            &pending_questions,
            &request_counter,
        );
        assert_eq!(
            text_updates[0]["params"]["update"]["sessionUpdate"],
            "agent_message_chunk"
        );

        let tool_update = EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({
                "sessionID": session_id,
                "part": {
                    "type": "tool",
                    "id": tool_part_id,
                    "sessionId": session_id,
                    "messageId": message_id,
                    "tool": "edit",
                    "callId": "call_edit",
                    "state": {
                        "status": "completed",
                        "input": {
                            "filePath": "TASK.md",
                            "oldString": "before",
                            "newString": "after"
                        },
                        "output": "Replaced 1 occurrence",
                        "metadata": {},
                        "title": "Edited TASK.md",
                        "time": { "start": 1, "end": 2 }
                    },
                    "metadata": null
                }
            }),
        );
        let tool_updates = acp::acp_live_outputs_for_event(
            &tool_update,
            &session_id,
            &mut live,
            &pending_permissions,
            &pending_questions,
            &request_counter,
        );
        assert!(tool_updates.iter().any(|update| {
            update["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && update["params"]["update"]["status"] == "completed"
                && update["params"]["update"]["kind"] == "edit"
        }));

        let todo_update = EventPayload::new(
            event_type::TODO_UPDATED,
            json!({
                "sessionID": session_id,
                "todos": [{
                    "content": "finish ACP bridge",
                    "status": "in_progress",
                    "priority": "high"
                }]
            }),
        );
        let todo_updates = acp::acp_live_outputs_for_event(
            &todo_update,
            &session_id,
            &mut live,
            &pending_permissions,
            &pending_questions,
            &request_counter,
        );
        assert_eq!(todo_updates[0]["params"]["update"]["sessionUpdate"], "plan");

        let permission_update = EventPayload::new(
            event_type::PERMISSION_ASKED,
            json!({
                "id": "per_test",
                "sessionId": session_id,
                "messageId": message_id,
                "title": "Allow edit?",
                "permission": "edit",
                "patterns": ["TASK.md"],
                "always": ["TASK.md"],
                "tool": { "callID": "call_edit" },
                "metadata": {
                    "tool": "edit",
                    "input": { "filePath": "TASK.md" }
                }
            }),
        );
        let permission_updates = acp::acp_live_outputs_for_event(
            &permission_update,
            &session_id,
            &mut live,
            &pending_permissions,
            &pending_questions,
            &request_counter,
        );
        assert_eq!(
            permission_updates[0]["method"],
            "session/request_permission"
        );
        assert_eq!(
            pending_permissions
                .lock()
                .unwrap()
                .get("neoism_permission_1"),
            Some(&"per_test".to_string())
        );
    }

    #[test]
    fn split_model_ref_preserves_slashes_in_model_id() {
        let (provider, model) = split_model_ref("openrouter/openai/gpt-5.5-pro").unwrap();
        assert_eq!(provider, "openrouter");
        assert_eq!(model, "openai/gpt-5.5-pro");
        assert!(split_model_ref("gpt-5.5-pro").is_none());
    }
}
