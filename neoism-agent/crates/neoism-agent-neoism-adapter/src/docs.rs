use neoism_agent_service_api::{
    DocumentationPage, DocumentationPageSummary, DocumentationSearchHit,
    DocumentationService, ServiceError,
};

pub(crate) struct NeoismDocumentationService;

impl DocumentationService for NeoismDocumentationService {
    fn version(&self) -> Option<&str> {
        Some(neoism_product_docs::DOCS_VERSION)
    }

    fn list(&self) -> Result<Vec<DocumentationPageSummary>, ServiceError> {
        Ok(neoism_product_docs::BUNDLED_DOCS
            .iter()
            .map(|doc| DocumentationPageSummary {
                path: doc.path.to_string(),
                title: neoism_product_docs::title(doc).to_string(),
            })
            .collect())
    }

    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<DocumentationSearchHit>, ServiceError> {
        let terms = query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        let mut hits = neoism_product_docs::BUNDLED_DOCS
            .iter()
            .filter_map(|doc| {
                let title = neoism_product_docs::title(doc);
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
                        DocumentationSearchHit {
                            path: doc.path.to_string(),
                            title: title.to_string(),
                            snippet: snippet(doc.body, &terms),
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| right.0.cmp(&left.0));
        Ok(hits.into_iter().take(limit).map(|(_, hit)| hit).collect())
    }

    fn read(&self, path: &str) -> Result<DocumentationPage, ServiceError> {
        let normalized = path.trim().trim_start_matches('/');
        neoism_product_docs::bundled_doc(normalized)
            .map(page)
            .ok_or_else(|| {
                ServiceError::new(format!("unknown product documentation page {path}"))
            })
    }
}

fn page(doc: &neoism_product_docs::BundledDoc) -> DocumentationPage {
    DocumentationPage {
        path: doc.path.to_string(),
        title: neoism_product_docs::title(doc).to_string(),
        content: doc.body.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_searches_product_owned_docs() {
        assert!(NeoismDocumentationService
            .read("Start Here.md")
            .unwrap()
            .content
            .contains("Welcome to Neoism"));
        assert!(NeoismDocumentationService
            .search("wallpaper", 8)
            .unwrap()
            .iter()
            .any(|hit| hit.path == "Neoism/Appearance.md"));
    }

    #[test]
    fn shared_bundle_exposes_version_and_searchable_api_reference() {
        let service = NeoismDocumentationService;
        assert_eq!(service.version(), Some(neoism_product_docs::DOCS_VERSION));
        let page = service
            .list()
            .unwrap()
            .into_iter()
            .find(|page| page.path.starts_with("API/"))
            .expect("generated API reference is bundled with guides");
        let body = service.read(&page.path).unwrap();
        assert!(!body.content.is_empty());
        assert!(service
            .search(&page.title, 20)
            .unwrap()
            .iter()
            .any(|hit| hit.path == page.path));
    }

    #[test]
    fn json_render_catalog_is_available_on_demand_without_workspace_files() {
        let service = NeoismDocumentationService;
        let path = "Neoism Agent/JSON Render.md";
        let hint = include_str!("../../neoism-agent-builtins/src/plugin/system_prompt/json_render_hint.md");
        assert!(hint.contains(path));
        assert!(service.list().unwrap().iter().any(|page| page.path == path));
        assert!(service.search("json-render", 1).unwrap().iter().any(|hit| hit.path == path));
        let page = service.read(path).unwrap();
        assert!(page.content.contains("- Diagram:"));
        assert!(page.content.contains("Structured graph data, NOT Mermaid syntax"));
        for expected in ["```json-render", "Stack:", "Card:", "Text:", "Metric:", "Badge:", "Progress:", "LineGraph:", "Separator:", "Tabs:", "Collapsible:", "Filter:", "Input:", "Select:", "Checkbox:", "changeType?", "2 to 32", "finite numbers", "Team performance", "Sample data", "Weekly revenue", "$12,400", "+18%", "Deals closed", "Retention", "32768", "64 elements", "tree depth 8", "no state bindings", "Agent requests belong in the chat composer, NEVER inside generated UI", "Form and Button are not supported", "Local field values are not sent anywhere", "six-digit RGB hex", "#60A5FA"] {
            assert!(page.content.contains(expected), "missing {expected}");
        }
        assert!(hint.len() * 10 < page.content.len());
        assert!(!page.content.contains("- Form:"));
        assert!(!page.content.contains("- Button:"));
    }

    #[test]
    fn skill_authoring_query_finds_complete_instructions() {
        let service = NeoismDocumentationService;
        let hit = service
            .search("skills SKILL.md create custom skill location format", 8)
            .unwrap()
            .into_iter()
            .find(|hit| hit.path == "Neoism Agent/Skills.md")
            .expect("Skills documentation should match authoring queries");
        let page = service.read(&hit.path).unwrap();
        for required in [
            "<project>/.neoism/skills/database-migrations/SKILL.md",
            "~/.config/neoism/skills/database-migrations/SKILL.md",
            "name: database-migrations",
            "description:",
            "agent.skills.paths",
            "/skills",
        ] {
            assert!(page.content.contains(required), "missing {required}");
        }
    }
}
