use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::neoism::agent::side_panel::{NeoismAgentSessionEntry, SessionGoal};
use crate::neoism::icon::AgentKind;

use super::pane::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentOutputKind, NeoismAgentTodo,
    NeoismAgentUsage,
};
use super::picker::NeoismAgentPickerOption;

use neoism_ui::panels::agent_pane::api_mapping::{
    config_defaults_from_json, model_context_limit_from_providers_json,
    model_options_from_providers_json, session_state_from_json, ConfigDefaults,
    SessionState,
};
use neoism_ui::panels::agent_pane::session_group::{
    group_session_options, SessionOptionInput,
};

const SUBAGENT_TREE_MAX_DEPTH: usize = 5;
const SUBAGENT_TREE_MAX_ROWS: usize = 80;

/// When the ACTIVE workspace is remote-joined, the agent pane must
/// talk to the HOST machine's agent-server (chats, threads, SSE all
/// live there), reached through the host daemon's `/agent` reverse
/// proxy. `None` = local default.
static AGENT_SERVER_OVERRIDE: std::sync::RwLock<Option<String>> =
    std::sync::RwLock::new(None);

pub fn set_agent_server_override(url: Option<String>) {
    let normalized = url.map(|url| url.trim_end_matches('/').to_string());
    if let Ok(mut slot) = AGENT_SERVER_OVERRIDE.write() {
        if *slot != normalized {
            tracing::info!(
                target: "neoism::workspaces",
                override_url = ?normalized,
                "agent server override changed"
            );
            *slot = normalized;
        }
    }
}

pub(super) fn neoism_agent_server() -> String {
    if let Ok(slot) = AGENT_SERVER_OVERRIDE.read() {
        if let Some(url) = slot.as_ref() {
            return url.clone();
        }
    }
    std::env::var("NEOISM_SERVER")
        .unwrap_or_else(|_| "http://127.0.0.1:4096".to_string())
        .trim_end_matches('/')
        .to_string()
}

pub(super) fn fetch_model_options(
    server: &str,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let value = api_request_json(server, "GET", "/config/providers", None)?
        .ok_or_else(|| "Neoism Agent returned an empty provider response".to_string())?;
    Ok(model_options_from_providers_json(&value))
}

pub(super) fn fetch_model_context_limit(
    server: &str,
    model_ref: &str,
) -> Result<Option<u64>, String> {
    let value = api_request_json(server, "GET", "/config/providers", None)?
        .ok_or_else(|| "Neoism Agent returned an empty provider response".to_string())?;
    Ok(model_context_limit_from_providers_json(&value, model_ref))
}

pub(super) fn fetch_agent_options(
    server: &str,
    directory: Option<&str>,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let path = directory
        .map(|dir| format!("/agent?directory={}", percent_encode(dir)))
        .unwrap_or_else(|| "/agent".to_string());
    let value = api_request_json(server, "GET", &path, None)?
        .ok_or_else(|| "Neoism Agent returned an empty agent response".to_string())?;
    let agents = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed agents".to_string())?;

    let mut out = vec![NeoismAgentPickerOption::new(
        "session default",
        "Use Neoism Agent default",
        "default",
        "",
    )];
    out.extend(
        agents
            .iter()
            .filter(|agent| {
                !agent
                    .get("hidden")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            // Subagent-only definitions (mode == "subagent", e.g.
            // explore/general) are Task-tool targets, not top-level
            // agents — the picker shows primaries (build/plan) plus
            // whatever the user's config adds.
            .filter(|agent| {
                agent
                    .get("mode")
                    .and_then(Value::as_str)
                    .is_none_or(|mode| mode != "subagent")
            })
            .filter_map(|agent| {
                let name = agent.get("name").and_then(Value::as_str)?.to_string();
                let description = agent
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| agent.get("mode").and_then(Value::as_str))
                    .unwrap_or("agent")
                    .to_string();
                Some(NeoismAgentPickerOption::new(
                    &name,
                    &description,
                    "agent",
                    &name,
                ))
            }),
    );
    Ok(out)
}

pub(super) fn fetch_skill_options(
    server: &str,
    directory: Option<&str>,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let path = directory
        .map(|dir| format!("/skill?directory={}", percent_encode(dir)))
        .unwrap_or_else(|| "/skill".to_string());
    let value = api_request_json(server, "GET", &path, None)?
        .ok_or_else(|| "Neoism Agent returned an empty skill response".to_string())?;
    let skills = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed skills".to_string())?;
    Ok(skills
        .iter()
        .filter_map(|skill| {
            let name = skill.get("name").and_then(Value::as_str)?.to_string();
            let description = skill
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("SKILL.md")
                .to_string();
            let footer = skill
                .get("path")
                .and_then(Value::as_str)
                .map(short_skill_path)
                .unwrap_or_else(|| "skill".to_string());
            Some(NeoismAgentPickerOption::new(
                &name,
                &description,
                &footer,
                &name,
            ))
        })
        .collect())
}

/// Fetch the recent sessions for `directory`, newest-first, capped to 24.
fn fetch_sessions_sorted(
    server: &str,
    directory: Option<&str>,
) -> Result<Vec<Value>, String> {
    let path = directory
        .filter(|directory| !directory.trim().is_empty())
        .map(|dir| format!("/session?roots=true&directory={}", percent_encode(dir)))
        .unwrap_or_else(|| "/session?roots=true".to_string());
    let value = api_request_json(server, "GET", &path, None)?
        .ok_or_else(|| "Neoism Agent returned an empty session response".to_string())?;
    let sessions = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed sessions".to_string())?;
    let mut sessions = sessions.to_vec();
    sessions.sort_by(|a, b| session_updated_at(b).cmp(&session_updated_at(a)));
    sessions.truncate(24);
    Ok(sessions)
}

pub(super) fn fetch_session_options(
    server: &str,
    current_id: Option<&str>,
    directory: Option<&str>,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let sessions = fetch_sessions_sorted(server, directory)?;
    let inputs = sessions
        .iter()
        .filter_map(|session| session_option_input(session, current_id))
        .collect::<Vec<_>>();
    Ok(group_session_options(inputs))
}

/// Flat side-panel session entries (no header rows — the side panel injects
/// its own date-group headers). Carries `updated_ms` + `pinned` so the panel
/// can sort + group them identically to the `/sessions` picker.
pub(super) fn fetch_session_entries(
    server: &str,
    current_id: Option<&str>,
    directory: Option<&str>,
) -> Result<Vec<NeoismAgentSessionEntry>, String> {
    let sessions = fetch_sessions_sorted(server, directory)?;
    let _ = current_id;
    Ok(sessions.iter().filter_map(session_entry).collect())
}

pub(super) fn fetch_subagent_options(
    server: &str,
    session_id: &str,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let current =
        api_request_json(server, "GET", &format!("/session/{session_id}"), None)?
            .ok_or_else(|| {
                "Neoism Agent returned an empty session response".to_string()
            })?;
    let parent_id = current
        .get("parentId")
        .or_else(|| current.get("parentID"))
        .and_then(Value::as_str)
        .unwrap_or(session_id)
        .to_string();
    let children = api_request_json(
        server,
        "GET",
        &format!("/session/{parent_id}/children"),
        None,
    )?
    .ok_or_else(|| "Neoism Agent returned an empty child session response".to_string())?;
    let children = children
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed child sessions".to_string())?;

    let mut out = vec![NeoismAgentPickerOption::new(
        "main session",
        &parent_id,
        "return",
        &parent_id,
    )];
    let mut children = children.iter().collect::<Vec<_>>();
    children.sort_by(|a, b| session_updated_at(b).cmp(&session_updated_at(a)));
    out.extend(children.into_iter().filter_map(|session| {
        let id = session.get("id").and_then(Value::as_str)?.to_string();
        let title = session
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("subagent")
            .to_string();
        let footer = session
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("agent")
            .to_string();
        Some(NeoismAgentPickerOption::new(&title, &id, &footer, &id))
    }));
    Ok(out)
}

pub(super) fn fetch_subagent_entries(
    server: &str,
    session_id: &str,
) -> Result<Vec<NeoismAgentSessionEntry>, String> {
    let statuses = fetch_session_statuses(server).unwrap_or_default();
    let current =
        api_request_json(server, "GET", &format!("/session/{session_id}"), None)?
            .ok_or_else(|| {
                "Neoism Agent returned an empty session response".to_string()
            })?;
    let root = fetch_root_session(server, current)?;
    let root_id = root
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Neoism Agent returned a root session without an id".to_string())?
        .to_string();

    let mut entries = vec![NeoismAgentSessionEntry::new(
        root_id.clone(),
        "main session",
        "return",
    )];
    let mut visited = HashSet::new();
    visited.insert(root_id.clone());
    collect_subagent_entries(server, &root_id, &statuses, 1, &mut entries, &mut visited)?;
    Ok(entries)
}

fn fetch_root_session(server: &str, mut session: Value) -> Result<Value, String> {
    let mut visited = HashSet::new();
    for _ in 0..SUBAGENT_TREE_MAX_DEPTH {
        let Some(parent_id) = session_parent_id(&session) else {
            break;
        };
        if !visited.insert(parent_id.clone()) {
            break;
        }
        let parent =
            api_request_json(server, "GET", &format!("/session/{parent_id}"), None);
        let Ok(Some(parent)) = parent else {
            break;
        };
        session = parent;
    }
    Ok(session)
}

fn collect_subagent_entries(
    server: &str,
    parent_id: &str,
    statuses: &HashMap<String, SessionStatusSnapshot>,
    depth: usize,
    entries: &mut Vec<NeoismAgentSessionEntry>,
    visited: &mut HashSet<String>,
) -> Result<(), String> {
    if depth > SUBAGENT_TREE_MAX_DEPTH || entries.len() >= SUBAGENT_TREE_MAX_ROWS {
        return Ok(());
    }
    let children = api_request_json(
        server,
        "GET",
        &format!("/session/{parent_id}/children"),
        None,
    )?
    .ok_or_else(|| "Neoism Agent returned an empty child session response".to_string())?;
    let children = children
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed child sessions".to_string())?;

    let mut children = children.iter().collect::<Vec<_>>();
    children.sort_by(|a, b| session_updated_at(b).cmp(&session_updated_at(a)));
    for child in children {
        if entries.len() >= SUBAGENT_TREE_MAX_ROWS {
            break;
        }
        let Some(id) = child.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !visited.insert(id.to_string()) {
            continue;
        }
        if let Some(entry) = subagent_entry_from_session(child, statuses, depth) {
            entries.push(entry);
        }
        collect_subagent_entries(server, id, statuses, depth + 1, entries, visited)?;
    }
    Ok(())
}

fn subagent_entry_from_session(
    session: &Value,
    statuses: &HashMap<String, SessionStatusSnapshot>,
    depth: usize,
) -> Option<NeoismAgentSessionEntry> {
    let id = session.get("id").and_then(Value::as_str)?.to_string();
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("subagent")
        .to_string();
    let footer = session
        .get("externalAgent")
        .and_then(external_agent_label)
        .or_else(|| {
            session
                .get("agent")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| "subagent".to_string());
    let agent_kind =
        session_external_agent_kind(session).or_else(|| AgentKind::from_label(&footer));
    Some(
        NeoismAgentSessionEntry::new(id, title, footer)
            .with_depth(depth)
            .with_agent_kind(agent_kind)
            .with_runtime_status(session_explicit_runtime_status(session, statuses)),
    )
}

fn session_parent_id(session: &Value) -> Option<String> {
    session
        .get("parentId")
        .or_else(|| session.get("parentID"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|id| !id.is_empty())
}

fn external_agent_label(external: &Value) -> Option<String> {
    external
        .get("agent")
        .or_else(|| external.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn session_external_agent_kind(session: &Value) -> Option<AgentKind> {
    let external = session.get("externalAgent")?;
    external
        .get("provider")
        .or_else(|| external.get("agent"))
        .and_then(Value::as_str)
        .and_then(AgentKind::from_label)
}

fn session_explicit_runtime_status(
    session: &Value,
    statuses: &HashMap<String, SessionStatusSnapshot>,
) -> Option<String> {
    let id = session.get("id").and_then(Value::as_str);
    id.and_then(|id| statuses.get(id))
        .and_then(|status| normalize_explicit_runtime_status(&status.kind))
        .or_else(|| {
            session
                .get("externalAgent")
                .and_then(|external| external.get("status"))
                .and_then(Value::as_str)
                .or_else(|| session.get("status").and_then(Value::as_str))
                .and_then(normalize_explicit_runtime_status)
        })
        // A child session that exists but is neither in the running-status map
        // nor carries an explicit running status has finished its run — a
        // native sub-agent is simply dropped from the runs map when it
        // completes (no "idle" status lingers). Report it completed so the
        // side panel terminalizes the branch instead of leaving it stuck on
        // its last "working" delta.
        .or(Some("completed"))
        .map(str::to_string)
}

fn normalize_explicit_runtime_status(status: &str) -> Option<&'static str> {
    match status.trim().to_ascii_lowercase().as_str() {
        "created" | "active" | "busy" | "running" => Some("running"),
        "blocked" | "retry" | "waiting_permission" | "waiting-permission" => {
            Some("blocked")
        }
        "completed" | "complete" | "idle" | "done" => Some("completed"),
        "failed" | "error" | "errored" | "stopped" | "aborted" => Some("error"),
        _ => None,
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct SessionStatusSnapshot {
    pub kind: String,
    pub started_at: Option<u64>,
    pub queue_count: usize,
    pub preview: Option<String>,
    pub parent_session_id: Option<String>,
}

pub(super) fn fetch_session_statuses(
    server: &str,
) -> Result<HashMap<String, SessionStatusSnapshot>, String> {
    let value = api_request_json(server, "GET", "/session/status", None)?
        .ok_or_else(|| "Neoism Agent returned an empty status response".to_string())?;
    let statuses = value
        .as_object()
        .ok_or_else(|| "Neoism Agent returned malformed session statuses".to_string())?;
    Ok(statuses
        .iter()
        .filter_map(|(session_id, status)| {
            let kind = status.get("type").and_then(Value::as_str)?;
            let queue = status.get("queue");
            Some((
                session_id.clone(),
                SessionStatusSnapshot {
                    kind: kind.to_string(),
                    started_at: status
                        .get("startedAt")
                        .or_else(|| status.get("started"))
                        .and_then(Value::as_u64),
                    queue_count: queue
                        .and_then(|queue| queue.get("count"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize,
                    preview: queue
                        .and_then(|queue| queue.get("preview"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    parent_session_id: status
                        .get("parentSessionID")
                        .or_else(|| status.get("parentSessionId"))
                        .or_else(|| status.get("parentID"))
                        .or_else(|| status.get("parentId"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            ))
        })
        .collect())
}

pub(super) fn fetch_config_defaults(
    server: &str,
    directory: Option<&str>,
) -> Result<ConfigDefaults, String> {
    let path = directory
        .map(|dir| format!("/config?directory={}", percent_encode(dir)))
        .unwrap_or_else(|| "/config".to_string());
    let value = api_request_json(server, "GET", &path, None)?
        .ok_or_else(|| "Neoism Agent returned an empty config response".to_string())?;
    Ok(config_defaults_from_json(&value))
}

pub(super) fn fetch_session_state(
    server: &str,
    session_id: &str,
) -> Result<SessionState, String> {
    let value = api_request_json(server, "GET", &format!("/session/{session_id}"), None)?
        .ok_or_else(|| "Neoism Agent returned an empty session response".to_string())?;
    Ok(session_state_from_json(&value))
}

pub(super) fn fetch_session_messages(
    server: &str,
    session_id: &str,
) -> Result<Vec<NeoismAgentMessage>, String> {
    fetch_session_messages_page(server, session_id, None, 80).map(|page| page.blocks)
}

/// One page of session history. `blocks` is the expanded client-side
/// render blocks (one stored message fans out into several). `raw_count`
/// is the number of *stored messages* the server returned — the only
/// reliable "is there more history" signal, since block count and message
/// count diverge (a message yields many blocks; some yield none).
pub(super) struct SessionMessagePage {
    pub blocks: Vec<NeoismAgentMessage>,
    pub raw_count: usize,
}

pub(super) fn fetch_session_messages_page(
    server: &str,
    session_id: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<SessionMessagePage, String> {
    let started = super::perf::now();
    let mut path = format!(
        "/session/{session_id}/message?order=desc&limit={}&slim=true",
        limit.max(1)
    );
    if let Some(cursor) = cursor.filter(|cursor| !cursor.is_empty()) {
        path.push_str("&cursor=");
        path.push_str(&percent_encode(cursor));
    }
    let value = api_request_json_with_read_timeout(
        server,
        "GET",
        &path,
        None,
        Duration::from_secs(10),
    )?
    .ok_or_else(|| "Neoism Agent returned an empty message response".to_string())?;
    let messages = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed messages".to_string())?;
    let raw_messages = messages.len();
    let raw_bytes = value.to_string().len();
    let blocks = message_blocks_from_response(messages, true);
    if super::perf::enabled() {
        let tool_blocks = blocks
            .iter()
            .filter(|message| {
                matches!(
                    message.kind,
                    NeoismAgentMessageKind::Tool | NeoismAgentMessageKind::Subtask
                )
            })
            .count();
        let text_bytes: usize = blocks.iter().map(|message| message.text.len()).sum();
        tracing::info!(
            target: "neoism::agent_ui_perf",
            session_id,
            raw_messages,
            blocks = blocks.len(),
            tool_blocks,
            raw_bytes,
            text_bytes,
            elapsed_us = super::perf::elapsed_us(started),
            "agent fetch session messages"
        );
    }
    Ok(SessionMessagePage {
        blocks,
        raw_count: raw_messages,
    })
}

fn message_blocks_from_response(
    messages: &[Value],
    newest_first: bool,
) -> Vec<NeoismAgentMessage> {
    neoism_ui::panels::agent_pane::api_mapping::message_blocks_from_response(
        messages,
        newest_first,
    )
    .into_iter()
    .map(NeoismAgentMessage::from)
    .collect()
}

pub(super) fn api_request_json(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Option<Value>, String> {
    crate::agent_server::ensure_started_for_request();
    let response = http_request(server, method, path, body, Duration::from_millis(900))?;
    response_json(response)
}

pub(super) fn api_request_json_with_read_timeout(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
    read_timeout: Duration,
) -> Result<Option<Value>, String> {
    crate::agent_server::ensure_started_for_request();
    let response = http_request(server, method, path, body, read_timeout)?;
    response_json(response)
}

/// Fetch the session's persistent goal. `GET /session/:id/goal` returns
/// `{ goal, researchEnabled }`; we lift just the goal object into the
/// shared [`SessionGoal`] shape (or `None` when no goal is set).
pub(super) fn fetch_session_goal(
    server: &str,
    session_id: &str,
) -> Result<Option<SessionGoal>, String> {
    let value =
        api_request_json(server, "GET", &format!("/session/{session_id}/goal"), None)?
            .unwrap_or(Value::Null);
    Ok(value.get("goal").and_then(SessionGoal::from_json))
}

fn response_json(response: HttpResponse) -> Result<Option<Value>, String> {
    if response.body.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&response.body)
        .map(Some)
        .map_err(|error| format!("Neoism Agent returned invalid JSON: {error}"))
}

pub(super) fn prompt_model_json(model: &str, thinking: Option<&str>) -> Option<Value> {
    neoism_ui::panels::agent_pane::api_mapping::prompt_model_json(model, thinking)
}

pub(super) fn session_model_json(model: &str, thinking: Option<&str>) -> Option<Value> {
    neoism_ui::panels::agent_pane::api_mapping::session_model_json(model, thinking)
}

pub(super) fn normalize_model_ref(model: &str) -> String {
    neoism_ui::panels::agent_pane::api_mapping::normalize_model_ref(model)
}

pub(super) fn normalize_thinking(value: &str) -> String {
    neoism_ui::panels::agent_pane::api_mapping::normalize_thinking(value)
}

pub(super) fn first_interaction_id(
    server: &str,
    path: &str,
    session_id: Option<&str>,
) -> Result<Option<String>, String> {
    Ok(first_interaction_value(server, path, session_id)?
        .and_then(|item| item.get("id").and_then(Value::as_str).map(str::to_string)))
}

pub(super) fn first_interaction_value(
    server: &str,
    path: &str,
    session_id: Option<&str>,
) -> Result<Option<Value>, String> {
    let value = api_request_json(server, "GET", path, None)?.ok_or_else(|| {
        "Neoism Agent returned an empty interaction response".to_string()
    })?;
    let items = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed interactions".to_string())?;
    Ok(items
        .iter()
        .find(|item| {
            session_id.is_none_or(|session_id| {
                item_session_id(item).as_deref() == Some(session_id)
            })
        })
        .cloned())
}

pub(super) fn permission_reply_alias(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "a" | "always" => "always",
        "n" | "no" | "deny" | "reject" => "reject",
        _ => "once",
    }
}

pub(super) fn is_permission_reply(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "once" | "always" | "reject" | "y" | "a" | "n" | "yes" | "no" | "deny"
    )
}

pub(super) fn format_queue(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "queue updated".to_string();
    };
    let count = value.get("count").and_then(Value::as_u64).unwrap_or(0);
    let running = if value
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "running"
    } else {
        "idle"
    };
    let Some(items) = value.get("items").and_then(Value::as_array) else {
        return format!("queue: {count} - {running}");
    };
    if items.is_empty() {
        return format!("queue: {count} - {running}");
    }
    let mut lines = vec![format!("queue: {count} - {running}")];
    lines.extend(items.iter().map(|item| {
        let index = item.get("index").and_then(Value::as_u64).unwrap_or(0) + 1;
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("(empty)");
        format!("{index}. {text}")
    }));
    lines.join("\n")
}

pub(super) fn format_mcp_status(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "no MCP status".to_string();
    };
    let Some(servers) = value.as_object() else {
        return "malformed MCP status".to_string();
    };
    if servers.is_empty() {
        return "no MCP servers configured".to_string();
    }

    let mut connected = 0;
    let mut ready = 0;
    let mut disabled = 0;
    let mut needs_auth = 0;
    let mut failed = 0;
    let mut lines = Vec::new();

    for (name, status) in servers {
        let (label, detail) = mcp_status_label(status);
        match label {
            "connected" => connected += 1,
            "ready" => ready += 1,
            "disabled" => disabled += 1,
            "needs auth" | "needs registration" => needs_auth += 1,
            _ => failed += 1,
        }
        let mut line = format!("{name} - {label}");
        if let Some(detail) = detail.filter(|detail| !detail.trim().is_empty()) {
            line.push_str(" - ");
            line.push_str(&detail);
        }
        lines.push(line);
    }

    let mut summary = Vec::new();
    if connected > 0 {
        summary.push(format!("{connected} connected"));
    }
    if ready > 0 {
        summary.push(format!("{ready} ready"));
    }
    if disabled > 0 {
        summary.push(format!("{disabled} disabled"));
    }
    if needs_auth > 0 {
        summary.push(format!("{needs_auth} auth needed"));
    }
    if failed > 0 {
        summary.push(format!("{failed} failed"));
    }
    if summary.is_empty() {
        summary.push("0 configured".to_string());
    }

    lines.insert(0, summary.join(", "));
    lines.join("\n")
}

pub(super) fn format_permissions(
    value: Option<&Value>,
    session_id: Option<&str>,
) -> String {
    let items = filter_session_items(value, session_id);
    if items.is_empty() {
        return "no pending permissions".to_string();
    }
    items
        .into_iter()
        .map(|item| {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("permission");
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Allow tool?");
            let patterns = item
                .get("patterns")
                .and_then(Value::as_array)
                .map(|patterns| {
                    patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            if patterns.is_empty() {
                format!("{id} - {title}")
            } else {
                format!("{id} - {title} - {patterns}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn format_questions(
    value: Option<&Value>,
    session_id: Option<&str>,
) -> String {
    let items = filter_session_items(value, session_id);
    if items.is_empty() {
        return "no pending questions".to_string();
    }
    items
        .into_iter()
        .map(|item| {
            let id = item.get("id").and_then(Value::as_str).unwrap_or("question");
            format!("{id} - {} ({})", question_label(item), question_count(item))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mcp_status_label(status: &Value) -> (&'static str, Option<String>) {
    let status_type = status
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match status_type {
        "connected" => ("connected", None),
        "disabled" => ("disabled", None),
        "needs_auth" => ("needs auth", None),
        "needs_client_registration" => (
            "needs registration",
            status
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
        "failed" => {
            let error = status
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if error == "MCP client runtime is not connected yet" {
                ("ready", None)
            } else {
                ("failed", Some(error.to_string()))
            }
        }
        _ => ("unknown", Some(status.to_string())),
    }
}

pub(super) fn question_count(item: &Value) -> usize {
    item.get("questions")
        .and_then(Value::as_array)
        .map(|questions| questions.len().max(1))
        .unwrap_or(1)
}

pub(super) fn question_answers(input: &str, count: usize) -> Vec<Vec<String>> {
    if count <= 1 {
        return vec![vec![input.trim().to_string()]];
    }
    input
        .split(';')
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
        .map(|answer| vec![answer.to_string()])
        .collect()
}

/// One session surfaced by semantic transcript search, joined with its
/// title from the session list and deduped to the best (closest) message
/// hit per session.
#[derive(Clone, Debug)]
pub(crate) struct NeoismAgentSemanticSessionHit {
    pub session_id: String,
    pub title: String,
    pub excerpt: String,
    pub distance: f64,
}

/// Query the agent server's semantic transcript search (`/search/semantic`)
/// and join hits with the directory's session list. `Ok(None)` means the
/// server reports semantic search unavailable (no vector backend or no
/// embeddings key) — callers should stop asking. Sessions outside this
/// directory's list are dropped, which also scopes the server's global
/// search to the current workspace.
pub(crate) fn fetch_semantic_session_hits(
    server: &str,
    query: &str,
    current_session: Option<&str>,
    directory: Option<&str>,
) -> Result<Option<Vec<NeoismAgentSemanticSessionHit>>, String> {
    let path = format!("/search/semantic?q={}&limit=20", percent_encode(query));
    let value = api_request_json(server, "GET", &path, None)?
        .ok_or_else(|| "empty semantic search response".to_string())?;
    if !value
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let hits = value
        .get("hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if hits.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let entries =
        fetch_session_entries(server, current_session, directory).unwrap_or_default();
    let titles: HashMap<&str, &str> = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry.title.as_str()))
        .collect();
    let mut best: Vec<NeoismAgentSemanticSessionHit> = Vec::new();
    for hit in &hits {
        let Some(session_id) = hit.get("sessionId").and_then(Value::as_str) else {
            continue;
        };
        let Some(title) = titles.get(session_id) else {
            continue;
        };
        let distance = hit.get("distance").and_then(Value::as_f64).unwrap_or(1.0);
        let excerpt = hit
            .get("excerpt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match best
            .iter_mut()
            .find(|existing| existing.session_id == session_id)
        {
            Some(existing) => {
                if distance < existing.distance {
                    existing.distance = distance;
                    existing.excerpt = excerpt;
                }
            }
            None => best.push(NeoismAgentSemanticSessionHit {
                session_id: session_id.to_string(),
                title: (*title).to_string(),
                excerpt,
                distance,
            }),
        }
    }
    best.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    Ok(Some(best))
}

pub(super) fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

struct HttpResponse {
    body: String,
}

pub(super) struct EventStreamConnection {
    pub stream: TcpStream,
    pub initial_body: Vec<u8>,
    pub chunked: bool,
}

pub(super) fn open_event_stream(
    server: &str,
    session_id: &str,
) -> Result<EventStreamConnection, String> {
    crate::agent_server::ensure_started_for_request();
    let server = server.trim().trim_end_matches('/');
    let (host, port, base_path) = parse_http_server(server)?;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("failed to resolve Neoism Agent server: {error}"))?
        .next()
        .ok_or_else(|| "failed to resolve Neoism Agent server".to_string())?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(900))
        .map_err(|error| format!("Neoism Agent is not reachable at {server}: {error}"))?;
    // Short read timeout so the SSE reader wakes up on every brief lull
    // — keeps the in-flight token deltas flushing into the pane each
    // frame instead of bunching into a single burst after a long wait.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(40)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(900)));

    let path = request_path(
        &base_path,
        &format!(
            "/event?sessionID={}&since=9223372036854775807&limit=1",
            percent_encode(session_id)
        ),
    );
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).map_err(|error| {
        format!("failed to write Neoism Agent event request: {error}")
    })?;

    let mut response = Vec::new();
    let header_started = Instant::now();
    loop {
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err("Neoism Agent closed the event stream".to_string()),
            Ok(n) => {
                response.extend_from_slice(&chunk[..n]);
                if let Some(header_end) = find_header_end(&response) {
                    let headers = String::from_utf8_lossy(&response[..header_end]);
                    let mut lines = headers.lines();
                    let status_line = lines.next().ok_or_else(|| {
                        "Neoism Agent returned an empty event response".to_string()
                    })?;
                    let mut status_parts = status_line.splitn(3, ' ');
                    let _http = status_parts.next();
                    let status = status_parts
                        .next()
                        .and_then(|status| status.parse::<u16>().ok())
                        .ok_or_else(|| {
                            "Neoism Agent returned an invalid event status".to_string()
                        })?;
                    let reason = status_parts.next().unwrap_or_default();
                    if !(200..300).contains(&status) {
                        let body = String::from_utf8_lossy(&response[header_end + 4..]);
                        let suffix = if body.trim().is_empty() {
                            String::new()
                        } else {
                            format!(": {}", body.trim())
                        };
                        return Err(format!(
                            "Neoism Agent event stream HTTP {status} {reason}{suffix}"
                        ));
                    }
                    let chunked = headers
                        .to_ascii_lowercase()
                        .contains("transfer-encoding: chunked");
                    return Ok(EventStreamConnection {
                        stream,
                        initial_body: response[header_end + 4..].to_vec(),
                        chunked,
                    });
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if header_started.elapsed() > Duration::from_secs(3) {
                    return Err(
                        "Neoism Agent event stream timed out waiting for headers"
                            .to_string(),
                    );
                }
            }
            Err(error) => {
                return Err(format!(
                    "failed to read Neoism Agent event headers: {error}"
                ));
            }
        }
    }
}

fn http_request(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
    read_timeout: Duration,
) -> Result<HttpResponse, String> {
    let server = server.trim().trim_end_matches('/');
    let (host, port, base_path) = parse_http_server(server)?;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("failed to resolve Neoism Agent server: {error}"))?
        .next()
        .ok_or_else(|| "failed to resolve Neoism Agent server".to_string())?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(350))
        .map_err(|error| format!("Neoism Agent is not reachable at {server}: {error}"))?;
    let _ = stream.set_read_timeout(Some(read_timeout));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(900)));

    let request_path = request_path(&base_path, path);
    let body_text = body
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("failed to encode Neoism Agent request: {error}"))?;
    let mut request = format!(
        "{method} {request_path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if let Some(body_text) = body_text.as_ref() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body_text.len()));
    }
    request.push_str("\r\n");
    if let Some(body_text) = body_text.as_ref() {
        request.push_str(body_text);
    }
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("failed to write Neoism Agent request: {error}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("failed to read Neoism Agent response: {error}"))?;
    let header_end = find_header_end(&response)
        .ok_or_else(|| "Neoism Agent returned a malformed HTTP response".to_string())?;
    let headers = String::from_utf8_lossy(&response[..header_end]);
    let body = &response[header_end + 4..];
    let mut lines = headers.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| "Neoism Agent returned an empty HTTP response".to_string())?;
    let mut status_parts = status_line.splitn(3, ' ');
    let _http = status_parts.next();
    let status = status_parts
        .next()
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| "Neoism Agent returned an invalid HTTP status".to_string())?;
    let reason = status_parts.next().unwrap_or_default();
    let body = if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked_body(body)?
    } else {
        body.to_vec()
    };
    let body = String::from_utf8(body)
        .map_err(|error| format!("Neoism Agent returned non-UTF8 data: {error}"))?;
    if !(200..300).contains(&status) {
        let suffix = if body.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", body.trim())
        };
        return Err(format!("Neoism Agent HTTP {status} {reason}{suffix}"));
    }
    Ok(HttpResponse { body })
}

fn parse_http_server(server: &str) -> Result<(String, u16, String), String> {
    let rest = server.strip_prefix("http://").ok_or_else(|| {
        format!("unsupported Neoism Agent server '{server}'; expected http://")
    })?;
    let (host_port, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = host_port
        .split_once(':')
        .map(|(host, port)| {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("invalid Neoism Agent port '{port}'"))?;
            Ok::<_, String>((host.to_string(), port))
        })
        .unwrap_or_else(|| Ok((host_port.to_string(), 80)))?;
    Ok((host, port, base_path.to_string()))
}

fn request_path(base_path: &str, path: &str) -> String {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if base_path.is_empty() {
        path
    } else {
        format!("/{base_path}{path}")
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn decode_chunked_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut rest = body;
    let mut out = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "Neoism Agent returned malformed chunked data".to_string())?;
        let size_line = String::from_utf8_lossy(&rest[..line_end]);
        let size_hex = size_line.split(';').next().unwrap_or(&size_line).trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| "Neoism Agent returned malformed chunk size".to_string())?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size + 2 {
            return Err("Neoism Agent returned truncated chunked data".to_string());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

/// Build a `/sessions` picker option (title only — the date-group header
/// carries the day; a pin marker + current-session dot render inline) paired
/// with the raw `updated` timestamp used to bucket it under a date header.
fn session_option_input(
    session: &Value,
    current_id: Option<&str>,
) -> Option<SessionOptionInput> {
    let id = session.get("id").and_then(Value::as_str)?.to_string();
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled")
        .to_string();
    let updated_ms = session_updated_at(session);
    let mut option = NeoismAgentPickerOption::new(&title, "", "", &id);
    option.is_current = Some(id.as_str()) == current_id;
    option.pinned = session_pinned(session);
    Some(SessionOptionInput { option, updated_ms })
}

/// Flat side-panel entry for one session.
fn session_entry(session: &Value) -> Option<NeoismAgentSessionEntry> {
    let id = session.get("id").and_then(Value::as_str)?.to_string();
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled")
        .to_string();
    Some(
        NeoismAgentSessionEntry::new(id, title, "")
            .with_updated_ms(session_updated_at(session))
            .with_pinned(session_pinned(session)),
    )
}

/// Whether a session JSON carries the flattened `pinned` flag.
fn session_pinned(session: &Value) -> bool {
    session
        .get("pinned")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// `POST /session/:id/pin` — toggle the session's pinned flag. Returns the new
/// pinned state read back from the updated session info.
pub(super) fn set_session_pinned(
    server: &str,
    session_id: &str,
    pinned: bool,
) -> Result<bool, String> {
    let body = serde_json::json!({ "pinned": pinned });
    let value = api_request_json(
        server,
        "POST",
        &format!("/session/{session_id}/pin"),
        Some(&body),
    )?;
    Ok(value.as_ref().map(session_pinned).unwrap_or(pinned))
}

/// `DELETE /session/:id` — permanently delete a session.
pub(super) fn delete_session(server: &str, session_id: &str) -> Result<(), String> {
    api_request_json(server, "DELETE", &format!("/session/{session_id}"), None)?;
    Ok(())
}

/// `PATCH /session/:id` with `{ title }` — rename a session.
pub(super) fn rename_session(
    server: &str,
    session_id: &str,
    title: &str,
) -> Result<(), String> {
    let body = serde_json::json!({ "title": title });
    api_request_json(
        server,
        "PATCH",
        &format!("/session/{session_id}"),
        Some(&body),
    )?;
    Ok(())
}

fn session_updated_at(session: &Value) -> u64 {
    session
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

#[allow(dead_code)]
fn model_label(model: Option<&Value>) -> String {
    let Some(model) = model else {
        return "server default".to_string();
    };
    let provider = model
        .get("providerId")
        .or_else(|| model.get("provider_id"))
        .and_then(Value::as_str)
        .unwrap_or("provider");
    let id = model
        .get("id")
        .or_else(|| model.get("modelId"))
        .or_else(|| model.get("model_id"))
        .and_then(Value::as_str)
        .unwrap_or("model");
    format!("{provider}/{id}")
}

pub(super) fn part_block(part: &Value) -> Option<NeoismAgentMessage> {
    neoism_ui::panels::agent_pane::api_mapping::part_block(part)
        .map(NeoismAgentMessage::from)
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentTodo> for NeoismAgentTodo {
    fn from(todo: neoism_ui::panels::agent_pane::state::NeoismAgentTodo) -> Self {
        Self {
            status: todo.status,
            content: todo.content,
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentUsage> for NeoismAgentUsage {
    fn from(usage: neoism_ui::panels::agent_pane::state::NeoismAgentUsage) -> Self {
        Self {
            input: usage.input,
            output: usage.output,
            reasoning: usage.reasoning,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            total: usage.total,
            cost_micros: usage.cost_micros,
            context_limit: usage.context_limit,
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind>
    for NeoismAgentMessageKind
{
    fn from(kind: neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind) -> Self {
        match kind {
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::User => {
                Self::User
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Assistant => {
                Self::Assistant
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Reasoning => {
                Self::Reasoning
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Tool => {
                Self::Tool
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::System => {
                Self::System
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Subtask => {
                Self::Subtask
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Compaction => {
                Self::Compaction
            }
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind>
    for NeoismAgentOutputKind
{
    fn from(kind: neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind) -> Self {
        match kind {
            neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind::Text => {
                Self::Text
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind::Code => {
                Self::Code
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind::Todos => {
                Self::Todos
            }
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentMessage>
    for NeoismAgentMessage
{
    fn from(message: neoism_ui::panels::agent_pane::state::NeoismAgentMessage) -> Self {
        let todos = message
            .todos
            .into_iter()
            .map(NeoismAgentTodo::from)
            .collect();
        let mut out = match message.kind {
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::User => {
                NeoismAgentMessage::user(message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Assistant => {
                NeoismAgentMessage::assistant(message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Reasoning => {
                NeoismAgentMessage::reasoning(message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Tool => {
                NeoismAgentMessage::tool(
                    message.title,
                    message.text,
                    message.status,
                    message.tool,
                    message.output_kind.into(),
                    message.lang,
                    todos,
                )
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::System => {
                NeoismAgentMessage::system(message.title, message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Subtask => {
                NeoismAgentMessage::subtask(message.title, message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Compaction => {
                NeoismAgentMessage::compaction(message.text, message.status)
            }
        };
        out.id = message.id;
        out.line_offset = message.line_offset;
        out.detail = message.detail;
        out.usage = message.usage.map(NeoismAgentUsage::from);
        out
    }
}

fn short_path(path: &str) -> String {
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() <= 2 {
        path.to_string()
    } else {
        parts[parts.len() - 2..].join("/")
    }
}

fn short_skill_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return "skill".to_string();
    }
    for marker in ["/skills/", "/skill/"] {
        if let Some(index) = path.find(marker) {
            return path[index + marker.len()..].to_string();
        }
    }
    short_path(path)
}

fn filter_session_items<'a>(
    value: Option<&'a Value>,
    session_id: Option<&str>,
) -> Vec<&'a Value> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    session_id.is_none_or(|session_id| {
                        item_session_id(item).as_deref() == Some(session_id)
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn item_session_id(item: &Value) -> Option<String> {
    item.get("sessionId")
        .or_else(|| item.get("sessionID"))
        .or_else(|| item.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn question_label(item: &Value) -> String {
    item.get("questions")
        .and_then(Value::as_array)
        .and_then(|questions| questions.first())
        .and_then(|question| {
            question
                .get("question")
                .or_else(|| question.get("label"))
                .and_then(Value::as_str)
        })
        .unwrap_or("Question")
        .to_string()
}
