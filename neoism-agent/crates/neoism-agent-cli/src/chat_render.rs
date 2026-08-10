use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use serde_json::Value;

use crate::chat_markdown::MarkdownStreamRenderer;
use crate::chat_tool_render::{
    render_completed_tool, render_running_task, tool_target, TruncatedOutput,
};
use crate::{BOLD, DIM, ITALIC, RED, RESET, WHITE};

const BULLET: &str = "●";

pub(crate) struct ChatRenderState {
    pub(crate) printed_text: bool,
    pub(crate) reasoning_parts: BTreeSet<String>,
    tool_statuses: BTreeMap<String, String>,
    text: MarkdownStreamRenderer,
    reasoning: MarkdownStreamRenderer,
    in_reasoning: bool,
    activity: String,
    pending_truncated: Vec<TruncatedOutput>,
}

impl Default for ChatRenderState {
    fn default() -> Self {
        let mut reasoning = MarkdownStreamRenderer::default();
        reasoning.set_line_style(format!("{DIM}{ITALIC}"));
        Self {
            printed_text: false,
            reasoning_parts: BTreeSet::new(),
            tool_statuses: BTreeMap::new(),
            text: MarkdownStreamRenderer::default(),
            reasoning,
            in_reasoning: false,
            activity: "Working".to_string(),
            pending_truncated: Vec::new(),
        }
    }
}

impl ChatRenderState {
    pub(crate) fn take_pending_truncated(&mut self) -> Vec<TruncatedOutput> {
        std::mem::take(&mut self.pending_truncated)
    }

    fn set_activity(&mut self, value: impl Into<String>) {
        self.activity = value.into();
    }

    pub(crate) fn text_delta(&mut self, delta: &str) -> anyhow::Result<()> {
        self.finish_reasoning()?;
        if !self.printed_text {
            println!();
            self.text
                .set_first_line_prefix(format!("{WHITE}{BOLD}{BULLET}{RESET} "));
        }
        self.text.push(delta)?;
        self.printed_text = true;
        self.set_activity("Writing");
        Ok(())
    }

    pub(crate) fn reasoning_delta(&mut self, delta: &str) -> anyhow::Result<()> {
        self.text.finish()?;
        if !self.in_reasoning {
            println!();
            println!("{DIM}{ITALIC}✻ Thinking{RESET}");
            self.in_reasoning = true;
        }
        self.reasoning.push(delta)?;
        self.set_activity("Thinking");
        Ok(())
    }

    pub(crate) fn finish_reasoning(&mut self) -> anyhow::Result<()> {
        if self.in_reasoning {
            self.reasoning.finish()?;
            println!("{RESET}");
            self.in_reasoning = false;
            std::io::stdout().flush()?;
        }
        Ok(())
    }

    pub(crate) fn part_updated(&mut self, part: &Value) -> anyhow::Result<()> {
        match part.get("type").and_then(Value::as_str).unwrap_or_default() {
            "reasoning" => {
                if let Some(id) = part.get("id").and_then(Value::as_str) {
                    self.reasoning_parts.insert(id.to_string());
                }
                if part
                    .get("time")
                    .and_then(|time| time.get("end"))
                    .is_some_and(|end| !end.is_null())
                {
                    self.finish_reasoning()?;
                }
            }
            "tool" => self.tool_updated(part)?,
            _ => {}
        }
        Ok(())
    }

    fn tool_updated(&mut self, part: &Value) -> anyhow::Result<()> {
        self.finish_reasoning()?;
        self.text.finish()?;
        let id = part
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let tool = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
        let state = part.get("state").unwrap_or(&Value::Null);
        let status = state
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let dedupe_key = format!(
            "{status}:{}:{}",
            state
                .get("title")
                .or_else(|| state.get("error"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            part.get("metadata")
                .and_then(|metadata| metadata.get("sessionId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
        if self.tool_statuses.get(&id) == Some(&dedupe_key) {
            return Ok(());
        }
        self.tool_statuses.insert(id, dedupe_key);
        let input = state.get("input").unwrap_or(&Value::Null);
        let metadata = part.get("metadata").unwrap_or(&Value::Null);
        match status {
            "pending" => {
                self.set_activity(format!("Preparing {}", tool_summary_group(tool)));
            }
            "running" => {
                if tool == "task" {
                    render_running_task(input, metadata);
                    std::io::stdout().flush()?;
                }
                self.set_activity(format!(
                    "Running {}",
                    format_tool_activity(tool, input)
                ));
            }
            "completed" => {
                let output = state
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let metadata = state.get("metadata").unwrap_or(metadata);
                let title = state
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(truncated) =
                    render_completed_tool(tool, input, output, metadata, title)?
                {
                    self.pending_truncated.push(truncated);
                }
                std::io::stdout().flush()?;
                self.set_activity("Working");
            }
            "error" => {
                let error = state
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("tool error");
                println!(
                    "{WHITE}{BOLD}{BULLET}{RESET} {BOLD}{}{RESET}({DIM}error{RESET})",
                    tool_summary_group(tool)
                );
                println!("    {RED}└ {error}{RESET}");
                self.set_activity("Working");
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> anyhow::Result<()> {
        self.finish_reasoning()?;
        self.text.finish()
    }
}

fn format_tool_activity(tool: &str, input: &Value) -> String {
    let target = tool_target(tool, input);
    if target.is_empty() {
        tool_summary_group(tool).to_string()
    } else {
        format!("{} {DIM}{target}{RESET}", tool_summary_group(tool))
    }
}

fn tool_summary_group(tool: &str) -> &str {
    match tool {
        "read" => "read",
        "grep" | "glob" | "websearch" => "search",
        "webfetch" => "fetch",
        "bash" => "exec",
        "write" | "edit" | "apply_patch" => "edit",
        "task" => "task",
        "question" => "question",
        "todowrite" => "todo",
        other => other,
    }
}
