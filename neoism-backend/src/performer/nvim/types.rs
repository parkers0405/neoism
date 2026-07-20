use super::*;

/// Concrete writer type carried through nvim-rs. Matches neovide's
/// shape so Handler bounds line up.
pub type NeovimWriter = Box<dyn futures::AsyncWrite + Send + Unpin + 'static>;

/// Configuration for spawning a `nvim --embed` instance backing an
/// editor pane. Held by `ContextSource::Editor` (Phase 2c) so the
/// `ContextManager` can build editor panes the same way it builds
/// shell panes.
#[derive(Clone, Debug, Default)]
pub struct NvimSpawnConfig {
    /// Absolute path to the `nvim` binary. `None` → search `$PATH`.
    pub nvim_binary: Option<PathBuf>,
    /// Optional file to open at startup (`nvim --embed <file>`).
    pub initial_file: Option<PathBuf>,
    /// Working directory for the nvim child. Defaults to the parent
    /// process's cwd if `None`.
    pub cwd: Option<PathBuf>,
    /// Extra Ex commands run over RPC before the initial file opens.
    /// `--clean` already skips user config; Rio uses these to prepend
    /// its managed runtime and run IDE-mode setup.
    pub init_commands: Vec<String>,
    /// Initial UI dimensions to send via `ui_attach`. Renderer can
    /// `resize` later as the pane geometry settles.
    pub initial_cols: u64,
    pub initial_rows: u64,
}

/// One redraw notification, kept as raw msgpack until Phase 2c parses
/// it into a typed `RedrawEvent`. Each notification corresponds to
/// one element of nvim's `redraw` notify args.
#[derive(Debug)]
pub struct RedrawNotification {
    pub raw: Value,
}

/// Out-of-band notification fired by our IDE init lua when a buffer's
/// `modified` flag changes. The renderer drains these per-frame to
/// flip the dirty-dot on the matching buffer-tab. Path is whatever
/// `nvim_buf_get_name` returned — usually absolute, sometimes empty
/// (filtered upstream).
#[derive(Clone, Debug)]
pub struct BufModifiedNotification {
    pub path: PathBuf,
    pub modified: bool,
}

/// Out-of-band notification fired on `BufEnter` — the user navigated
/// to a different buffer (via Tab cycling, `:bnext`, `:edit`, our
/// finder, or any other path). The renderer uses this to keep our
/// chrome `buffer_tabs` strip and `file_tree` highlight in sync with
/// nvim's actual current buffer; without it, opening a file via the
/// finder leaves the tab strip pointing at the previous buffer.
#[derive(Clone, Debug)]
pub struct BufEnterNotification {
    pub path: PathBuf,
}

/// Out-of-band notification fired when embedded nvim changes cwd. The
/// renderer uses this to keep workspace-rooted chrome (finder, tree,
/// new editors, and LSP fallback roots) aligned with the active editor.
#[derive(Clone, Debug)]
pub struct CwdNotification {
    pub path: PathBuf,
}

/// Severity hint for a `RioNotify`. Maps to the chrome notifications
/// surface's accent color. Stays as a tiny enum (no log levels) so the
/// chrome side stays decoupled from `tracing` / `log`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyLevel {
    Info,
    Warn,
    Error,
}

impl NotifyLevel {
    pub(crate) fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "warn" | "warning" => NotifyLevel::Warn,
            "error" | "err" => NotifyLevel::Error,
            _ => NotifyLevel::Info,
        }
    }
}

/// Out-of-band toast pushed by nvim (or our own lua glue) for things
/// that used to land in the cmdline message area — `:w`, LSP status,
/// errors. The renderer drains these per-frame and feeds the chrome
/// `Notifications` panel.
#[derive(Clone, Debug)]
pub struct RioNotify {
    pub message: String,
    pub level: NotifyLevel,
}

pub(crate) const MAX_NVIM_NOTIFICATIONS_PER_DRAIN: usize = 32;
pub(crate) const MAX_NVIM_MODALS_PER_DRAIN: usize = 4;

/// In-file location for the "true winbar" tail of the breadcrumbs row.
/// Fired on `CursorMoved` / `CursorHold` and (best-effort) carries the
/// enclosing function/method/class name from treesitter when one
/// resolves. Empty `symbol` means we couldn't resolve a name — caller
/// falls back to just `Ln C` formatting.
#[derive(Clone, Debug)]
pub struct WinbarNotification {
    pub line: u64,
    pub col: u64,
    pub symbol: String,
    /// Total line count of the current buffer at the time of the
    /// emission. Used by the status line's "lines" pill (`cur/total`).
    /// Zero when the lua side couldn't resolve it (older clients).
    pub total_lines: u64,
}

#[derive(Clone, Debug)]
pub struct LspStatusNotification {
    pub state: String,
    pub name: Option<String>,
    pub binary: Option<String>,
    pub filetype: Option<String>,
}

/// One LSP server in a per-buffer snapshot. `state` is one of
/// `"active"` / `"initializing"` / `"missing"` / `"errored"`.
/// `source` is the runtime binary source: managed Neoism install,
/// PATH binary, or missing.
/// `message` carries the last `vim.notify` text that mentioned this
/// server name (best-effort, lua-side substring match) so the popup
/// can render an error / status line under the row without hovering.
#[derive(Clone, Debug)]
pub struct LspSnapshotServer {
    pub name: String,
    pub binary: String,
    pub filetype: String,
    pub state: String,
    pub source: Option<String>,
    pub message: Option<String>,
    pub level: Option<String>,
}

/// Comprehensive snapshot of every LSP server known for a buffer's
/// filetype, plus any client actually attached. Emitted by lua on
/// BufEnter / LspAttach / LspDetach so the status-line popup can show
/// the Zed-style "all servers + their state" list without depending
/// on the running tally of `rio_lsp_status` events.
#[derive(Clone, Debug)]
pub struct LspSnapshotNotification {
    pub filetype: String,
    pub servers: Vec<LspSnapshotServer>,
}

/// Last `vim.notify` text we matched to a specific LSP server name.
/// Lets the popup show the most recent stderr / startup error on
/// hover even when the server keeps its `"active"` state.
#[derive(Clone, Debug)]
pub struct LspMessageNotification {
    pub server: String,
    pub text: String,
    pub level: String,
}

/// One row in the diagnostics popup. `lnum` is 1-based (already
/// adjusted from nvim's 0-based representation in lua), so it can be
/// fed straight into `:<lnum>` when the user clicks an item.
#[derive(Clone, Debug)]
pub struct DiagnosticItem {
    pub lnum: u64,
    pub col: u64,
    /// Zero-based range end, when the originating LSP supplied it.
    pub end_line: u64,
    pub end_col: u64,
    /// 1=error, 2=warn, 3=info, 4=hint (nvim's `vim.diagnostic.severity`).
    pub severity: u8,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
    pub code_description: Option<String>,
    pub tags: Vec<String>,
    pub related_information: Vec<DiagnosticRelatedInformation>,
}

#[derive(Clone, Debug)]
pub struct DiagnosticRelatedInformation {
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub message: String,
}

/// Latest snapshot of `vim.diagnostic.get(0)` for the current buffer.
/// Counts cover ALL diagnostics; `items` is truncated server-side so
/// the rpc payload stays bounded — the popup pages or scrolls if the
/// buffer is huge.
#[derive(Clone, Debug, Default)]
pub struct DiagnosticsNotification {
    pub error: u64,
    pub warn: u64,
    pub info: u64,
    pub hint: u64,
    pub file_path: Option<PathBuf>,
    pub items: Vec<DiagnosticItem>,
}

/// One yank-flash event. `row_top` / `row_bot` are 0-based screen
/// rows (relative to the window's top visible line) plus an optional
/// inclusive column span so the renderer can paint the yanked text
/// instead of the whole pane width. Inclusive on both ends — a
/// single-line yank reports `row_top == row_bot`.
#[derive(Clone, Debug)]
pub struct YankFlashNotification {
    pub row_top: u32,
    pub row_bot: u32,
    pub col_left: Option<u32>,
    pub col_right: Option<u32>,
}

/// One row in the `/`-search dropdown. `lnum` is 1-based and `col` is
/// a 1-based byte column, so they can be piped straight into the lua
/// preview/commit helpers. `text` is already truncated and tab-expanded
/// by the lua side.
#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub lnum: u64,
    pub col: u64,
    pub text: String,
}

/// Latest set of `/`-search matches for the current buffer. Replaced
/// wholesale on each query change — older payloads are stale.
#[derive(Clone, Debug, Default)]
pub struct SearchMatchesNotification {
    pub matches: Vec<SearchMatch>,
}

/// Snapshot for the Rust-owned minimap overlay. `lines == None` means
/// this is a cheap viewport/cursor-only update and the renderer should
/// keep its cached line sample for the buffer.
#[derive(Clone, Debug, Default)]
pub struct MinimapNotification {
    pub path: Option<PathBuf>,
    pub changedtick: u64,
    pub total_lines: u64,
    pub top_line: u64,
    pub bottom_line: u64,
    pub cursor_line: u64,
    pub sample_stride: u64,
    pub lines: Option<Vec<String>>,
    pub git_changes: Vec<MinimapGitChange>,
}

#[derive(Clone, Debug, Default)]
pub struct MinimapGitChange {
    pub line: u64,
    pub kind: String,
}

/// Generic Rust-owned modal request from the managed nvim runtime. This
/// covers information surfaces that would normally render in nvim UI
/// chrome (`:LspInfo`, longer errors, future diagnostics lists), while
/// keeping the actual drawing and input handling in Rio.
#[derive(Clone, Debug)]
pub struct ModalNotification {
    pub title: String,
    pub body: String,
    pub level: NotifyLevel,
    pub actions: Vec<ModalActionNotification>,
}

#[derive(Clone, Debug)]
pub struct ModalActionNotification {
    pub label: String,
    pub hint: String,
    pub command: String,
}

#[derive(Clone, Debug)]
pub struct TreesitterMissingNotification {
    pub lang: String,
    pub filetype: String,
}
/// Commands sent from the renderer thread into the tokio runtime
/// thread. Kept private — `NvimEmbedMachine` exposes typed methods.
#[derive(Debug)]
pub(crate) enum NvimCommand {
    Input(String),
    Mouse {
        button: String,
        action: String,
        modifier: String,
        grid: i64,
        row: i64,
        col: i64,
    },
    MouseMany {
        button: String,
        action: String,
        modifier: String,
        grid: i64,
        row: i64,
        col: i64,
        count: u32,
    },
    /// Run an Ex command (e.g. `edit foo.rs`) — used when our chrome
    /// tree wants to swap the buffer in an existing editor pane rather
    /// than spawn a new one.
    Command(String),
    Shutdown,
}
/// Discriminator for what kind of process drives a pane. `Pty` keeps
/// today's behavior (shell command, PTY performer); `Editor` swaps in
/// `NvimEmbedMachine`.
#[derive(Clone, Debug)]
pub enum ContextSource {
    /// Conventional shell pane (existing behavior).
    Pty,
    /// Editor pane backed by an embedded nvim instance.
    Editor(NvimSpawnConfig),
}

impl Default for ContextSource {
    fn default() -> Self {
        Self::Pty
    }
}
