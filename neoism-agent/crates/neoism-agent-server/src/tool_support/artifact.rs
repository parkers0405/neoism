use anyhow::Context as _;
use neoism_agent_core::{MessageInfo, Part, ToolState};
use serde_json::{json, Value};
use std::fs;

use super::{ToolContext, ToolExecutionResult};

const DEFAULT_ARTIFACT_READ_LIMIT: usize = 2000;
const DEFAULT_ARTIFACT_SEARCH_LIMIT: usize = 50;

#[derive(Clone, Debug)]
pub(crate) struct ToolArtifact {
    pub(crate) id: String,
    pub(crate) uri: String,
    pub(crate) title: String,
    pub(crate) tool: String,
    pub(crate) path: String,
    pub(crate) byte_count: u64,
    pub(crate) summary: String,
}

pub(crate) fn metadata(
    session_id: Option<&str>,
    tool: &str,
    title: &str,
    output_path: &str,
    output: &str,
) -> Value {
    let id = id_for(session_id, tool, output_path);
    let uri = format!("artifact://tool-output/{id}");
    json!({
        "id": id,
        "uri": uri,
        "kind": "tool-output",
        "tool": tool,
        "title": title,
        "path": output_path,
        "byteCount": output.as_bytes().len() as u64,
        "summary": summarize(output),
    })
}

pub(crate) fn artifact_from_metadata(value: &Value) -> Option<ToolArtifact> {
    Some(ToolArtifact {
        id: value.get("id")?.as_str()?.to_string(),
        uri: value.get("uri")?.as_str()?.to_string(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Tool output")
            .to_string(),
        tool: value
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string(),
        path: value.get("path")?.as_str()?.to_string(),
        byte_count: value.get("byteCount").and_then(Value::as_u64).unwrap_or(0),
        summary: value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

pub(crate) async fn read_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let artifact = resolve_artifact(&context, &arguments).await?;
    let offset = usize_arg(&arguments, "offset").unwrap_or(1).max(1);
    let limit = usize_arg(&arguments, "limit")
        .unwrap_or(DEFAULT_ARTIFACT_READ_LIMIT)
        .max(1);
    let content = fs::read_to_string(&artifact.path)
        .with_context(|| format!("failed to read {}", artifact.path))?;
    let lines = content.lines().collect::<Vec<_>>();
    if offset > lines.len().saturating_add(1) {
        anyhow::bail!(
            "offset {offset} is out of range for {} ({} lines)",
            artifact.uri,
            lines.len()
        );
    }
    let start = offset - 1;
    let mut output = format!(
        "<artifact>{}</artifact>\n<title>{}</title>\n<tool>{}</tool>\n<path>{}</path>\n<summary>{}</summary>\n",
        artifact.uri, artifact.title, artifact.tool, artifact.path, artifact.summary
    );
    let mut shown = 0;
    for (index, line) in lines.iter().skip(start).take(limit).enumerate() {
        shown += 1;
        output.push_str(&format!("{}: {}\n", start + index + 1, line));
    }
    if shown > 0 && start + shown < lines.len() {
        output.push_str(&format!(
            "\n(Showing lines {}-{} of {}. Use offset={} to continue.)",
            start + 1,
            start + shown,
            lines.len(),
            start + shown + 1
        ));
    }
    Ok(ToolExecutionResult {
        title: format!("Read {}", artifact.uri),
        output,
        metadata: Some(json!({ "artifact": artifact_metadata_json(&artifact) })),
    })
}

pub(crate) async fn search_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let artifact = resolve_artifact(&context, &arguments).await?;
    let query = crate::tool::args::required_string(&arguments, "query")?.to_string();
    let limit = usize_arg(&arguments, "limit")
        .unwrap_or(DEFAULT_ARTIFACT_SEARCH_LIMIT)
        .max(1);
    let content = fs::read_to_string(&artifact.path)
        .with_context(|| format!("failed to read {}", artifact.path))?;
    let mut matches = String::new();
    let mut match_count = 0;
    for (index, line) in content.lines().enumerate() {
        if line.contains(&query) {
            if match_count > 0 {
                matches.push('\n');
            }
            matches.push_str(&format!("{}: {}", index + 1, line));
            match_count += 1;
            if match_count >= limit {
                break;
            }
        }
    }
    let output = if match_count == 0 {
        format!("No matches for {query:?} in {}", artifact.uri)
    } else {
        format!(
            "<artifact>{}</artifact>\n<title>{}</title>\n<query>{}</query>\n{}",
            artifact.uri, artifact.title, query, matches
        )
    };
    Ok(ToolExecutionResult {
        title: format!("Search {}", artifact.uri),
        output,
        metadata: Some(
            json!({ "artifact": artifact_metadata_json(&artifact), "query": query }),
        ),
    })
}

async fn resolve_artifact(
    context: &ToolContext,
    arguments: &Value,
) -> anyhow::Result<ToolArtifact> {
    let id_or_uri =
        crate::tool::args::required_string(arguments, "artifact")?.to_string();
    let id = id_or_uri
        .strip_prefix("artifact://tool-output/")
        .unwrap_or(&id_or_uri);
    let Some(state) = context.state() else {
        anyhow::bail!("artifact tools require session state");
    };
    let Some(session_id) = context.session_id() else {
        anyhow::bail!("artifact tools require a session id");
    };
    let messages = state.inner.store.list_messages(session_id).await?;
    for message in messages {
        if let MessageInfo::Assistant(_) = message.info {
            for part in message.parts {
                let Part::Tool(tool) = part else { continue };
                let ToolState::Completed { metadata, .. } = tool.state else {
                    continue;
                };
                let Some(artifact) =
                    metadata.get("artifact").and_then(artifact_from_metadata)
                else {
                    continue;
                };
                if artifact.id == id || artifact.uri == id_or_uri {
                    return Ok(artifact);
                }
            }
        }
    }
    anyhow::bail!("unknown artifact {id_or_uri}")
}

fn artifact_metadata_json(artifact: &ToolArtifact) -> Value {
    json!({
        "id": artifact.id,
        "uri": artifact.uri,
        "kind": "tool-output",
        "tool": artifact.tool,
        "title": artifact.title,
        "path": artifact.path,
        "byteCount": artifact.byte_count,
        "summary": artifact.summary,
    })
}

fn id_for(session_id: Option<&str>, tool: &str, path: &str) -> String {
    let raw = format!("{}:{tool}:{path}", session_id.unwrap_or("session"));
    let mut hash: u64 = 1469598103934665603;
    for byte in raw.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{:016x}", hash)
}

fn summarize(output: &str) -> String {
    let mut line_count = 0;
    let mut first_non_empty = None;
    let mut test_lines = Vec::new();
    let mut error_lines = Vec::new();
    let mut file_mentions = Vec::new();

    for line in output.lines() {
        line_count += 1;
        let trimmed = line.trim();
        if first_non_empty.is_none() && !trimmed.is_empty() {
            first_non_empty = Some(trimmed);
        }
        if test_lines.len() < 8 || error_lines.len() < 8 {
            let lower = line.to_ascii_lowercase();
            if test_lines.len() < 8
                && (lower.contains("test result:")
                    || lower.contains("failures:")
                    || lower.contains("failed")
                    || lower.contains("passed"))
            {
                test_lines.push(trimmed);
            }
            if error_lines.len() < 8
                && (lower.contains("error")
                    || lower.contains("failed")
                    || lower.contains("panic")
                    || lower.contains("exception")
                    || lower.contains("warning"))
            {
                error_lines.push(trimmed);
            }
        }
        if file_mentions.len() < 6 {
            if let Some(mention) = likely_file_mention(line) {
                file_mentions.push(mention);
            }
        }
    }

    let mut parts = vec![format!(
        "{} lines, {} bytes",
        line_count,
        output.as_bytes().len()
    )];
    if !test_lines.is_empty() {
        parts.push(format!("test/status lines: {}", test_lines.join(" | ")));
    }
    if !error_lines.is_empty() {
        parts.push(format!("notable lines: {}", error_lines.join(" | ")));
    } else if let Some(first) = first_non_empty {
        parts.push(format!("starts with: {first}"));
    }
    if !file_mentions.is_empty() {
        parts.push(format!("mentioned files: {}", file_mentions.join(", ")));
    }
    let summary = parts.join("; ");
    if summary.chars().count() > 1000 {
        summary.chars().take(1000).collect::<String>() + "..."
    } else {
        summary
    }
}

fn likely_file_mention(line: &str) -> Option<String> {
    line.split_whitespace()
        .map(|part| {
            part.trim_matches(|ch: char| matches!(ch, ':' | ',' | ')' | '(' | '[' | ']'))
        })
        .find(|part| {
            part.contains('/')
                && (part.ends_with(".rs")
                    || part.ends_with(".ts")
                    || part.ends_with(".tsx")
                    || part.ends_with(".js")
                    || part.ends_with(".py")
                    || part.ends_with(".go")
                    || part.ends_with(".md"))
        })
        .map(ToOwned::to_owned)
}

fn usize_arg(arguments: &Value, key: &str) -> Option<usize> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_extracts_notable_lines() {
        let summary = summarize("ok\nerror: bad thing in src/main.rs\nwarning: careful");

        assert!(summary.contains("3 lines"));
        assert!(summary.contains("error: bad thing"));
        assert!(summary.contains("warning: careful"));
        assert!(summary.contains("src/main.rs"));
    }
}
