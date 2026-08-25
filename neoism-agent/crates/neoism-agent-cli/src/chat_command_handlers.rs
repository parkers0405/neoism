use std::io::Write;

use anyhow::Context;
use serde_json::{json, Value};

use crate::chat_blockers::{
    parse_permission_reply_and_id, submit_permission_reply, submit_question_answer,
    submit_question_reject,
};
use crate::chat_commands::print_chat_command_menu;
use crate::chat_markdown::MarkdownStreamRenderer;
use crate::chat_picker::{
    fetch_sessions, fetch_subagent_sessions, pick_agent, pick_model, pick_session,
    pick_subagent_session, pick_think, relative_time,
};
use crate::chat_session::{
    abort_session_if_busy, create_cli_session, ensure_empty_response,
    fetch_permission_requests, fetch_question_requests, fetch_session_messages,
    fetch_session_queue, fetch_session_undo_tree, message_info_id, message_text,
    next_redo_message_id, normalize_model_ref, persist_session_model,
    print_provider_model_list, print_session_messages, print_session_replay,
    undo_cursor_message_id, user_model_from_cli_model,
};
use crate::chat_status::{
    first_pending_id, pending_id_or_first, print_pending_permissions,
    print_pending_questions, print_queue_mutation, print_session_queue,
};
use crate::chat_ui::BottomPrompt;
use crate::{
    print_json, redact_secrets, request_with_dir, response_json, BOLD, CYAN, DIM, RESET,
};

pub(crate) enum CommandOutcome {
    Continue,
    Exit,
    Attach,
    Prefill(String),
}

pub(crate) async fn handle_chat_command(
    client: &reqwest::Client,
    server: &str,
    dir: Option<&str>,
    session_id: &mut String,
    current_model: &mut Option<String>,
    current_agent: &mut Option<String>,
    current_variant: &mut Option<String>,
    input: &str,
    mut ui: Option<&mut BottomPrompt>,
) -> anyhow::Result<CommandOutcome> {
    let mut parts = input.split_whitespace();
    let command = parts.next().unwrap_or_default();
    match command {
        "/" => {
            print_chat_command_menu(None);
        }
        "/help" | "/?" => {
            let query = parts.collect::<Vec<_>>().join(" ");
            print_chat_command_menu((!query.trim().is_empty()).then_some(query.as_str()));
        }
        "/quit" | "/exit" | "/q" => return Ok(CommandOutcome::Exit),
        "/clear" => {
            print!("\x1b[2J\x1b[H");
            std::io::stdout().flush()?;
        }
        "/expand" | "/open" => {
            if let Some(ui) = ui {
                if !ui.expand_pending()? {
                    println!("{DIM}nothing to expand{RESET}");
                }
            } else {
                println!("expand is only available in the interactive TUI");
            }
        }
        "/session" | "/sessions" | "/ses" => {
            if let Some(ui) = ui {
                if let Some(picked) = pick_session(client, server, dir, ui).await? {
                    let value = response_json(
                        client
                            .get(format!("{server}/v2/sessions/{picked}"))
                            .send()
                            .await?,
                    )
                    .await?;
                    *session_id = picked;
                    apply_session_state(
                        &value,
                        current_model,
                        current_agent,
                        current_variant,
                    );
                    ui.before_output()?;
                    println!();
                    println!("{BOLD}{CYAN}›{RESET} {DIM}session{RESET} {BOLD}{session_id}{RESET}");
                    print_session_replay(client, server, session_id, 20).await?;
                }
            } else {
                let entries = fetch_sessions(client, server, dir).await?;
                if entries.is_empty() {
                    println!("no sessions");
                } else {
                    for entry in entries.iter().take(20) {
                        let marker = if entry.id == *session_id { "*" } else { " " };
                        println!(
                            "{marker} {} {} ({})",
                            &entry.id[..entry.id.len().min(12)],
                            entry.title,
                            relative_time(entry.updated)
                        );
                    }
                }
            }
        }
        "/new" => {
            *session_id = create_cli_session(
                client,
                server,
                dir,
                current_model.as_deref(),
                current_agent.as_deref(),
                current_variant.as_deref(),
            )
            .await?;
            println!("session: {session_id}");
        }
        "/model" => {
            if let Some(model) = parts.next() {
                let normalized = normalize_model_ref(model, "openai");
                user_model_from_cli_model(&normalized, current_variant.as_deref())?;
                *current_model = Some(normalized);
                if !session_id.is_empty() {
                    persist_session_model(
                        client,
                        server,
                        session_id,
                        current_model.as_deref().unwrap_or_default(),
                        current_variant.as_deref(),
                    )
                    .await?;
                }
                println!("model: {}", current_model.as_deref().unwrap_or_default());
            } else if let Some(ui) = ui {
                if let Some(picked) =
                    pick_model(client, server, ui, current_model.as_deref()).await?
                {
                    user_model_from_cli_model(&picked, current_variant.as_deref())?;
                    *current_model = Some(picked);
                    if !session_id.is_empty() {
                        persist_session_model(
                            client,
                            server,
                            session_id,
                            current_model.as_deref().unwrap_or_default(),
                            current_variant.as_deref(),
                        )
                        .await?;
                    }
                    ui.before_output()?;
                    println!();
                    println!(
                        "{BOLD}{CYAN}›{RESET} {DIM}model{RESET} {BOLD}{}{RESET}",
                        current_model.as_deref().unwrap_or_default()
                    );
                }
            } else {
                println!(
                    "model: {}",
                    current_model.as_deref().unwrap_or("server default")
                );
                print_provider_model_list(
                    client,
                    server,
                    "openai",
                    current_model.as_deref(),
                )
                .await?;
            }
        }
        "/models" => {
            let provider = parts.next().unwrap_or("openai");
            print_provider_model_list(client, server, provider, current_model.as_deref())
                .await?;
        }
        "/think" | "/reasoning" => {
            if let Some(value) = parts.next() {
                let value = value.to_ascii_lowercase();
                match value.as_str() {
                    "off" | "default" => {
                        *current_variant = None;
                        if let Some(model) =
                            current_model.as_deref().filter(|_| !session_id.is_empty())
                        {
                            persist_session_model(
                                client, server, session_id, model, None,
                            )
                            .await?;
                        }
                        println!("think: model default");
                    }
                    "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
                        *current_variant = Some(if value == "max" {
                            "xhigh".to_string()
                        } else {
                            value
                        });
                        if let Some(model) =
                            current_model.as_deref().filter(|_| !session_id.is_empty())
                        {
                            persist_session_model(
                                client,
                                server,
                                session_id,
                                model,
                                current_variant.as_deref(),
                            )
                            .await?;
                        }
                        println!(
                            "think: {}",
                            current_variant.as_deref().unwrap_or("model default")
                        );
                    }
                    _ => {
                        println!(
                            "usage: /think [off|none|minimal|low|medium|high|xhigh]"
                        );
                    }
                }
            } else if let Some(ui) = ui {
                if let Some(picked) = pick_think(ui, current_variant.as_deref()).await? {
                    *current_variant = picked;
                    if let Some(model) =
                        current_model.as_deref().filter(|_| !session_id.is_empty())
                    {
                        persist_session_model(
                            client,
                            server,
                            session_id,
                            model,
                            current_variant.as_deref(),
                        )
                        .await?;
                    }
                    ui.before_output()?;
                    println!();
                    println!(
                        "{BOLD}{CYAN}›{RESET} {DIM}think{RESET} {BOLD}{}{RESET}",
                        current_variant.as_deref().unwrap_or("model default")
                    );
                }
            } else {
                println!(
                    "think: {}",
                    current_variant.as_deref().unwrap_or("model default")
                );
            }
        }
        "/agent" => {
            if let Some(name) = parts.next() {
                let _: Value = response_json(
                    request_with_dir(client.get(format!("{server}/v2/agents/{name}")), dir)
                        .send()
                        .await?,
                )
                .await?;
                *current_agent = Some(name.to_string());
                println!("agent: {name}");
            } else if let Some(ui) = ui {
                if let Some(picked) =
                    pick_agent(client, server, dir, ui, current_agent.as_deref()).await?
                {
                    *current_agent = Some(picked);
                    ui.before_output()?;
                    println!();
                    println!(
                        "{BOLD}{CYAN}›{RESET} {DIM}agent{RESET} {BOLD}{}{RESET}",
                        current_agent.as_deref().unwrap_or_default()
                    );
                }
            } else {
                println!(
                    "agent: {}",
                    current_agent.as_deref().unwrap_or("session default")
                );
                let value = response_json(
                    request_with_dir(client.get(format!("{server}/v2/agents")), dir)
                        .send()
                        .await?,
                )
                .await?;
                print_agent_list(value, current_agent.as_deref())?;
            }
        }
        "/agents" => {
            let value = response_json(
                request_with_dir(client.get(format!("{server}/v2/agents")), dir)
                    .send()
                    .await?,
            )
            .await?;
            print_agent_list(value, current_agent.as_deref())?;
        }
        "/sub-agent" | "/subagents" => {
            if let Some(id) = parts.next() {
                let value = response_json(
                    client.get(format!("{server}/v2/sessions/{id}")).send().await?,
                )
                .await?;
                *session_id = id.to_string();
                apply_session_state(
                    &value,
                    current_model,
                    current_agent,
                    current_variant,
                );
                println!("session: {session_id}");
                print_session_replay(client, server, session_id, 20).await?;
            } else if let Some(ui) = ui {
                if let Some(picked) =
                    pick_subagent_session(client, server, ui, session_id).await?
                {
                    let value = response_json(
                        client
                            .get(format!("{server}/v2/sessions/{}", picked.id))
                            .send()
                            .await?,
                    )
                    .await?;
                    *session_id = picked.id;
                    apply_session_state(
                        &value,
                        current_model,
                        current_agent,
                        current_variant,
                    );
                    ui.before_output()?;
                    println!();
                    println!(
                        "{BOLD}{CYAN}›{RESET} {DIM}session{RESET} {BOLD}{}{RESET}",
                        session_id
                    );
                    if matches!(picked.status.as_str(), "working" | "retrying") {
                        return Ok(CommandOutcome::Attach);
                    }
                    print_session_replay(client, server, session_id, 20).await?;
                }
            } else {
                let entries = fetch_subagent_sessions(client, server, session_id).await?;
                if entries.is_empty() {
                    println!("no subagent sessions for this session");
                } else {
                    for entry in entries {
                        let marker = if entry.id == *session_id { "*" } else { " " };
                        println!("{marker} {:<40} {}", entry.primary, entry.secondary);
                    }
                }
            }
        }
        "/messages" => {
            let limit = parts.next().and_then(|value| value.parse::<usize>().ok());
            print_session_messages(client, server, session_id, limit).await?;
        }
        "/compact" => {
            if let Some(ui) = ui.as_deref_mut() {
                ui.render_status("Compacting session context", "")?;
            }
            abort_session_if_busy(client, server, session_id).await?;
            ensure_empty_response(
                client
                    .post(format!("{server}/v2/sessions/{session_id}/compact"))
                    .send()
                    .await?,
            )
            .await?;
            let session = response_json(
                client
                    .get(format!("{server}/v2/sessions/{session_id}"))
                    .send()
                    .await?,
            )
            .await?;
            if let Some(ui) = ui {
                ui.before_output()?;
            }
            print_compaction_marker(&session)?;
        }
        "/goal" => {
            let rest = parts.collect::<Vec<_>>().join(" ");
            let trimmed = rest.trim();
            if trimmed.is_empty() {
                // Show the active goal.
                let value = response_json(
                    client
                        .get(format!("{server}/v2/plugins/dev.neoism.goals/{session_id}"))
                        .send()
                        .await?,
                )
                .await?;
                print_goal(&value)?;
            } else if trimmed.eq_ignore_ascii_case("clear") {
                let value = response_json(
                    client
                        .delete(format!("{server}/v2/plugins/dev.neoism.goals/{session_id}"))
                        .send()
                        .await?,
                )
                .await?;
                println!("goal cleared");
                print_goal(&value)?;
            } else {
                let value = response_json(
                    client
                        .post(format!("{server}/v2/plugins/dev.neoism.goals/{session_id}"))
                        .json(&json!({ "text": trimmed }))
                        .send()
                        .await?,
                )
                .await?;
                print_goal(&value)?;
            }
        }
        "/undo" => {
            abort_session_if_busy(client, server, session_id).await?;
            let messages = fetch_session_messages(client, server, session_id).await?;
            let tree = fetch_session_undo_tree(client, server, session_id).await?;
            let Some(message_id) = undo_cursor_message_id(&tree) else {
                println!("nothing to undo");
                return Ok(CommandOutcome::Continue);
            };
            let prefill = messages
                .iter()
                .find(|message| {
                    message_info_id(message).as_deref() == Some(message_id.as_str())
                })
                .and_then(message_text)
                .unwrap_or_default();
            let _: Value = response_json(
                client
                    .post(format!("{server}/v2/sessions/{session_id}/revert"))
                    .json(&json!({ "messageID": message_id }))
                    .send()
                    .await?,
            )
            .await?;
            println!("undid {message_id}");
            if !prefill.trim().is_empty() {
                return Ok(CommandOutcome::Prefill(prefill));
            }
        }
        "/redo" => {
            abort_session_if_busy(client, server, session_id).await?;
            let tree = fetch_session_undo_tree(client, server, session_id).await?;
            let Some(revert_id) = tree
                .get("revert")
                .and_then(|revert| {
                    revert.get("messageID").or_else(|| revert.get("messageId"))
                })
                .and_then(Value::as_str)
                .map(ToString::to_string)
            else {
                println!("nothing to redo");
                return Ok(CommandOutcome::Continue);
            };
            if let Some(next_id) = next_redo_message_id(&tree, &revert_id) {
                let _: Value = response_json(
                    client
                        .post(format!("{server}/v2/sessions/{session_id}/revert"))
                        .json(&json!({ "messageID": next_id }))
                        .send()
                        .await?,
                )
                .await?;
                println!("redid {next_id}");
            } else {
                let _: Value = response_json(
                    client
                        .post(format!("{server}/v2/sessions/{session_id}/unrevert"))
                        .send()
                        .await?,
                )
                .await?;
                println!("redid reverted messages");
            }
        }
        "/tools" => {
            let value = response_json(
                request_with_dir(
                    client.get(format!("{server}/v2/tools")),
                    dir,
                )
                .send()
                .await?,
            )
            .await?;
            print_tool_id_list(value)?;
        }
        "/skills" | "/skill" => {
            let value = response_json(
                request_with_dir(client.get(format!("{server}/v2/skills")), dir)
                    .send()
                    .await?,
            )
            .await?;
            print_skill_list(value)?;
        }
        "/mcp" => {
            let value = response_json(
                request_with_dir(client.get(format!("{server}/v2/plugins/dev.neoism.mcp")), dir)
                    .send()
                    .await?,
            )
            .await?;
            print_json(value)?;
        }
        "/providers" => {
            let value =
                response_json(client.get(format!("{server}/v2/providers")).send().await?)
                    .await?;
            print_json(value)?;
        }
        "/auth" => {
            let mut value =
                response_json(client.get(format!("{server}/v2/providers/openai/auth")).send().await?)
                    .await?;
            redact_secrets(&mut value);
            print_json(value)?;
        }
        "/doctor" => {
            let health = response_json(
                client.get(format!("{server}/v2/health")).send().await?,
            )
            .await?;
            print_json(json!({ "server": server, "health": health }))?;
        }
        "/abort" => {
            let value = response_json(
                client
                    .post(format!("{server}/v2/sessions/{session_id}/abort"))
                    .send()
                    .await?,
            )
            .await?;
            print_json(value)?;
        }
        "/queue" => match parts.next() {
            None | Some("list") | Some("show") => {
                let value = fetch_session_queue(client, server, session_id).await?;
                print_session_queue(&value)?;
            }
            Some("clear") => {
                let value = response_json(
                    client
                        .delete(format!("{server}/v2/sessions/{session_id}/queue"))
                        .send()
                        .await?,
                )
                .await?;
                print_queue_mutation("cleared", &value)?;
            }
            Some("pop") => {
                let value = response_json(
                    client
                        .post(format!("{server}/v2/sessions/{session_id}/queue/pop"))
                        .send()
                        .await?,
                )
                .await?;
                print_queue_mutation("popped", &value)?;
            }
            Some(other) => {
                println!("usage: /queue [clear|pop]");
                println!("unknown queue command: {other}");
            }
        },
        "/permissions" => {
            let value = fetch_permission_requests(client, server).await?;
            print_pending_permissions(&value, session_id)?;
        }
        "/permit" => {
            let (reply, id_arg) =
                parse_permission_reply_and_id(parts.next(), parts.next());
            let permissions = fetch_permission_requests(client, server).await?;
            let Some(id) = id_arg
                .map(ToString::to_string)
                .or_else(|| first_pending_id(&permissions, session_id))
            else {
                println!("no pending permissions");
                return Ok(CommandOutcome::Continue);
            };
            submit_permission_reply(client, server, &id, reply).await?;
            println!("permission {id}: {reply}");
            return Ok(CommandOutcome::Attach);
        }
        "/questions" => {
            let value = fetch_question_requests(client, server).await?;
            print_pending_questions(&value, session_id)?;
        }
        "/answer" => {
            let answer = parts.collect::<Vec<_>>().join(" ");
            if answer.trim().is_empty() {
                println!("usage: /answer <text>");
                return Ok(CommandOutcome::Continue);
            }
            let questions = fetch_question_requests(client, server).await?;
            let Some(id) = first_pending_id(&questions, session_id) else {
                println!("no pending questions");
                return Ok(CommandOutcome::Continue);
            };
            submit_question_answer(client, server, &id, &answer, 0).await?;
            println!("answered {id}");
            return Ok(CommandOutcome::Attach);
        }
        "/reject" | "/deny" => {
            let id_arg = parts.next().map(ToString::to_string);
            let questions = fetch_question_requests(client, server).await?;
            if let Some(id) =
                pending_id_or_first(&questions, session_id, id_arg.as_deref())
            {
                submit_question_reject(client, server, &id).await?;
                println!("rejected question {id}");
                return Ok(CommandOutcome::Attach);
            }
            let permissions = fetch_permission_requests(client, server).await?;
            if let Some(id) =
                pending_id_or_first(&permissions, session_id, id_arg.as_deref())
            {
                submit_permission_reply(client, server, &id, "reject").await?;
                println!("permission {id}: reject");
                return Ok(CommandOutcome::Attach);
            }
            println!("no pending permissions or questions");
        }
        _ => {
            println!("unknown command: {command}. Try / or /help");
            let query = command.trim_start_matches('/');
            if !query.is_empty() {
                print_chat_command_menu(Some(query));
            }
        }
    }
    Ok(CommandOutcome::Continue)
}

fn print_agent_list(value: Value, current_agent: Option<&str>) -> anyhow::Result<()> {
    let agents = value.as_array().context("agent response was not a list")?;
    println!("{BOLD}agents{RESET}");
    for agent in agents {
        if agent
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let name = agent
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let mode = agent.get("mode").and_then(Value::as_str).unwrap_or("all");
        let native = agent
            .get("native")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let marker = if current_agent == Some(name) {
            "*"
        } else {
            " "
        };
        let native_label = if native { "native" } else { "custom" };
        let description = agent
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        println!(
            "{marker} {CYAN}{name:<16}{RESET} {mode:<9} {native_label:<7} {description}"
        );
    }
    Ok(())
}

pub(crate) fn apply_session_state(
    session: &Value,
    current_model: &mut Option<String>,
    current_agent: &mut Option<String>,
    current_variant: &mut Option<String>,
) {
    *current_agent = session
        .get("agent")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if let Some(model) = session.get("model") {
        let provider_id = model.get("providerId").and_then(Value::as_str);
        let model_id = model.get("id").and_then(Value::as_str);
        if let (Some(provider_id), Some(model_id)) = (provider_id, model_id) {
            *current_model = Some(format!("{provider_id}/{model_id}"));
            *current_variant = model
                .get("variant")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            return;
        }
    }
    *current_model = None;
    *current_variant = None;
}

fn print_goal(value: &Value) -> anyhow::Result<()> {
    let goal = value.get("goal").unwrap_or(&Value::Null);
    let research_enabled = value
        .get("researchEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match goal.get("text").and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() => {
            println!("{BOLD}goal{RESET} {text}");
            if let Some(research) = goal.get("research").and_then(Value::as_array) {
                if !research.is_empty() {
                    println!("{DIM}research notes: {}{RESET}", research.len());
                    for note in research {
                        if let Some(source) = note.get("source").and_then(Value::as_str) {
                            println!("  {CYAN}{source}{RESET}");
                        }
                    }
                }
            }
        }
        _ => println!("{DIM}no goal set{RESET}"),
    }
    if !research_enabled {
        println!("{DIM}(web research disabled: set FIRECRAWL_API_KEY to enable){RESET}");
    }
    Ok(())
}

fn print_compaction_marker(session: &Value) -> anyhow::Result<()> {
    let summary = session.get("summary").unwrap_or(&Value::Null);
    let kind = summary
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("summary");
    let count = summary
        .get("messageCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    println!(
        "{DIM}──────────────── context compacted · {kind} · {count} messages ────────────────{RESET}"
    );
    if let Some(text) = summary.get("text").and_then(Value::as_str) {
        let text = text.trim();
        if !text.is_empty() {
            let mut renderer = MarkdownStreamRenderer::default();
            renderer.set_first_line_prefix(format!("{DIM}└{RESET} "));
            renderer.push(text)?;
            renderer.finish()?;
            println!();
        }
    }
    Ok(())
}

fn print_tool_id_list(value: Value) -> anyhow::Result<()> {
    if let Some(items) = value.as_array() {
        println!("{BOLD}tools{RESET}");
        for item in items {
            if let Some(tool) = item
                .as_str()
                .or_else(|| item.get("id").and_then(Value::as_str))
            {
                println!("  {CYAN}{tool}{RESET}");
            } else {
                println!("  {}", serde_json::to_string(item)?);
            }
        }
        return Ok(());
    }
    print_json(value)
}

fn print_skill_list(value: Value) -> anyhow::Result<()> {
    let Some(items) = value.as_array() else {
        return print_json(value);
    };
    println!("{BOLD}skills{RESET}");
    if items.is_empty() {
        println!("  {DIM}none discovered{RESET}");
        return Ok(());
    }
    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let description = item
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let path = item.get("path").and_then(Value::as_str).unwrap_or_default();
        if path.is_empty() {
            println!("  {CYAN}{name:<20}{RESET} {description}");
        } else {
            println!("  {CYAN}{name:<20}{RESET} {description} {DIM}{path}{RESET}");
        }
    }
    Ok(())
}
