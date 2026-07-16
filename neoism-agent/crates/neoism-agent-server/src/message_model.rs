use neoism_agent_core::{
    FilePart, MessageInfo, MessageWithParts, Part, ProviderAttachment, ProviderMessage,
    ProviderRole, ProviderToolCall, ToolPart, ToolState,
};
use serde_json::Value;
use std::collections::HashSet;

const SUBTASK_COMPLETION_SYSTEM_MARKER: &str =
    "Neoism runtime notification: background subagent completion.";
const BACKGROUND_TASK_COMPLETION_SYSTEM_MARKER: &str =
    "Neoism runtime notification: background shell task completion.";
const RECENT_FULL_TOOL_RESULTS: usize = 4;
const RECENT_TOOL_OUTPUT_MAX_CHARS: usize = 12 * 1024;
const OLD_TOOL_OUTPUT_MAX_CHARS: usize = 512;
// Matches opencode's TOOL_OUTPUT_MAX_CHARS for compaction requests. The
// request that triggers compaction is already near the context limit, so tool
// outputs must be aggressively truncated or the summarize request itself
// overflows the very model that is supposed to shrink the session.
const COMPACTION_TOOL_OUTPUT_MAX_CHARS: usize = 2_000;

pub(crate) fn is_runtime_system_notification(system: &str) -> bool {
    system.contains(SUBTASK_COMPLETION_SYSTEM_MARKER)
        || system.contains(BACKGROUND_TASK_COMPLETION_SYSTEM_MARKER)
}

pub(crate) fn provider_messages(messages: &[MessageWithParts]) -> Vec<ProviderMessage> {
    let recent_full_tool_calls = recent_full_tool_calls(messages);
    provider_messages_with_options(
        messages,
        MessageModelOptions {
            include_attachments: true,
            recent_full_tool_calls: &recent_full_tool_calls,
            old_tool_output_max_chars: OLD_TOOL_OUTPUT_MAX_CHARS,
            preserve_subagent_outputs: true,
        },
    )
}

pub(crate) fn compaction_provider_messages(
    messages: &[MessageWithParts],
) -> Vec<ProviderMessage> {
    provider_messages_with_options(
        messages,
        MessageModelOptions {
            include_attachments: false,
            recent_full_tool_calls: &HashSet::new(),
            old_tool_output_max_chars: COMPACTION_TOOL_OUTPUT_MAX_CHARS,
            // Compaction requests are already near the context limit;
            // subagent outputs get no exemption there.
            preserve_subagent_outputs: false,
        },
    )
}

struct MessageModelOptions<'a> {
    include_attachments: bool,
    recent_full_tool_calls: &'a HashSet<String>,
    old_tool_output_max_chars: usize,
    preserve_subagent_outputs: bool,
}

/// Subagent results are the condensed product of an entire child session —
/// truncating them to `OLD_TOOL_OUTPUT_MAX_CHARS` a few steps later erases
/// everything the fan-out paid for and makes the model re-spawn agents for
/// answers it already has. They stay at the full recent-output cap instead.
fn is_subagent_result_tool(tool: &str) -> bool {
    matches!(tool, "task" | "task_result" | "background_task_result")
}

fn provider_messages_with_options(
    messages: &[MessageWithParts],
    options: MessageModelOptions<'_>,
) -> Vec<ProviderMessage> {
    messages
        .iter()
        .flat_map(|message| match &message.info {
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
                        values.push(ProviderMessage::text(ProviderRole::System, content));
                        return values;
                    }
                }
                values.push(user_provider_message(
                    &message.parts,
                    options.include_attachments,
                ));
                values
            }
            MessageInfo::Assistant(_) => {
                assistant_provider_messages(&message.parts, &options)
            }
        })
        .filter(|message| {
            !message.content.trim().is_empty()
                || !message.tool_calls.is_empty()
                || matches!(message.role, ProviderRole::Tool)
        })
        .collect()
}

fn recent_full_tool_calls(messages: &[MessageWithParts]) -> HashSet<String> {
    let mut calls = HashSet::new();
    for part in messages
        .iter()
        .rev()
        .flat_map(|message| message.parts.iter().rev())
    {
        let Part::Tool(part) = part else {
            continue;
        };
        if !matches!(
            part.state,
            ToolState::Completed { .. } | ToolState::Error { .. }
        ) {
            continue;
        }
        calls.insert(part.call_id.clone());
        if calls.len() >= RECENT_FULL_TOOL_RESULTS {
            break;
        }
    }
    calls
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
    options: &MessageModelOptions<'_>,
) -> Vec<ProviderMessage> {
    let content = visible_part_text(parts);
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

    let mut messages = Vec::new();
    if !content.trim().is_empty() || !tool_calls.is_empty() {
        messages.push(ProviderMessage::assistant_tool_call(content, tool_calls));
    }
    messages.extend(
        tool_parts
            .into_iter()
            .flat_map(|part| tool_result_messages(part, options)),
    );
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
    options: &MessageModelOptions<'_>,
) -> Vec<ProviderMessage> {
    let recent = options.recent_full_tool_calls.contains(&part.call_id)
        || (options.preserve_subagent_outputs && is_subagent_result_tool(&part.tool));
    let output_limit = if recent {
        RECENT_TOOL_OUTPUT_MAX_CHARS
    } else {
        options.old_tool_output_max_chars
    };
    let mut result = match &part.state {
        ToolState::Completed { output, .. } => ProviderMessage::tool_result(
            &part.call_id,
            &part.tool,
            &tool_output_for_prompt(part, output, output_limit, recent, false),
            false,
        ),
        ToolState::Error { error, .. } => ProviderMessage::tool_result(
            &part.call_id,
            &part.tool,
            &tool_output_for_prompt(part, error, output_limit, recent, true),
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
    result.attachments = attachments.clone();
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
    max_chars: usize,
    recent: bool,
    error: bool,
) -> String {
    if let Some(reference) = tool_output_reference(part, output, recent, error) {
        return reference;
    }
    truncate_tool_output(output, max_chars)
}

fn tool_output_reference(
    part: &ToolPart,
    output: &str,
    recent: bool,
    error: bool,
) -> Option<String> {
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
    let preview_limit = if recent { 1_024 } else { 0 };
    let preview = truncate_tool_output(output, preview_limit);
    let kind = if error { "error" } else { "output" };
    let mut lines = vec![
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
    if preview_limit > 0 && !preview.trim().is_empty() {
        lines.push(String::new());
        lines.push("Recent preview:".to_string());
        lines.push(preview);
    }
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

fn tool_state_metadata(part: &ToolPart) -> Option<&Value> {
    match &part.state {
        ToolState::Completed { metadata, .. } => Some(metadata),
        ToolState::Pending { .. } | ToolState::Running { .. } => None,
        ToolState::Error { .. } => None,
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
    let ToolState::Completed { metadata, .. } = &part.state else {
        return Vec::new();
    };
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
        IdKind, MessageWithParts, PartTime, TextPart, TokenUsage, ToolPart, UserMessage,
        UserModel,
    };

    #[test]
    fn provider_messages_include_tool_results() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let part_id = Id::ascending(IdKind::Part);
        let messages = provider_messages(&[MessageWithParts {
            info: MessageInfo::Assistant(AssistantMessage {
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CompletedTime {
                    created: 1,
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
    fn provider_messages_reference_spilled_tool_output_instead_of_replaying_preview() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let part_id = Id::ascending(IdKind::Part);
        let large_output = "x".repeat(80_000);
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
                    output: large_output,
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
        assert!(messages[1]
            .content
            .contains("Artifact: artifact://tool-output/abc123"));
        assert!(messages[1].content.contains("Summary: big output summary"));
        assert!(messages[1].content.len() < 2_000);
    }

    #[test]
    fn provider_messages_shrink_old_unspilled_tool_results() {
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

        assert!(old_tool.content.len() < 900);
        assert!(old_tool.content.contains("omitted"));
    }

    #[test]
    fn provider_messages_surface_tool_media_attachments_as_user_message() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let part_id = Id::ascending(IdKind::Part);
        let messages = provider_messages(&[MessageWithParts {
            info: MessageInfo::Assistant(AssistantMessage {
                id: message_id.clone(),
                session_id: session_id.clone(),
                time: CompletedTime {
                    created: 1,
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
        assert_eq!(messages[1].attachments.len(), 1);
        assert!(matches!(messages[2].role, ProviderRole::User));
        assert_eq!(
            messages[2].attachments[0].filename.as_deref(),
            Some("shot.png")
        );
    }

    fn assistant_info(message_id: Id, session_id: Id) -> MessageInfo {
        MessageInfo::Assistant(AssistantMessage {
            id: message_id,
            session_id,
            time: CompletedTime {
                created: 1,
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
                    variant: None,
                },
                system: None,
                tools: None,
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
                    variant: None,
                },
                system: None,
                tools: None,
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
                    variant: None,
                },
                system: Some("legacy agent prompt that should not replay".to_string()),
                tools: None,
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
    fn runtime_subtask_completion_is_system_only_provider_context() {
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
                    variant: None,
                },
                system: Some(SUBTASK_COMPLETION_SYSTEM_MARKER.to_string()),
                tools: None,
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
        assert!(matches!(messages[0].role, ProviderRole::System));
        assert!(messages[0]
            .content
            .contains(SUBTASK_COMPLETION_SYSTEM_MARKER));
        assert!(messages[0].content.contains("full summary"));
    }

    #[test]
    fn older_tool_results_are_truncated_for_prompt_replay() {
        let session_id = Id::ascending(IdKind::Session);
        let mut history = Vec::new();
        for index in 0..=RECENT_FULL_TOOL_RESULTS {
            let message_id = Id::ascending(IdKind::Message);
            history.push(MessageWithParts {
                info: MessageInfo::Assistant(AssistantMessage {
                    id: message_id.clone(),
                    session_id: session_id.clone(),
                    time: CompletedTime {
                        created: index as u64,
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
                        output: "x".repeat(OLD_TOOL_OUTPUT_MAX_CHARS + 100),
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

        assert!(tool_messages[0].content.contains("truncated"));
        assert!(!tool_messages
            .last()
            .expect("recent tool result")
            .content
            .contains("truncated"));
    }
}
