//! UI-only projection of the single authoritative compose audit record.
use super::ordinary_tool_block;
use crate::panels::agent_pane::state::NeoismAgentMessage;
use serde_json::{json, Value};
use std::io::{self, Write};

const MAX_CALLS: usize = 64;
struct BoundedJson(Vec<u8>, usize);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len().min(self.1.saturating_sub(self.0.len()));
        self.0.extend_from_slice(&bytes[..count]);
        if count < bytes.len() { return Err(io::Error::other("preview limit")); }
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
fn bounded(value: &Value, budget: usize) -> String {
    if let Some(text) = value.as_str() {
        let mut end = text.len().min(budget);
        while !text.is_char_boundary(end) { end -= 1; }
        return if end < text.len() { format!("{}… [preview truncated]", &text[..end]) } else { text.to_owned() };
    }
    let mut writer = BoundedJson(Vec::new(), budget);
    let truncated = serde_json::to_writer(&mut writer, value).is_err();
    let mut text = String::from_utf8_lossy(&writer.0).into_owned();
    if truncated { text.push_str("… [preview truncated]"); }
    text
}
fn bounded_value(value: &Value, budget: usize) -> Value {
    let text = bounded(value, budget);
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}
fn trace(part: &Value) -> &Value {
    let live = part.get("metadata").and_then(|m| m.get("composition"));
    let final_trace = part["state"].get("metadata").and_then(|m| m.get("composition"))
        .or_else(|| part.get("metadata").and_then(|m| m.get("toolResult")).and_then(|m| m.get("composition")));
    if matches!(part["state"]["status"].as_str(), Some("completed" | "error" | "interrupted")) {
        final_trace.or(live)
    } else { live.or(final_trace) }.unwrap_or(&Value::Null)
}
pub(super) fn presentation(part: &Value) -> (String, String, String) {
    let trace = trace(part);
    let count = trace["toolCalls"].as_array().map_or(0, Vec::len);
    let title = if count == 0 { "Tool Composition".into() } else {
        format!("Tool Composition({count} {})", if count == 1 { "call" } else { "calls" })
    };
    let mut result = Vec::new();
    for (key, label) in [("value", "Selected output"), ("error", "Error"), ("warnings", "Warnings"), ("logs", "Logs")] {
        if let Some(value) = trace.get(key).filter(|v| !v.is_null()) {
            result.push(format!("{label}:\n{}", bounded(value, 4096)));
        }
    }
    if let Some(error) = part["state"].get("error").filter(|v| !v.is_null()) {
        result.push(format!("Parent error: {}", bounded(error, 4096)));
    }
    if trace["truncated"].as_bool() == Some(true) { result.push("Server output truncated".into()); }
    let source = trace.get("code").and_then(Value::as_str)
        .or_else(|| part["state"]["input"]["code"].as_str())
        .or_else(|| part["state"]["input"]["source"].as_str())
        .or_else(|| part["state"]["input"]["program"].as_str()).unwrap_or("");
    (title, String::new(), json!({"neoismToolDetail":"composition", "source":source, "result":result.join("\n\n")}).to_string())
}
fn append_notice(child: &mut NeoismAgentMessage, notice: &str) {
    if let Ok(mut structured) = serde_json::from_str::<Value>(&child.detail) {
        if structured.get("neoismToolDetail").is_some() {
            let prior = structured["compositionNotice"].as_str().unwrap_or("");
            structured["compositionNotice"] = json!(format!("{prior}\n{notice}"));
            child.detail = structured.to_string();
            return;
        }
    }
    child.detail.push_str(&format!("\n{notice}"));
}
pub(super) fn children(part: &Value) -> Vec<NeoismAgentMessage> {
    let parent = part["id"].as_str().unwrap_or("");
    trace(part)["toolCalls"].as_array().into_iter().flatten().take(MAX_CALLS).enumerate().map(|(index, call)| {
        let path = call["path"].as_str().unwrap_or("tool");
        let audit = &call["presentation"];
        let tool = audit["tool"].as_str().unwrap_or_else(|| path.strip_prefix("neoism.").unwrap_or("execute"));
        let input = bounded_value(&call["input"], 8192);
        let input = if tool == "execute" { json!({"action":"call", "tool":bounded(&Value::String(path.into()), 1024), "arguments":input}) } else { input };
        let metadata = bounded_value(&audit["metadata"], 16384);
        let output = audit.get("output").filter(|v| !v.is_null()).map(|v| bounded(v, 16384));
        let error = call.get("error").filter(|v| !v.is_null()).map(|v| bounded(v, 4096));
        let status = call["status"].as_str().unwrap_or("running");
        // Retained audit output is standard native output even when execution was interrupted.
        // Feed it through the ordinary completed-output parser, then restore audit lifecycle.
        let mapping_status = if output.is_some() { "completed" } else { status };
        let mut state = json!({"status":mapping_status, "input":input, "metadata":metadata});
        if let Some(output) = &output { state["output"] = json!(output); }
        if let Some(error) = &error { state["error"] = json!(error); }
        let mut child = ordinary_tool_block(tool, &state, Some(&metadata));
        child.status = status.to_owned();
        let call_id = call["id"].as_str().filter(|id| !id.is_empty()).map(str::to_owned).unwrap_or_else(|| index.to_string());
        child.id = format!("{parent}/compose/{call_id}");
        child.composition_child = true;
        child.tool_batch_id = None;
        // Legacy audit rows cannot truthfully recover output from the selected parent value.
        if output.is_none() {
            child.detail = format!("Input:\n{}\n\nOutput not retained in this record.{}", bounded(&call["input"], 8192), error.map(|e| format!("\nError: {e}")).unwrap_or_default());
        } else if let Some(error) = error {
            if !child.detail.contains(&error) { append_notice(&mut child, &format!("Error: {error}")); }
        }
        if audit["isError"].as_bool() == Some(true) && status == "completed" { child.status = "error".into(); }
        if audit["truncated"].as_bool() == Some(true) || metadata["truncated"].as_bool() == Some(true) || call["detailsTruncated"].as_bool() == Some(true) {
            append_notice(&mut child, "Audit details/output truncated; only retained content is shown.");
        }
        child
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::agent_pane::api_mapping::part_block;
    fn part() -> Value {
        json!({"id":"p", "type":"tool", "tool":"compose", "state":{"status":"running", "input":{"code":"old"}},
            "metadata":{"toolBatchID":"batch", "composition":{"code":"return 42;", "toolCalls":[
                {"id":"a", "path":"neoism.grep", "status":"completed", "input":{"pattern":"x", "path":"src/lib.rs"}, "presentation":{"tool":"grep", "output":"src/lib.rs:1:x"}},
                {"id":"b", "path":"neoism.grep", "status":"partial_unknown", "input":{"pattern":"x", "path":"src/lib.rs"}, "error":"interrupted"}
            ]}}})
    }
    #[test]
    fn composition_typed_children_keep_identity_and_honest_legacy_errors() {
        let p = part(); let m = part_block(&p).unwrap();
        assert!(m.text.is_empty()); assert!(!m.detail.contains("toolCalls"));
        assert_eq!(serde_json::from_str::<Value>(&m.detail).unwrap()["source"], "return 42;");
        let c = &m.composition_children;
        assert_eq!(c.len(), 2); assert_eq!(c[0].id, "p/compose/a"); assert_eq!(c[1].id, "p/compose/b");
        assert_eq!(c[0].title, c[1].title); assert_eq!(c[0].tool, "grep");
        assert!(c[0].detail.contains("src/lib.rs:1:x"));
        assert!(c[1].detail.contains("Output not retained")); assert!(c[1].detail.contains("interrupted"));
        assert!(c.iter().all(|c| c.composition_child && c.tool_batch_id.is_none()));
        assert_eq!(part_block(&p).unwrap().composition_children, *c);
    }
    #[test]
    fn composition_native_read_edit_and_mcp_use_ordinary_mapping() {
        let mut p = part();
        p["metadata"]["composition"]["toolCalls"] = json!([
            {"id":"r", "path":"neoism.read", "status":"completed", "input":{"filePath":"src/lib.rs"}, "presentation":{"tool":"read", "output":"<path>src/lib.rs</path>\n<content>\n1: let x = 1;\n</content>"}},
            {"id":"e", "path":"neoism.edit", "status":"completed", "input":{"filePath":"src/lib.rs", "oldString":"a", "newString":"b"}, "presentation":{"tool":"edit", "output":"edited"}},
            {"id":"m", "path":"docs.search", "status":"error", "input":{"q":"x"}, "error":"failed", "presentation":{"tool":"execute", "output":"failure", "isError":true, "metadata":{"mcp":{"client":"docs", "tool":"search", "runtimeId":"docs.search"}}}}
        ]);
        let m = part_block(&p).unwrap(); let c = &m.composition_children;
        assert_eq!(c[0].title, "Read(src/lib.rs)"); assert_eq!(c[1].title, "Edit(src/lib.rs)");
        assert!(c[1].detail.contains("neoismToolDetail")); assert_eq!(c[2].title, "Docs Search"); assert!(c[2].detail.contains("failed"));
    }
    #[test]
    fn composition_truncated_and_partial_outputs_remain_honest() {
        let mut p = part();
        p["metadata"]["composition"]["toolCalls"] = json!([
            {"id":"a", "path":"neoism.grep", "input":{"path":"src/lib.rs"}, "status":"partial_unknown", "error":"interrupted", "presentation":{"tool":"grep", "output":"retained grep output", "truncated":true}},
            {"id":"b", "path":"neoism.read", "input":{"filePath":"src/lib.rs"}, "status":"error", "error":"failed", "detailsTruncated":true},
            {"id":"c", "path":"neoism.edit", "input":{"filePath":"src/lib.rs", "oldString":"a", "newString":"b"}, "status":"completed", "presentation":{"tool":"edit", "output":"edited", "truncated":true}}
        ]);
        let m = part_block(&p).unwrap(); let c = &m.composition_children;
        assert_eq!(c[0].status, "partial_unknown");
        assert!(c[0].detail.contains("retained grep output")); assert!(c[0].detail.contains("interrupted")); assert!(c[0].detail.contains("truncated"));
        assert!(c[1].detail.contains("Output not retained")); assert!(c[1].detail.contains("failed")); assert!(c[1].detail.contains("truncated"));
        let detail: Value = serde_json::from_str(&c[2].detail).unwrap();
        assert_eq!(detail["neoismToolDetail"], "edit"); assert!(detail["compositionNotice"].as_str().unwrap().contains("truncated"));
    }
    #[test]
    fn composition_bounds_and_terminal_trace() {
        let mut p = part();
        p["metadata"]["composition"]["toolCalls"] = json!(vec![json!({"path":"neoism.read", "input":vec!["x";100000], "status":"partial_unknown"});100]);
        let m = part_block(&p).unwrap(); assert_eq!(m.composition_children.len(), 64);
        assert!(m.composition_children.iter().all(|c| c.detail.len() < 9000));
        p["state"]["status"] = json!("completed");
        p["state"]["metadata"] = json!({"composition":{"code":"actual", "toolCalls":[], "value":42}});
        let m = part_block(&p).unwrap(); assert!(m.composition_children.is_empty()); assert!(m.detail.contains("actual")); assert!(m.detail.contains("42"));
    }
}
