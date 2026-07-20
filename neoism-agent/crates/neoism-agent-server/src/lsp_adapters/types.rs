use std::{collections::BTreeMap, env, path::Path};

use serde_json::Value;

use super::super::lsp_languages::{
    LanguageSpec as BuiltinLanguageSpec, LspTransportSpec, WorkspaceRootStrategySpec,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::lsp) struct ResolvedLanguageRoute {
    pub(in crate::lsp) id: String,
    pub(in crate::lsp) document_language_id: String,
    pub(in crate::lsp) extensions: Vec<String>,
    pub(in crate::lsp) filename_patterns: Vec<String>,
}

impl ResolvedLanguageRoute {
    pub(in crate::lsp) fn match_priority(&self, path: &Path) -> Option<usize> {
        let file_name = path.file_name()?.to_str()?;
        if self
            .filename_patterns
            .iter()
            .filter(|pattern| !pattern.contains('*'))
            .any(|pattern| file_name.eq_ignore_ascii_case(pattern))
        {
            return Some(300 + file_name.len());
        }
        if let Some(score) = self
            .filename_patterns
            .iter()
            .filter(|pattern| pattern.contains('*'))
            .filter(|pattern| wildcard_filename_matches(pattern, file_name))
            .map(|pattern| 200 + pattern.bytes().filter(|byte| *byte != b'*').count())
            .max()
        {
            return Some(score);
        }
        let extension = path.extension()?.to_str()?;
        self.extensions
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            .then_some(100)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::lsp) enum ResolvedLspTransport {
    Stdio {
        command: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Tcp {
        host: String,
        port: u16,
        built_in: bool,
    },
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::lsp) enum AdapterOrigin {
    BuiltIn,
    Configured,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::lsp) enum WorkspaceRootStrategy {
    NearestMarker,
    CargoMetadata { manifest: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::lsp) struct ResolvedCatalogPackage {
    pub(in crate::lsp) package_id: String,
    pub(in crate::lsp) executable: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::lsp) struct LanguageAdapter {
    pub(in crate::lsp) id: String,
    pub(in crate::lsp) name: String,
    pub(in crate::lsp) catalog_packages: Vec<ResolvedCatalogPackage>,
    pub(in crate::lsp) transport: ResolvedLspTransport,
    pub(in crate::lsp) routes: Vec<ResolvedLanguageRoute>,
    pub(in crate::lsp) markers: Vec<String>,
    pub(in crate::lsp) root_strategy: WorkspaceRootStrategy,
    pub(in crate::lsp) workspace_symbols: bool,
    pub(in crate::lsp) completion: bool,
    pub(in crate::lsp) hover: bool,
    pub(in crate::lsp) definition: bool,
    pub(in crate::lsp) references: bool,
    pub(in crate::lsp) implementation: bool,
    pub(in crate::lsp) call_hierarchy: bool,
    pub(in crate::lsp) diagnostics: bool,
    pub(in crate::lsp) document_symbols: bool,
    pub(in crate::lsp) formatting: bool,
    pub(in crate::lsp) code_actions: bool,
    pub(in crate::lsp) rename: bool,
    pub(in crate::lsp) initialization_options: Option<Value>,
    pub(in crate::lsp) settings: Option<Value>,
    pub(in crate::lsp) origin: AdapterOrigin,
    pub(in crate::lsp) configuration_error: Option<String>,
}

impl LanguageAdapter {
    pub(in crate::lsp) fn from_builtin(spec: &BuiltinLanguageSpec) -> Self {
        let transport = match spec.transport {
            LspTransportSpec::Stdio { command } => ResolvedLspTransport::Stdio {
                command: command.iter().map(|part| (*part).to_string()).collect(),
                env: BTreeMap::new(),
            },
            LspTransportSpec::Tcp {
                default_host,
                default_port,
                host_env,
                port_env,
            } => {
                let host = host_env
                    .and_then(|name| env::var(name).ok())
                    .filter(|host| !host.trim().is_empty())
                    .unwrap_or_else(|| default_host.to_string());
                let port = port_env
                    .and_then(|name| env::var(name).ok())
                    .and_then(|port| port.parse::<u16>().ok())
                    .unwrap_or(default_port);
                ResolvedLspTransport::Tcp {
                    host,
                    port,
                    built_in: true,
                }
            }
        };
        Self {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            catalog_packages: spec
                .catalog_packages
                .iter()
                .map(|package| ResolvedCatalogPackage {
                    package_id: package.package_id.to_string(),
                    executable: package.executable.to_string(),
                })
                .collect(),
            transport,
            routes: spec
                .routes
                .iter()
                .map(|route| ResolvedLanguageRoute {
                    id: route.id.to_string(),
                    document_language_id: route.document_language_id.to_string(),
                    extensions: route
                        .extensions
                        .iter()
                        .map(|extension| (*extension).to_string())
                        .collect(),
                    filename_patterns: route
                        .filename_patterns
                        .iter()
                        .map(|pattern| (*pattern).to_string())
                        .collect(),
                })
                .collect(),
            markers: spec
                .markers
                .iter()
                .map(|marker| (*marker).to_string())
                .collect(),
            root_strategy: match spec.root_strategy {
                WorkspaceRootStrategySpec::NearestMarker => {
                    WorkspaceRootStrategy::NearestMarker
                }
                WorkspaceRootStrategySpec::CargoMetadata { manifest } => {
                    WorkspaceRootStrategy::CargoMetadata {
                        manifest: manifest.to_string(),
                    }
                }
            },
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
            initialization_options: None,
            settings: None,
            origin: AdapterOrigin::BuiltIn,
            configuration_error: None,
        }
    }

    pub(super) fn empty_configured(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            catalog_packages: Vec::new(),
            transport: ResolvedLspTransport::Invalid,
            routes: Vec::new(),
            markers: Vec::new(),
            root_strategy: WorkspaceRootStrategy::NearestMarker,
            workspace_symbols: true,
            completion: true,
            hover: true,
            definition: true,
            references: true,
            implementation: true,
            call_hierarchy: true,
            diagnostics: true,
            document_symbols: true,
            formatting: true,
            code_actions: true,
            rename: true,
            initialization_options: None,
            settings: None,
            origin: AdapterOrigin::Configured,
            configuration_error: None,
        }
    }

    pub(in crate::lsp) fn route_for_path(
        &self,
        path: &Path,
    ) -> Option<&ResolvedLanguageRoute> {
        best_route_in(&self.routes, path)
    }

    pub(in crate::lsp) fn match_priority(&self, path: &Path) -> Option<usize> {
        self.routes
            .iter()
            .filter_map(|route| route.match_priority(path))
            .max()
    }

    pub(in crate::lsp) fn matches_path(&self, path: &Path) -> bool {
        self.match_priority(path).is_some()
    }

    pub(in crate::lsp) fn logical_language_for_path(&self, path: &Path) -> Option<&str> {
        self.route_for_path(path).map(|route| route.id.as_str())
    }

    #[cfg(test)]
    pub(in crate::lsp) fn document_language_id_for_path(
        &self,
        path: &Path,
    ) -> Option<&str> {
        self.route_for_path(path)
            .map(|route| route.document_language_id.as_str())
    }

    pub(in crate::lsp) fn extensions(&self) -> impl Iterator<Item = &str> {
        self.routes
            .iter()
            .flat_map(|route| route.extensions.iter().map(String::as_str))
    }

    pub(in crate::lsp) fn filename_patterns(&self) -> impl Iterator<Item = &str> {
        self.routes
            .iter()
            .flat_map(|route| route.filename_patterns.iter().map(String::as_str))
    }

    pub(in crate::lsp) fn filename_matches(&self, file_name: &str) -> bool {
        self.filename_patterns().any(|pattern| {
            if pattern.contains('*') {
                wildcard_filename_matches(pattern, file_name)
            } else {
                file_name.eq_ignore_ascii_case(pattern)
            }
        })
    }

    pub(in crate::lsp) fn is_valid(&self) -> bool {
        self.configuration_error.is_none()
            && !self.routes.is_empty()
            && !matches!(self.transport, ResolvedLspTransport::Invalid)
    }

    pub(in crate::lsp) fn command(&self) -> &[String] {
        match &self.transport {
            ResolvedLspTransport::Stdio { command, .. } => command,
            ResolvedLspTransport::Tcp { .. } | ResolvedLspTransport::Invalid => &[],
        }
    }
}

pub(in crate::lsp) fn best_route_in<'a>(
    routes: &'a [ResolvedLanguageRoute],
    path: &Path,
) -> Option<&'a ResolvedLanguageRoute> {
    routes
        .iter()
        .filter_map(|route| route.match_priority(path).map(|score| (score, route)))
        .max_by_key(|(score, _)| *score)
        .map(|(_, route)| route)
}

fn wildcard_filename_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return value == pattern;
    }
    let mut cursor = 0;
    if let Some(first) = parts.first().filter(|part| !part.is_empty()) {
        if !value.starts_with(first) {
            return false;
        }
        cursor = first.len();
    }
    for (index, part) in parts.iter().enumerate().skip(1) {
        if part.is_empty() {
            continue;
        }
        let is_last = index + 1 == parts.len();
        if is_last && !pattern.ends_with('*') {
            return value[cursor..].ends_with(part);
        }
        let Some(found) = value[cursor..].find(part) else {
            return false;
        };
        cursor += found + part.len();
    }
    true
}
