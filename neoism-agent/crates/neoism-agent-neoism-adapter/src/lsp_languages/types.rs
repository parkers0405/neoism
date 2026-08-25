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
