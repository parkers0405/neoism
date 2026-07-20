#[path = "lsp_languages/registry.rs"]
mod registry;
#[cfg(test)]
#[path = "lsp_languages/tests.rs"]
mod tests;
#[path = "lsp_languages/types.rs"]
mod types;

#[cfg(test)]
pub(super) use registry::{
    adapter_by_id, best_adapter_for_path, document_language_id_for_path,
};
pub(super) use registry::{
    adapter_supports_catalog_package, logical_language_for_path, LANGUAGE_SPECS,
};
#[cfg(test)]
use types::wildcard_filename_matches;
use types::{CatalogPackageSpec, LanguageRoute};
pub(super) use types::{
    LanguageSpec, LspOperation, LspTransportSpec, WorkspaceRootStrategySpec,
    WorkspaceScan,
};
