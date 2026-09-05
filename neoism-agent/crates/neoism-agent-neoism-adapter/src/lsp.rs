#[path = "lsp_languages.rs"]
mod lsp_languages;

use std::sync::Arc;

use neoism_agent_service_api::{
    LanguageCapabilityService, LanguageCapabilitySnapshot, LanguageCatalogPackage,
    LanguageRootPolicy, LanguageRouteCapability, LanguageServerCapability,
    LanguageServerOperations, LanguageServerTransport,
};

use lsp_languages::{LspTransportSpec, WorkspaceRootStrategySpec, LANGUAGE_SPECS};

#[derive(Clone)]
pub(crate) struct NeoismLanguageCapabilityService {
    snapshot: Arc<LanguageCapabilitySnapshot>,
}

impl NeoismLanguageCapabilityService {
    pub(crate) fn new() -> Self {
        let languages = LANGUAGE_SPECS
            .iter()
            .map(|spec| LanguageServerCapability {
                id: spec.id.to_string(),
                name: spec.name.to_string(),
                catalog_packages: spec
                    .catalog_packages
                    .iter()
                    .map(|package| LanguageCatalogPackage {
                        package_id: package.package_id.to_string(),
                        executable: package.executable.to_string(),
                    })
                    .collect(),
                transport: match spec.transport {
                    LspTransportSpec::Stdio { command } => {
                        LanguageServerTransport::Stdio {
                            command: command
                                .iter()
                                .map(|part| (*part).to_string())
                                .collect(),
                        }
                    }
                    LspTransportSpec::Tcp {
                        default_host,
                        default_port,
                        host_env,
                        port_env,
                    } => LanguageServerTransport::Tcp {
                        default_host: default_host.to_string(),
                        default_port,
                        host_env: host_env.map(str::to_string),
                        port_env: port_env.map(str::to_string),
                    },
                },
                routes: spec
                    .routes
                    .iter()
                    .map(|route| LanguageRouteCapability {
                        id: route.id.to_string(),
                        document_language_id: route.document_language_id.to_string(),
                        extensions: route
                            .extensions
                            .iter()
                            .map(|value| (*value).to_string())
                            .collect(),
                        filename_patterns: route
                            .filename_patterns
                            .iter()
                            .map(|value| (*value).to_string())
                            .collect(),
                    })
                    .collect(),
                markers: spec
                    .markers
                    .iter()
                    .map(|marker| (*marker).to_string())
                    .collect(),
                root_policy: match spec.root_strategy {
                    WorkspaceRootStrategySpec::NearestMarker => {
                        LanguageRootPolicy::NearestMarker
                    }
                    WorkspaceRootStrategySpec::CargoMetadata { manifest } => {
                        LanguageRootPolicy::CargoMetadata {
                            manifest: manifest.to_string(),
                        }
                    }
                },
                capabilities: LanguageServerOperations {
                    workspace_symbols: spec.workspace_symbols,
                    completion: spec.completion,
                    hover: spec.hover,
                    definition: spec.definition,
                    references: spec.references,
                    implementation: spec.implementation,
                    call_hierarchy: spec.call_hierarchy,
                    diagnostics: spec.diagnostics,
                    document_symbols: spec.document_symbols,
                    formatting: spec.formatting,
                    code_actions: spec.code_actions,
                    rename: spec.rename,
                },
            })
            .collect::<Vec<_>>();
        Self {
            snapshot: Arc::new(LanguageCapabilitySnapshot {
                generation: 1,
                languages: Arc::from(languages),
            }),
        }
    }
}

impl LanguageCapabilityService for NeoismLanguageCapabilityService {
    fn snapshot(&self) -> Arc<LanguageCapabilitySnapshot> {
        self.snapshot.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neoism_snapshot_preserves_the_owned_catalog() {
        let snapshot = NeoismLanguageCapabilityService::new().snapshot();
        assert_eq!(snapshot.languages.len(), LANGUAGE_SPECS.len());
        for (projected, original) in snapshot.languages.iter().zip(LANGUAGE_SPECS) {
            assert_eq!(projected.id, original.id);
            assert_eq!(projected.routes.len(), original.routes.len());
            assert_eq!(
                projected.catalog_packages.len(),
                original.catalog_packages.len()
            );
        }
    }
}
