use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// How an adapter is reached. Transport is adapter metadata, not a language
/// special case in the client: Godot happens to expose TCP while the other
/// built-ins currently speak over a child process' stdio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::lsp) enum LspTransportSpec {
    Stdio {
        command: &'static [&'static str],
    },
    Tcp {
        default_host: &'static str,
        default_port: u16,
        host_env: Option<&'static str>,
        port_env: Option<&'static str>,
    },
}

/// Declarative policy for selecting the folder passed to an LSP server.
///
/// Most servers want the closest directory containing one of their root
/// markers. Cargo workspaces are different: the closest `Cargo.toml` may be a
/// member manifest, while rust-analyzer needs Cargo's resolved workspace root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::lsp) enum WorkspaceRootStrategySpec {
    NearestMarker,
    CargoMetadata { manifest: &'static str },
}

/// One document-language route owned by an adapter.
///
/// `id` is Neoism's stable logical language name. `document_language_id` is
/// the value sent in LSP `TextDocumentItem.languageId`; keeping them separate
/// is important for servers such as vscode-css-language-server (CSS/SCSS/Less)
/// and typescript-language-server (TypeScript/TSX/JavaScript/JSX).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::lsp) struct LanguageRoute {
    pub(in crate::lsp) id: &'static str,
    pub(in crate::lsp) document_language_id: &'static str,
    pub(in crate::lsp) extensions: &'static [&'static str],
    pub(in crate::lsp) filename_patterns: &'static [&'static str],
}

/// An installable catalog package that provides the executable used by an
/// adapter. Package identity and executable identity are deliberately
/// separate: Mason's `json-lsp` package exposes
/// `vscode-json-language-server`, while `pyright` exposes
/// `pyright-langserver`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::lsp) struct CatalogPackageSpec {
    pub(in crate::lsp) package_id: &'static str,
    pub(in crate::lsp) executable: &'static str,
}

impl LanguageRoute {
    pub(in crate::lsp) fn match_priority(&self, path: &Path) -> Option<usize> {
        let file_name = path.file_name()?.to_str()?;
        let exact = self
            .filename_patterns
            .iter()
            .filter(|pattern| !pattern.contains('*'))
            .any(|pattern| file_name.eq_ignore_ascii_case(pattern));
        if exact {
            return Some(300 + file_name.len());
        }

        let glob_specificity = self
            .filename_patterns
            .iter()
            .filter(|pattern| pattern.contains('*'))
            .filter(|pattern| wildcard_filename_matches(pattern, file_name))
            .map(|pattern| 200 + pattern.bytes().filter(|byte| *byte != b'*').count())
            .max();
        if glob_specificity.is_some() {
            return glob_specificity;
        }

        let extension = path.extension()?.to_str()?;
        self.extensions
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            .then_some(100)
    }
}

/// A server adapter is process/connection metadata plus all document routes
/// that share that server instance. Package catalogs may supply the binary,
/// but this registry is the runtime source of truth for whether and how it can
/// attach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::lsp) struct LanguageSpec {
    /// Stable adapter/server identity. This is deliberately not languageId.
    pub(in crate::lsp) id: &'static str,
    pub(in crate::lsp) name: &'static str,
    pub(in crate::lsp) catalog_packages: &'static [CatalogPackageSpec],
    pub(in crate::lsp) transport: LspTransportSpec,
    pub(in crate::lsp) routes: &'static [LanguageRoute],
    pub(in crate::lsp) markers: &'static [&'static str],
    pub(in crate::lsp) root_strategy: WorkspaceRootStrategySpec,
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
}

impl LanguageSpec {
    pub(in crate::lsp) fn route_for_path(
        &self,
        path: &Path,
    ) -> Option<&'static LanguageRoute> {
        best_route_in(self.routes, path)
    }

    pub(in crate::lsp) fn match_priority(&self, path: &Path) -> Option<usize> {
        self.routes
            .iter()
            .filter_map(|route| route.match_priority(path))
            .max()
    }

    #[cfg(test)]
    pub(in crate::lsp) fn language_id_for_path(
        &self,
        path: &Path,
    ) -> Option<&'static str> {
        self.route_for_path(path)
            .map(|route| route.document_language_id)
    }

    pub(in crate::lsp) fn logical_language_for_path(
        &self,
        path: &Path,
    ) -> Option<&'static str> {
        self.route_for_path(path).map(|route| route.id)
    }

    /// Match a catalog row by both its stable package id and the executable it
    /// exposes. Command-only matching is ambiguous and can advertise an
    /// install that the selected adapter cannot actually consume.
    pub(in crate::lsp) fn supports_catalog_package(
        &self,
        package_id: &str,
        command: &str,
    ) -> bool {
        let executable = normalized_executable(command);
        self.catalog_packages.iter().any(|package| {
            package.package_id.eq_ignore_ascii_case(package_id)
                && package.executable.eq_ignore_ascii_case(executable.as_str())
        })
    }
}

fn normalized_executable(command: &str) -> String {
    // Accept catalog commands serialized on a different host OS as well
    // (`C:\\...\\server.exe` can be inspected on Unix and vice versa).
    let executable = command
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(command);
    [".exe", ".cmd", ".bat"]
        .iter()
        .find_map(|suffix| {
            executable
                .to_ascii_lowercase()
                .strip_suffix(suffix)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| executable.to_string())
}

pub(in crate::lsp) fn best_route_in<'a>(
    routes: &'a [LanguageRoute],
    path: &Path,
) -> Option<&'a LanguageRoute> {
    routes
        .iter()
        .filter_map(|route| route.match_priority(path).map(|score| (score, route)))
        .max_by_key(|(score, _)| *score)
        .map(|(_, route)| route)
}

pub(super) fn wildcard_filename_matches(pattern: &str, value: &str) -> bool {
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

#[derive(Clone, Debug, Default)]
pub(in crate::lsp) struct WorkspaceScan {
    pub(in crate::lsp) files: usize,
    pub(in crate::lsp) extensions: BTreeMap<String, usize>,
    pub(in crate::lsp) markers: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::lsp) enum LspOperation {
    WorkspaceSymbols,
    Completion,
    SignatureHelp,
    Hover,
    Definition,
    References,
    Implementation,
    CallHierarchy,
    DocumentHighlight,
    InlayHints,
    Diagnostics,
    DocumentSymbols,
    Formatting,
    CodeActions,
    Rename,
}
