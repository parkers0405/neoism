use std::collections::HashMap;

use serde_json::{json, Value};

use super::state::{
    picker::NeoismAgentPickerOption, NeoismAgentImage, NeoismAgentMessage,
    NeoismAgentMessageKind, NeoismAgentOutputKind, NeoismAgentTodo, NeoismAgentUsage,
};

const SUBTASK_COMPLETION_SYSTEM_MARKER: &str =
    "Neoism runtime notification: background subagent completion.";
const SUBTASK_COMPLETION_MESSAGE_PREFIX: &str = "msg_subtask_completion_";
/// Marker the agent-server stamps on a background-job completion prompt's
/// `info.system` (see `background_job.rs`). The prompt is injected with
/// `role: "user"` so the model sees the captured output, but the human did
/// NOT type it — so we render it as a system notice, never a user bubble.
const BACKGROUND_TASK_COMPLETION_SYSTEM_MARKER: &str =
    "Neoism runtime notification: background shell task completion.";
const BACKGROUND_TASK_COMPLETION_MESSAGE_PREFIX: &str = "msg_background_completion_";

fn is_background_task_completion(system: &str, message_id: &str) -> bool {
    system.contains(BACKGROUND_TASK_COMPLETION_SYSTEM_MARKER)
        || message_id.starts_with(BACKGROUND_TASK_COMPLETION_MESSAGE_PREFIX)
}

/// The compact, durable transcript card for a finished background shell
/// task. The agent-server persists the completion as a user-role
/// runtime-notification prompt (`msg_background_completion_{job}`, see
/// `background_job.rs`); the desktop's live SSE path synthesizes a
/// `background_task_result` tool card with id `background-task-{job}` the
/// moment `session.background_task.completed` fires. Mapping the persisted
/// prompt to that SAME card (same id, same shape) means every history
/// snapshot REGENERATES the card in place of the client-injected copy —
/// the server copy replaces it instead of a refresh wiping it — and the
/// web host gets the identical card from the daemon's shared mapping.
fn background_completion_card(fallback_id: &str, text: &str) -> NeoismAgentMessage {
    let job_id = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("job_id:"))
        .map(str::trim)
        .and_then(|value| value.split_whitespace().next())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            fallback_id
                .strip_prefix(BACKGROUND_TASK_COMPLETION_MESSAGE_PREFIX)
                .filter(|suffix| !suffix.is_empty())
                .map(str::to_string)
        });
    let status = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("status:"))
        .map(str::trim)
        .and_then(|value| value.split_whitespace().next())
        .unwrap_or("completed");
    let summary = match job_id.as_deref() {
        Some(job_id) => {
            format!("job_id: {job_id}\nstatus: {status}\nbackground shell task finished")
        }
        None => format!("status: {status}\nbackground shell task finished"),
    };
    let mut card = agent_message_tool(
        "background_task_result",
        summary,
        status,
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    card.detail = text.to_string();
    card.id = match job_id {
        Some(job_id) => format!("background-task-{job_id}"),
        None => fallback_id.to_string(),
    };
    card
}

/// The notice's own opening line. Live `message.part.updated` events
/// carry the part, not the parent message's `info`, so the `system`
/// marker can be absent and `messageID` can still be empty on the first
/// delta. Without a text-based check the part fell through to
/// `agent_message_user` and painted a USER BUBBLE for a frame or two,
/// which the next history refresh then re-mapped to a hidden System row
/// - the "it shows up and then gets torn out while I'm looking at it"
/// flicker. Matching the text too keeps live and history agreeing from
/// the very first frame.
const SUBTASK_COMPLETION_TEXT_MARKER: &str = "Subagent finished.";

fn is_subtask_completion(system: &str, message_id: &str) -> bool {
    system.contains(SUBTASK_COMPLETION_SYSTEM_MARKER)
        || message_id.starts_with(SUBTASK_COMPLETION_MESSAGE_PREFIX)
}

/// `is_subtask_completion`, plus the text fallback for live parts whose
/// envelope carries neither marker yet.
fn is_subtask_completion_text(system: &str, message_id: &str, text: &str) -> bool {
    is_subtask_completion(system, message_id)
        || text
            .trim_start()
            .starts_with(SUBTASK_COMPLETION_TEXT_MARKER)
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct ConfigDefaults {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub input_help_visible: Option<bool>,
    pub sidebar_visible: Option<bool>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub parent_id: Option<String>,
    pub directory: Option<String>,
}

pub fn model_options_from_providers_json(value: &Value) -> Vec<NeoismAgentPickerOption> {
    let providers = value
        .get("providers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut groups = Vec::new();
    for provider in providers {
        let provider_id = provider.get("id").and_then(Value::as_str).unwrap_or("");
        if provider_id.is_empty() {
            continue;
        }
        let provider_name = provider
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(provider_id);
        let Some(models) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        let mut options = Vec::new();
        for (model_key, model) in models {
            let model_id = model.get("id").and_then(Value::as_str).unwrap_or(model_key);
            let title = model
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(model_id);
            let context = model
                .get("limit")
                .and_then(|limit| limit.get("context"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let free = model_is_free(provider_id, model);
            let footer = match (free, context > 0) {
                (true, true) => "Free".to_string(),
                (true, false) => "Free".to_string(),
                (false, true) => {
                    format!("{}k ctx", (context as f32 / 1000.0).round() as u64)
                }
                (false, false) => String::new(),
            };
            options.push(NeoismAgentPickerOption::model(
                title,
                provider_name,
                &footer,
                &format!("{provider_id}/{model_id}"),
            ));
        }
        if !options.is_empty() {
            options.sort_by(|a, b| {
                model_option_rank(a)
                    .cmp(&model_option_rank(b))
                    .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            });
            groups.push((provider_id.to_string(), provider_name.to_string(), options));
        }
    }
    groups.sort_by(|(left_id, left_name, _), (right_id, right_name, _)| {
        provider_rank(left_id)
            .cmp(&provider_rank(right_id))
            .then_with(|| left_name.to_lowercase().cmp(&right_name.to_lowercase()))
    });
    let mut out = Vec::new();
    for (_, provider_name, options) in groups {
        out.push(NeoismAgentPickerOption::header(&provider_name));
        out.extend(options);
    }
    out
}

fn model_is_free(provider_id: &str, model: &Value) -> bool {
    if provider_id != "opencode" {
        return false;
    }
    let Some(cost) = model.get("cost") else {
        return false;
    };
    cost.get("input").and_then(Value::as_f64).unwrap_or(1.0) == 0.0
        && cost.get("output").and_then(Value::as_f64).unwrap_or(1.0) == 0.0
}

fn model_option_rank(option: &NeoismAgentPickerOption) -> usize {
    if option.value.starts_with("opencode/") && option.footer.starts_with("Free") {
        0
    } else if option.value.starts_with("openai/") {
        1
    } else if option.value.starts_with("anthropic/") {
        2
    } else if option.value.starts_with("claude-code/") {
        3
    } else {
        4
    }
}

fn provider_rank(provider_id: &str) -> usize {
    match provider_id {
        "opencode" => 0,
        "openai" => 1,
        "anthropic" => 2,
        "claude-code" => 3,
        _ => 4,
    }
}

pub fn model_context_limit_from_providers_json(
    value: &Value,
    model_ref: &str,
) -> Option<u64> {
    let (provider_id, model_id) = split_model_ref(model_ref)?;
    let providers = value
        .get("providers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for provider in providers {
        if provider.get("id").and_then(Value::as_str) != Some(provider_id.as_str()) {
            continue;
        }
        let Some(models) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (key, model) in models {
            let candidate_id = model.get("id").and_then(Value::as_str).unwrap_or(key);
            if candidate_id != model_id.as_str() {
                continue;
            }
            return model
                .get("limit")
                .and_then(|limit| limit.get("context"))
                .and_then(Value::as_u64)
                .filter(|limit| *limit > 0);
        }
    }
    None
}

pub fn config_defaults_from_json(value: &Value) -> ConfigDefaults {
    let agent = value
        .get("defaultAgent")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let thinking = value
        .get("variant")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let input_help_visible = value
        .get("input-hints")
        .and_then(Value::as_bool);
    let sidebar_visible = value
        .get("sidebar")
        .and_then(Value::as_bool);
    ConfigDefaults {
        agent,
        model,
        thinking,
        input_help_visible,
        sidebar_visible,
    }
}

pub fn session_state_from_json(value: &Value) -> SessionState {
    let agent = value
        .get("agent")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let model_ref = value.get("model").and_then(|model| {
        let provider = model
            .get("providerId")
            .or_else(|| model.get("provider_id"))
            .and_then(Value::as_str)?;
        let model_id = model
            .get("id")
            .or_else(|| model.get("modelId"))
            .or_else(|| model.get("model_id"))
            .and_then(Value::as_str)?;
        Some(format!("{provider}/{model_id}"))
    });
    let thinking = value
        .get("model")
        .and_then(|m| m.get("variant"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let parent_id = value
        .get("parentId")
        .or_else(|| value.get("parentID"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let directory = value
        .get("directory")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    SessionState {
        agent,
        model: model_ref,
        thinking,
        parent_id,
        directory,
    }
}

pub fn prompt_model_json(model: &str, thinking: Option<&str>) -> Option<Value> {
    split_model_ref(model).map(|(provider_id, model_id)| {
        json!({
            "providerId": provider_id,
            "modelId": model_id,
            "variant": thinking.filter(|value| !value.is_empty()),
        })
    })
}

pub fn session_model_json(model: &str, thinking: Option<&str>) -> Option<Value> {
    split_model_ref(model).map(|(provider_id, model_id)| {
        json!({
            "providerId": provider_id,
            "id": model_id,
            "variant": thinking.filter(|value| !value.is_empty()),
        })
    })
}

pub fn normalize_model_ref(model: &str) -> String {
    let model = model.trim();
    if model.is_empty()
        || matches!(
            model.to_ascii_lowercase().as_str(),
            "default" | "server-default" | "server" | "model-default"
        )
    {
        return String::new();
    }
    if model.contains('/') {
        model.to_string()
    } else {
        format!("openai/{model}")
    }
}

pub fn normalize_thinking(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "" | "default" | "model" | "none" | "off" => String::new(),
        "max" => "xhigh".to_string(),
        _ => value,
    }
}

fn split_model_ref(model: &str) -> Option<(String, String)> {
    let model = model.trim();
    if model.is_empty() || model == "server default" {
        return None;
    }
    if let Some((provider_id, model_id)) = model.split_once('/') {
        if provider_id.is_empty() || model_id.is_empty() {
            return None;
        }
        return Some((provider_id.to_string(), model_id.to_string()));
    }
    Some(("openai".to_string(), model.to_string()))
}

fn agent_message_new(
    kind: NeoismAgentMessageKind,
    text: impl Into<String>,
) -> NeoismAgentMessage {
    NeoismAgentMessage {
        id: String::new(),
        kind,
        title: String::new(),
        text: text.into(),
        status: String::new(),
        tool: String::new(),
        output_kind: NeoismAgentOutputKind::Text,
        lang: String::new(),
        line_offset: None,
        todos: Vec::new(),
        detail: String::new(),
        usage: None,
        author: None,
        images: Vec::new(),
    }
}

fn agent_message_user(text: impl Into<String>) -> NeoismAgentMessage {
    agent_message_new(NeoismAgentMessageKind::User, text)
}

fn agent_message_assistant(text: impl Into<String>) -> NeoismAgentMessage {
    agent_message_new(NeoismAgentMessageKind::Assistant, text)
}

fn agent_message_reasoning(text: impl Into<String>) -> NeoismAgentMessage {
    let mut message = agent_message_new(NeoismAgentMessageKind::Reasoning, text);
    message.title = "Thinking".to_string();
    message
}

fn agent_message_tool(
    title: impl Into<String>,
    text: impl Into<String>,
    status: impl Into<String>,
    tool: impl Into<String>,
    output_kind: NeoismAgentOutputKind,
    lang: impl Into<String>,
    todos: Vec<NeoismAgentTodo>,
) -> NeoismAgentMessage {
    let mut message = agent_message_new(NeoismAgentMessageKind::Tool, text);
    message.title = title.into();
    message.status = status.into();
    message.tool = tool.into();
    message.output_kind = output_kind;
    message.lang = lang.into();
    message.todos = todos;
    message
}

fn agent_message_subtask(
    title: impl Into<String>,
    text: impl Into<String>,
) -> NeoismAgentMessage {
    let mut message = agent_message_new(NeoismAgentMessageKind::Subtask, text);
    message.title = title.into();
    message
}

fn agent_message_system(
    title: impl Into<String>,
    text: impl Into<String>,
) -> NeoismAgentMessage {
    let mut message = agent_message_new(NeoismAgentMessageKind::System, text);
    message.title = title.into();
    message
}

pub fn message_blocks_from_response(
    messages: &[Value],
    newest_first: bool,
) -> Vec<NeoismAgentMessage> {
    // OpenCode measures an answer from its parent user prompt, rather than
    // from the instant the provider-side assistant record happened to be
    // opened. Build the lookup before ordering the response so this remains
    // correct whether the endpoint returned newest-first or oldest-first.
    let user_created = messages
        .iter()
        .filter_map(|message| {
            let info = message.get("info")?;
            (info.get("role").and_then(Value::as_str) == Some("user"))
                .then(|| {
                    Some((
                        info.get("id")?.as_str()?.to_string(),
                        info.get("time")?.get("created")?.as_u64()?,
                    ))
                })
                .flatten()
        })
        .collect::<HashMap<_, _>>();
    // Background completion replies are parented to a synthetic runtime user
    // message, not the human request that launched the task. Recover that
    // durable origin through the parent task part so the final footer measures
    // the whole user-visible operation (including the subagent wait).
    let task_origins = messages
        .iter()
        .filter_map(|message| {
            let info = message.get("info")?;
            (info.get("role").and_then(Value::as_str) == Some("assistant"))
                .then_some(message)
        })
        .flat_map(|message| {
            let info = message.get("info").unwrap_or(&Value::Null);
            let parent_id = info
                .get("parentID")
                .or_else(|| info.get("parentId"))
                .or_else(|| info.get("parent_id"))
                .and_then(Value::as_str);
            let created = parent_id.and_then(|id| user_created.get(id)).copied();
            message
                .get("parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(move |part| {
                    Some((task_session_id_from_part(part)?, created?))
                })
        })
        .collect::<HashMap<_, _>>();
    let runtime_origins = messages
        .iter()
        .filter_map(|message| {
            let info = message.get("info")?;
            if info.get("role").and_then(Value::as_str) != Some("user")
                || !info
                    .get("system")
                    .and_then(Value::as_str)
                    .is_some_and(|system| {
                        system.contains(SUBTASK_COMPLETION_SYSTEM_MARKER)
                    })
            {
                return None;
            }
            let message_id = info.get("id")?.as_str()?.to_string();
            let origin = message
                .get("parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .flat_map(task_ids_from_completion_text)
                .filter_map(|task_id| task_origins.get(task_id).copied())
                .min()?;
            Some((message_id, origin))
        })
        .collect::<HashMap<_, _>>();
    let mut indexes = (0..messages.len()).collect::<Vec<_>>();
    if newest_first {
        indexes.reverse();
    }
    let mut out = Vec::new();
    for index in indexes {
        let message = &messages[index];
        let response_started_at = message
            .get("info")
            .and_then(|info| {
                info.get("parentID")
                    .or_else(|| info.get("parentId"))
                    .or_else(|| info.get("parent_id"))
            })
            .and_then(Value::as_str)
            .and_then(|parent_id| {
                runtime_origins
                    .get(parent_id)
                    .or_else(|| user_created.get(parent_id))
            })
            .copied();
        out.extend(message_blocks_with_start(message, response_started_at));
    }
    out
}

fn task_session_id_from_part(part: &Value) -> Option<String> {
    if part.get("type").and_then(Value::as_str) != Some("tool")
        || part.get("tool").and_then(Value::as_str) != Some("task")
    {
        return None;
    }
    let state = part.get("state").unwrap_or(&Value::Null);
    for metadata in [state.get("metadata"), part.get("metadata")]
        .into_iter()
        .flatten()
    {
        if let Some(session_id) = metadata
            .get("sessionId")
            .or_else(|| metadata.get("sessionID"))
            .or_else(|| metadata.get("session_id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            return Some(session_id.to_string());
        }
    }
    state
        .get("output")
        .and_then(Value::as_str)
        .and_then(|output| task_ids_from_completion_text(output).next())
        .map(str::to_string)
}

fn task_ids_from_completion_text(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter_map(|line| {
        line.trim()
            .strip_prefix("task_id:")
            .and_then(|value| value.trim().split_whitespace().next())
            .filter(|value| !value.is_empty())
    })
}

pub fn message_blocks(message: &Value) -> Vec<NeoismAgentMessage> {
    message_blocks_with_start(message, None)
}

fn message_blocks_with_start(
    message: &Value,
    response_started_at: Option<u64>,
) -> Vec<NeoismAgentMessage> {
    let role = message
        .get("info")
        .and_then(|info| info.get("role"))
        .and_then(Value::as_str)
        .unwrap_or("system");
    let parts = message
        .get("parts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    if role == "user" {
        if parts
            .iter()
            .any(|part| part.get("type").and_then(Value::as_str) == Some("compaction"))
        {
            return Vec::new();
        }
        let info = message.get("info").unwrap_or(&Value::Null);
        let text = parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        let id = message
            .get("info")
            .and_then(|info| info.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let images = parts
            .iter()
            .filter(|part| {
                part.get("type").and_then(Value::as_str) == Some("file")
                    && part
                        .get("mime")
                        .and_then(Value::as_str)
                        .is_some_and(|mime| mime.starts_with("image/"))
            })
            .filter_map(image_from_part)
            .collect::<Vec<_>>();
        if text.is_empty() && images.is_empty() {
            return Vec::new();
        }
        let system = info.get("system").and_then(Value::as_str).unwrap_or("");
        if is_background_task_completion(system, &id) {
            // Injected as a user-role turn so the model sees the captured
            // output, but the user didn't send it. Map it to the durable
            // completion card (NOT a user bubble, and not a hidden system
            // notice) so the card the live event painted survives every
            // history refresh under the same identity.
            return vec![background_completion_card(&id, &text)];
        }
        let message = if is_subtask_completion_text(system, &id, &text) {
            agent_message_system("Subagent", text)
        } else {
            agent_message_user(text)
        };
        let mut message = message;
        message.images = images;
        message.id = id;
        // Shared-session author (wired): the agent-server stamps the true
        // sender's display name onto the user message `info` under the key
        // `author` (from `PromptRequest.author` in `session_prompt::
        // append_prompt`), so a peer's prompt shows THEIR presence orb + name
        // on history reload. Absent (older client, or the local sender's own
        // message) → `None`, and the renderer falls back to the local
        // presence name / "You".
        message.author = info
            .get("author")
            .and_then(Value::as_str)
            .map(str::to_string);
        return vec![message];
    }

    if role == "assistant" && is_compaction_summary_message(parts) {
        let text = parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        let mut block = NeoismAgentMessage::compaction(text, "summary");
        block.id = message
            .get("info")
            .and_then(|info| info.get("id"))
            .and_then(Value::as_str)
            .or_else(|| {
                parts
                    .iter()
                    .find(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                    .and_then(|part| part.get("id"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default()
            .to_string();
        return vec![block];
    }

    let mut blocks = parts.iter().filter_map(part_block).collect::<Vec<_>>();
    if role == "assistant" {
        normalize_assistant_reasoning_order(&mut blocks);
        if let Some(footer) = assistant_response_footer(message, response_started_at) {
            if let Some(answer) = blocks
                .iter_mut()
                .rfind(|block| block.kind == NeoismAgentMessageKind::Assistant)
            {
                answer.status = footer;
            }
        }
        // Show a run that ended in a TERMINAL error. Transient errors retry
        // silently (see session_retry) and, if they recover, leave no
        // `info.error` — so nothing shows and the run just keeps going. An
        // `info.error` is present only when the error was fatal (non-retryable)
        // or a transient one that exhausted every retry — exactly the
        // "errors that have to stop", which the user DOES want to see. Render
        // it here so it's visible even from a reloaded snapshot (the live
        // `session.error` broadcast is lost if the SSE dropped when it fired).
        if let Some(text) = message
            .get("info")
            .and_then(|info| info.get("error"))
            .and_then(assistant_error_message)
        {
            blocks.push(agent_message_system("Agent error", text));
        }
    }
    blocks
}

fn assistant_response_footer(
    message: &Value,
    response_started_at: Option<u64>,
) -> Option<String> {
    let info = message.get("info")?;
    let intermediate_tool_step = info
        .get("finish")
        .and_then(Value::as_str)
        .is_some_and(|finish| matches!(finish, "tool-calls" | "unknown"));
    if intermediate_tool_step && info.get("error").is_none_or(Value::is_null) {
        return None;
    }
    let time = info.get("time")?;
    let completed = time.get("completed")?.as_u64()?;
    let created = response_started_at.or_else(|| time.get("created")?.as_u64())?;
    let agent = info
        .get("agent")
        .or_else(|| info.get("mode"))
        .and_then(Value::as_str)
        .map(display_agent_name)
        .filter(|value| !value.is_empty())?;
    let model = info
        .get("modelId")
        .or_else(|| info.get("modelID"))
        .or_else(|| info.get("model_id"))
        .and_then(Value::as_str)
        .map(display_model_name)
        .filter(|value| !value.is_empty())?;
    let duration = display_response_duration(completed.saturating_sub(created));
    Some(format!("{agent} · {model} · {duration}"))
}

fn display_agent_name(value: &str) -> String {
    value
        .split(|ch: char| ch == '-' || ch == '_' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(capitalize_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_model_name(value: &str) -> String {
    let value = value.rsplit('/').next().unwrap_or(value);
    if let Some(rest) = value
        .strip_prefix("gpt-")
        .or_else(|| value.strip_prefix("GPT-"))
    {
        let mut parts = rest.split(['-', '_']).filter(|part| !part.is_empty());
        let Some(version) = parts.next() else {
            return "GPT".to_string();
        };
        let suffix = parts.map(capitalize_word).collect::<Vec<_>>().join(" ");
        return if suffix.is_empty() {
            format!("GPT-{version}")
        } else {
            format!("GPT-{version} {suffix}")
        };
    }
    value
        .split(|ch: char| ch == '-' || ch == '_' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| match part.to_ascii_lowercase().as_str() {
            "gpt" => "GPT".to_string(),
            "api" => "API".to_string(),
            "ai" => "AI".to_string(),
            _ if part.chars().all(|ch| ch.is_ascii_digit() || ch == '.') => {
                part.to_string()
            }
            _ => capitalize_word(part),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize_word(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().chain(chars).collect()
}

fn display_response_duration(milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        return format!("{milliseconds}ms");
    }
    if milliseconds < 60_000 {
        return format!("{:.1}s", milliseconds as f64 / 1_000.0);
    }
    if milliseconds < 3_600_000 {
        let minutes = milliseconds / 60_000;
        let seconds = (milliseconds % 60_000) / 1_000;
        return format!("{minutes}m {seconds}s");
    }
    if milliseconds < 86_400_000 {
        let hours = milliseconds / 3_600_000;
        let minutes = (milliseconds % 3_600_000) / 60_000;
        return format!("{hours}h {minutes}m");
    }
    let days = milliseconds / 86_400_000;
    let hours = (milliseconds % 86_400_000) / 3_600_000;
    format!("{days}d {hours}h")
}

/// Extract a human-readable message from an assistant `info.error`, tolerating
/// the shapes it can take: a plain string, `{message}`, `{data:{message}}`, or
/// `{name, data:{message}}`. Returns `None` for an absent/empty error.
fn assistant_error_message(error: &Value) -> Option<String> {
    if let Some(text) = error.as_str() {
        let text = text.trim();
        return (!text.is_empty()).then(|| text.to_string());
    }
    error
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn normalize_assistant_reasoning_order(blocks: &mut Vec<NeoismAgentMessage>) {
    let mut index = 0;
    while index < blocks.len() {
        if blocks[index].kind != NeoismAgentMessageKind::Reasoning {
            index += 1;
            continue;
        }
        let Some(insert_at) = blocks[..index]
            .iter()
            .rposition(|message| message.kind == NeoismAgentMessageKind::Assistant)
        else {
            index += 1;
            continue;
        };
        let assistant = blocks.remove(insert_at);
        let reasoning_index = index.saturating_sub(1);
        blocks.insert(reasoning_index + 1, assistant);
        index += 1;
    }
}

fn is_compaction_summary_message(parts: &[Value]) -> bool {
    parts.iter().any(|part| {
        part.get("type").and_then(Value::as_str) == Some("compaction")
            && part
                .get("summary")
                .and_then(Value::as_bool)
                .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_usage_matches_opencode_normalized_bucket_sum() {
        let usage = usage_from_step_finish(&json!({
            "tokens": {
                "total": 9_999,
                "input": 100,
                "output": 20,
                "reasoning": 80,
                "cache": { "read": 5, "write": 3 }
            },
            "cost": 0.0
        }))
        .unwrap();

        assert_eq!(usage.total, 208);
    }

    #[test]
    fn running_apply_patch_keeps_header_without_growing_detail() {
        let part = json!({
            "id": "prt-patch",
            "type": "tool",
            "tool": "apply_patch",
            "state": {
                "status": "running",
                "input": {
                    "patchText": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n"
                }
            }
        });
        let message = part_block(&part).expect("tool part");
        assert_eq!(message.tool, "apply_patch");
        assert_eq!(message.status, "running");
        assert!(message.title.contains("ApplyPatch"));
        assert!(message.detail.is_empty());
    }

    #[test]
    fn completed_apply_patch_embeds_edit_detail() {
        let part = json!({
            "id": "prt-patch",
            "type": "tool",
            "tool": "apply_patch",
            "state": {
                "status": "completed",
                "input": {
                    "patchText": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n"
                },
                "metadata": {}
            }
        });
        let message = part_block(&part).expect("tool part");
        assert_eq!(message.status, "completed");
        assert!(message.detail.contains("neoismToolDetail"));
        assert!(message.detail.contains("patchText"));
    }

    #[test]
    fn provider_models_map_to_sorted_picker_options_and_context_limits() {
        let providers = json!({
            "providers": [
                {
                    "id": "anthropic",
                    "name": "Anthropic",
                    "models": {
                        "claude": {
                            "name": "Claude",
                            "limit": { "context": 200000 }
                        }
                    }
                },
                {
                    "id": "openai",
                    "name": "OpenAI",
                    "models": {
                        "gpt-key": {
                            "id": "gpt-5",
                            "name": "GPT-5",
                            "limit": { "context": 128000 }
                        }
                    }
                }
            ]
        });

        let options = model_options_from_providers_json(&providers);

        assert_eq!(options.len(), 4);
        assert!(options[0].is_header);
        assert_eq!(options[0].title, "OpenAI");
        assert_eq!(options[1].title, "GPT-5");
        assert_eq!(options[1].description, "");
        assert_eq!(options[1].section, "OpenAI");
        assert_eq!(options[1].footer, "128k ctx");
        assert_eq!(options[1].value, "openai/gpt-5");
        assert!(options[2].is_header);
        assert_eq!(options[2].title, "Anthropic");
        assert_eq!(options[3].value, "anthropic/claude");
        assert_eq!(
            model_context_limit_from_providers_json(&providers, "anthropic/claude"),
            Some(200000)
        );
        assert_eq!(
            model_context_limit_from_providers_json(&providers, "missing/model"),
            None
        );
    }

    #[test]
    fn model_options_mark_opencode_free_models_first() {
        let providers = json!({
            "providers": [
                {
                    "id": "openai",
                    "name": "OpenAI",
                    "models": {
                        "gpt": {
                            "id": "gpt",
                            "name": "GPT",
                            "limit": { "context": 128000 }
                        }
                    }
                },
                {
                    "id": "opencode",
                    "name": "OpenCode Zen",
                    "models": {
                        "free": {
                            "id": "free",
                            "name": "Free Model",
                            "cost": { "input": 0, "output": 0 },
                            "limit": { "context": 200000 }
                        }
                    }
                }
            ]
        });

        let options = model_options_from_providers_json(&providers);

        assert!(options[0].is_header);
        assert_eq!(options[0].title, "OpenCode Zen");
        assert_eq!(options[1].value, "opencode/free");
        assert_eq!(options[1].footer, "Free");
        assert!(options[2].is_header);
        assert_eq!(options[2].title, "OpenAI");
        assert_eq!(options[3].value, "openai/gpt");
    }

    #[test]
    fn config_json_uses_canonical_keys_and_session_migrations_remain() {
        let config = config_defaults_from_json(&json!({
            "defaultAgent": "build",
            "model": "openai/gpt-5",
            "variant": "high",
            "input-hints": false
        }));
        assert_eq!(config.agent.as_deref(), Some("build"));
        assert_eq!(config.model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(config.thinking.as_deref(), Some("high"));
        assert_eq!(config.input_help_visible, Some(false));

        let session = session_state_from_json(&json!({
            "agent": "review",
            "parentID": "ses-parent",
            "directory": "/tmp/project",
            "model": {
                "provider_id": "openai",
                "model_id": "gpt-5",
                "variant": "xhigh"
            }
        }));
        assert_eq!(session.agent.as_deref(), Some("review"));
        assert_eq!(session.model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(session.thinking.as_deref(), Some("xhigh"));
        assert_eq!(session.parent_id.as_deref(), Some("ses-parent"));
        assert_eq!(session.directory.as_deref(), Some("/tmp/project"));
    }

    #[test]
    fn model_json_helpers_normalize_defaults_and_variants() {
        assert_eq!(normalize_model_ref("default"), "");
        assert_eq!(normalize_model_ref("gpt-5"), "openai/gpt-5");
        assert_eq!(normalize_thinking("MAX"), "xhigh");
        assert_eq!(normalize_thinking("off"), "");

        assert_eq!(
            prompt_model_json("anthropic/claude", Some("xhigh")),
            Some(json!({
                "providerId": "anthropic",
                "modelId": "claude",
                "variant": "xhigh"
            }))
        );
        assert_eq!(
            session_model_json("gpt-5", Some("")),
            Some(json!({
                "providerId": "openai",
                "id": "gpt-5",
                "variant": null
            }))
        );
    }

    #[test]
    fn newest_first_message_response_is_rendered_chronologically() {
        let newest = json!({
            "info": { "id": "msg-new", "role": "assistant" },
            "parts": [{ "id": "prt-new", "type": "text", "text": "new reply" }]
        });
        let oldest = json!({
            "info": { "id": "msg-old", "role": "user" },
            "parts": [{ "id": "prt-old", "type": "text", "text": "old prompt" }]
        });

        let blocks = message_blocks_from_response(&[newest, oldest], true);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, NeoismAgentMessageKind::User);
        assert_eq!(blocks[0].text, "old prompt");
        assert_eq!(blocks[1].kind, NeoismAgentMessageKind::Assistant);
        assert_eq!(blocks[1].text, "new reply");
    }

    #[test]
    fn completed_answer_gets_opencode_style_agent_model_and_duration_footer() {
        let assistant = json!({
            "info": {
                "id": "msg-answer",
                "role": "assistant",
                "parentID": "msg-prompt",
                "agent": "build",
                "modelId": "gpt-5.6-sol",
                "time": { "created": 1_200, "completed": 219_000 }
            },
            "parts": [
                { "id": "prt-first", "type": "text", "text": "working" },
                { "id": "prt-final", "type": "text", "text": "done" }
            ]
        });
        let user = json!({
            "info": {
                "id": "msg-prompt",
                "role": "user",
                "time": { "created": 1_000 }
            },
            "parts": [{ "id": "prt-prompt", "type": "text", "text": "please fix it" }]
        });

        let blocks = message_blocks_from_response(&[assistant, user], true);

        assert_eq!(blocks[1].status, "");
        assert_eq!(blocks[2].status, "Build · GPT-5.6 Sol · 3m 38s");
    }

    #[test]
    fn background_subagent_answer_duration_starts_at_human_request() {
        let final_answer = json!({
            "info": {
                "id": "msg-final",
                "role": "assistant",
                "parentID": "msg-runtime-completion",
                "agent": "build",
                "modelId": "gpt-5.6-sol",
                "time": { "created": 180_000, "completed": 194_769 }
            },
            "parts": [{ "id": "prt-final", "type": "text", "text": "done" }]
        });
        let runtime_completion = json!({
            "info": {
                "id": "msg-runtime-completion",
                "role": "user",
                "system": SUBTASK_COMPLETION_SYSTEM_MARKER,
                "time": { "created": 171_000 }
            },
            "parts": [{
                "id": "prt-runtime-completion",
                "type": "text",
                "text": "Subagent finished.\ntask_id: ses-child\nstatus: completed"
            }]
        });
        let task_launch = json!({
            "info": {
                "id": "msg-launch",
                "role": "assistant",
                "parentID": "msg-human",
                "finish": "tool-calls",
                "time": { "created": 1_100, "completed": 2_000 }
            },
            "parts": [{
                "id": "prt-task",
                "type": "tool",
                "tool": "task",
                "state": {
                    "status": "completed",
                    "metadata": { "sessionId": "ses-child", "background": true },
                    "output": "task_id: ses-child\nstatus: running"
                }
            }]
        });
        let human = json!({
            "info": {
                "id": "msg-human",
                "role": "user",
                "time": { "created": 1_000 }
            },
            "parts": [{ "id": "prt-human", "type": "text", "text": "inspect it" }]
        });

        let blocks = message_blocks_from_response(
            &[final_answer, runtime_completion, task_launch, human],
            true,
        );
        let answer = blocks
            .iter()
            .find(|block| block.text == "done")
            .expect("final answer");
        let task = blocks
            .iter()
            .find(|block| block.tool == "task")
            .expect("background task card");

        assert_eq!(answer.status, "Build · GPT-5.6 Sol · 3m 13s");
        assert_eq!(task.status, "running");
    }

    #[test]
    fn response_duration_and_model_labels_follow_opencode_units() {
        assert_eq!(display_response_duration(450), "450ms");
        assert_eq!(display_response_duration(12_350), "12.3s");
        assert_eq!(display_response_duration(3_660_000), "1h 1m");
        assert_eq!(display_model_name("openai/gpt-5.6-sol"), "GPT-5.6 Sol");
        assert_eq!(display_agent_name("code-review"), "Code Review");
    }

    #[test]
    fn intermediate_tool_call_message_does_not_get_a_response_footer() {
        let blocks = message_blocks(&json!({
            "info": {
                "id": "msg-tool-step",
                "role": "assistant",
                "agent": "build",
                "modelId": "gpt-5.6-sol",
                "finish": "tool-calls",
                "time": { "created": 1_000, "completed": 2_000 }
            },
            "parts": [{ "id": "prt-progress", "type": "text", "text": "checking" }]
        }));

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].status.is_empty());
    }

    #[test]
    fn subtask_completion_notification_renders_as_a_hidden_system_notice() {
        let message = json!({
            "info": {
                "id": "msg-subtask-done",
                "role": "user",
                "system": SUBTASK_COMPLETION_SYSTEM_MARKER,
            },
            "parts": [{
                "id": "prt-subtask-done",
                "type": "text",
                "text": "Subagent finished.\ntask_id: ses_child"
            }]
        });

        let blocks = message_blocks(&message);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, NeoismAgentMessageKind::System);
        assert_eq!(blocks[0].title, "Subagent");
    }

    /// The live broadcast and the history reload must classify the
    /// subagent notice the SAME way. If live renders it as a user bubble
    /// (visible) and history as a System notice (hidden), the block flashes
    /// onto the screen and is torn away on the next refresh.
    #[test]
    fn subtask_completion_is_hidden_on_the_first_live_frame_too() {
        let text =
            "Subagent finished. task_id: ses_abc agent: @explore status: completed";

        // Worst case: the live part carries NEITHER the system marker nor a
        // message id (first delta of the stream).
        let bare = part_block(&json!({
            "id": "prt-live",
            "type": "text",
            "role": "user",
            "text": text,
        }))
        .expect("live part maps to a block");
        assert_eq!(bare.kind, NeoismAgentMessageKind::System);

        // With the id present it must classify identically.
        let tagged = part_block(&json!({
            "id": "prt-live",
            "type": "text",
            "messageID": "msg_subtask_completion_ses_abc",
            "role": "user",
            "text": text,
        }))
        .expect("live part maps to a block");

        let history = message_blocks(&json!({
            "info": {
                "id": "msg_subtask_completion_ses_abc",
                "role": "user",
                "system": SUBTASK_COMPLETION_SYSTEM_MARKER,
            },
            "parts": [{"id": "prt-hist", "type": "text", "text": text}]
        }));

        assert_eq!(history.len(), 1);
        assert_eq!(tagged.kind, history[0].kind);
        assert_eq!(history[0].kind, NeoismAgentMessageKind::System);
    }

    /// A normal user prompt must NOT be swallowed by the text heuristic.
    #[test]
    fn ordinary_user_text_is_still_a_user_bubble() {
        let block = part_block(&json!({
            "id": "prt-user",
            "type": "text",
            "role": "user",
            "text": "Subagent finished? tell me about it",
        }))
        .expect("user part maps to a block");
        assert_eq!(block.kind, NeoismAgentMessageKind::User);
    }

    #[test]
    fn background_completion_reserved_id_never_renders_as_a_user_message() {
        let message = json!({
            "info": {
                "id": "msg_background_completion_job_123",
                "role": "user"
            },
            "parts": [{
                "id": "prt-background-done",
                "type": "text",
                "text": "Background shell task finished.\njob_id: job_123\nstatus: completed"
            }]
        });

        let blocks = message_blocks(&message);

        // Maps to the durable completion card under the live event's
        // identity — a history refresh replaces the client-injected copy
        // instead of wiping it (and never renders a fake user bubble).
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, NeoismAgentMessageKind::Tool);
        assert_eq!(blocks[0].tool, "background_task_result");
        assert_eq!(blocks[0].id, "background-task-job_123");
        assert_eq!(blocks[0].status, "completed");
        assert!(blocks[0].text.contains("job_id: job_123"));
        assert!(blocks[0].text.contains("background shell task finished"));
        assert!(blocks[0].detail.contains("Background shell task finished."));
    }

    #[test]
    fn live_background_completion_reserved_id_never_renders_as_a_user_message() {
        let block = part_block(&json!({
            "id": "prt-background-done",
            "messageID": "msg_background_completion_job_123",
            "type": "text",
            "role": "user",
            "text": "Background shell task finished.\njob_id: job_123\nstatus: completed"
        }))
        .expect("runtime completion block");

        assert_eq!(block.kind, NeoismAgentMessageKind::Tool);
        assert_eq!(block.tool, "background_task_result");
        assert_eq!(block.id, "background-task-job_123");
    }

    #[test]
    fn background_completion_card_falls_back_to_the_reserved_id_suffix() {
        // No job_id line in the text: the job id still recovers from the
        // reserved message id so the live/persisted identities stay equal.
        let message = json!({
            "info": {
                "id": "msg_background_completion_job_9",
                "role": "user"
            },
            "parts": [{
                "id": "prt-background-done",
                "type": "text",
                "text": "Background shell task finished."
            }]
        });

        let blocks = message_blocks(&message);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, "background-task-job_9");
        assert_eq!(blocks[0].status, "completed");
    }

    #[test]
    fn markerless_subtask_completion_uses_reserved_message_identity() {
        let block = part_block(&json!({
            "id": "prt-subtask-done",
            "messageID": "msg_subtask_completion_ses_child",
            "type": "text",
            "role": "user",
            "text": "Subagent finished.\ntask_id: ses_child"
        }))
        .expect("runtime completion block");

        assert_eq!(block.kind, NeoismAgentMessageKind::System);
        assert_eq!(block.title, "Subagent");
        assert_eq!(block.id, "msg_subtask_completion_ses_child");
    }

    #[test]
    fn assistant_parts_keep_server_order_for_finish_refresh() {
        let message = json!({
            "info": { "id": "msg-tools", "role": "assistant" },
            "parts": [
                { "id": "text-before", "type": "text", "text": "before" },
                {
                    "id": "read-a",
                    "type": "tool",
                    "tool": "read",
                    "state": {
                        "status": "completed",
                        "input": { "path": "/repo/src/a.rs" },
                        "output": "a body"
                    }
                },
                {
                    "id": "grep-b",
                    "type": "tool",
                    "tool": "grep",
                    "state": {
                        "status": "completed",
                        "input": { "pattern": "Thing" },
                        "output": "b body"
                    }
                },
                {
                    "id": "list-c",
                    "type": "tool",
                    "tool": "list",
                    "state": {
                        "status": "completed",
                        "input": { "path": "/repo/src" },
                        "output": "c body"
                    }
                },
                { "id": "text-after", "type": "text", "text": "after" }
            ]
        });

        let blocks = message_blocks(&message);

        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[0].id, "text-before");
        assert_eq!(blocks[1].id, "read-a");
        assert_eq!(blocks[2].id, "grep-b");
        assert_eq!(blocks[3].id, "list-c");
        assert_eq!(blocks[4].id, "text-after");
    }

    #[test]
    fn slash_heavy_multiline_bash_command_stays_a_bounded_title_preview() {
        let command = concat!(
            "f=/tmp/neoism-donkey-116-lab/plugins/",
            "NeoismDonkeyDisconnectDupeIntegrationHarness/integration-result.txt\n",
            "if test -f \"$f\" && grep -q occupied-refusal \"$f\"; then cat \"$f\"; fi"
        );
        let state = json!({ "input": { "command": command } });

        let title = tool_title("bash", &state);

        assert_eq!(
            title,
            format!(
                "Bash({})",
                truncate_line(command, TOOL_TITLE_TARGET_MAX_CHARS)
            )
        );
        assert!(!title.contains('\n'));
        assert!(!title.starts_with("Bash(NeoismDonkeyDisconnectDupeIntegrationHarness/"));
    }

    #[test]
    fn tool_filesystem_targets_still_use_compact_paths() {
        let state = json!({ "input": { "path": "/repo/src/panels/agent.rs" } });

        assert_eq!(tool_title("read", &state), "Read(panels/agent.rs)");
    }

    #[test]
    fn assistant_reasoning_parts_render_before_final_text_on_refresh() {
        let message = json!({
            "info": { "id": "msg-reasoning", "role": "assistant" },
            "parts": [
                { "id": "text-final", "type": "text", "text": "final answer" },
                { "id": "reason-1", "type": "reasoning", "text": "thought one" },
                { "id": "reason-2", "type": "reasoning", "text": "thought two" }
            ]
        });

        let blocks = message_blocks(&message);

        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].id, "reason-1");
        assert_eq!(blocks[1].id, "reason-2");
        assert_eq!(blocks[2].id, "text-final");
    }

    #[test]
    fn assistant_reasoning_refresh_preserves_prior_tool_order() {
        let message = json!({
            "info": { "id": "msg-reasoning-tools", "role": "assistant" },
            "parts": [
                { "id": "text-final", "type": "text", "text": "final answer" },
                {
                    "id": "tool-1",
                    "type": "tool",
                    "tool": "bash",
                    "state": { "status": "completed", "input": "echo ok", "output": "ok" }
                },
                { "id": "reason-1", "type": "reasoning", "text": "post tool thought" }
            ]
        });

        let blocks = message_blocks(&message);

        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].id, "tool-1");
        assert_eq!(blocks[0].kind, NeoismAgentMessageKind::Tool);
        assert_eq!(blocks[1].id, "reason-1");
        assert_eq!(blocks[1].kind, NeoismAgentMessageKind::Reasoning);
        assert_eq!(blocks[2].id, "text-final");
        assert_eq!(blocks[2].kind, NeoismAgentMessageKind::Assistant);
    }

    #[test]
    fn assistant_compaction_summary_renders_as_compaction_card() {
        let message = json!({
            "info": { "id": "msg-summary", "role": "assistant" },
            "parts": [
                {
                    "id": "prt-marker",
                    "type": "compaction",
                    "messageID": "msg-summary",
                    "summary": true,
                    "reason": "summary"
                },
                { "id": "prt-text", "type": "text", "text": "## Goal\n- Preserve state" }
            ]
        });

        let blocks = message_blocks(&message);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, "msg-summary");
        assert_eq!(blocks[0].kind, NeoismAgentMessageKind::Compaction);
        assert_eq!(blocks[0].text, "## Goal\n- Preserve state");
    }

    #[test]
    fn live_user_part_uses_parent_message_identity() {
        let message = part_block(&json!({
            "id": "part-user-1",
            "messageID": "msg-user-1",
            "type": "text",
            "role": "user",
            "text": "rare delayed echo"
        }))
        .expect("user part");

        assert_eq!(message.kind, NeoismAgentMessageKind::User);
        assert_eq!(message.id, "msg-user-1");
    }

    #[test]
    fn live_user_part_ignores_empty_parent_message_identity() {
        let message = part_block(&json!({
            "id": "part-user-1",
            "messageID": "",
            "type": "text",
            "role": "user",
            "text": "fallback identity"
        }))
        .expect("user part");

        assert_eq!(message.id, "part-user-1");
    }

    #[test]
    fn live_user_image_uses_parent_message_identity() {
        let message = part_block(&json!({
            "id": "part-image-1",
            "messageID": "msg-user-1",
            "type": "file",
            "role": "user",
            "mime": "image/png",
            "filename": "clipboard.png",
            "url": "data:image/png;base64,AA=="
        }))
        .expect("user image part");

        assert_eq!(message.kind, NeoismAgentMessageKind::User);
        assert_eq!(message.id, "msg-user-1");
        assert_eq!(message.images.len(), 1);
        assert_eq!(message.images[0].url, "data:image/png;base64,AA==");
    }
}

pub fn part_block(part: &Value) -> Option<NeoismAgentMessage> {
    let id = part
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let kind = part.get("type").and_then(Value::as_str)?;
    let mut message = match kind {
        // Parts carry no role of their own; the agent-server tags the
        // user-prompt parts it broadcasts with `role: "user"` so
        // remote viewers render them as user bubbles, not assistant
        // text.
        "text" if part.get("role").and_then(Value::as_str) == Some("user") => {
            // A user-role part carrying a runtime-notification `system` marker
            // (stamped by the agent-server on background-job / subagent
            // completion prompts) is NOT a human turn — render it as a system
            // notice, not a user bubble, live. History reload does the same in
            // `message_blocks`.
            let system = part.get("system").and_then(Value::as_str).unwrap_or("");
            let message_id = part
                .get("messageID")
                .or_else(|| part.get("messageId"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                // Both completion cards return EARLY on purpose: the tail of
                // this function rewrites `message.id` to the parent message
                // id for any user-role text part, which would clobber the
                // card's durable identity and break the replace-in-place
                // contract with `message_blocks`.
                let fallback_id = if message_id.is_empty() {
                    id.as_str()
                } else {
                    message_id
                };
                if is_background_task_completion(system, message_id) {
                    // Same durable completion card as `message_blocks` — the
                    // live broadcast and the history reload must agree on the
                    // card's identity so refreshes replace instead of wipe.
                    return Some(background_completion_card(fallback_id, text));
                }
            }
            part.get("text").and_then(Value::as_str).map(|text| {
                if is_subtask_completion_text(system, message_id, text) {
                    // Hidden system notice, same as `message_blocks`. It must
                    // be classified this way on the FIRST frame; anything
                    // visible here gets torn away by the next refresh.
                    agent_message_system("Subagent", text)
                } else {
                    agent_message_user(text)
                }
            })
        }
        "text" => part
            .get("text")
            .and_then(Value::as_str)
            .map(agent_message_assistant),
        "reasoning" => part
            .get("text")
            .and_then(Value::as_str)
            .map(agent_message_reasoning),
        "compaction"
            if part
                .get("summary")
                .and_then(Value::as_bool)
                .unwrap_or(false) =>
        {
            Some(NeoismAgentMessage::compaction(
                String::new(),
                part.get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("summary"),
            ))
        }
        "compaction" => None,
        "agent" => part.get("name").and_then(Value::as_str).map(|name| {
            agent_message_subtask(format!("Agent({name})"), format!("@{name}"))
        }),
        "subtask" => Some(subtask_block(part)),
        "tool" => Some(tool_block(part)),
        "file"
            if part
                .get("mime")
                .and_then(Value::as_str)
                .is_some_and(|mime| mime.starts_with("image/")) =>
        {
            image_from_part(part).map(|image| {
                let mut message = agent_message_user("");
                message.images.push(image);
                message
            })
        }
        "file" => part
            .get("filename")
            .and_then(Value::as_str)
            .or_else(|| part.get("url").and_then(Value::as_str))
            .map(|name| agent_message_system("File", name)),
        "step-finish" => step_finish_block(part),
        _ => None,
    }?;
    if kind == "compaction" {
        if let Some(message_id) = part
            .get("messageID")
            .or_else(|| part.get("messageId"))
            .or_else(|| part.get("message_id"))
            .and_then(Value::as_str)
        {
            message.id = message_id.to_string();
            return Some(message);
        }
    }
    // Shared-session author (wired): the agent-server stamps the true
    // sender's display name onto each broadcast user part under the key
    // `author`, next to `role: "user"` (see `session_prompt::append_prompt`),
    // so a remote peer's LIVE prompt renders THEIR presence orb + name. Absent
    // (older client, or the local sender's own optimistic echo) → `None` and
    // the local presence name is used as the fallback seed.
    if message.kind == NeoismAgentMessageKind::User {
        message.author = part
            .get("author")
            .and_then(Value::as_str)
            .map(str::to_string);
        tracing::info!(
            target: "neoism::author",
            author = ?message.author,
            "live user part received (author=None means the SENDER's client didn't stamp it — usually an older build; a real name means it flowed through)"
        );
    }
    // History collapses a user turn into one row keyed by the parent message
    // id. The server broadcasts its text and image-file parts separately;
    // both must carry that same identity or the image becomes a duplicate
    // user card. Text also covers runtime/system notices injected as
    // user-role turns. Non-image file notices remain independent cards.
    let originated_as_user_part = part.get("role").and_then(Value::as_str)
        == Some("user")
        && (kind == "text" || message.kind == NeoismAgentMessageKind::User);
    message.id = if originated_as_user_part {
        part.get("messageID")
            .or_else(|| part.get("messageId"))
            .or_else(|| part.get("message_id"))
            .and_then(Value::as_str)
            .filter(|message_id| !message_id.trim().is_empty())
            .unwrap_or(&id)
            .to_string()
    } else {
        id
    };
    Some(message)
}

fn image_from_part(part: &Value) -> Option<NeoismAgentImage> {
    let url = part.get("url").and_then(Value::as_str)?.to_string();
    let mime = part.get("mime").and_then(Value::as_str)?.to_string();
    Some(NeoismAgentImage {
        filename: part
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("image")
            .to_string(),
        url,
        mime,
    })
}

fn subtask_block(part: &Value) -> NeoismAgentMessage {
    let agent = part.get("agent").and_then(Value::as_str).unwrap_or("agent");
    let description = part
        .get("description")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(agent);
    let prompt = part.get("prompt").and_then(Value::as_str).unwrap_or("");
    agent_message_subtask(
        format!("Task({description})"),
        format!("@{agent}\n{prompt}").trim().to_string(),
    )
}

fn tool_block(part: &Value) -> NeoismAgentMessage {
    let tool = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
    let state = part.get("state").unwrap_or(&Value::Null);
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending");
    let display_status = task_runtime_status(tool, state).unwrap_or(status);
    let mut todos = if is_todo_tool(tool) {
        todos_from_state(state)
    } else {
        Vec::new()
    };
    let raw_detail = if tool == "task" {
        state
            .get("output")
            .and_then(Value::as_str)
            .filter(|output| !output.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| raw_tool_body(tool, display_status, state, &todos))
    } else {
        raw_tool_body(tool, display_status, state, &todos)
    };
    let output = unwrap_tool_output(&raw_detail);
    let preview = tool_body(tool, display_status, state, &todos);
    let output_kind = if !todos.is_empty() {
        NeoismAgentOutputKind::Todos
    } else {
        NeoismAgentOutputKind::Text
    };
    if output_kind == NeoismAgentOutputKind::Todos && output.content.is_empty() {
        todos = todos_from_state(state);
    }
    let title = if let Some(path) = output.path.as_deref() {
        let base = tool_title(tool, state);
        if base.contains('(') {
            base
        } else {
            format!("{base}({})", short_path(path))
        }
    } else {
        tool_title(tool, state)
    };
    let mut message = agent_message_tool(
        title,
        preview,
        display_status,
        tool,
        output_kind,
        output
            .lang
            .unwrap_or_else(|| infer_lang_from_title(&tool_title(tool, state))),
        todos,
    );
    message.detail = if is_unsettled_edit_tool(tool, display_status) {
        String::new()
    } else {
        edit_tool_detail(tool, state).unwrap_or(output.content)
    };
    message.line_offset = output.line_offset;
    message
}

fn task_runtime_status<'a>(tool: &str, state: &'a Value) -> Option<&'a str> {
    if tool != "task" {
        return None;
    }
    state
        .get("metadata")
        .and_then(|metadata| metadata.get("status"))
        .and_then(Value::as_str)
        .or_else(|| {
            state
                .get("output")
                .and_then(Value::as_str)
                .and_then(task_status_from_output)
        })
}

fn task_status_from_output(output: &str) -> Option<&str> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("status:")
            .map(str::trim)
            .filter(|status| !status.is_empty())
    })
}

fn step_finish_block(part: &Value) -> Option<NeoismAgentMessage> {
    let usage = usage_from_step_finish(part)?;
    let price = if usage.cost_micros > 0 {
        format!(" - {}", format_cost_micros(usage.cost_micros))
    } else {
        String::new()
    };
    let cache = usage.cache_read.saturating_add(usage.cache_write);
    let cache_suffix = if cache > 0 {
        format!(", {cache} cache")
    } else {
        String::new()
    };
    let mut message = agent_message_system(
        "Context",
        format!(
            "{} tokens ({} in, {} out, {} thinking{cache_suffix}){price}",
            usage.total, usage.input, usage.output, usage.reasoning
        ),
    );
    message.usage = Some(usage);
    Some(message)
}

fn usage_from_step_finish(part: &Value) -> Option<NeoismAgentUsage> {
    let tokens = part.get("tokens")?;
    let input = token_field(tokens, &["input"]);
    let output = token_field(tokens, &["output"]);
    let reasoning = token_field(tokens, &["reasoning"]);
    let cache = tokens.get("cache").unwrap_or(&Value::Null);
    let cache_read = token_field(cache, &["read"])
        .saturating_add(token_field(tokens, &["cacheRead", "cache_read"]));
    let cache_write = token_field(cache, &["write"])
        .saturating_add(token_field(tokens, &["cacheWrite", "cache_write"]));
    // Match OpenCode v2's UI/ACP context metric exactly. These buckets are
    // normalized by the server (cache removed from input, reasoning removed
    // from output), so summing all five reconstructs the full context usage.
    let total = input
        .saturating_add(output)
        .saturating_add(reasoning)
        .saturating_add(cache_read)
        .saturating_add(cache_write);
    let cost = part
        .get("cost")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .max(0.0);
    Some(NeoismAgentUsage {
        input,
        output,
        reasoning,
        cache_read,
        cache_write,
        total,
        cost_micros: (cost * 1_000_000.0).round().clamp(0.0, u64::MAX as f64) as u64,
        context_limit: None,
    })
}

fn token_field(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or(0)
}

fn format_cost_micros(micros: u64) -> String {
    let dollars = micros as f64 / 1_000_000.0;
    if dollars < 0.01 {
        format!("${dollars:.6}")
    } else {
        format!("${dollars:.4}")
    }
}

const TOOL_TITLE_TARGET_MAX_CHARS: usize = 96;

fn tool_title(tool: &str, state: &Value) -> String {
    let name = tool_name(tool);
    let input = state.get("input").unwrap_or(&Value::Null);
    let path = input
        .get("path")
        .or_else(|| input.get("filePath"))
        .or_else(|| input.get("file_path"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    if let Some(path) = path {
        return format!(
            "{name}({})",
            truncate_line(&short_path(path), TOOL_TITLE_TARGET_MAX_CHARS)
        );
    }

    // A shell command is not a filesystem path. Passing it through
    // `short_path` made every slash a path separator and retained an enormous
    // tail after the final two slashes. Multiline commands could consequently
    // consume the four-row title cap and make the body look vertically cut
    // off. Keep the useful OpenCode-style `Bash(command)` label, but normalize
    // it to a bounded single-line preview.
    let inline_target = input
        .get("command")
        .or_else(|| input.get("pattern"))
        .or_else(|| input.get("q"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    inline_target
        .map(|target| {
            format!(
                "{name}({})",
                truncate_line(target, TOOL_TITLE_TARGET_MAX_CHARS)
            )
        })
        .unwrap_or(name)
}

fn tool_body(
    tool: &str,
    status: &str,
    state: &Value,
    todos: &[NeoismAgentTodo],
) -> String {
    if status == "completed" {
        if is_todo_tool(tool) {
            if todos.is_empty() {
                return "todos updated".to_string();
            }
            return todos
                .iter()
                .map(|todo| format!("{} {}", todo_marker(&todo.status), todo.content))
                .collect::<Vec<_>>()
                .join("\n");
        }
        let title = state
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let output = state
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !title.is_empty() {
            return title.to_string();
        }
        return summarize_tool_output(tool, output);
    }
    if status == "error" {
        return state
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("tool failed")
            .trim()
            .to_string();
    }
    state
        .get("input")
        .and_then(tool_input_summary)
        .or_else(|| {
            state
                .get("input")
                .and_then(|input| serde_json::to_string(input).ok())
                .map(|input| truncate_line(&input, 160))
        })
        .unwrap_or_default()
}

fn raw_tool_body(
    tool: &str,
    status: &str,
    state: &Value,
    todos: &[NeoismAgentTodo],
) -> String {
    if status == "completed" {
        if is_todo_tool(tool) {
            if todos.is_empty() {
                return "todos updated".to_string();
            }
            return todos
                .iter()
                .map(|todo| format!("{} {}", todo_marker(&todo.status), todo.content))
                .collect::<Vec<_>>()
                .join("\n");
        }
        let title = state
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let output = state
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !title.is_empty() && !output.is_empty() {
            return format!("{title}\n{output}");
        }
        return if !title.is_empty() {
            title.to_string()
        } else {
            output.to_string()
        };
    }
    if status == "error" {
        return state
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("tool failed")
            .trim()
            .to_string();
    }
    state
        .get("input")
        .and_then(|input| serde_json::to_string_pretty(input).ok())
        .unwrap_or_default()
}

fn summarize_tool_output(tool: &str, output: &str) -> String {
    if output.trim().is_empty() {
        return "completed".to_string();
    }
    let line_count = output.lines().count().max(1);
    let lower = tool.to_ascii_lowercase();
    if lower.contains("read") || lower.contains("grep") || lower.contains("glob") {
        return if line_count == 1 {
            "1 line".to_string()
        } else {
            format!("{line_count} lines")
        };
    }
    let first = output
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(output)
        .trim();
    if line_count > 1 {
        format!("{}  (+{} lines)", truncate_line(first, 120), line_count - 1)
    } else {
        truncate_line(first, 160)
    }
}

fn tool_input_summary(input: &Value) -> Option<String> {
    for key in [
        "command",
        "path",
        "filePath",
        "file_path",
        "pattern",
        "query",
        "q",
    ] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(truncate_line(value, 160));
            }
        }
    }
    None
}

struct UnwrappedToolOutput {
    content: String,
    lang: Option<String>,
    path: Option<String>,
    line_offset: Option<usize>,
}

fn unwrap_tool_output(raw: &str) -> UnwrappedToolOutput {
    let path = extract_tag(raw, "path").map(str::to_string);
    let mut content = extract_tag(raw, "content")
        .map(str::to_string)
        .unwrap_or_else(|| raw.to_string());
    if path.is_some() && content == raw {
        content = strip_tag(&strip_tag(raw, "path"), "type");
    }
    content = content.trim_matches('\n').to_string();
    let mut first_line = None;
    content = content
        .lines()
        .map(|line| strip_line_prefix(line, &mut first_line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_matches('\n')
        .to_string();
    let lang = path.as_deref().and_then(lang_from_path);
    UnwrappedToolOutput {
        content,
        lang,
        path,
        line_offset: first_line.map(|line| line.saturating_sub(1)),
    }
}

fn extract_tag<'a>(raw: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = raw.find(&open)? + open.len();
    let end = raw[start..].find(&close)? + start;
    Some(raw[start..end].trim())
}

fn strip_tag(raw: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = raw.find(&open) else {
        return raw.to_string();
    };
    let Some(end) = raw[start + open.len()..].find(&close) else {
        return raw.to_string();
    };
    let end = start + open.len() + end + close.len();
    format!("{}{}", &raw[..start], &raw[end..])
}

fn strip_line_prefix(line: &str, first_line: &mut Option<usize>) -> String {
    let trimmed = line.trim_start();
    let Some(colon) = trimmed.find(':') else {
        return line.to_string();
    };
    let prefix = trimmed[..colon].trim_start_matches("Line").trim();
    if prefix.is_empty() || !prefix.chars().all(|ch| ch.is_ascii_digit()) {
        return line.to_string();
    }
    if first_line.is_none() {
        *first_line = prefix.parse::<usize>().ok();
    }
    trimmed[colon + 1..].trim_start().to_string()
}

fn todos_from_state(state: &Value) -> Vec<NeoismAgentTodo> {
    let todos = state
        .get("metadata")
        .and_then(|metadata| metadata.get("todos"))
        .and_then(Value::as_array)
        .or_else(|| {
            state
                .get("input")
                .and_then(|input| input.get("todos"))
                .and_then(Value::as_array)
        });
    todos
        .map(|todos| {
            todos
                .iter()
                .map(|todo| {
                    let status = todo
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("pending")
                        .replace('-', "_");
                    let status = match status.as_str() {
                        "completed" => "completed",
                        "in_progress" | "active" => "in_progress",
                        _ => "pending",
                    };
                    NeoismAgentTodo {
                        status: status.to_string(),
                        content: todo
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("(empty)")
                            .trim()
                            .to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn todo_marker(status: &str) -> &'static str {
    match status {
        "completed" => "x",
        "in_progress" => ">",
        _ => " ",
    }
}

fn is_todo_tool(tool: &str) -> bool {
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "todowrite" | "todo_write" | "todo"
    )
}

fn is_edit_tool(tool: &str) -> bool {
    let normalized = tool
        .chars()
        .filter(|ch| *ch != '_' && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "applypatch" | "patch" | "edit" | "write" | "multiedit"
    )
}

fn is_unsettled_edit_tool(tool: &str, status: &str) -> bool {
    let normalized = tool
        .chars()
        .filter(|ch| *ch != '_' && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(normalized.as_str(), "applypatch" | "patch")
        && matches!(
            status.trim().to_ascii_lowercase().as_str(),
            "pending" | "running" | "streaming"
        )
}

fn edit_tool_detail(tool: &str, state: &Value) -> Option<String> {
    if !is_edit_tool(tool) {
        return None;
    }
    let input = state.get("input").cloned().unwrap_or(Value::Null);
    let metadata = state.get("metadata").cloned().unwrap_or(Value::Null);
    if input.is_null() && metadata.is_null() {
        return None;
    }
    serde_json::to_string(&json!({
        "neoismToolDetail": "edit",
        "tool": tool,
        "input": input,
        "metadata": metadata,
    }))
    .ok()
}

fn tool_name(tool: &str) -> String {
    if tool.is_empty() {
        return "Tool".to_string();
    }
    if is_todo_tool(tool) {
        return "TodoWrite".to_string();
    }
    tool.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

fn infer_lang_from_title(title: &str) -> String {
    let lower = title.to_ascii_lowercase();
    if lower.contains("bash")
        || lower.contains("shell")
        || lower.contains("command")
        || lower.contains("grep")
        || lower.contains("glob")
        || lower.contains("find")
    {
        return "shell".to_string();
    }
    if let Some(path) = title
        .split('(')
        .nth(1)
        .and_then(|rest| rest.split(')').next())
    {
        if let Some(lang) = lang_from_path(path) {
            return lang;
        }
    }
    String::new()
}

fn lang_from_path(path: &str) -> Option<String> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    let lang = match ext.as_str() {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "go" => "go",
        "lua" => "lua",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" => "bash",
        "md" | "markdown" => "markdown",
        _ => return None,
    };
    Some(lang.to_string())
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

fn truncate_line(value: &str, max_chars: usize) -> String {
    let value = value.replace(['\n', '\r'], " ");
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}
