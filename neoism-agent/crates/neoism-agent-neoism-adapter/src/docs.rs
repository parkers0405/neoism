use neoism_agent_service_api::{
    DocumentationPage, DocumentationPageSummary, DocumentationSearchHit,
    DocumentationService, ServiceError,
};

pub(crate) struct NeoismDocumentationService;

impl DocumentationService for NeoismDocumentationService {
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
            .search("shader", 8)
            .unwrap()
            .iter()
            .any(|hit| hit.path == "Neoism/Appearance.md"));
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
