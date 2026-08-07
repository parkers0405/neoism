use neoism_agent_core::{McpContent, McpToolCallResult, McpToolInfo};
use serde_json::{json, Value};

pub(crate) const DOCS_MCP_ID: &str = "neoism-docs";

pub(crate) fn tools() -> Vec<McpToolInfo> {
    vec![
        tool(
            "docs.list",
            "List Neoism's bundled product documentation",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "docs.search",
            "Search Neoism's bundled product documentation",
            json!({
                "type":"object",
                "properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20}},
                "required":["query"]
            }),
        ),
        tool(
            "docs.read",
            "Read one bundled Neoism documentation page by path",
            json!({
                "type":"object",
                "properties":{"path":{"type":"string"}},
                "required":["path"]
            }),
        ),
    ]
}

pub(crate) fn call_tool(
    tool_name: &str,
    arguments: Value,
) -> anyhow::Result<McpToolCallResult> {
    let output = match tool_name {
        "docs.list" => {
            json!({"documents": neoism_workspace_index::docs::BUNDLED_DOCS.iter().map(|doc| json!({
            "path": doc.path,
            "title": neoism_workspace_index::docs::title(doc),
        })).collect::<Vec<_>>() })
        }
        "docs.read" => {
            let path = required_string(&arguments, "path")?;
            let doc =
                neoism_workspace_index::docs::bundled_doc(&path).ok_or_else(|| {
                    anyhow::anyhow!("unknown Neoism documentation page {path}")
                })?;
            json!({"path":doc.path,"title":neoism_workspace_index::docs::title(doc),"content":doc.body})
        }
        "docs.search" => {
            let query = required_string(&arguments, "query")?;
            let limit = arguments
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(8)
                .clamp(1, 20) as usize;
            let terms = query
                .split_whitespace()
                .map(|term| term.to_lowercase())
                .collect::<Vec<_>>();
            let mut hits = neoism_workspace_index::docs::BUNDLED_DOCS
                .iter()
                .filter_map(|doc| {
                    let title = neoism_workspace_index::docs::title(doc);
                    let title_lower = title.to_lowercase();
                    let body_lower = doc.body.to_lowercase();
                    let score = terms
                        .iter()
                        .map(|term| {
                            usize::from(title_lower.contains(term)) * 10
                                + body_lower.matches(term).count()
                        })
                        .sum::<usize>();
                    (score > 0).then(|| {
                        (
                            score,
                            json!({
                                "path":doc.path,
                                "title":title,
                                "snippet":snippet(doc.body, &terms),
                            }),
                        )
                    })
                })
                .collect::<Vec<_>>();
            hits.sort_by(|a, b| b.0.cmp(&a.0));
            json!({"query":query,"hits":hits.into_iter().take(limit).map(|(_, hit)| hit).collect::<Vec<_>>()})
        }
        other => anyhow::bail!("unknown Neoism Docs MCP tool {other}"),
    };
    Ok(McpToolCallResult {
        content: vec![McpContent::Text {
            text: serde_json::to_string_pretty(&output)?,
            annotations: None,
        }],
        is_error: None,
    })
}

fn snippet(body: &str, terms: &[String]) -> String {
    body.lines()
        .find(|line| {
            let lower = line.to_lowercase();
            terms.iter().any(|term| lower.contains(term))
        })
        .unwrap_or_else(|| body.lines().next().unwrap_or_default())
        .trim()
        .chars()
        .take(240)
        .collect()
}

fn required_string(arguments: &Value, key: &str) -> anyhow::Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("{key} is required"))
}

fn tool(
    name: &'static str,
    description: &'static str,
    input_schema: Value,
) -> McpToolInfo {
    McpToolInfo {
        name: name.to_string(),
        description: Some(description.to_string()),
        input_schema,
        client: DOCS_MCP_ID.to_string(),
        annotations: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_embedded_docs_without_a_vault_file() {
        let result = call_tool("docs.read", json!({"path":"Start Here.md"})).unwrap();
        let McpContent::Text { text, .. } = &result.content[0] else {
            panic!("expected text")
        };
        assert!(text.contains("Welcome to Neoism"));
    }

    #[test]
    fn search_finds_configuration_docs() {
        let result = call_tool("docs.search", json!({"query":"shader"})).unwrap();
        let McpContent::Text { text, .. } = &result.content[0] else {
            panic!("expected text")
        };
        assert!(text.contains("Neoism/Appearance.md"));
    }
}
