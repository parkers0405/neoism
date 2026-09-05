use std::collections::{BTreeMap, BTreeSet};

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
