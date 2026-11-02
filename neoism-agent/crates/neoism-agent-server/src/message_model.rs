use neoism_agent_core::{
    FilePart, MessageInfo, MessageWithParts, Part, ProviderAttachment, ProviderMessage,
    ProviderReasoning, ProviderRole, ProviderToolCall, ToolPart, ToolState,
};
use serde_json::Value;

const SUBTASK_COMPLETION_NOTIFICATION_KIND: &str =
    "runtime notification: background subagent completion.";
const BACKGROUND_TASK_COMPLETION_NOTIFICATION_KIND: &str =
    "runtime notification: background shell task completion.";
// Truncate tool outputs for compaction requests. The request that triggers
// compaction is already near the context limit, so tool
// outputs must be aggressively truncated or the summarize request itself
// overflows the very model that is supposed to shrink the session.
const COMPACTION_TOOL_OUTPUT_MAX_CHARS: usize = 2_000;
// Stateful execution normally applies the central 50 KiB truncator before a
// result is persisted. Keep replay bounded too so legacy/imported records or a
// malformed tool result cannot expand every later provider request without
// limit.
const NORMAL_TOOL_OUTPUT_MAX_CHARS: usize = 51_200;
// Tool-driven image reads are persisted as base64 data URLs. Keep enough room
// for one large screenshot, but never replay an unbounded collection of old
// images into every later provider request.
const NORMAL_TOOL_MEDIA_MAX_BYTES: usize = 20 * 1024 * 1024;
const TOOL_MEDIA_MESSAGE_PREFIX: &str = "[Tool ";
const TOOL_MEDIA_MESSAGE_SUFFIX: &str = " returned media attachments]";

pub(crate) fn is_runtime_system_notification(system: &str) -> bool {
    system.contains(SUBTASK_COMPLETION_NOTIFICATION_KIND)
        || system.contains(BACKGROUND_TASK_COMPLETION_NOTIFICATION_KIND)
}

pub(crate) fn provider_messages(messages: &[MessageWithParts]) -> Vec<ProviderMessage> {
    provider_messages_with_options(
        messages,
        MessageModelOptions {
            include_attachments: true,
            tool_output_max_chars: Some(NORMAL_TOOL_OUTPUT_MAX_CHARS),
            current_model: None,
        },
        &[],
    )
}

pub(crate) fn provider_messages_for_model(
    messages: &[MessageWithParts],
    provider_id: &str,
    model_id: &str,
) -> Vec<ProviderMessage> {
    provider_messages_with_options(
        messages,
        MessageModelOptions {
            include_attachments: true,
            tool_output_max_chars: Some(NORMAL_TOOL_OUTPUT_MAX_CHARS),
            current_model: Some((provider_id.to_string(), model_id.to_string())),
        },
        &[],
    )
}

pub(crate) fn compaction_provider_messages(
    messages: &[MessageWithParts],
) -> Vec<ProviderMessage> {
    provider_messages_with_options(
        messages,
        MessageModelOptions {
            include_attachments: false,
            tool_output_max_chars: Some(COMPACTION_TOOL_OUTPUT_MAX_CHARS),
            current_model: None,
        },
        &[],
    )
}

pub(crate) fn compaction_provider_messages_for_model(
    messages: &[MessageWithParts],
    provider_id: &str,
    model_id: &str,
) -> Vec<ProviderMessage> {
    provider_messages_with_options(
        messages,
        MessageModelOptions {
            include_attachments: false,
            tool_output_max_chars: Some(COMPACTION_TOOL_OUTPUT_MAX_CHARS),
            current_model: Some((provider_id.to_string(), model_id.to_string())),
        },
        &[],
    )
}

/// Ambient updates are anchored AFTER a persisted message, never inside a tool
/// exchange. Missing/removed anchors and initial (`None`) updates precede the
/// visible history, in their stored order. Initial prompt instructions remain
/// the caller's responsibility and are not repeated here.
pub(crate) fn provider_messages_with_updates(
    messages: &[MessageWithParts],
    current_model: Option<(&str, &str)>,
    updates: &[(Option<String>, String)],
) -> Vec<ProviderMessage> {
    provider_messages_with_options(
        messages,
        MessageModelOptions {
            include_attachments: true,
            tool_output_max_chars: Some(NORMAL_TOOL_OUTPUT_MAX_CHARS),
            current_model: current_model
                .map(|(provider, model)| (provider.into(), model.into())),
        },
        updates,
    )
}

fn history_message_id(message: &MessageWithParts) -> &str {
    match &message.info {
        MessageInfo::User(info) => info.id.as_str(),
        MessageInfo::Assistant(info) => info.id.as_str(),
    }
}

fn instruction_reminder(content: &str) -> ProviderMessage {
    ProviderMessage::text(
        ProviderRole::User,
        format!("<system-reminder>\n{content}\n</system-reminder>"),
    )
}

struct MessageModelOptions {
    include_attachments: bool,
    /// Normal provider turns replay the centrally-bounded result verbatim.
    /// Only the already-near-limit compaction request applies a second bound.
    tool_output_max_chars: Option<usize>,
    /// Opaque provider reasoning can only be replayed to the exact model that
    /// produced it. A model switch gets the durable visible reasoning text.
    current_model: Option<(String, String)>,
}

fn provider_messages_with_options(
    messages: &[MessageWithParts],
    options: MessageModelOptions,
    updates: &[(Option<String>, String)],
) -> Vec<ProviderMessage> {
    let mut seen_instructions = std::collections::HashSet::new();
    let mut provider_messages = messages
        .iter()
        .map(|message| match &message.info {
            MessageInfo::User(user) => {
                let mut values = Vec::new();
                if let Some(system) = user
                    .system
                    .as_ref()
                    .filter(|system| !system.trim().is_empty())
                {
                    if is_runtime_system_notification(system) {
                        let text = visible_part_text(&message.parts);
                        let content = if text.trim().is_empty() {
                            system.clone()
                        } else {
                            format!("{system}\n\n{text}")
                        };
                        // Keep runtime notifications in their chronological
                        // position. Provider adapters fold System messages
                        // into the global instruction block, which both
                        // separates a completion from the turn it should wake
                        // and buries it among every older notification. The
                        // persisted `UserMessage.system` marker still keeps
                        // this out of user bubbles in the UI; provider-side it
                        // is a trusted runtime-state turn, immediately before
                        // the reply it triggers.
                        values.push(ProviderMessage::text(ProviderRole::User, content));
                        return values;
                    }
                }
                values.push(user_provider_message(
                    &message.parts,
                    options.include_attachments,
                ));
                values
            }
            MessageInfo::Assistant(assistant) => {
                let same_model =
                    options
                        .current_model
                        .as_ref()
                        .is_none_or(|(provider, model)| {
                            assistant.provider_id == *provider
                                && assistant.model_id == *model
                        });
                let mut replay = assistant_provider_messages(
                    &message.parts,
                    &options,
                    same_model,
                    &mut seen_instructions,
                );
                if same_model {
                    if let Some(first) = replay
                        .first_mut()
                        .filter(|message| matches!(message.role, ProviderRole::Assistant))
                    {
                        first.provider_replay = assistant.provider_replay.clone();
                    } else if assistant.provider_replay.is_some() {
                        let mut first =
                            ProviderMessage::text(ProviderRole::Assistant, "");
                        first.provider_replay = assistant.provider_replay.clone();
                        replay.insert(0, first);
                    }
                }
                replay
            }
        })
        .zip(messages)
        .flat_map(|(mut replay, message)| {
            replay.extend(updates.iter().filter_map(|(anchor, content)| {
                (anchor.as_deref() == Some(history_message_id(message)))
                    .then(|| instruction_reminder(content))
            }));
            replay
        })
        .filter(|message| {
            !message.content.trim().is_empty()
                || !message.tool_calls.is_empty()
                || !message.reasoning.is_empty()
                || message.provider_replay.is_some()
                || matches!(message.role, ProviderRole::Tool)
        })
        .collect::<Vec<_>>();
    let visible_ids = messages
        .iter()
        .map(history_message_id)
        .collect::<std::collections::HashSet<_>>();
    let mut prelude = updates
        .iter()
        .filter_map(|(anchor, content)| {
            anchor
                .as_deref()
                .is_none_or(|id| !visible_ids.contains(id))
                .then(|| instruction_reminder(content))
        })
        .collect::<Vec<_>>();
    prelude.append(&mut provider_messages);
    provider_messages = prelude;
    if options.include_attachments {
        bound_tool_media_replay(&mut provider_messages, NORMAL_TOOL_MEDIA_MAX_BYTES);
    }
    provider_messages
}

fn tool_instructions(tool: &ToolPart) -> Vec<crate::instruction::LoadedInstruction> {
    if !matches!(tool.tool.as_str(), "read" | "compose")
        // Failed compositions still carry successful read-leaf effects. A
        // failed read itself never admits instructions.
        || !(matches!(tool.state, ToolState::Completed { .. })
            || (tool.tool == "compose" && matches!(tool.state, ToolState::Error { .. })))
        || tool_output_was_compacted(tool)
    {
        return Vec::new();
    }
    tool_state_metadata(tool)
        .and_then(|metadata| metadata.get("instructionContext"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| serde_json::from_value(item.clone()).ok())
        .collect()
}

/// Admit successful read effects immediately before persisting a read/compose
/// result, including successful leaves retained by a failed composition.
/// `messages` must be the currently model-visible history, after compaction and
/// revert filtering, not the complete stored transcript. No session-global set:
/// removing the original entry must make a later discovery eligible again.
/// Replay also deduplicates legacy records, but admission is essential: saving a
/// duplicate would let its newer contents reappear when the first is pruned.
pub(crate) fn admit_instruction_context(
    messages: &[MessageWithParts],
    tool: &str,
    metadata: &mut Value,
) {
    if metadata.get("instructionContext").is_none() {
        return;
    }
    if !matches!(tool, "read" | "compose") {
        if let Some(object) = metadata.as_object_mut() {
            object.remove("instructionContext");
        }
        return;
    }
    let mut seen = messages
        .iter()
        .filter(|message| matches!(message.info, MessageInfo::Assistant(_)))
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            Part::Tool(tool) => Some(tool),
            _ => None,
        })
        .flat_map(tool_instructions)
        .map(|instruction| instruction.filepath)
        .collect::<std::collections::HashSet<_>>();
    let Some(object) = metadata.as_object_mut() else {
        return;
    };
    let Some(items) = object.remove("instructionContext") else {
        return;
    };
    let admitted = items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            serde_json::from_value::<crate::instruction::LoadedInstruction>(item.clone())
                .ok()
        })
        .filter(|instruction| seen.insert(instruction.filepath.clone()))
        .collect::<Vec<_>>();
    object.insert(
        "loaded".into(),
        serde_json::json!(admitted.iter().map(|item| &item.filepath).collect::<Vec<_>>()),
    );
    if !admitted.is_empty() {
        object.insert("instructionContext".into(), serde_json::json!(admitted));
    }
}

fn bound_tool_media_replay(messages: &mut [ProviderMessage], max_bytes: usize) {
    let mut remaining = max_bytes;
    let mut kept_data_attachment = false;
    for message in messages.iter_mut().rev() {
        if !matches!(message.role, ProviderRole::User)
            || !message.content.starts_with(TOOL_MEDIA_MESSAGE_PREFIX)
            || !message.content.ends_with(TOOL_MEDIA_MESSAGE_SUFFIX)
        {
            continue;
        }
        message.attachments.retain(|attachment| {
            let bytes = attachment
                .url
                .starts_with("data:")
                .then_some(attachment.url.len())
                .unwrap_or(0);
            if bytes > remaining && kept_data_attachment {
                return false;
            }
            kept_data_attachment |= bytes > 0;
            remaining = remaining.saturating_sub(bytes);
            true
        });
        if message.attachments.is_empty() {
            message.content = message
                .content
                .strip_suffix(']')
                .map(|content| format!("{content} omitted from replay budget]"))
                .unwrap_or_else(|| "[Tool media omitted from replay budget]".to_string());
        }
    }
}

fn user_provider_message(parts: &[Part], include_attachments: bool) -> ProviderMessage {
    let mut message = ProviderMessage::text(ProviderRole::User, visible_part_text(parts));
    if include_attachments {
        message.attachments = parts
            .iter()
            .filter_map(|part| match part {
                Part::File(part) => Some(ProviderAttachment {
                    mime: part.mime.clone(),
                    url: part.url.clone(),
                    filename: part.filename.clone(),
                }),
                _ => None,
            })
            .collect();
    }
    message
}

fn assistant_provider_messages(
    parts: &[Part],
    options: &MessageModelOptions,
    replay_opaque_reasoning: bool,
    seen_instructions: &mut std::collections::HashSet<String>,
) -> Vec<ProviderMessage> {
    let mut content = visible_part_text(parts);
    if !replay_opaque_reasoning {
        let visible_reasoning = parts
            .iter()
            .filter_map(|part| match part {
                Part::Reasoning(part) if !part.text.trim().is_empty() => {
                    Some(part.text.trim())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !visible_reasoning.is_empty() {
            if !content.is_empty() {
                content.push_str("\n\n");
            }
            content.push_str("Previous model reasoning summary:\n");
            content.push_str(&visible_reasoning);
        }
    }
    let tool_parts = parts
        .iter()
        .filter_map(|part| match part {
            Part::Tool(part) => Some(part),
            _ => None,
        })
        .collect::<Vec<_>>();
    let tool_calls = tool_parts
        .iter()
        .map(|part| ProviderToolCall {
            id: part.call_id.clone(),
            name: part.tool.clone(),
            input: tool_input(part),
        })
        .collect::<Vec<_>>();

    let reasoning = replay_opaque_reasoning
        .then(|| parts)
        .into_iter()
        .flatten()
        .filter_map(|part| match part {
            Part::Reasoning(part) => part.metadata.as_ref()?.get("openai"),
            _ => None,
        })
        .filter_map(|metadata| {
            Some(ProviderReasoning {
                summary: metadata.get("summary")?.as_array()?.clone(),
                encrypted_content: metadata
                    .get("encryptedContent")?
                    .as_str()?
                    .to_string(),
            })
        })
        .collect::<Vec<_>>();

    let mut messages = Vec::new();
    if !content.trim().is_empty() || !tool_calls.is_empty() || !reasoning.is_empty() {
        let mut message = ProviderMessage::assistant_tool_call(content, tool_calls);
        message.reasoning = reasoning;
        messages.push(message);
    }
    // Keep a batch's results together; media must not split the tool exchange.
    let mut media = Vec::new();
    for part in tool_parts {
        for message in tool_result_messages(part, options) {
            if matches!(message.role, ProviderRole::Tool) {
                messages.push(message);
            } else {
                media.push(message);
            }
        }
    }
    messages.extend(media);
    // Like runtime notifications, trusted instruction reminders must use a
    // chronological User turn: System is hoisted by Anthropic/Responses. Keep
    // all results together before reminders so parallel tool exchanges remain
    // valid. The discovery order within/between reads is never path-sorted.
    for part in parts {
        let Part::Tool(tool) = part else { continue };
        for instruction in tool_instructions(tool) {
            if seen_instructions.insert(instruction.filepath) {
                messages.push(instruction_reminder(&instruction.content));
            }
        }
    }
    messages
}

fn visible_part_text(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            Part::Text(part) => Some(part.text.clone()),
            Part::Agent(part) => Some(format!("[agent: {}]", part.name)),
            Part::Subtask(_) => {
                Some("The following tool was executed by the user".to_string())
            }
            Part::File(part) => Some(file_part_placeholder(part)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A compact text reference for a file part.
///
/// The actual file bytes reach the model through the structured
/// [`ProviderAttachment`] path (image/file blocks), so the text content only
/// needs a short human-readable marker. Critically, a base64 `data:` URL is
/// NEVER inlined here: a pasted screenshot is hundreds of KB of base64 that the
/// model would tokenize as text — sent a second time on top of the real image
/// block, and re-sent on every subsequent turn (and never stripped by
/// compaction). That is the entire source of the "images balloon the context"
/// bug. Real (non-`data:`) URLs stay inline since they are small and useful.
fn file_part_placeholder(part: &FilePart) -> String {
    let label = part
        .filename
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(part.mime.as_str());
    let kind = if part.mime.starts_with("image/") {
        "image"
    } else {
        "file"
    };
    if part.url.starts_with("data:") {
        format!("[{kind}: {label}]")
    } else {
        format!("[{kind}: {label}] {}", part.url)
    }
}

fn tool_input(part: &ToolPart) -> Value {
    match &part.state {
        ToolState::Pending { input, .. }
        | ToolState::Running { input, .. }
        | ToolState::Completed { input, .. }
        | ToolState::Error { input, .. } => input.clone(),
    }
}

fn tool_result_messages(
    part: &ToolPart,
    options: &MessageModelOptions,
) -> Vec<ProviderMessage> {
    let result = match &part.state {
        ToolState::Completed { output, .. } => ProviderMessage::tool_result(
            &part.call_id,
            &part.tool,
            &tool_output_for_prompt(part, output, options.tool_output_max_chars, false),
            false,
        ),
        ToolState::Error { error, .. } => ProviderMessage::tool_result(
            &part.call_id,
            &part.tool,
            &tool_output_for_prompt(part, error, options.tool_output_max_chars, true),
            true,
        ),
        ToolState::Pending { .. } | ToolState::Running { .. } => {
            ProviderMessage::tool_result(
                &part.call_id,
                &part.tool,
                "Tool execution was interrupted",
                true,
            )
        }
    };
    let attachments = if options.include_attachments {
        tool_attachments(part)
    } else {
        Vec::new()
    };
    if attachments.is_empty() {
        return vec![result];
    }
    let mut media = ProviderMessage::text(
        ProviderRole::User,
        format!("[Tool {} returned media attachments]", part.tool),
    );
    media.attachments = attachments;
    vec![result, media]
}

fn tool_output_for_prompt(
    part: &ToolPart,
    output: &str,
    max_chars: Option<usize>,
    error: bool,
) -> String {
    if tool_output_was_compacted(part) {
        return "[Old tool result content cleared]".to_string();
    }
    let Some(max_chars) = max_chars else {
        return output.to_string();
    };
    // The central truncator's persisted preview is already within the normal
    // replay budget and carries useful head/tail context. Artifact-only
    // substitution remains reserved for the tighter compaction request.
    if max_chars == NORMAL_TOOL_OUTPUT_MAX_CHARS
        && output.chars().count() <= NORMAL_TOOL_OUTPUT_MAX_CHARS
    {
        let artifact_uri = tool_output_artifact(part)
            .and_then(|artifact| artifact.get("uri"))
            .and_then(Value::as_str);
        if let Some(uri) = artifact_uri.filter(|uri| !output.contains(uri)) {
            return format!(
                "{output}\n\nArtifact: {uri}\nUse artifact_read or artifact_search for the full output."
            );
        }
        return output.to_string();
    }
    if let Some(reference) = tool_output_reference(part, error) {
        return reference;
    }
    truncate_tool_output(output, max_chars)
}

fn tool_output_was_compacted(part: &ToolPart) -> bool {
    tool_state_metadata(part)
        .and_then(|metadata| metadata.get("compacted"))
        .is_some_and(|value| !value.is_null())
}

fn tool_output_reference(part: &ToolPart, error: bool) -> Option<String> {
    if !tool_output_was_truncated(part) {
        return None;
    }
    let path = tool_output_path(part)?;
    let artifact = tool_output_artifact(part);
    let artifact_uri = artifact
        .as_ref()
        .and_then(|artifact| artifact.get("uri"))
        .and_then(Value::as_str);
    let artifact_summary = artifact
        .as_ref()
        .and_then(|artifact| artifact.get("summary"))
        .and_then(Value::as_str);
    let kind = if error { "error" } else { "output" };
    let lines = vec![
        format!(
            "[Tool {} {kind} was too large for prompt replay.]",
            part.tool
        ),
        artifact_uri
            .map(|uri| format!("Artifact: {uri}"))
            .unwrap_or_else(|| format!("Full output saved to: {path}")),
        artifact_summary
            .filter(|summary| !summary.trim().is_empty())
            .map(|summary| format!("Summary: {summary}"))
            .unwrap_or_else(|| "Use artifact_read/artifact_search or Read/Grep to inspect only the needed section.".to_string()),
    ];
    Some(lines.join("\n"))
}

fn tool_output_artifact(part: &ToolPart) -> Option<&Value> {
    tool_state_metadata(part).and_then(|metadata| metadata.get("artifact"))
}

fn tool_output_was_truncated(part: &ToolPart) -> bool {
    matches!(
        tool_state_metadata(part).and_then(|metadata| metadata.get("truncated")),
        Some(Value::Bool(true))
    )
}

fn tool_output_path(part: &ToolPart) -> Option<String> {
    tool_state_metadata(part)
        .and_then(|metadata| metadata.get("outputPath"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn tool_state_metadata(part: &ToolPart) -> Option<&Value> {
    match &part.state {
        ToolState::Completed { metadata, .. } => Some(metadata),
        ToolState::Pending { .. } | ToolState::Running { .. } => None,
        ToolState::Error { .. } => {
            part.metadata.as_ref().and_then(|m| m.get("toolResult"))
        }
    }
}

fn truncate_tool_output(output: &str, max_chars: usize) -> String {
    let char_count = output.chars().count();
    if char_count <= max_chars {
        return output.to_string();
    }
    if max_chars == 0 {
        return format!(
            "[Tool output truncated for prompt replay: omitted {char_count} chars. Use a narrower tool call or result lookup tool if more detail is needed.]"
        );
    }
    let head_chars = max_chars / 2;
    let tail_chars = max_chars.saturating_sub(head_chars);
    let head = output.chars().take(head_chars).collect::<String>();
    let tail = output
        .chars()
        .skip(char_count.saturating_sub(tail_chars))
        .collect::<String>();
    let omitted = char_count.saturating_sub(head_chars + tail_chars);
    format!(
        "{head}\n\n[Tool output truncated for prompt replay: omitted {omitted} chars. Use a narrower tool call or result lookup tool if more detail is needed.]\n\n{tail}"
    )
}

fn tool_attachments(part: &ToolPart) -> Vec<ProviderAttachment> {
    let Some(metadata) = tool_state_metadata(part) else {
        return Vec::new();
    };
    if tool_output_was_compacted(part) {
        return Vec::new();
    }
    metadata
        .get("attachments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|attachment| {
            let mime = attachment.get("mime").and_then(Value::as_str)?;
            let url = attachment.get("url").and_then(Value::as_str)?;
            Some(ProviderAttachment {
                mime: mime.to_string(),
                url: url.to_string(),
                filename: attachment
                    .get("filename")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{
        AssistantMessage, AssistantPath, CompletedTime, CreatedTime, FilePart, Id,
        IdKind, MessageWithParts, PartTime, ReasoningPart, TextPart, TokenUsage,
        ToolPart, UserMessage, UserModel,
    };

    #[test]
    fn provider_messages_include_tool_results() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let part_id = Id::ascending(IdKind::Part);
        let messages = provider_messages(&[MessageWithParts {
            info: MessageInfo::Assistant(AssistantMessage {
                provider_replay: None,
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CompletedTime {
                    created: 1,
                    streamed: Some(2),
                    completed: Some(2),
                },
                parent_id: Id::ascending(IdKind::Message),
                mode: "subagent".to_string(),
                agent: "build".to_string(),
                path: AssistantPath {
                    cwd: "/tmp".to_string(),
                    root: "/tmp".to_string(),
                },
                cost: 0.0,
                tokens: TokenUsage::default(),
                model_id: "stub".to_string(),
                provider_id: "neoism".to_string(),
                finish: Some("tool-calls".to_string()),
                error: None,
            }),
            parts: vec![Part::Tool(ToolPart {
                id: part_id,
                session_id,
                message_id,
                tool: "read".to_string(),
                call_id: "call_1".to_string(),
                state: ToolState::Completed {
                    input: serde_json::json!({ "path": "src/lib.rs" }),
                    output: "file contents".to_string(),
                    metadata: serde_json::json!({}),
                    title: "Read src/lib.rs".to_string(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
                metadata: None,
            })],
        }]);

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, ProviderRole::Assistant));
        assert_eq!(messages[0].tool_calls[0].name, "read");
        assert_eq!(messages[0].tool_calls[0].input["path"], "src/lib.rs");
        assert!(matches!(messages[1].role, ProviderRole::Tool));
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(messages[1].content, "file contents");
    }

    #[test]
    fn provider_messages_preserve_encrypted_reasoning() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let messages = provider_messages(&[MessageWithParts {
            info: assistant_info(message_id.clone(), session_id.clone()),
            parts: vec![Part::Reasoning(ReasoningPart {
                id: Id::ascending(IdKind::Part),
                session_id,
                message_id,
                text: "Inspected files".to_string(),
                time: PartTime {
                    start: 1,
                    end: Some(2),
                },
                metadata: Some(serde_json::json!({
                    "openai": {
                        "summary": [{ "type": "summary_text", "text": "Inspected files" }],
                        "encryptedContent": "ciphertext"
                    }
                })),
            })],
        }]);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].reasoning.len(), 1);
        assert_eq!(messages[0].reasoning[0].encrypted_content, "ciphertext");
        assert_eq!(
            messages[0].reasoning[0].summary[0]["text"],
            "Inspected files"
        );
    }

    #[test]
    fn model_switch_drops_opaque_reasoning_and_keeps_visible_summary() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let messages = provider_messages_for_model(
            &[MessageWithParts {
                info: assistant_info(message_id.clone(), session_id.clone()),
                parts: vec![Part::Reasoning(ReasoningPart {
                    id: Id::ascending(IdKind::Part),
                    session_id,
                    message_id,
                    text: "Inspected the runtime and found the lock.".to_string(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                    metadata: Some(serde_json::json!({
                        "openai": {
                            "summary": [{ "type": "summary_text", "text": "opaque" }],
                            "encryptedContent": "ciphertext"
                        }
                    })),
                })],
            }],
            "anthropic",
            "claude-next",
        );

        assert_eq!(messages.len(), 1);
        assert!(messages[0].reasoning.is_empty());
        assert!(messages[0]
            .content
            .contains("Inspected the runtime and found the lock."));
        assert!(!messages[0].content.contains("ciphertext"));
    }

    #[test]
    fn provider_messages_preserve_centrally_bounded_spilled_tool_output() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let part_id = Id::ascending(IdKind::Part);
        let bounded_output =
            "bounded preview\n\nFull output saved to: /tmp/neoism-tool-output.txt";
        let messages = provider_messages(&[MessageWithParts {
            info: assistant_info(message_id.clone(), session_id.clone()),
            parts: vec![Part::Tool(ToolPart {
                id: part_id,
                session_id,
                message_id,
                tool: "bash".to_string(),
                call_id: "call_large".to_string(),
                state: ToolState::Completed {
                    input: serde_json::json!({ "command": "big-output" }),
                    output: bounded_output.to_string(),
                    metadata: serde_json::json!({
                        "truncated": true,
                        "outputPath": "/tmp/neoism-tool-output.txt",
                        "artifact": {
                            "id": "abc123",
                            "uri": "artifact://tool-output/abc123",
                            "title": "Run big-output",
                            "tool": "bash",
                            "path": "/tmp/neoism-tool-output.txt",
                            "byteCount": 80000,
                            "summary": "big output summary"
                        }
                    }),
                    title: "Run big-output".to_string(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
                metadata: None,
            })],
        }]);

        assert_eq!(messages.len(), 2);
        assert!(messages[1].content.starts_with(bounded_output));
        assert!(messages[1]
            .content
            .contains("Artifact: artifact://tool-output/abc123"));
    }

    #[test]
    fn provider_messages_preserve_old_centrally_bounded_tool_results() {
        let session_id = Id::ascending(IdKind::Session);
        let mut transcript = Vec::new();
        for index in 0..6 {
            let message_id = Id::ascending(IdKind::Message);
            transcript.push(MessageWithParts {
                info: assistant_info(message_id.clone(), session_id.clone()),
                parts: vec![Part::Tool(ToolPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session_id.clone(),
                    message_id,
                    tool: "bash".to_string(),
                    call_id: format!("call_{index}"),
                    state: ToolState::Completed {
                        input: serde_json::json!({ "command": index }),
                        output: "o".repeat(4_000),
                        metadata: serde_json::json!({}),
                        title: "Run".to_string(),
                        time: PartTime {
                            start: 1,
                            end: Some(2),
                        },
                    },
                    metadata: None,
                })],
            });
        }

        let messages = provider_messages(&transcript);
        let old_tool = messages
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("call_0"))
            .expect("old tool result");

        assert_eq!(old_tool.content, "o".repeat(4_000));
    }

    #[test]
    fn provider_messages_defensively_bound_legacy_oversized_tool_results() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let messages = provider_messages(&[MessageWithParts {
            info: assistant_info(message_id.clone(), session_id.clone()),
            parts: vec![Part::Tool(ToolPart {
                id: Id::ascending(IdKind::Part),
                session_id,
                message_id,
                tool: "legacy".to_string(),
                call_id: "call_legacy_large".to_string(),
                state: ToolState::Completed {
                    input: serde_json::json!({}),
                    output: "x".repeat(NORMAL_TOOL_OUTPUT_MAX_CHARS * 2),
                    metadata: serde_json::json!({}),
                    title: "Legacy output".to_string(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
                metadata: None,
            })],
        }]);
        let result = messages
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("call_legacy_large"))
            .expect("tool result");

        assert!(result.content.len() < NORMAL_TOOL_OUTPUT_MAX_CHARS + 512);
        assert!(result
            .content
            .contains("Tool output truncated for prompt replay"));
    }

    #[test]
    fn provider_messages_clear_compacted_tool_results_and_media() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let messages = provider_messages(&[MessageWithParts {
            info: assistant_info(message_id.clone(), session_id.clone()),
            parts: vec![Part::Tool(ToolPart {
                id: Id::ascending(IdKind::Part),
                session_id,
                message_id,
                tool: "read".to_string(),
                call_id: "call_old".to_string(),
                state: ToolState::Completed {
                    input: serde_json::json!({ "path": "old.txt" }),
                    output: "old contents".repeat(1_000),
                    metadata: serde_json::json!({
                        "compacted": 42,
                        "attachments": [{
                            "mime": "image/png",
                            "url": "data:image/png;base64,abc"
                        }]
                    }),
                    title: "Read old.txt".to_string(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
                metadata: None,
            })],
        }]);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, "[Old tool result content cleared]");
        assert!(messages[1].attachments.is_empty());
    }

    #[test]
    fn provider_messages_surface_tool_media_attachments_as_user_message() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let part_id = Id::ascending(IdKind::Part);
        let messages = provider_messages(&[MessageWithParts {
            info: MessageInfo::Assistant(AssistantMessage {
                provider_replay: None,
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CompletedTime {
                    created: 1,
                    streamed: Some(2),
                    completed: Some(2),
                },
                parent_id: Id::ascending(IdKind::Message),
                mode: "subagent".to_string(),
                agent: "build".to_string(),
                path: AssistantPath {
                    cwd: "/tmp".to_string(),
                    root: "/tmp".to_string(),
                },
                cost: 0.0,
                tokens: TokenUsage::default(),
                model_id: "stub".to_string(),
                provider_id: "neoism".to_string(),
                finish: Some("tool-calls".to_string()),
                error: None,
            }),
            parts: vec![Part::Tool(ToolPart {
                id: part_id,
                session_id,
                message_id,
                tool: "read".to_string(),
                call_id: "call_media".to_string(),
                state: ToolState::Completed {
                    input: serde_json::json!({ "path": "shot.png" }),
                    output: "Image read successfully".to_string(),
                    metadata: serde_json::json!({
                        "attachments": [{
                            "mime": "image/png",
                            "url": "data:image/png;base64,abc",
                            "filename": "shot.png"
                        }]
                    }),
                    title: "Read shot.png".to_string(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
                metadata: None,
            })],
        }]);

        assert_eq!(messages.len(), 3);
        assert!(matches!(messages[1].role, ProviderRole::Tool));
        assert!(messages[1].attachments.is_empty());
        assert!(matches!(messages[2].role, ProviderRole::User));
        assert_eq!(
            messages[2].attachments[0].filename.as_deref(),
            Some("shot.png")
        );
    }

    #[test]
    fn parallel_tool_results_precede_media_in_replay_and_compaction() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let history =
            vec![MessageWithParts {
            info: assistant_info(message_id.clone(), session_id.clone()),
            parts: (0..3).map(|index| Part::Tool(ToolPart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id: message_id.clone(),
                tool: "read".to_string(),
                call_id: format!("call_{index}"),
                state: match index {
                    0 => ToolState::Completed {
                        input: serde_json::json!({ "path": "shot.png" }),
                        output: "Image read successfully".to_string(),
                        metadata: serde_json::json!({
                            "attachments": [{
                                "mime": "image/png",
                                "url": "data:image/png;base64,abc",
                                "filename": "shot.png"
                            }]
                        }),
                        title: "Read shot.png".to_string(),
                        time: PartTime { start: 1, end: Some(2) },
                    },
                    1 => ToolState::Error {
                        input: serde_json::json!({ "path": "missing.png" }),
                        error: "File not found".to_string(),
                        time: PartTime { start: 1, end: Some(2) },
                    },
                    _ => ToolState::Running {
                        input: serde_json::json!({ "path": "interrupted.png" }),
                        time: PartTime { start: 1, end: None },
                    },
                },
                metadata: None,
            })).collect(),
        }];
        for messages in [
            provider_messages(&history),
            provider_messages_for_model(&history, "anthropic", "claude-sonnet-4"),
            compaction_provider_messages(&history),
        ] {
            assert_eq!(messages[0].tool_calls.len(), 3);
            for index in 0..3 {
                let result = &messages[index + 1];
                assert!(matches!(result.role, ProviderRole::Tool));
                assert_eq!(
                    result.tool_call_id.as_deref(),
                    Some(messages[0].tool_calls[index].id.as_str())
                );
                assert_eq!(result.tool_error.unwrap_or(false), index != 0);
            }
            assert_eq!(messages[3].content, "Tool execution was interrupted");
        }
        let messages = provider_messages(&history);
        assert_eq!(messages.len(), 5);
        assert_eq!(
            messages[4].attachments[0].filename.as_deref(),
            Some("shot.png")
        );
        assert_eq!(compaction_provider_messages(&history).len(), 4);
    }

    #[test]
    fn tool_media_replay_keeps_newest_attachments_within_aggregate_budget() {
        let mut old = ProviderMessage::text(
            ProviderRole::User,
            "[Tool read returned media attachments]",
        );
        old.attachments.push(ProviderAttachment {
            mime: "image/png".to_string(),
            url: "data:old".to_string(),
            filename: Some("old.png".to_string()),
        });
        let mut recent = ProviderMessage::text(
            ProviderRole::User,
            "[Tool read returned media attachments]",
        );
        recent.attachments.push(ProviderAttachment {
            mime: "image/png".to_string(),
            url: "data:new".to_string(),
            filename: Some("new.png".to_string()),
        });
        let mut messages = vec![old, recent];

        bound_tool_media_replay(&mut messages, 8);

        assert!(messages[0].attachments.is_empty());
        assert!(messages[0].content.contains("omitted from replay budget"));
        assert_eq!(messages[1].attachments.len(), 1);
        assert_eq!(
            messages[1].attachments[0].filename.as_deref(),
            Some("new.png")
        );
    }

    fn assistant_info(message_id: Id, session_id: Id) -> MessageInfo {
        MessageInfo::Assistant(AssistantMessage {
            provider_replay: None,
            id: message_id,
            session_id,
            time: CompletedTime {
                created: 1,
                streamed: Some(2),
                completed: Some(2),
            },
            parent_id: Id::ascending(IdKind::Message),
            mode: "subagent".to_string(),
            agent: "build".to_string(),
            path: AssistantPath {
                cwd: "/tmp".to_string(),
                root: "/tmp".to_string(),
            },
            cost: 0.0,
            tokens: TokenUsage::default(),
            model_id: "stub".to_string(),
            provider_id: "neoism".to_string(),
            finish: Some("tool-calls".to_string()),
            error: None,
        })
    }

    #[test]
    fn canonical_replay_is_durable_model_scoped_and_keeps_tool_result_bounds() {
        let session = Id::ascending(IdKind::Session);
        let id = Id::ascending(IdKind::Message);
        let mut info = assistant_info(id.clone(), session.clone());
        let MessageInfo::Assistant(assistant) = &mut info else {
            unreachable!()
        };
        assistant.provider_id = "openai".into();
        assistant.model_id = "gpt-6.1-sol".into();
        assistant.provider_replay = Some(neoism_agent_core::ProviderReplay {
            format: "openai-responses".into(),
            items: vec![neoism_agent_core::ProviderReplayItem {
                index: 0,
                item: serde_json::json!({"id":"msg","type":"message","role":"assistant","phase":"commentary",
                    "content":[{"type":"output_text","text":"Checking","annotations":[]}]}),
            }],
        });
        let history = vec![MessageWithParts {
            info,
            parts: vec![Part::Tool(ToolPart {
                id: Id::ascending(IdKind::Part),
                session_id: session,
                message_id: id,
                tool: "read".into(),
                call_id: "call".into(),
                metadata: None,
                state: ToolState::Completed {
                    input: serde_json::json!({"filePath":"x"}),
                    output: "x".repeat(100_000),
                    metadata: serde_json::json!({}),
                    title: "read".into(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
            })],
        }];
        let history: Vec<MessageWithParts> =
            serde_json::from_value(serde_json::to_value(history).unwrap()).unwrap();
        let replay = provider_messages_for_model(&history, "openai", "gpt-6.1-sol");
        assert_eq!(
            replay[0].provider_replay.as_ref().unwrap().items[0].item["phase"],
            "commentary"
        );
        assert!(replay[1].content.len() < 52_000);
        for (provider, model) in
            [("openai", "gpt-6-astra"), ("other-provider", "gpt-6.1-sol")]
        {
            assert!(provider_messages_for_model(&history, provider, model)
                .iter()
                .all(|m| m.provider_replay.is_none()));
            assert!(
                compaction_provider_messages_for_model(&history, provider, model)
                    .iter()
                    .all(|m| m.provider_replay.is_none())
            );
        }
        let compacted =
            compaction_provider_messages_for_model(&history, "openai", "gpt-6.1-sol");
        assert!(compacted[0].provider_replay.is_some());
        assert!(compacted[1].content.len() < 3_000);
        assert!(compacted[1].content.contains("Tool output truncated"));
        let mut legacy = serde_json::to_value(&history).unwrap();
        legacy[0]["info"]
            .as_object_mut()
            .unwrap()
            .remove("providerReplay");
        let legacy: Vec<MessageWithParts> = serde_json::from_value(legacy).unwrap();
        assert!(provider_messages(&legacy)[0].provider_replay.is_none());
    }

    #[test]
    fn nearby_instructions_keep_first_discovery_in_history_and_readmit_after_pruning() {
        let session = Id::ascending(IdKind::Session);
        let message = Id::ascending(IdKind::Message);
        let part = |name: &str, call: &str, content: &str| {
            Part::Tool(ToolPart {
                id: Id::ascending(IdKind::Part),
                session_id: session.clone(),
                message_id: message.clone(),
                tool: name.into(),
                call_id: call.into(),
                metadata: None,
                state: ToolState::Completed {
                    input: serde_json::json!({"filePath":"src/lib.rs"}),
                    output: "1: source code".into(),
                    metadata: serde_json::json!({"instructionContext":[{
                        "filepath":"/project/src/AGENTS.md", "content":content,
                    }]}),
                    title: "Read source".into(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
            })
        };
        let mut history = vec![MessageWithParts {
            info: assistant_info(message.clone(), session.clone()),
            parts: vec![
                part("read", "first", "old directory rule"),
                part("read", "second", "changed directory rule"),
            ],
        }];
        let replay = provider_messages(&history);
        assert!(matches!(replay[0].role, ProviderRole::Assistant));
        assert!(matches!(replay[1].role, ProviderRole::Tool));
        assert!(matches!(replay[2].role, ProviderRole::Tool));
        assert!(matches!(replay[3].role, ProviderRole::User));
        assert_eq!(
            replay[3].content,
            "<system-reminder>\nold directory rule\n</system-reminder>"
        );
        assert_eq!(
            replay
                .iter()
                .filter(|m| matches!(m.role, ProviderRole::Tool))
                .count(),
            2
        );
        history[0]
            .parts
            .push(part("compose", "composed", "updated directory rule"));
        for replay in [
            provider_messages(&history),
            compaction_provider_messages(&history),
        ] {
            let reminders = replay
                .iter()
                .filter(|m| m.content.starts_with("<system-reminder>"))
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                reminders,
                ["<system-reminder>\nold directory rule\n</system-reminder>"]
            );
        }
        for part in &mut history[0].parts {
            if let Part::Tool(ToolPart {
                state: ToolState::Completed { metadata, .. },
                ..
            }) = part
            {
                metadata["compacted"] = serde_json::json!(123);
            }
        }
        assert!(!provider_messages(&history)
            .iter()
            .any(|m| m.content.starts_with("<system-reminder>")));
        history[0]
            .parts
            .push(part("read", "after-pruning", "updated directory rule"));
        assert_eq!(
            provider_messages(&history).last().unwrap().content,
            "<system-reminder>\nupdated directory rule\n</system-reminder>"
        );
        // Reverting/removing the old discoveries likewise admits the new read.
        history[0].parts = vec![part("compose", "after-revert", "new discovery")];
        assert_eq!(
            provider_messages(&history).last().unwrap().content,
            "<system-reminder>\nnew discovery\n</system-reminder>"
        );
        history[0].parts = vec![part("execute", "external-tool", "untrusted metadata")];
        assert!(!provider_messages(&history)
            .iter()
            .any(|m| m.content.starts_with("<system-reminder>")));
    }

    #[test]
    fn nearby_instruction_admission_retains_nearest_first_and_drops_duplicates() {
        let session = Id::ascending(IdKind::Session);
        let message = Id::ascending(IdKind::Message);
        let mut metadata = serde_json::json!({"instructionContext":[
            {"filepath":"/project/src/deep/AGENTS.md", "content":"nearest first"},
            {"filepath":"/project/src/AGENTS.md", "content":"parent second"},
            {"filepath":"/project/src/deep/AGENTS.md", "content":"duplicate changed"}
        ]});
        admit_instruction_context(&[], "read", &mut metadata);
        assert_eq!(metadata["instructionContext"].as_array().unwrap().len(), 2);
        let mut history = vec![MessageWithParts {
            info: assistant_info(message.clone(), session.clone()),
            parts: vec![Part::Tool(ToolPart {
                id: Id::ascending(IdKind::Part),
                session_id: session,
                message_id: message,
                tool: "read".into(),
                call_id: "read-dir".into(),
                metadata: None,
                state: ToolState::Completed {
                    input: serde_json::json!({"filePath":"src/deep"}),
                    output: "directory entries".into(),
                    metadata: metadata.clone(),
                    title: "Read directory".into(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
            })],
        }];
        let replay = provider_messages(&history);
        assert_eq!(replay.len(), 4);
        assert!(matches!(replay[0].role, ProviderRole::Assistant));
        assert!(matches!(replay[1].role, ProviderRole::Tool));
        assert_eq!(
            replay[2].content,
            "<system-reminder>\nnearest first\n</system-reminder>"
        );
        assert_eq!(
            replay[3].content,
            "<system-reminder>\nparent second\n</system-reminder>"
        );
        let updates = vec![
            (None, "initial update".into()),
            (Some("removed-message".into()), "missing anchor update".into()),
            (
                Some(history_message_id(&history[0]).into()),
                "after batch".into(),
            ),
            (
                Some(history_message_id(&history[0]).into()),
                "after batch second".into(),
            ),
        ];
        for current_model in [None, Some(("neoism", "stub"))] {
            let with_updates =
                provider_messages_with_updates(&history, current_model, &updates);
            assert_eq!(with_updates.len(), 8);
            assert_eq!(
                with_updates[0].content,
                instruction_reminder("initial update").content
            );
            assert_eq!(
                with_updates[1].content,
                instruction_reminder("missing anchor update").content
            );
            assert!(matches!(with_updates[2].role, ProviderRole::Assistant));
            assert!(matches!(with_updates[3].role, ProviderRole::Tool));
            assert_eq!(with_updates[4].content, replay[2].content);
            assert_eq!(with_updates[5].content, replay[3].content);
            assert_eq!(
                with_updates[6].content,
                instruction_reminder("after batch").content
            );
            assert_eq!(
                with_updates[7].content,
                instruction_reminder("after batch second").content
            );
            assert!(with_updates
                .iter()
                .all(|item| !matches!(item.role, ProviderRole::System)));
        }
        let removed_anchor = provider_messages_with_updates(&[], None, &updates);
        assert_eq!(removed_anchor.len(), 4);
        assert_eq!(
            removed_anchor[2].content,
            instruction_reminder("after batch").content
        );
        assert_eq!(
            removed_anchor[3].content,
            instruction_reminder("after batch second").content
        );
        let candidate = serde_json::json!({"instructionContext":[
            {"filepath":"/project/src/deep/AGENTS.md", "content":"edited on disk"}
        ]});
        let mut duplicate = candidate.clone();
        admit_instruction_context(&history, "compose", &mut duplicate);
        assert!(duplicate.get("instructionContext").is_none());
        assert_eq!(duplicate["loaded"], serde_json::json!([]));
        let mut repeated_read = history[0].parts[0].clone();
        if let Part::Tool(tool) = &mut repeated_read {
            tool.call_id = "repeated-read".into();
            if let ToolState::Completed { metadata, .. } = &mut tool.state {
                *metadata = duplicate;
            }
        }
        history[0].parts.push(repeated_read);
        // Prune only the first discovery. The skipped duplicate must not
        // resurrect its edited contents without a fresh read.
        if let Part::Tool(ToolPart {
            state: ToolState::Completed { metadata, .. },
            ..
        }) = &mut history[0].parts[0]
        {
            metadata["compacted"] = serde_json::json!(123);
        }
        assert!(!provider_messages(&history)
            .iter()
            .any(|item| item.content.starts_with("<system-reminder>")));
        let mut after_compaction = candidate.clone();
        admit_instruction_context(&history, "read", &mut after_compaction);
        assert_eq!(
            after_compaction["instructionContext"][0]["content"],
            "edited on disk"
        );
        history.clear();
        let mut after_revert = candidate;
        admit_instruction_context(&history, "compose", &mut after_revert);
        assert_eq!(
            after_revert["instructionContext"][0]["content"],
            "edited on disk"
        );
    }

    #[test]
    fn provider_messages_preserve_user_file_attachments() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let file_part_id = Id::ascending(IdKind::Part);
        let text_part_id = Id::ascending(IdKind::Part);
        let messages = provider_messages(&[MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CreatedTime { created: 1 },
                agent: "build".to_string(),
                model: UserModel {
                    provider_id: "openai".to_string(),
                    model_id: "gpt-5.5".to_string(),
                    connection_id: None,
                    variant: None,
                },
                system: None,
                tools: None,
                author: None,
            }),
            parts: vec![
                Part::Text(TextPart {
                    id: text_part_id,
                    session_id: session_id.clone(),
                    message_id: message_id.clone(),
                    text: "inspect".to_string(),
                    synthetic: None,
                    time: None,
                }),
                Part::File(FilePart {
                    id: file_part_id,
                    session_id,
                    message_id,
                    mime: "image/png".to_string(),
                    url: "data:image/png;base64,abc".to_string(),
                    filename: Some("shot.png".to_string()),
                }),
            ],
        }]);

        assert_eq!(messages.len(), 1);
        // The base64 data URL must NOT be inlined into the text content — only
        // a compact placeholder. The bytes still travel via the structured
        // attachment below.
        assert_eq!(messages[0].content, "inspect\n[image: shot.png]");
        assert_eq!(messages[0].attachments.len(), 1);
        assert_eq!(messages[0].attachments[0].mime, "image/png");
        assert_eq!(messages[0].attachments[0].url, "data:image/png;base64,abc");
    }

    #[test]
    fn compaction_provider_messages_strip_user_file_attachments() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let file_part_id = Id::ascending(IdKind::Part);
        let text_part_id = Id::ascending(IdKind::Part);
        let messages = compaction_provider_messages(&[MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CreatedTime { created: 1 },
                agent: "build".to_string(),
                model: UserModel {
                    provider_id: "openai".to_string(),
                    model_id: "gpt-5.5".to_string(),
                    connection_id: None,
                    variant: None,
                },
                system: None,
                tools: None,
                author: None,
            }),
            parts: vec![
                Part::Text(TextPart {
                    id: text_part_id,
                    session_id: session_id.clone(),
                    message_id: message_id.clone(),
                    text: "inspect".to_string(),
                    synthetic: None,
                    time: None,
                }),
                Part::File(FilePart {
                    id: file_part_id,
                    session_id,
                    message_id,
                    mime: "image/png".to_string(),
                    url: "data:image/png;base64,abc".to_string(),
                    filename: Some("shot.png".to_string()),
                }),
            ],
        }]);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].attachments.len(), 0);
        assert_eq!(messages[0].content, "inspect\n[image: shot.png]");
    }

    #[test]
    fn compaction_provider_messages_strip_tool_media_attachments() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let part_id = Id::ascending(IdKind::Part);
        let messages = compaction_provider_messages(&[MessageWithParts {
            info: assistant_info(message_id.clone(), session_id.clone()),
            parts: vec![Part::Tool(ToolPart {
                id: part_id,
                session_id,
                message_id,
                tool: "read".to_string(),
                call_id: "call_media".to_string(),
                state: ToolState::Completed {
                    input: serde_json::json!({ "path": "shot.png" }),
                    output: "Image read successfully".to_string(),
                    metadata: serde_json::json!({
                        "attachments": [{
                            "mime": "image/png",
                            "url": "data:image/png;base64,abc",
                            "filename": "shot.png"
                        }]
                    }),
                    title: "Read shot.png".to_string(),
                    time: PartTime {
                        start: 1,
                        end: Some(2),
                    },
                },
                metadata: None,
            })],
        }]);

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[1].role, ProviderRole::Tool));
        assert_eq!(messages[1].attachments.len(), 0);
    }

    #[test]
    fn provider_messages_skip_non_runtime_user_system_history() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let text_part_id = Id::ascending(IdKind::Part);
        let messages = provider_messages(&[MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CreatedTime { created: 1 },
                agent: "build".to_string(),
                model: UserModel {
                    provider_id: "openai".to_string(),
                    model_id: "gpt-5.5".to_string(),
                    connection_id: None,
                    variant: None,
                },
                system: Some("legacy agent prompt that should not replay".to_string()),
                tools: None,
                author: None,
            }),
            parts: vec![Part::Text(TextPart {
                id: text_part_id,
                session_id,
                message_id,
                text: "hello".to_string(),
                synthetic: None,
                time: None,
            })],
        }]);

        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0].role, ProviderRole::User));
        assert_eq!(messages[0].content, "hello");
        assert!(!messages[0].content.contains("legacy agent prompt"));
    }

    #[test]
    fn runtime_subtask_completion_stays_in_chronological_provider_context() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let text_part_id = Id::ascending(IdKind::Part);
        let messages = provider_messages(&[MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CreatedTime { created: 1 },
                agent: "build".to_string(),
                model: UserModel {
                    provider_id: "openai".to_string(),
                    model_id: "gpt-5.5".to_string(),
                    connection_id: None,
                    variant: None,
                },
                system: Some(
                    "Agent runtime notification: background subagent completion."
                        .to_string(),
                ),
                tools: None,
                author: None,
            }),
            parts: vec![Part::Text(TextPart {
                id: text_part_id,
                session_id,
                message_id,
                text: "Subagent finished.\n<task_result>\nfull summary\n</task_result>"
                    .to_string(),
                synthetic: None,
                time: None,
            })],
        }]);

        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0].role, ProviderRole::User));
        assert!(messages[0]
            .content
            .contains(SUBTASK_COMPLETION_NOTIFICATION_KIND));
        assert!(messages[0].content.contains("full summary"));
    }

    #[test]
    fn older_tool_results_remain_available_for_prompt_replay() {
        let session_id = Id::ascending(IdKind::Session);
        let mut history = Vec::new();
        for index in 0..10 {
            let message_id = Id::ascending(IdKind::Message);
            history.push(MessageWithParts {
                info: MessageInfo::Assistant(AssistantMessage {
                    provider_replay: None,
                    id: message_id.clone(),
                    session_id: session_id.clone(),
                    time: CompletedTime {
                        created: index as u64,
                        streamed: Some(index as u64 + 1),
                        completed: Some(index as u64 + 1),
                    },
                    parent_id: Id::ascending(IdKind::Message),
                    mode: "build".to_string(),
                    agent: "build".to_string(),
                    path: AssistantPath {
                        cwd: "/tmp".to_string(),
                        root: "/tmp".to_string(),
                    },
                    cost: 0.0,
                    tokens: TokenUsage::default(),
                    model_id: "stub".to_string(),
                    provider_id: "neoism".to_string(),
                    finish: Some("tool-calls".to_string()),
                    error: None,
                }),
                parts: vec![Part::Tool(ToolPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session_id.clone(),
                    message_id,
                    tool: "read".to_string(),
                    call_id: format!("call_{index}"),
                    state: ToolState::Completed {
                        input: serde_json::json!({ "path": format!("file-{index}.rs") }),
                        output: "x".repeat(4_000),
                        metadata: serde_json::json!({}),
                        title: format!("Read file-{index}.rs"),
                        time: PartTime {
                            start: index as u64,
                            end: Some(index as u64 + 1),
                        },
                    },
                    metadata: None,
                })],
            });
        }

        let tool_messages = provider_messages(&history)
            .into_iter()
            .filter(|message| matches!(message.role, ProviderRole::Tool))
            .collect::<Vec<_>>();

        assert_eq!(tool_messages.len(), 10);
        assert!(tool_messages
            .iter()
            .all(|message| message.content == "x".repeat(4_000)));
    }
}
