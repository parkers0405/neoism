//! Presentation-only projection of the compose parent trace. Never creates child parts.
use serde_json::{json, Value};
use std::io::{self, Write};

const MAX_CALLS: usize = 64;
const VALUE_BYTES: usize = 4096;

// Stop serialization at the budget rather than first allocating an enormous JSON string.
struct BoundedJson(Vec<u8>, usize);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len().min(self.1.saturating_sub(self.0.len()));
        self.0.extend_from_slice(&bytes[..count]);
        if count < bytes.len() {
            return Err(io::Error::other("preview limit"));
        }
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded(value: &Value, budget: usize) -> String {
    if let Some(text) = value.as_str() {
        let mut chars = text.chars();
        let mut result: String = chars.by_ref().take(budget / 4).collect();
        if chars.next().is_some() {
            result.push_str("… [preview truncated]");
        }
        return result;
    }
    let mut writer = BoundedJson(Vec::new(), budget);
    let truncated = serde_json::to_writer(&mut writer, value).is_err();
    let mut text = String::from_utf8_lossy(&writer.0).into_owned();
    if truncated {
        text.push_str("… [preview truncated]");
    }
    text
}
fn inline(value: &Value, budget: usize) -> String {
    bounded(value, budget).replace(['\n', '\r', '\t'], " ")
}

pub(super) fn presentation(part: &Value) -> (String, String, String) {
    let state = &part["state"];
    let live_trace = part
        .get("metadata")
        .and_then(|metadata| metadata.get("composition"));
    let final_trace = state
        .get("metadata")
        .and_then(|metadata| metadata.get("composition"))
        .or_else(|| {
            part.get("metadata")
                .and_then(|metadata| metadata.get("toolResult"))
                .and_then(|metadata| metadata.get("composition"))
        });
    // Completion can retain the last live snapshot at the part root. Prefer the
    // persisted result once terminal so selected output is not hidden by it.
    let trace = if matches!(state["status"].as_str(), Some("completed" | "error")) {
        final_trace.or(live_trace)
    } else {
        live_trace.or(final_trace)
    }
    .unwrap_or(&Value::Null);
    let calls = trace.get("toolCalls").and_then(Value::as_array);
    let count = calls.map_or(0, Vec::len);
    let title = if count == 0 {
        "Tool Composition".to_owned()
    } else {
        format!(
            "Tool Composition({count} {})",
            if count == 1 { "call" } else { "calls" }
        )
    };
    let mut children = Vec::new();
    for call in calls.into_iter().flatten().take(MAX_CALLS) {
        let status = match call["status"].as_str() {
            Some("completed") => "completed",
            Some("error") => "error",
            Some("interrupted") => "interrupted",
            Some("partial_unknown") => "partial_unknown",
            Some("running") | None => "running",
            _ => "unknown",
        };
        let marker = match status {
            "completed" => "✓",
            "error" => "✗",
            "interrupted" | "partial_unknown" | "unknown" => "!",
            _ => "·",
        };
        let path = call
            .get("path")
            .map(|v| inline(v, 384))
            .unwrap_or_else(|| "tool".into());
        let input = call
            .get("input")
            .map(|v| inline(v, 128))
            .unwrap_or_default();
        let error = call
            .get("error")
            .filter(|v| !v.is_null())
            .map(|v| format!(" — {}", inline(v, 384)))
            .unwrap_or_default();
        children.push(format!("{marker} {path} [{status}] {input}{error}"));
    }
    if count > MAX_CALLS {
        children.push(format!(
            "… {} more calls (preview bounded)",
            count - MAX_CALLS
        ));
    }
    if children.is_empty() {
        children.push("Waiting for tool calls".into());
    }
    let preview = children.join("\n");
    let mut result = Vec::new();
    if let Some(ok) = trace.get("ok").and_then(Value::as_bool) {
        result.push(
            if ok {
                "Composition succeeded"
            } else {
                "Composition failed; completed calls remain applied"
            }
            .into(),
        );
    }
    for (key, label) in [
        ("value", "Selected output"),
        ("error", "Error"),
        ("warnings", "Warnings"),
        ("logs", "Logs"),
    ] {
        if let Some(value) = trace.get(key).filter(|v| !v.is_null()) {
            result.push(format!("{label}:\n{}", bounded(value, VALUE_BYTES)));
        }
    }
    if matches!(state["status"].as_str(), Some("error" | "interrupted")) {
        if let Some(error) = state.get("error").filter(|v| !v.is_null()) {
            result.push(format!(
                "Parent {}: {}",
                state["status"].as_str().unwrap_or("error"),
                bounded(error, VALUE_BYTES)
            ));
        }
    }
    if trace["truncated"].as_bool() == Some(true) {
        result.push("Server output truncated".into());
    }
    let source = state
        .get("input")
        .and_then(|v| {
            v.get("code")
                .or_else(|| v.get("source"))
                .or_else(|| v.get("program"))
        })
        .map(|v| bounded(v, 131072))
        .unwrap_or_default();
    let detail = json!({"neoismToolDetail":"composition", "calls":preview,
        "result":result.join("\n\n"), "source":source})
    .to_string();
    (title, preview, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::agent_pane::api_mapping::part_block;

    #[test]
    fn live_and_final_trace_keep_one_identity_source_and_effects() {
        let mut part = json!({"id":"parent", "type":"tool", "tool":"compose",
            "state":{"status":"running", "input":{"code":"return await tools.docs.search({q: 'x'});"}},
            "metadata":{"toolBatchID":"batch", "composition":{"toolCalls":[
                {"id":"one", "path":"docs.search", "input":{"q":"x"}, "status":"running"}]}}});
        let live = part_block(&part).unwrap();
        assert_eq!(live.id, "parent");
        assert_eq!(live.tool, "compose");
        assert_eq!(live.title, "Tool Composition(1 call)");
        assert_eq!(live.tool_batch_id.as_deref(), Some("batch"));
        assert!(live.text.contains("[running]"));
        let source =
            serde_json::from_str::<Value>(&live.detail).unwrap()["source"].clone();
        for status in ["completed", "error", "interrupted"] {
            part["state"]["status"] = json!(status);
            part["metadata"]["composition"] = json!({"toolCalls":[
                {"id":"one","path":"docs.search","input":{},"status":"completed"},
                {"id":"two","path":"docs.write","input":{},"status":status,"error":"stopped"}],
                "ok":status == "completed", "value":{"selected":42}, "error":"parent failed", "logs":["log"], "warnings":["warning"], "truncated":true});
            let top = part_block(&part).unwrap();
            let detail: Value = serde_json::from_str(&top.detail).unwrap();
            assert_eq!(detail["source"], source);
            assert!(top.text.contains("docs.search [completed]"));
            assert!(top.text.contains(&format!("[{status}]")));
            assert!(detail["result"]
                .as_str()
                .unwrap()
                .contains("Selected output"));
            assert!(detail["result"]
                .as_str()
                .unwrap()
                .contains("Server output truncated"));
            part["state"]["metadata"] = part["metadata"].take();
            assert_eq!(part_block(&part).unwrap().detail, top.detail);
            part["metadata"] = part["state"]["metadata"].take();
        }
    }

    #[test]
    fn terminal_result_replaces_stale_live_progress() {
        for status in ["completed", "error"] {
            let mut part = json!({"type":"tool", "tool":"compose",
                "state":{"status":status, "input":{"code":"return 42;"}},
                "metadata":{"composition":{"toolCalls":[{"path":"neoism.read", "status":"running"}]}}});
            let final_trace = json!({"ok":status == "completed", "value":42,
                "toolCalls":[{"path":"neoism.read", "status":"completed"}]});
            if status == "completed" {
                part["state"]["metadata"] = json!({"composition":final_trace});
            } else {
                part["metadata"]["toolResult"] = json!({"composition":final_trace});
            }
            let message = part_block(&part).unwrap();
            assert!(message.text.contains("[completed]"));
            assert!(!message.text.contains("[running]"));
            assert!(message.detail.contains("Selected output"));
            assert!(message.detail.contains("42"));
        }
    }

    #[test]
    fn composition_metadata_does_not_reclassify_execute_or_native_tools() {
        for (tool, input, title) in [
            (
                "execute",
                json!({"action":"call", "tool":"docs.search"}),
                "Docs Search",
            ),
            ("execute", json!({"action":"search"}), "Find Tools"),
            ("read", json!({"filePath":"src/lib.rs"}), "Read(src/lib.rs)"),
        ] {
            let mut part = json!({"type":"tool", "tool":tool, "state":{"status":"running", "input":input},
                "metadata":{"toolBatchID":"native-batch"}});
            let original = part_block(&part).unwrap();
            part["metadata"]["composition"] =
                json!({"toolCalls":[{"path":"other.tool", "status":"running"}]});
            let mapped = part_block(&part).unwrap();
            assert_eq!(mapped.title, title);
            assert_eq!(mapped.detail, original.detail);
            assert_eq!(mapped.tool, tool);
            assert_eq!(mapped.tool_batch_id.as_deref(), Some("native-batch"));
        }
    }

    #[test]
    fn composition_live_metadata_updates_replace_one_card() {
        use crate::panels::agent_pane::state::NeoismAgentPane;
        let mut pane = NeoismAgentPane::default();
        let mut part = json!({"id":"parent", "type":"tool", "tool":"compose",
            "state":{"status":"running", "input":{"code":"return 42;"}},
            "metadata":{"composition":{"toolCalls":[{"path":"docs.write", "status":"running", "input":{}}]}}});
        pane.upsert_part_message(part_block(&part).unwrap());
        part["metadata"]["composition"]["toolCalls"][0]["status"] = json!("completed");
        pane.upsert_part_message(part_block(&part).unwrap());
        assert_eq!(pane.messages.len(), 1);
        assert!(pane.messages[0].text.contains("[completed]"));
        assert!(pane.messages[0].detail.contains("return 42;"));
        part["state"]["status"] = json!("error");
        part["state"]["error"] = json!("parent failed");
        pane.upsert_part_message(part_block(&part).unwrap());
        assert_eq!(pane.messages.len(), 1);
        assert_eq!(pane.messages[0].status, "error");
        assert!(pane.messages[0].text.contains("[completed]"));
        assert!(pane.messages[0].detail.contains("parent failed"));
    }

    #[test]
    fn complete_bounded_execution_history_is_inspectable() {
        let calls: Vec<Value> = (0..64)
            .map(|index| json!({"path":format!("neoism.tool_{index}"), "status":"completed", "input":{}}))
            .collect();
        let part = json!({"type":"tool", "tool":"compose",
            "state":{"status":"completed", "input":{"code":"return 1;"},
                "metadata":{"composition":{"toolCalls":calls, "ok":true, "value":1}}}});
        let message = part_block(&part).unwrap();
        assert!(message.text.contains("neoism.tool_63"));
        assert!(!message.text.contains("more calls"));
    }

    #[test]
    fn huge_trace_and_values_have_bounded_previews() {
        let calls = vec![
            json!({"path":"docs.search", "status":"completed", "input":vec!["x";10000]});
            1000
        ];
        let part = json!({"type":"tool", "tool":"compose", "state":{"status":"completed", "input":{"code":"return 1;"}},
            "metadata":{"composition":{"toolCalls":calls, "value":vec!["x";100000]}}});
        let message = part_block(&part).unwrap();
        assert_eq!(message.title, "Tool Composition(1000 calls)");
        assert!(message.text.contains("936 more calls"));
        assert!(message.detail.len() < 25000);
        assert!(message.detail.contains("preview truncated"));
    }
}
