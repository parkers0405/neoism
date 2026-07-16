// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Palette mode + row shapes consumed by the filter / render pipelines.

use super::actions::{
    HostKind, PaletteAction, PaletteBufferEntry, PaletteServerEntry, PaletteShaderEntry,
    PaletteWorkspaceEntry,
};

/// What the palette is currently browsing and filtering over.
///
/// `Commands` is the default — fuzzy-matches against the static
/// `COMMANDS` list and dispatches a `PaletteAction` on Enter.
///
/// `Fonts` is entered via the `ListFonts` command. The palette stays
/// open, its content is replaced with the owned list of font family
/// names, and Enter closes the palette (no font-switching action yet).
/// The list is owned so the filter pass doesn't keep a borrow on the
/// sugarloaf FontLibrary.
pub(crate) enum PaletteMode {
    Commands,
    Fonts(Vec<String>),
    Themes(Vec<String>),
    Shaders(Vec<PaletteShaderEntry>),
    Buffers(Vec<PaletteBufferEntry>),
    Workspaces(Vec<PaletteWorkspaceEntry>),
    Servers(Vec<PaletteServerEntry>),
    /// Vim-style ex-command capture. The shared command palette is the
    /// default `:` surface now; this mode remains for explicit raw-nvim
    /// command capture paths (e.g. `Go to Line…`).
    Ex,
    /// Forward `/` search capture. Mirrors `Ex` but Enter goes through
    /// the managed nvim search bridge so live preview and commit use
    /// the same literal matching rules.
    Search,
}

/// One row in the filtered result list. Variants carry exactly the
/// data the render pass needs — no `&'static Command` vs `&str`
/// lifetime mixing.
pub(crate) enum PaletteRow<'a> {
    Command {
        /// Zed-style service namespace prefix (e.g. `nvim`, `markdown`,
        /// `neoism-agent`). Rendered as `{service}: {title}` via
        /// [`PaletteRow::display_title`].
        service: &'a str,
        title: &'a str,
        shortcut: &'a str,
        action: PaletteAction,
    },
    Font {
        family: &'a str,
    },
    Theme {
        name: &'a str,
    },
    Shader {
        entry: &'a PaletteShaderEntry,
    },
    Buffer {
        entry: &'a PaletteBufferEntry,
    },
    /// Host header separator in the grouped Workspaces tree. Renders the
    /// kind glyph, the host label, an online dot, and (for non-local
    /// hosts) the `daemon_url`. It is **non-selectable** — the selection
    /// pipeline skips it, mirroring how a folder-group label sits above
    /// its file children. The drag target for 5D-drag will be this row.
    WorkspaceHost {
        /// Owning host id. Read by the future 5D-drag drop handler (a
        /// `Workspace` dragged onto this header issues a move to
        /// `host_id`); unused by render/selection today.
        #[allow(dead_code)]
        host_id: &'a str,
        label: &'a str,
        kind: HostKind,
        daemon_url: Option<&'a str>,
        online: bool,
    },
    WorkspaceCreate,
    Workspace {
        entry: &'a PaletteWorkspaceEntry,
    },
    Server {
        entry: &'a PaletteServerEntry,
    },
    ServerAdd,
    /// "+ Create server" — host the machine's own daemon for a chosen
    /// folder and auto-join it (the flip side of ServerAdd's join-by-URL).
    ServerCreate,
    /// Vim ex command suggestion — a pure visual aid backed by the
    /// curated `EX_COMMANDS` list. Has no action: Enter always
    /// dispatches the literal query, Tab fills the query with `name`.
    Ex {
        name: &'a str,
        hint: &'a str,
    },
    /// Recent `/` search query. Surfaced in Search mode so the user
    /// can re-run a prior search without retyping it. Tab fills the
    /// query with `term`; Enter dispatches it.
    Search {
        term: &'a str,
    },
    /// Live buffer match for the current `/` query. The lua side
    /// scans the active editor's buffer and pushes one row per
    /// matching line; selecting a row sends the cursor to `lnum`/`col`
    /// and previews a temporary highlight on it.
    BufferMatch {
        lnum: u64,
        col: u64,
        text: &'a str,
    },
}

impl<'a> PaletteRow<'a> {
    pub(crate) fn title(&self) -> &'a str {
        match *self {
            PaletteRow::Command { title, .. } => title,
            PaletteRow::Font { family } => family,
            PaletteRow::Theme { name } => name,
            PaletteRow::Shader { entry } => entry.title.as_str(),
            PaletteRow::Buffer { entry } => entry.title.as_str(),
            PaletteRow::WorkspaceHost { label, .. } => label,
            PaletteRow::WorkspaceCreate => "+",
            PaletteRow::Workspace { entry } => entry.title.as_str(),
            PaletteRow::Server { entry } => entry.name.as_str(),
            PaletteRow::ServerAdd => "+ Add server",
            PaletteRow::ServerCreate => "+ Create server",
            PaletteRow::Ex { name, .. } => name,
            PaletteRow::Search { term } => term,
            PaletteRow::BufferMatch { text, .. } => text,
        }
    }

    /// Display title for the render pass. Command rows render Zed-style
    /// as `{service}: {title}` (e.g. `nvim: Write File`); every other
    /// row kind renders its plain title. Owned because the prefixed form
    /// has to be built per-frame.
    pub(crate) fn display_title(&self) -> String {
        match *self {
            PaletteRow::Command { service, title, .. } => {
                format!("{service}: {title}")
            }
            _ => self.title().to_owned(),
        }
    }

    pub(crate) fn shortcut(&self) -> &'a str {
        match *self {
            PaletteRow::Command { shortcut, .. } => shortcut,
            PaletteRow::Font { .. } => "",
            PaletteRow::Theme { .. } => "theme",
            PaletteRow::Shader { entry } => entry.detail.as_str(),
            PaletteRow::Buffer { entry } => entry.detail.as_str(),
            // Host headers surface the dialable daemon endpoint on the
            // right (non-local only). Local hosts keep the slot empty so
            // the row reads as plain "⌂ local".
            PaletteRow::WorkspaceHost { daemon_url, .. } => daemon_url.unwrap_or(""),
            PaletteRow::WorkspaceCreate => "",
            PaletteRow::Workspace { entry } => entry.detail.as_str(),
            PaletteRow::Server { entry } => entry.address.as_str(),
            PaletteRow::ServerAdd => "",
            PaletteRow::ServerCreate => "",
            PaletteRow::Ex { hint, .. } => hint,
            PaletteRow::Search { .. } => "recent",
            // Buffer-match rows show their line number in the
            // shortcut slot so the column reads naturally as
            // `<text>   1234`.
            PaletteRow::BufferMatch { .. } => "",
        }
    }

    pub(crate) fn action(&self) -> Option<PaletteAction> {
        // PaletteAction is no longer Copy, so match by reference and
        // clone the carried action out rather than moving from `*self`.
        match self {
            PaletteRow::Command { action, .. } => Some(action.clone()),
            PaletteRow::WorkspaceCreate => Some(PaletteAction::CreateWorkspace),
            PaletteRow::Server { entry } => Some(PaletteAction::SelectServer {
                id: entry.id.clone(),
            }),
            PaletteRow::ServerAdd => Some(PaletteAction::AddServer),
            PaletteRow::ServerCreate => Some(PaletteAction::CreateServer),
            PaletteRow::Font { .. }
            | PaletteRow::Theme { .. }
            | PaletteRow::Shader { .. }
            | PaletteRow::Buffer { .. }
            | PaletteRow::WorkspaceHost { .. }
            | PaletteRow::Workspace { .. }
            | PaletteRow::Ex { .. }
            | PaletteRow::Search { .. }
            | PaletteRow::BufferMatch { .. } => None,
        }
    }

    /// Whether this row can hold the selection cursor. Host header rows
    /// are pure separators (like a folder-group label) and are skipped
    /// by selection movement, hit-testing, and Enter; everything else
    /// is selectable.
    pub(crate) fn is_selectable(&self) -> bool {
        !matches!(self, PaletteRow::WorkspaceHost { .. })
    }
}
