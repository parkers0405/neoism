// File / grep search-mode enums (fuzzy / exact / regex).

use crate::services::{SearchFileMode, SearchGrepMode};

/// What the finder is searching over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinderMode {
    /// `rg --files`-collected paths, fuzzy-filtered in-memory.
    Files,
    /// `rg <query>` — re-run on each query change (debounced).
    Grep,
    /// Git porcelain changed files for the current repository.
    #[allow(dead_code)]
    GitChanges,
    /// In-buffer line search over the active code pane's lines
    /// (nvim `/`). Plain case-sensitive substring, fully in-memory —
    /// no ripgrep, no services.
    BufferLines,
    /// In-buffer search & replace over the active code pane (the
    /// Ctrl+P-style replace surface): the query is typed as
    /// `pattern/replacement` (`:s` escaping); rows list lines matching
    /// the PATTERN part; Enter replaces every occurrence in the file.
    BufferReplace,
    /// LSP find-references results (`gr` on the code pane). Rows are
    /// pre-computed `path:line  text` hits installed by the host;
    /// typing fuzzy-filters them in-memory — no services.
    References,
    /// LSP document symbols of the active code pane (VS Code Ctrl+P
    /// `@`). Rows are pre-computed `{kind} {name} {line}` entries
    /// installed by the host; typing fuzzy-filters them in-memory —
    /// no services.
    Symbols,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum FileSearchMode {
    #[default]
    Fuzzy,
    Exact,
}

impl FileSearchMode {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Fuzzy => Self::Exact,
            Self::Exact => Self::Fuzzy,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Fuzzy => "fuzzy",
            Self::Exact => "exact",
        }
    }

    pub(super) fn as_service_mode(self) -> SearchFileMode {
        match self {
            Self::Fuzzy => SearchFileMode::Fuzzy,
            Self::Exact => SearchFileMode::Exact,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum GrepSearchMode {
    #[default]
    Fuzzy,
    Exact,
    Regex,
}

impl GrepSearchMode {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Fuzzy => Self::Exact,
            Self::Exact => Self::Regex,
            Self::Regex => Self::Fuzzy,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Fuzzy => "fuzzy",
            Self::Exact => "exact",
            Self::Regex => "regex",
        }
    }

    pub(super) fn as_service_mode(self) -> SearchGrepMode {
        match self {
            Self::Fuzzy => SearchGrepMode::Fuzzy,
            Self::Exact => SearchGrepMode::Exact,
            Self::Regex => SearchGrepMode::Regex,
        }
    }
}
