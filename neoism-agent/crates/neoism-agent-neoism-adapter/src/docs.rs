use std::path::Path;

use neoism_agent_service_api::{
    BuiltinMcpCallResult, BuiltinMcpContent, BuiltinMcpService, BuiltinMcpTool,
    DocumentationPage, DocumentationPageSummary, DocumentationSearchHit,
    DocumentationService, ServiceError,
};
use serde_json::{json, Value};

const MCP_ID: &str = "neoism-docs";

pub(crate) struct NeoismDocumentationService;

impl DocumentationService for NeoismDocumentationService {
    fn list(&self) -> Result<Vec<DocumentationPageSummary>, ServiceError> {
        Ok(neoism_product_docs::BUNDLED_DOCS.iter().map(|doc| DocumentationPageSummary {
            path: doc.path.to_string(),
            title: neoism_product_docs::title(doc).to_string(),
        }).collect())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<DocumentationSearchHit>, ServiceError> {
        let terms = query.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
        let mut hits = neoism_product_docs::BUNDLED_DOCS.iter().filter_map(|doc| {
            let title = neoism_product_docs::title(doc);
            let title_lower = title.to_lowercase();
            let body_lower = doc.body.to_lowercase();
            let score = terms.iter().map(|term| usize::from(title_lower.contains(term)) * 10 + body_lower.matches(term).count()).sum::<usize>();
            (score > 0).then(|| (score, DocumentationSearchHit { path: doc.path.to_string(), title: title.to_string(), snippet: snippet(doc.body, &terms) }))
        }).collect::<Vec<_>>();
        hits.sort_by(|left, right| right.0.cmp(&left.0));
        Ok(hits.into_iter().take(limit).map(|(_, hit)| hit).collect())
    }

    fn read(&self, path: &str) -> Result<DocumentationPage, ServiceError> {
        let normalized = path.trim().trim_start_matches('/');
        neoism_product_docs::bundled_doc(normalized).map(page)
            .ok_or_else(|| ServiceError::new(format!("unknown product documentation page {path}")))
    }
}

impl BuiltinMcpService for NeoismDocumentationService {
    fn id(&self) -> &str { MCP_ID }

    fn tools(&self) -> Vec<BuiltinMcpTool> {
        vec![
            tool("docs.list", "List bundled product documentation", json!({"type":"object","properties":{}})),
            tool("docs.search", "Search bundled product documentation", json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20}},"required":["query"]})),
            tool("docs.read", "Read one bundled product documentation page by path", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})),
        ]
    }

    fn call_tool(&self, _working_directory: &Path, tool: &str, arguments: Value) -> Result<BuiltinMcpCallResult, ServiceError> {
        let output = match tool {
            "docs.list" => json!({"documents":self.list()?.into_iter().map(|doc| json!({"path":doc.path,"title":doc.title})).collect::<Vec<_>>() }),
            "docs.read" => {
                let doc = self.read(&required_string(&arguments, "path")?)?;
                json!({"path":doc.path,"title":doc.title,"content":doc.content})
            }
            "docs.search" => {
                let query = required_string(&arguments, "query")?;
                let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(8).clamp(1, 20) as usize;
                let hits = self.search(&query, limit)?.into_iter().map(|hit| json!({"path":hit.path,"title":hit.title,"snippet":hit.snippet})).collect::<Vec<_>>();
                json!({"query":query,"hits":hits})
            }
            other => return Err(ServiceError::new(format!("unknown documentation MCP tool {other}"))),
        };
        Ok(BuiltinMcpCallResult { content: vec![BuiltinMcpContent::Text { text: serde_json::to_string_pretty(&output).map_err(|error| ServiceError::new(error.to_string()))?, annotations: None }], is_error: None })
    }
}

fn page(doc: &neoism_product_docs::BundledDoc) -> DocumentationPage {
    DocumentationPage { path: doc.path.to_string(), title: neoism_product_docs::title(doc).to_string(), content: doc.body.to_string() }
}

fn snippet(body: &str, terms: &[String]) -> String {
    body.lines().find(|line| { let lower = line.to_lowercase(); terms.iter().any(|term| lower.contains(term)) })
        .unwrap_or_else(|| body.lines().next().unwrap_or_default()).trim().chars().take(240).collect()
}

fn required_string(arguments: &Value, key: &str) -> Result<String, ServiceError> {
    arguments.get(key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
        .ok_or_else(|| ServiceError::new(format!("{key} is required")))
}

fn tool(name: &str, description: &str, input_schema: Value) -> BuiltinMcpTool {
    BuiltinMcpTool { name: name.to_string(), description: Some(description.to_string()), input_schema, annotations: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_searches_product_owned_docs() {
        assert!(NeoismDocumentationService.read("Start Here.md").unwrap().content.contains("Welcome to Neoism"));
        assert!(NeoismDocumentationService.search("shader", 8).unwrap().iter().any(|hit| hit.path == "Neoism/Appearance.md"));
    }
}