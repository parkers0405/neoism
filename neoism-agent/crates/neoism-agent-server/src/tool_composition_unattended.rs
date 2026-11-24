//! Unattended composition deliberately does not use the interactive dispatch
//! bridge. Only the read-only built-in documentation listing is supported.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use futures::future::BoxFuture;
use neoism_agent_composition::{ExecutionResult, Limits, ToolDescriptor, ToolHost};
use neoism_agent_core::{PermissionAction, PermissionRule};
use neoism_agent_service_api::DocumentationService;
use serde_json::{json, Value};

use crate::{state::AppState, workspace_runtime::PluginGenerationLease};

const DOCS_ID: &str = "docs";
const DOCS_PATH: &str = "neoism.docs";
const DOCS_TARGET: &str = "*"; // The parameter-free listing targets the whole catalog.
const MAX_RESULT_BYTES: usize = 16 * 1024;
const MAX_SOURCE_BYTES: usize = 8 * 1024;

pub(crate) fn validate_inputs(code: &str, tool_ids: &[String]) -> Result<(), String> {
    if !matches!(tool_ids, [id] if id == DOCS_ID) {
        return Err("unattended composition requires the exact tool ID docs".into());
    }
    if code.len() > MAX_SOURCE_BYTES || code.trim().is_empty() || code.contains('\0') {
        return Err("invalid unattended composition source".into());
    }
    Ok(())
}

/// The caller must supply a live generation for the admitted workspace and a
/// trusted effective permission snapshot. Neither may come from workflow text.
/// This seam is for the shipped bundled documentation service; embedders
/// must not wire it with a service that performs I/O or unbounded work.
/// This runs no model and cannot call the general tool executor, even if a
/// registry entry is replaced. Authorization to run the workflow is separate.
pub(crate) async fn run(
    state: &AppState,
    generation: PluginGenerationLease,
    permissions: &[PermissionRule],
    tool_ids: &[String],
    code: &str,
    cancel: Arc<AtomicBool>,
) -> Result<ExecutionResult, String> {
    validate_inputs(code, tool_ids)?;
    if cancel.load(Ordering::SeqCst) {
        return Err("unattended composition cancelled".into());
    }
    let host = Host::admit(state, generation, permissions, tool_ids)?;
    Ok(neoism_agent_composition::execute(code, Arc::new(host), limits(), cancel).await)
}

pub(super) fn limits() -> Limits {
    Limits {
        wall_clock: Duration::from_secs(5),
        max_steps: 20_000,
        max_calls: 4,
        concurrency: 1,
        max_source_bytes: MAX_SOURCE_BYTES,
        max_output_bytes: 32 * 1024,
        max_log_bytes: 4 * 1024,
        max_data_bytes: 2 * 1024 * 1024,
        max_depth: 32,
        cleanup_timeout: Duration::from_millis(100),
    }
}

#[derive(Clone)]
struct Host {
    generation: PluginGenerationLease,
    permissions: Arc<[PermissionRule]>,
    docs: Arc<dyn DocumentationService>,
}

impl Host {
    fn admit(
        state: &AppState,
        generation: PluginGenerationLease,
        permissions: &[PermissionRule],
        tool_ids: &[String],
    ) -> Result<Self, String> {
        // Reject the entire request rather than silently running with a subset.
        // This also rejects duplicates, runtime paths, wildcards, and aliases.
        if !matches!(tool_ids, [id] if id == DOCS_ID) {
            return Err("unattended composition requires the exact tool ID docs".into());
        }
        if state.services().hosted || state.services().workspace_worker.is_some() {
            return Err("unattended composition requires a local runtime".into());
        }
        Self::check(&generation, permissions)?;
        let docs = state
            .services()
            .documentation
            .clone()
            .ok_or_else(|| "unattended docs capability unavailable".to_string())?;
        Ok(Self {
            generation,
            permissions: Arc::from(permissions),
            docs,
        })
    }

    fn check(
        generation: &PluginGenerationLease,
        permissions: &[PermissionRule],
    ) -> Result<(), String> {
        generation
            .ensure_active()
            .map_err(|error| error.to_string())?;
        if !generation.runtime_tools.contains_key(DOCS_ID)
            || !crate::agent_tool_registry::tool_contribution(generation, DOCS_ID)
                .is_some_and(|tool| {
                    tool.plugin_id
                        == neoism_agent_builtins::plugin::documentation_tools::ID
                })
        {
            return Err("unattended docs capability unavailable".into());
        }
        // The winning effective rule itself must be explicit. A later global
        // Allow cannot stand in for an exact grant; Ask never prompts here.
        let rule = crate::permission::evaluate(DOCS_ID, DOCS_TARGET, permissions);
        if rule.permission != DOCS_ID
            || rule.pattern != DOCS_TARGET
            || rule.action != PermissionAction::Allow
        {
            return Err("unattended docs capability requires explicit permission".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_service_api::{
        DocumentationPage, DocumentationPageSummary, DocumentationSearchHit, ServiceError,
    };
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Docs(AtomicUsize);

    impl DocumentationService for Docs {
        fn list(&self) -> Result<Vec<DocumentationPageSummary>, ServiceError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok((0..500)
                .map(|index| DocumentationPageSummary {
                    path: format!("Page {index}.md"),
                    title: format!("Page {index}"),
                })
                .collect())
        }

        fn search(
            &self,
            _: &str,
            _: usize,
        ) -> Result<Vec<DocumentationSearchHit>, ServiceError> {
            panic!("unattended composition must not reach search")
        }

        fn read(&self, _: &str) -> Result<DocumentationPage, ServiceError> {
            panic!("unattended composition must not reach read")
        }
    }

    fn rule(permission: &str, action: PermissionAction) -> PermissionRule {
        PermissionRule {
            permission: permission.into(),
            pattern: "*".into(),
            action,
        }
    }

    #[tokio::test]
    async fn unattended_composition_catalog_and_dispatch_are_exact_and_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        let docs = Arc::new(Docs::default());
        let state = AppState::open_database_with_services(
            dir.path().join("state.db"),
            crate::standard_services().with_documentation(docs.clone()),
        )
        .await
        .unwrap();
        let snapshot =
            crate::agent_tool_registry::acquire_workspace_plugin_snapshot(&state, root)
                .await
                .unwrap()
                .snapshot;
        let permit = rule(DOCS_ID, PermissionAction::Allow);
        let ids = [DOCS_ID.to_owned()];
        let cancel = Arc::new(AtomicBool::new(false));
        let admit = |rules: &[PermissionRule], ids: &[String]| {
            Host::admit(&state, snapshot.clone(), rules, ids)
        };

        assert!(admit(&[], &ids).is_err());
        assert!(admit(&[rule("*", PermissionAction::Allow)], &ids).is_err());
        for action in [PermissionAction::Ask, PermissionAction::Deny] {
            let rules = [permit.clone(), rule(DOCS_ID, action)];
            assert!(admit(&rules, &ids).is_err());
            let broad_override = [
                rules[0].clone(),
                rules[1].clone(),
                rule("*", PermissionAction::Allow),
            ];
            assert!(admit(&broad_override, &ids).is_err());
            let mut denied = admit(std::slice::from_ref(&permit), &ids).unwrap();
            denied.permissions = Arc::from(rules);
            assert!(denied.catalog().is_empty());
            assert!(denied
                .call(
                    DOCS_PATH.into(),
                    json!({"operation": "list"}),
                    cancel.clone(),
                )
                .await
                .is_err());
        }
        for ids in [
            vec![],
            vec!["neoism.docs".into()],
            vec!["*".into()],
            vec![DOCS_ID.into(), DOCS_ID.into()],
            vec![DOCS_ID.into(), "mcp__sample__list".into()],
        ] {
            assert!(admit(std::slice::from_ref(&permit), &ids).is_err());
        }
        let rules = [permit];
        let host = admit(&rules, &ids).unwrap();
        assert_eq!(
            host.catalog()
                .into_iter()
                .map(|tool| tool.path)
                .collect::<Vec<_>>(),
            [DOCS_PATH]
        );
        for (path, input) in [
            ("neoism.bash", json!({"operation": "list"})),
            ("docs", json!({"operation": "list"})),
            (
                DOCS_PATH,
                json!({"operation": "read", "path": "Start Here.md"}),
            ),
            (DOCS_PATH, json!({"operation": "list", "extra": true})),
        ] {
            assert!(host.call(path.into(), input, cancel.clone()).await.is_err());
        }
        assert_eq!(docs.0.load(Ordering::SeqCst), 0);
        let result = run(
            &state,
            snapshot.clone(),
            &rules,
            &ids,
            "return await tools.neoism.docs({operation: 'list'});",
            cancel.clone(),
        )
        .await
        .unwrap();
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(result.value["documents"][0]["path"], "Page 0.md");
        assert_eq!(result.value["documents"].as_array().unwrap().len(), 256);
        assert_eq!(result.value["truncated"], true);
        let discovery = run(
            &state,
            snapshot.clone(),
            &rules,
            &ids,
            "return search({});",
            cancel.clone(),
        )
        .await
        .unwrap();
        assert!(discovery.ok, "{:?}", discovery.error);
        assert_eq!(discovery.value["total"], 1);
        assert_eq!(discovery.value["tools"][0]["path"], "tools.neoism.docs");
        let result = run(
            &state,
            snapshot,
            &rules,
            &ids,
            "return await tools.neoism.bash({operation: 'list'});",
            cancel,
        )
        .await
        .unwrap();
        assert!(!result.ok);
        assert_eq!(docs.0.load(Ordering::SeqCst), 1);

        let neutral_dir = tempfile::tempdir().unwrap();
        let neutral = AppState::open_database(neutral_dir.path().join("state.db"))
            .await
            .unwrap();
        let neutral_snapshot =
            crate::agent_tool_registry::acquire_workspace_plugin_snapshot(
                &neutral,
                neutral_dir.path().to_str().unwrap(),
            )
            .await
            .unwrap()
            .snapshot;
        assert!(Host::admit(&neutral, neutral_snapshot.clone(), &rules, &ids).is_err());
        let unavailable = Host {
            generation: neutral_snapshot,
            ..host
        };
        assert!(unavailable.catalog().is_empty());
        assert!(unavailable
            .call(
                DOCS_PATH.into(),
                json!({"operation": "list"}),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .is_err());
        assert_eq!(docs.0.load(Ordering::SeqCst), 1);
    }
}

impl ToolHost for Host {
    fn catalog(&self) -> Vec<ToolDescriptor> {
        if Self::check(&self.generation, &self.permissions).is_err() {
            return Vec::new();
        }
        vec![ToolDescriptor {
            path: DOCS_PATH.into(),
            description: "List bundled product documentation titles and paths; no file reads or search.".into(),
            signature: "neoism.docs(input: { operation: 'list' }): Promise<{ documents: { path: string; title: string }[], truncated: boolean }>".into(),
        }]
    }

    fn call(
        &self,
        path: String,
        input: Value,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        let host = self.clone();
        Box::pin(async move {
            if path != DOCS_PATH || input != json!({"operation": "list"}) {
                return Err("unattended capability or input not admitted".into());
            }
            Host::check(&host.generation, &host.permissions)?;
            if cancel.load(Ordering::SeqCst) {
                return Err("unattended composition cancelled".into());
            }
            // This host-owned service is not selected from the plugin registry.
            // The shipped service enumerates embedded metadata only. Never pass
            // interpreter-controlled paths or operations to the service.
            let pages = host
                .docs
                .list()
                .map_err(|_| "unattended docs listing failed".to_string())?;
            let total = pages.len();
            let mut bytes = super::serialized_size(
                &json!({ "documents": [], "truncated": false }),
                MAX_RESULT_BYTES,
            )?;
            let mut documents = Vec::with_capacity(total.min(256));
            for page in pages.into_iter().take(256) {
                let document = json!({ "path": page.path, "title": page.title });
                let Ok(size) = super::serialized_size(&document, MAX_RESULT_BYTES) else {
                    break;
                };
                bytes += size + usize::from(!documents.is_empty());
                if bytes > MAX_RESULT_BYTES {
                    break;
                }
                documents.push(document);
            }
            let result = json!({"truncated": documents.len() < total, "documents": documents});
            super::serialized_size(&result, MAX_RESULT_BYTES)?;
            Ok(result)
        })
    }
}
