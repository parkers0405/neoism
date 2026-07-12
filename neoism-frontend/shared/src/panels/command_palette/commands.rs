// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Static catalog of palette commands and ex-command suggestions.
//!
//! Commands are organised Zed-style: each command belongs to a logical
//! [`CommandService`] namespace and the palette renders entries as
//! `{service}: {command}` (e.g. `nvim: Write File`, `markdown: ...`,
//! `neoism-agent: Neoism Agent`). The namespace is purely a display +
//! grouping concern today; routing of a logical verb (e.g. "save") to
//! the focused surface is handled by `command_visible_for_surface` +
//! the host's `save_current_document` dispatch, which already picks the
//! markdown / nvim / draw handler based on which surface owns focus.

use super::actions::PaletteAction;

/// The service a palette command logically belongs to. Drives the
/// `{service}: {command}` display prefix and gives us a single place to
/// grow toward a fully service-namespaced registry (Zed's model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandService {
    /// App-level / window / tab / split commands with no narrower owner.
    Neoism,
    /// nvim ex-style buffer commands (write, search in a code buffer, …).
    Nvim,
    /// Markdown-note commands.
    Markdown,
    /// Jupyter-compatible notebook commands.
    Notebook,
    /// `.neodraw` sketch commands.
    Draw,
    /// The native Neoism agent + agent CLIs (claude / codex / opencode).
    Agent,
    /// Language-server actions (hover, code action, rename, …).
    Lsp,
    /// Workspace / multi-host / buffer navigation.
    Workspace,
}

impl CommandService {
    /// The lowercase namespace shown before the command, Zed-style
    /// (`nvim: w`, `neoism-agent: start chat`). Stable identifiers — the
    /// fuzzy matcher also scores against this so `nvim ` narrows to the
    /// nvim namespace.
    pub(crate) const fn prefix(self) -> &'static str {
        match self {
            CommandService::Neoism => "neoism",
            CommandService::Nvim => "nvim",
            CommandService::Markdown => "markdown",
            CommandService::Notebook => "notebook",
            CommandService::Draw => "draw",
            CommandService::Agent => "neoism-agent",
            CommandService::Lsp => "lsp",
            CommandService::Workspace => "workspace",
        }
    }

    /// Resolve a [`CommandService`] back from its lowercase [`prefix`]
    /// string. The palette carries only the prefix on each `Command` row
    /// (see [`super::modes::PaletteRow::Command`]), so the row renderer
    /// uses this to recover the service and draw its canonical [`icon`].
    ///
    /// [`prefix`]: CommandService::prefix
    /// [`icon`]: CommandService::icon
    pub(crate) fn from_prefix(prefix: &str) -> Option<CommandService> {
        match prefix {
            "neoism" => Some(CommandService::Neoism),
            "nvim" => Some(CommandService::Nvim),
            "markdown" => Some(CommandService::Markdown),
            "notebook" => Some(CommandService::Notebook),
            "draw" => Some(CommandService::Draw),
            "neoism-agent" => Some(CommandService::Agent),
            "lsp" => Some(CommandService::Lsp),
            "workspace" => Some(CommandService::Workspace),
            _ => None,
        }
    }

    /// Nerd-font glyph shown beside commands of this service — the single
    /// canonical icon source shared by the splash menu, the Alt+K command
    /// sheet, and the Alt+P command palette so the three never drift.
    /// Markdown uses the note glyph so it matches the splash "Notes" entry.
    pub(crate) const fn icon(self) -> &'static str {
        match self {
            CommandService::Neoism => "\u{f0c9}",
            CommandService::Nvim => "\u{f121}",
            CommandService::Markdown => "\u{f15c}",
            CommandService::Notebook => "\u{f02d}",
            CommandService::Draw => "\u{f303}",
            CommandService::Agent => "\u{f135}",
            CommandService::Lsp => "\u{f085}",
            CommandService::Workspace => "\u{f07b}",
        }
    }

    /// [`Self::icon`] with Mash Up Pack overrides applied (key
    /// `palette.<prefix>`). Use at draw time; `icon()` stays `const`
    /// so tables can embed the defaults.
    pub(crate) fn icon_themed(self) -> &'static str {
        let key = match self {
            CommandService::Neoism => "palette.neoism",
            CommandService::Nvim => "palette.nvim",
            CommandService::Markdown => "palette.markdown",
            CommandService::Notebook => "palette.notebook",
            CommandService::Draw => "palette.draw",
            CommandService::Agent => "palette.neoism-agent",
            CommandService::Lsp => "palette.lsp",
            CommandService::Workspace => "palette.workspace",
        };
        crate::primitives::look::icon_override(key)
            .and_then(|over| over.glyph)
            .unwrap_or(self.icon())
    }
}

pub(crate) struct Command {
    pub(crate) title: &'static str,
    pub(crate) shortcut: &'static str,
    pub(crate) action: PaletteAction,
    pub(crate) service: CommandService,
}

pub(crate) const COMMANDS: &[Command] = &[
    Command {
        title: "New Tab",
        shortcut: "Ctrl+Shift+T terminal tab",
        action: PaletteAction::TabCreate,
        service: CommandService::Neoism,
    },
    Command {
        title: "New Workspace",
        shortcut: "Ctrl+Shift+W top-level workspace",
        action: PaletteAction::CreateWorkspace,
        service: CommandService::Workspace,
    },
    Command {
        title: "Close Tab",
        shortcut: "Cmd+W",
        action: PaletteAction::TabClose,
        service: CommandService::Neoism,
    },
    Command {
        title: "Close Other Tabs",
        shortcut: "",
        action: PaletteAction::TabCloseUnfocused,
        service: CommandService::Neoism,
    },
    Command {
        title: "Next Tab",
        shortcut: "Ctrl+Tab",
        action: PaletteAction::SelectNextTab,
        service: CommandService::Neoism,
    },
    Command {
        title: "Previous Tab",
        shortcut: "Ctrl+Shift+Tab",
        action: PaletteAction::SelectPrevTab,
        service: CommandService::Neoism,
    },
    Command {
        title: "Split Right",
        shortcut: "Cmd+D",
        action: PaletteAction::SplitRight,
        service: CommandService::Neoism,
    },
    Command {
        title: "Split Down",
        shortcut: "Cmd+Shift+D",
        action: PaletteAction::SplitDown,
        service: CommandService::Neoism,
    },
    Command {
        title: "Next Split",
        shortcut: "",
        action: PaletteAction::SelectNextSplit,
        service: CommandService::Neoism,
    },
    Command {
        title: "Previous Split",
        shortcut: "",
        action: PaletteAction::SelectPrevSplit,
        service: CommandService::Neoism,
    },
    Command {
        title: "Close Split or Tab",
        shortcut: "q close current",
        action: PaletteAction::CloseCurrentSplitOrTab,
        service: CommandService::Neoism,
    },
    Command {
        title: "Settings",
        shortcut: "Cmd+,",
        action: PaletteAction::ConfigEditor,
        service: CommandService::Neoism,
    },
    Command {
        title: "New Window",
        shortcut: "Cmd+N",
        action: PaletteAction::WindowCreateNew,
        service: CommandService::Neoism,
    },
    Command {
        title: "Increase Font Size",
        shortcut: "Cmd++",
        action: PaletteAction::IncreaseFontSize,
        service: CommandService::Neoism,
    },
    Command {
        title: "Decrease Font Size",
        shortcut: "Cmd+-",
        action: PaletteAction::DecreaseFontSize,
        service: CommandService::Neoism,
    },
    Command {
        title: "Reset Font Size",
        shortcut: "Cmd+0",
        action: PaletteAction::ResetFontSize,
        service: CommandService::Neoism,
    },
    Command {
        title: "Toggle Vi Mode",
        shortcut: "",
        action: PaletteAction::ToggleViMode,
        service: CommandService::Nvim,
    },
    Command {
        title: "Toggle Fullscreen",
        shortcut: "",
        action: PaletteAction::ToggleFullscreen,
        service: CommandService::Neoism,
    },
    Command {
        title: "Toggle Appearance Theme",
        shortcut: "",
        action: PaletteAction::ToggleAppearanceTheme,
        service: CommandService::Neoism,
    },
    Command {
        title: "Theme Picker",
        shortcut: "",
        action: PaletteAction::OpenThemePicker,
        service: CommandService::Neoism,
    },
    Command {
        title: "Shaders",
        shortcut: "",
        action: PaletteAction::OpenShaders,
        service: CommandService::Neoism,
    },
    Command {
        title: "Mash Up Packs",
        shortcut: "",
        action: PaletteAction::OpenMashupPacks,
        service: CommandService::Neoism,
    },
    Command {
        title: "Copy",
        shortcut: "Cmd+C",
        action: PaletteAction::Copy,
        service: CommandService::Neoism,
    },
    Command {
        title: "Paste",
        shortcut: "Cmd+V",
        action: PaletteAction::Paste,
        service: CommandService::Neoism,
    },
    Command {
        // "save" verb: routes to whichever surface owns focus (markdown
        // note, nvim buffer, or .neodraw) — see `save_current_document`.
        title: "Write File",
        shortcut: ":w",
        action: PaletteAction::SaveDocument,
        service: CommandService::Nvim,
    },
    Command {
        title: "Go to Line…",
        shortcut: ":42",
        action: PaletteAction::GoToLine,
        service: CommandService::Nvim,
    },
    Command {
        title: "Go to File Start",
        shortcut: "gg",
        action: PaletteAction::NvimEx("normal! gg"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Go to File End",
        shortcut: "G",
        action: PaletteAction::NvimEx("normal! G"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Save All",
        shortcut: ":wall",
        action: PaletteAction::NvimEx("wall"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Close Buffer",
        shortcut: ":bdelete",
        action: PaletteAction::NvimEx("bdelete"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Close Other Buffers",
        shortcut: "",
        action: PaletteAction::NvimEx("%bdelete|edit #|bdelete #"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Reload File",
        shortcut: ":edit!",
        action: PaletteAction::NvimEx("edit!"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Discard Changes",
        shortcut: ":edit!",
        action: PaletteAction::NvimEx("edit!"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Next Buffer",
        shortcut: ":bnext",
        action: PaletteAction::NvimEx("bnext"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Previous Buffer",
        shortcut: ":bprevious",
        action: PaletteAction::NvimEx("bprevious"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Jump Back",
        shortcut: "Ctrl+O",
        action: PaletteAction::NvimEx("execute \"normal! \\<C-o>\""),
        service: CommandService::Nvim,
    },
    Command {
        title: "Jump Forward",
        shortcut: "Ctrl+I",
        action: PaletteAction::NvimEx("execute \"normal! \\<C-i>\""),
        service: CommandService::Nvim,
    },
    Command {
        title: "Last Edit Location",
        shortcut: "`.",
        action: PaletteAction::NvimEx("normal! `."),
        service: CommandService::Nvim,
    },
    Command {
        title: "Undo",
        shortcut: "u",
        action: PaletteAction::NvimEx("undo"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Redo",
        shortcut: "Ctrl+R",
        action: PaletteAction::NvimEx("redo"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Repeat Last Change",
        shortcut: ".",
        action: PaletteAction::NvimEx("normal! ."),
        service: CommandService::Nvim,
    },
    Command {
        title: "Toggle Line Numbers",
        shortcut: ":set number!",
        action: PaletteAction::NvimEx("set number!"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Toggle Relative Numbers",
        shortcut: ":set relativenumber!",
        action: PaletteAction::NvimEx("set relativenumber!"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Toggle Wrap",
        shortcut: ":set wrap!",
        action: PaletteAction::NvimEx("set wrap!"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Toggle Spell",
        shortcut: ":set spell!",
        action: PaletteAction::NvimEx("set spell!"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Clear Search Highlight",
        shortcut: ":nohlsearch",
        action: PaletteAction::NvimEx("nohlsearch"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Search Word Under Cursor",
        shortcut: "*",
        action: PaletteAction::NvimEx("normal! *"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Search Word Under Cursor Backward",
        shortcut: "#",
        action: PaletteAction::NvimEx("normal! #"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Select All",
        shortcut: "ggVG",
        action: PaletteAction::NvimEx("normal! ggVG"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Reindent File",
        shortcut: "gg=G",
        action: PaletteAction::NvimEx("normal! gg=G"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Sort File",
        shortcut: ":%sort",
        action: PaletteAction::NvimEx("%sort"),
        service: CommandService::Nvim,
    },
    Command {
        // Esc first so an in-flight Visual selection writes `'<`/`'>`
        // before the range sort reads them.
        title: "Sort Selection",
        shortcut: ":'<,'>sort",
        action: PaletteAction::NvimEx("execute \"normal! \\<Esc>\" | '<,'>sort"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Delete Trailing Whitespace",
        shortcut: "",
        action: PaletteAction::NvimEx("%s/\\s\\+$//e"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Show Registers",
        shortcut: ":registers",
        action: PaletteAction::NvimEx("registers"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Show Marks",
        shortcut: ":marks",
        action: PaletteAction::NvimEx("marks"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Show Jumps",
        shortcut: ":jumps",
        action: PaletteAction::NvimEx("jumps"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Move Line Up",
        shortcut: ":move -2",
        action: PaletteAction::NvimEx("move -2"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Move Line Down",
        shortcut: ":move +1",
        action: PaletteAction::NvimEx("move +1"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Duplicate Line",
        shortcut: ":copy .",
        action: PaletteAction::NvimEx("copy ."),
        service: CommandService::Nvim,
    },
    Command {
        title: "Join Lines",
        shortcut: "J",
        action: PaletteAction::NvimEx("normal! J"),
        service: CommandService::Nvim,
    },
    Command {
        title: "Run Current Cell",
        shortcut: "Ctrl+Enter",
        action: PaletteAction::RunNotebookCell,
        service: CommandService::Notebook,
    },
    Command {
        title: "Run Current Cell And Below",
        shortcut: ":runbelow",
        action: PaletteAction::RunNotebookCellAndBelow,
        service: CommandService::Notebook,
    },
    Command {
        title: "Run All Cells",
        shortcut: ":runall",
        action: PaletteAction::RunAllNotebookCells,
        service: CommandService::Notebook,
    },
    Command {
        title: "Insert Code Cell Above",
        shortcut: ":insertcodeabove",
        action: PaletteAction::InsertNotebookCodeCellAbove,
        service: CommandService::Notebook,
    },
    Command {
        title: "Insert Code Cell Below",
        shortcut: ":insertcodebelow",
        action: PaletteAction::InsertNotebookCodeCellBelow,
        service: CommandService::Notebook,
    },
    Command {
        title: "Insert Markdown Cell Above",
        shortcut: ":insertmarkdownabove",
        action: PaletteAction::InsertNotebookMarkdownCellAbove,
        service: CommandService::Notebook,
    },
    Command {
        title: "Insert Markdown Cell Below",
        shortcut: ":insertmarkdownbelow",
        action: PaletteAction::InsertNotebookMarkdownCellBelow,
        service: CommandService::Notebook,
    },
    Command {
        title: "Delete Current Cell",
        shortcut: ":deletecell",
        action: PaletteAction::DeleteNotebookCell,
        service: CommandService::Notebook,
    },
    Command {
        title: "Move Cell Up",
        shortcut: ":movecellup",
        action: PaletteAction::MoveNotebookCellUp,
        service: CommandService::Notebook,
    },
    Command {
        title: "Move Cell Down",
        shortcut: ":movecelldown",
        action: PaletteAction::MoveNotebookCellDown,
        service: CommandService::Notebook,
    },
    Command {
        title: "Interrupt Kernel",
        shortcut: ":interruptkernel",
        action: PaletteAction::InterruptNotebookKernel,
        service: CommandService::Notebook,
    },
    Command {
        title: "Clear All Outputs",
        shortcut: ":clearoutputs",
        action: PaletteAction::ClearNotebookOutputs,
        service: CommandService::Notebook,
    },
    Command {
        title: "Clear Current Cell Output",
        shortcut: ":clearoutput",
        action: PaletteAction::ClearNotebookCellOutput,
        service: CommandService::Notebook,
    },
    Command {
        title: "Restart Kernel",
        shortcut: ":restartkernel",
        action: PaletteAction::RestartNotebookKernel,
        service: CommandService::Notebook,
    },
    Command {
        title: "Search Forward",
        shortcut: "Cmd+F",
        action: PaletteAction::SearchForward,
        service: CommandService::Nvim,
    },
    Command {
        title: "Search Backward",
        shortcut: "",
        action: PaletteAction::SearchBackward,
        service: CommandService::Nvim,
    },
    Command {
        title: "Search Files",
        shortcut: "<leader>ff",
        action: PaletteAction::SearchFiles,
        service: CommandService::Workspace,
    },
    Command {
        title: "Search Words",
        shortcut: "<leader>fw",
        action: PaletteAction::SearchWords,
        service: CommandService::Workspace,
    },
    Command {
        title: "Search Git Changes",
        shortcut: "<leader>fg",
        action: PaletteAction::SearchGitChanges,
        service: CommandService::Workspace,
    },
    Command {
        title: "Git Diff",
        shortcut: "Alt+G",
        action: PaletteAction::ToggleGitDiffPanel,
        service: CommandService::Workspace,
    },
    Command {
        title: "Share Current Workspace",
        shortcut: "workspace share",
        action: PaletteAction::ShareCurrentWorkspace,
        service: CommandService::Workspace,
    },
    Command {
        title: "Stop Sharing Current Workspace",
        shortcut: "workspace private",
        action: PaletteAction::StopSharingCurrentWorkspace,
        service: CommandService::Workspace,
    },
    Command {
        title: "Leave Workspace",
        shortcut: "workspace leave",
        action: PaletteAction::LeaveWorkspace,
        service: CommandService::Workspace,
    },
    Command {
        title: "Send Current Workspace to Docker Sandbox",
        shortcut: "workspace docker",
        action: PaletteAction::SendCurrentWorkspaceToDockerSandbox,
        service: CommandService::Workspace,
    },
    Command {
        title: "Send Current Workspace to Cloud",
        shortcut: "workspace cloud",
        action: PaletteAction::SendCurrentWorkspaceToCloud,
        service: CommandService::Workspace,
    },
    Command {
        title: "Create Neoism Note",
        shortcut: "new note",
        action: PaletteAction::CreateNeoismNote,
        service: CommandService::Markdown,
    },
    Command {
        title: "Draw on Note",
        shortcut: "draw annotate ink",
        action: PaletteAction::DrawOnNote,
        service: CommandService::Draw,
    },
    Command {
        title: "Open Neoism Notes",
        shortcut: "notes sidebar",
        action: PaletteAction::OpenNeoismNotes,
        service: CommandService::Markdown,
    },
    Command {
        title: "Hover Documentation",
        shortcut: "K",
        action: PaletteAction::LspHover,
        service: CommandService::Lsp,
    },
    Command {
        title: "Code Actions",
        shortcut: "<leader>ca",
        action: PaletteAction::LspCodeAction,
        service: CommandService::Lsp,
    },
    Command {
        title: "Format Document",
        shortcut: "LSP",
        action: PaletteAction::LspFormat,
        service: CommandService::Lsp,
    },
    Command {
        title: "Go to Definition",
        shortcut: "gd",
        action: PaletteAction::LspDefinition,
        service: CommandService::Lsp,
    },
    Command {
        title: "Find References",
        shortcut: "gr",
        action: PaletteAction::LspReferences,
        service: CommandService::Lsp,
    },
    Command {
        title: "Rename Symbol",
        shortcut: "LSP",
        action: PaletteAction::LspRename,
        service: CommandService::Lsp,
    },
    Command {
        title: "Document Symbols",
        shortcut: "LSP",
        action: PaletteAction::LspDocumentSymbols,
        service: CommandService::Lsp,
    },
    Command {
        title: "Workspace Symbols",
        shortcut: "LSP",
        action: PaletteAction::LspWorkspaceSymbols,
        service: CommandService::Lsp,
    },
    Command {
        title: "Toggle Inlay Hints",
        shortcut: "LSP",
        action: PaletteAction::ToggleInlayHints,
        service: CommandService::Lsp,
    },
    Command {
        title: "Toggle Minimap",
        shortcut: ":minimap",
        action: PaletteAction::ToggleMinimap,
        service: CommandService::Nvim,
    },
    Command {
        title: "Clear History",
        shortcut: "",
        action: PaletteAction::ClearHistory,
        service: CommandService::Neoism,
    },
    Command {
        title: "List Fonts",
        shortcut: "",
        action: PaletteAction::ListFonts,
        service: CommandService::Neoism,
    },
    Command {
        title: "Buffers",
        shortcut: ":buffers workspace",
        action: PaletteAction::ListBuffers,
        service: CommandService::Workspace,
    },
    Command {
        title: "Workplaces",
        shortcut: "daemon switcher",
        action: PaletteAction::ShowWorkplaces,
        service: CommandService::Workspace,
    },
    Command {
        title: "Neoism Agent",
        shortcut: "Alt+A",
        action: PaletteAction::OpenNeoismAgent,
        service: CommandService::Agent,
    },
    Command {
        title: "Claude",
        shortcut: "agent run claude code",
        action: PaletteAction::RunClaude,
        service: CommandService::Agent,
    },
    Command {
        title: "Codex",
        shortcut: "agent run",
        action: PaletteAction::RunCodex,
        service: CommandService::Agent,
    },
    Command {
        title: "OpenCode",
        shortcut: "agent run opencode",
        action: PaletteAction::RunOpenCode,
        service: CommandService::Agent,
    },
    Command {
        title: "Quit",
        shortcut: "Cmd+Q",
        action: PaletteAction::Quit,
        service: CommandService::Neoism,
    },
];

/// Curated set of common nvim ex commands surfaced as live suggestions
/// while the user types in `:` mode. Not exhaustive — the ones a user
/// would actually want autocompleted from the first 1–3 keystrokes.
/// The `hint` column is a one-word reminder of what the command does
/// (rendered in the right-side shortcut slot of the palette row).
pub(crate) const EX_COMMANDS: &[(&str, &str)] = &[
    ("w", "save"),
    ("write", "save"),
    ("wall", "save all"),
    ("wq", "save+quit"),
    ("wqall", "save+quit all"),
    ("q", "quit current"),
    ("quit", "quit"),
    ("qall", "quit all"),
    ("qall!", "force quit all"),
    ("quit!", "force quit"),
    ("edit", "open file"),
    ("edit!", "reload+discard"),
    ("undo", "undo"),
    ("redo", "redo"),
    ("sort", "sort lines"),
    ("split", "horiz split"),
    ("vsplit", "vert split"),
    ("tabnew", "new tab"),
    ("tabclose", "close tab"),
    ("bnext", "next buffer"),
    ("bprev", "prev buffer"),
    ("bdelete", "close buffer"),
    ("buffers", "list buffers"),
    ("ls", "list buffers"),
    ("files", "list buffers"),
    ("nohlsearch", "clear hl"),
    ("source", "run vim file"),
    ("set", "option"),
    ("help", "help"),
    ("colorscheme", "theme"),
    ("ThemePicker", "theme picker"),
    ("Shaders", "shaders"),
    ("ShaderPicker", "shader picker"),
    ("LspInfo", "lsp status"),
    ("LspRestart", "restart lsp"),
    ("LspFormat", "format"),
    ("Hover", "docs"),
    ("CodeAction", "actions"),
    ("Definition", "goto"),
    ("References", "refs"),
    ("Rename", "symbol"),
    ("DocumentSymbols", "symbols"),
    ("WorkspaceSymbols", "symbols"),
    ("InlayHints", "toggle"),
    ("minimap", "toggle"),
    ("minimap on", "show"),
    ("minimap off", "hide"),
    ("SyntaxInfo", "syntax status"),
    ("checkhealth", "diagnose"),
    ("messages", "log"),
    ("runcell", "notebook"),
    ("runbelow", "notebook"),
    ("runall", "notebook"),
    ("clearoutput", "notebook"),
    ("interruptkernel", "notebook"),
    ("clearoutputs", "notebook"),
    ("restartkernel", "notebook"),
    ("Search Files", "<leader>ff"),
    ("Search Words", "<leader>fw"),
    ("Search Git Changes", "<leader>fg"),
    ("terminal", "open term"),
    ("tree", "file tree"),
    ("claude", "agent"),
    ("codex", "agent"),
    ("opencode", "agent"),
    ("opencode-acp", "agent protocol"),
    ("opencode-terminal", "agent tui"),
    ("registers", "yanks"),
    ("marks", "marks"),
    ("jumps", "jump list"),
];
