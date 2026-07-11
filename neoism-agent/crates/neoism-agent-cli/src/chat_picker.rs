use anyhow::Context;
use serde_json::Value;

use crate::chat_ui::{read_key, BottomPrompt, Key, PickerItem};
use crate::{request_with_dir, response_json, DIM, RESET};

pub(crate) struct SessionEntry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) updated: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct SubagentSessionEntry {
    pub(crate) id: String,
    pub(crate) primary: String,
    pub(crate) secondary: String,
    pub(crate) status: String,
}

pub(crate) async fn fetch_sessions(
    client: &reqwest::Client,
    server: &str,
    dir: Option<&str>,
) -> anyhow::Result<Vec<SessionEntry>> {
    let value = response_json(
        request_with_dir(
            client
                .get(format!("{server}/session"))
                .query(&[("roots", "true")]),
            dir,
        )
        .send()
        .await?,
    )
    .await?;
    let array = value.as_array().context("session list not an array")?;
    let mut entries: Vec<SessionEntry> = array
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("(untitled)")
                .to_string();
            let updated = item
                .get("time")
                .and_then(|t| t.get("updated"))
                .and_then(|n| n.as_u64())
                .unwrap_or(0);
            Some(SessionEntry { id, title, updated })
        })
        .collect();
    entries.sort_by(|a, b| b.updated.cmp(&a.updated));
    Ok(entries)
}

pub(crate) fn relative_time(updated_ms: u64) -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if updated_ms == 0 || updated_ms > now {
        return "—".to_string();
    }
    let diff_secs = (now - updated_ms) / 1000;
    if diff_secs < 60 {
        return format!("{diff_secs}s ago");
    }
    let diff_min = diff_secs / 60;
    if diff_min < 60 {
        return format!("{diff_min}m ago");
    }
    let diff_hr = diff_min / 60;
    if diff_hr < 24 {
        return format!("{diff_hr}h ago");
    }
    let diff_day = diff_hr / 24;
    if diff_day < 7 {
        return format!("{diff_day}d ago");
    }
    let diff_wk = diff_day / 7;
    if diff_wk < 5 {
        return format!("{diff_wk}w ago");
    }
    format!("{}mo ago", diff_day / 30)
}

async fn fetch_model_options(
    client: &reqwest::Client,
    server: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    // Returns Vec<(provider/model, description)>
    let value =
        response_json(client.get(format!("{server}/provider")).send().await?).await?;
    let providers = value
        .get("all")
        .and_then(Value::as_array)
        .context("server did not return provider list")?;
    let mut entries: Vec<(String, String)> = Vec::new();
    for provider in providers {
        let provider_id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let provider_name = provider
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(provider_id.as_str())
            .to_string();
        let Some(models) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (model_id, _) in models {
            entries.push((format!("{provider_id}/{model_id}"), provider_name.clone()));
        }
    }
    entries.sort();
    Ok(entries)
}

async fn pick_with_search(
    ui: &mut BottomPrompt,
    title: &str,
    items: Vec<(String, String)>,
    initial_selected: usize,
) -> anyhow::Result<Option<String>> {
    let mut query = String::new();
    let mut selected = initial_selected;
    loop {
        let filtered: Vec<&(String, String)> = if query.is_empty() {
            items.iter().collect()
        } else {
            let q = query.to_ascii_lowercase();
            items
                .iter()
                .filter(|(primary, secondary)| {
                    primary.to_ascii_lowercase().contains(&q)
                        || secondary.to_ascii_lowercase().contains(&q)
                })
                .collect()
        };
        if selected >= filtered.len() && !filtered.is_empty() {
            selected = filtered.len() - 1;
        }
        let picker_items: Vec<PickerItem> = filtered
            .iter()
            .map(|(primary, secondary)| PickerItem {
                primary: primary.clone(),
                secondary: secondary.clone(),
            })
            .collect();
        ui.render_picker_search(title, &query, &picker_items, selected)?;
        match read_key()? {
            Key::Up => {
                if !filtered.is_empty() {
                    if selected == 0 {
                        selected = filtered.len() - 1;
                    } else {
                        selected -= 1;
                    }
                }
            }
            Key::Down => {
                if !filtered.is_empty() {
                    selected = (selected + 1) % filtered.len();
                }
            }
            Key::Enter | Key::Right => {
                if let Some((value, _)) = filtered.get(selected) {
                    return Ok(Some((*value).clone()));
                }
            }
            Key::Esc | Key::CtrlC | Key::CtrlD => {
                return Ok(None);
            }
            Key::Char(ch) => {
                query.push(ch);
                selected = 0;
            }
            Key::Backspace => {
                query.pop();
                selected = 0;
            }
            _ => {}
        }
    }
}

pub(crate) async fn pick_model(
    client: &reqwest::Client,
    server: &str,
    ui: &mut BottomPrompt,
    current: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let entries = fetch_model_options(client, server).await?;
    if entries.is_empty() {
        ui.before_output()?;
        println!();
        println!("{DIM}no models available — check /providers and /auth{RESET}");
        return Ok(None);
    }
    let initial = current
        .and_then(|c| entries.iter().position(|(value, _)| value == c))
        .unwrap_or(0);
    pick_with_search(ui, "models", entries, initial).await
}

const THINK_OPTIONS: &[(&str, &str, Option<&str>)] = &[
    ("default", "let the model decide", None),
    ("minimal", "fastest, no reasoning", Some("minimal")),
    ("low", "light reasoning", Some("low")),
    ("medium", "balanced reasoning", Some("medium")),
    ("high", "deep reasoning", Some("high")),
    ("xhigh", "max reasoning effort", Some("xhigh")),
    (
        "ultra",
        "multi-agent orchestration (GPT-5.6 only)",
        Some("ultra"),
    ),
];

pub(crate) async fn pick_think(
    ui: &mut BottomPrompt,
    current: Option<&str>,
) -> anyhow::Result<Option<Option<String>>> {
    let entries: Vec<(String, String)> = THINK_OPTIONS
        .iter()
        .map(|(label, desc, _)| (label.to_string(), desc.to_string()))
        .collect();
    let current_label = current.unwrap_or("default");
    let initial = entries
        .iter()
        .position(|(label, _)| label == current_label)
        .unwrap_or(0);
    let Some(picked_label) = pick_with_search(ui, "thinking", entries, initial).await?
    else {
        return Ok(None);
    };
    let value = THINK_OPTIONS
        .iter()
        .find(|(label, _, _)| *label == picked_label.as_str())
        .map(|(_, _, value)| value.map(ToString::to_string))
        .unwrap_or(None);
    Ok(Some(value))
}

pub(crate) async fn pick_agent(
    client: &reqwest::Client,
    server: &str,
    dir: Option<&str>,
    ui: &mut BottomPrompt,
    current: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let value = response_json(
        request_with_dir(client.get(format!("{server}/agent")), dir)
            .send()
            .await?,
    )
    .await?;
    let agents = value.as_array().context("agent response was not a list")?;
    let entries: Vec<(String, String)> = agents
        .iter()
        .filter(|agent| {
            !agent
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|agent| {
            let name = agent.get("name").and_then(Value::as_str)?.to_string();
            let mode = agent.get("mode").and_then(Value::as_str).unwrap_or("all");
            let description = agent
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or(mode)
                .to_string();
            Some((name, description))
        })
        .collect();
    if entries.is_empty() {
        ui.before_output()?;
        println!();
        println!("{DIM}no agents available{RESET}");
        return Ok(None);
    }
    let initial = current
        .and_then(|c| entries.iter().position(|(name, _)| name == c))
        .unwrap_or(0);
    pick_with_search(ui, "agents", entries, initial).await
}

pub(crate) async fn fetch_subagent_sessions(
    client: &reqwest::Client,
    server: &str,
    current_session_id: &str,
) -> anyhow::Result<Vec<SubagentSessionEntry>> {
    let current = response_json(
        client
            .get(format!("{server}/session/{current_session_id}"))
            .send()
            .await?,
    )
    .await?;
    let main_id = current
        .get("parentId")
        .or_else(|| current.get("parentID"))
        .and_then(Value::as_str)
        .unwrap_or(current_session_id)
        .to_string();
    let main = if main_id == current_session_id {
        current
    } else {
        match response_json(
            client
                .get(format!("{server}/session/{main_id}"))
                .send()
                .await?,
        )
        .await
        {
            Ok(main) => main,
            Err(_) => current.clone(),
        }
    };
    let statuses = response_json(
        client
            .get(format!("{server}/session/status"))
            .send()
            .await?,
    )
    .await
    .unwrap_or(Value::Null);
    let children = response_json(
        client
            .get(format!("{server}/api/session/{main_id}/children"))
            .send()
            .await?,
    )
    .await
    .unwrap_or(Value::Null);

    let mut entries = Vec::new();
    entries.push(subagent_entry(&main, current_session_id, &statuses, true));

    let mut child_entries = children
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|session| subagent_entry(session, current_session_id, &statuses, false))
        .collect::<Vec<_>>();
    child_entries.sort_by(|left, right| {
        status_rank(&left.status)
            .cmp(&status_rank(&right.status))
            .then_with(|| right.secondary.cmp(&left.secondary))
    });
    entries.extend(child_entries);
    Ok(entries)
}

pub(crate) async fn pick_subagent_session(
    client: &reqwest::Client,
    server: &str,
    ui: &mut BottomPrompt,
    current_session_id: &str,
) -> anyhow::Result<Option<SubagentSessionEntry>> {
    let entries = fetch_subagent_sessions(client, server, current_session_id).await?;
    if entries.is_empty() {
        ui.before_output()?;
        println!();
        println!("{DIM}no subagent sessions for this session{RESET}");
        return Ok(None);
    }
    let items: Vec<PickerItem> = entries
        .iter()
        .map(|entry| PickerItem {
            primary: entry.primary.clone(),
            secondary: entry.secondary.clone(),
        })
        .collect();
    let mut selected = entries
        .iter()
        .position(|entry| entry.id == current_session_id)
        .unwrap_or(0);
    loop {
        ui.render_picker("sub-agents", &items, selected)?;
        match read_key()? {
            Key::Up => {
                if selected == 0 {
                    selected = entries.len() - 1;
                } else {
                    selected -= 1;
                }
            }
            Key::Down => {
                selected = (selected + 1) % entries.len();
            }
            Key::Enter | Key::Right | Key::Tab => {
                return Ok(Some(entries[selected].clone()));
            }
            Key::Esc | Key::CtrlC | Key::CtrlD => {
                return Ok(None);
            }
            _ => {}
        }
    }
}

fn subagent_entry(
    session: &Value,
    current_session_id: &str,
    statuses: &Value,
    main: bool,
) -> SubagentSessionEntry {
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let agent = session
        .get("agent")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("(untitled)");
    let updated = session
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let status =
        session_status_label(statuses, &id, if main { "main" } else { "completed" });
    let current = if id == current_session_id {
        "current · "
    } else {
        ""
    };
    let agent_label = agent.as_deref().unwrap_or("agent");
    let primary = if main {
        format!("main · @{agent_label}")
    } else {
        format!("@{agent_label} · {title}")
    };
    let secondary = format!(
        "{current}{status} · {} · {}",
        short_session_id(&id),
        relative_time(updated)
    );
    SubagentSessionEntry {
        id,
        primary,
        secondary,
        status,
    }
}

fn session_status_label(statuses: &Value, session_id: &str, fallback: &str) -> String {
    match statuses
        .get(session_id)
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
    {
        Some("busy") => "working".to_string(),
        Some("retry") => "retrying".to_string(),
        Some("idle") | None => fallback.to_string(),
        Some(other) => other.to_string(),
    }
}

fn status_rank(status: &str) -> u8 {
    match status {
        "working" | "retrying" => 0,
        "main" => 1,
        _ => 2,
    }
}

fn short_session_id(id: &str) -> String {
    id.chars().take(18).collect()
}

pub(crate) async fn pick_session(
    client: &reqwest::Client,
    server: &str,
    dir: Option<&str>,
    ui: &mut BottomPrompt,
) -> anyhow::Result<Option<String>> {
    let entries = fetch_sessions(client, server, dir).await?;
    if entries.is_empty() {
        ui.before_output()?;
        println!();
        println!("{DIM}no sessions yet — start typing to create one{RESET}");
        return Ok(None);
    }

    let items: Vec<PickerItem> = entries
        .iter()
        .map(|e| PickerItem {
            primary: e.title.clone(),
            secondary: relative_time(e.updated),
        })
        .collect();

    let mut selected = 0usize;
    loop {
        ui.render_picker("sessions", &items, selected)?;
        match read_key()? {
            Key::Up => {
                if selected == 0 {
                    selected = entries.len() - 1;
                } else {
                    selected -= 1;
                }
            }
            Key::Down => {
                selected = (selected + 1) % entries.len();
            }
            Key::Enter | Key::Right | Key::Tab => {
                return Ok(Some(entries[selected].id.clone()));
            }
            Key::Esc | Key::CtrlC | Key::CtrlD => {
                return Ok(None);
            }
            _ => {}
        }
    }
}
