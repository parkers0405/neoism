#[path = "lsp_languages/registry.rs"]
mod registry;
#[cfg(test)]
#[path = "lsp_languages/tests.rs"]
mod tests;
#[path = "lsp_languages/types.rs"]
mod types;

#[cfg(test)]
pub(super) use registry::adapter_by_id;
pub(super) use registry::LANGUAGE_SPECS;
use types::{CatalogPackageSpec, LanguageRoute};
pub(super) use types::{LanguageSpec, LspTransportSpec, WorkspaceRootStrategySpec};
