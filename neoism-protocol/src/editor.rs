//! Editor (nvim proxy) wire messages.
//!
//! The native frontend embeds nvim and consumes its `ext_linegrid`
//! redraw notifications directly. The web frontend doesn't have a
//! local nvim, so the daemon spawns one per session and pipes the
//! same redraw data over the existing `ws://.../session` socket,
//! envelope-tagged `Editor` / `EditorReply`.
//!
//! This module only defines the wire shapes — no I/O, no async. The
//! daemon side (`neoism-workspace-daemon::nvim`) owns the nvim
//! subprocess and translates msgpack redraw events into the
//! `EditorServerMessage` variants below.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Messages the client (web bridge) sends to drive the embedded nvim
/// session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditorClientMessage {
    /// Open `path` (workspace-relative) in the embedded nvim. Equivalent
    /// to `:edit <path>` after path-traversal validation on the daemon.
    OpenBuffer {
        path: PathBuf,
        /// Optional 0-based cursor target after opening the file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        /// Optional 0-based cursor target after opening the file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        character: Option<u32>,
        /// Optional web editor surface / pane route id. Older clients
        /// omit this; the daemon treats that as the primary surface.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Forward raw input bytes (as produced by the user's keypresses)
    /// to nvim's `nvim_input`. The bytes are the literal sequence nvim
    /// expects — e.g. `b"i"`, `b"<Esc>"`, `b":wq<CR>"`, etc.
    #[serde(alias = "NvimInput")]
    SendKeys {
        bytes: Vec<u8>,
        /// Optional web editor surface / pane route id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Run an Ex command through `nvim_command`. Desktop already has a
    /// large command-oriented editor integration surface; carrying this
    /// explicitly avoids trying to squeeze multi-line lua snippets through
    /// `nvim_input` key notation.
    Command {
        command: String,
        /// Optional web editor surface / pane route id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Forward GUI mouse input to nvim. Used for editor wheel scrolls
    /// so the web path matches the desktop `nvim_input_mouse` path.
    #[serde(alias = "NvimMouse")]
    MouseInput {
        button: String,
        action: String,
        modifier: String,
        grid: i64,
        row: i64,
        col: i64,
        count: u32,
        /// Optional web editor surface / pane route id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Resize the embedded ui to `width` x `height` cells. Triggers
    /// `nvim_ui_try_resize` and the daemon will follow up with
    /// `grid_resize` redraw events through the existing channel.
    #[serde(alias = "NvimResize")]
    Resize {
        width: u32,
        height: u32,
        /// Optional web editor surface / pane route id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Execute a Rust-owned LSP action for the active editor cursor.
    LspAction {
        action: EditorLspAction,
        /// Optional action payload, e.g. the requested symbol name for rename.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Optional web editor surface / pane route id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Apply one code action previously returned by
    /// [`EditorServerMessage::LspActionResult`]. The originating server and
    /// file travel with the opaque LSP payload so a multi-server workspace
    /// cannot resolve or execute the action on the wrong client.
    ApplyLspCodeAction {
        action: EditorLspCodeAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Request LSP completion items at the active editor cursor. Served
    /// asynchronously from the Rust engine (never blocks nvim), answered with
    /// `EditorServerMessage::LspCompletions`. `seq` is echoed back so a stale
    /// (superseded) response can be discarded after fast typing.
    LspComplete {
        seq: u64,
        /// Character whose insertion caused this request. The engine compares
        /// it with each server's advertised triggerCharacters; ordinary typed
        /// identifiers remain triggerKind=Invoked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_character: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Accept one completion from the latest popup. The daemon revalidates the
    /// document revision, resolves the opaque CompletionItem on its originating
    /// server when supported, and applies its primary/additional text edits as
    /// one editor operation.
    ApplyLspCompletion {
        item: EditorLspCompletionItem,
        #[serde(default)]
        replace_prefix: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Cancel a debounced completion request for this surface when the popup
    /// is dismissed or insert mode ends before another request supersedes it.
    CancelLspCompletion {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Request LSP hover docs at an explicit Neovim UI grid cell. The daemon
    /// asks Neovim to resolve this rendered cell to a buffer line + UTF-8 byte
    /// column, so tabs, wide Unicode, gutters, wrapping, folds, and horizontal
    /// scrolling never get mistaken for buffer coordinates. Unlike
    /// `LspAction::Hover`, this does not move the cursor.
    LspHoverAt {
        seq: u64,
        grid: i64,
        row: i64,
        col: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Detach + terminate the embedded nvim session.
    Close,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorLspAction {
    Hover,
    Definition,
    References,
    Implementation,
    DocumentSymbols,
    WorkspaceSymbols,
    Info,
    Format,
    CodeActions,
    Rename,
    ToggleInlayHints,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EditorLspLocation {
    pub uri: String,
    pub line: u32,
    pub character: u32,
}

/// One entry in a document-symbol result, flattened from the LSP symbol
/// tree so the picker can render a Zed-style outline. `depth` is the
/// nesting level (0 = top-level) used only for display indentation;
/// `uri`/`line`/`character` point at the symbol's selection range so
/// activating the row jumps the cursor onto the symbol name.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EditorLspSymbol {
    pub name: String,
    /// LSP symbol kind label (e.g. "function", "struct", "method").
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub uri: String,
    pub line: u32,
    pub character: u32,
    #[serde(default)]
    pub depth: u32,
}

/// A selectable `textDocument/codeAction` result.
///
/// Display metadata is typed so frontends never need to interpret arbitrary
/// server JSON. `payload` remains opaque until the daemon sends it back to the
/// originating language server and applies any resulting edit through its
/// workspace-containment and range-validation path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorLspCodeAction {
    pub server_id: String,
    pub file_path: PathBuf,
    /// Fingerprint of the live buffer revision used to request this action.
    /// The daemon rejects a selection after intervening edits and asks the
    /// user to request fresh fixes instead of applying stale ranges.
    #[serde(default)]
    pub document_revision: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default)]
    pub preferred: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// Original LSP `CodeAction | Command`; only the daemon interprets it.
    pub payload: serde_json::Value,
}

/// One completion candidate for the editor popup.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EditorLspCompletionItem {
    /// Exact server that produced the semantic item. `None` marks a local
    /// buffer-word fallback, which needs no completionItem/resolve request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    /// File/revision captured with the completion response. Acceptance is
    /// rejected after intervening edits or a buffer switch rather than
    /// applying stale LSP ranges to unrelated text.
    #[serde(default)]
    pub file_path: PathBuf,
    #[serde(default)]
    pub document_revision: String,
    pub label: String,
    /// Lowercase kind word ("function", "variable", "keyword", …) for the
    /// popup's leading tag/icon.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    /// Text inserted on accept (already prefix-replaced by the client).
    pub insert_text: String,
    /// Text the client filters against as the user keeps typing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_text: Option<String>,
    /// Server ordering key; client sorts by this then label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_text: Option<String>,
    #[serde(default)]
    pub preselect: bool,
    /// Original CompletionItem with list defaults expanded. Only the daemon
    /// interprets this payload for resolve and edit application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// Messages the daemon emits in response to (or independently of)
/// `EditorClientMessage` — primarily the parsed `redraw` notifications
/// from nvim's `ext_linegrid` UI surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditorServerMessage {
    /// One atomic nvim redraw notification. Desktop used to receive
    /// `grid_scroll` and its matching edge `grid_line`s in the same
    /// in-process batch; the daemon must preserve that boundary over
    /// WebSocket so smooth scroll never renders a half-applied edge.
    Batch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        messages: Vec<EditorServerMessage>,
    },
    /// A batch of grid cells updated by nvim. This is the canonical
    /// shipped aggregate for nvim's literal `grid_line` protocol:
    /// emitted on every `flush` after one or more `grid_line` /
    /// `grid_clear` events. `cells` only contains the deltas published
    /// in this batch, NOT the full grid contents; the client maintains
    /// the running snapshot. The spec-style `NvimGridLine` tag is
    /// accepted as a deserialize alias for compatibility, but outbound
    /// messages serialize as `GridUpdate`.
    #[serde(alias = "NvimGridLine")]
    GridUpdate {
        /// Optional web editor surface / pane route id. Omitted for
        /// legacy single-surface clients.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        grid_id: u32,
        /// Total grid dimensions as of the last `grid_resize`.
        width: u32,
        height: u32,
        /// Cells changed in this redraw batch.
        cells: Vec<GridCell>,
        /// Cursor position after the batch, if `grid_cursor_goto` was
        /// included.
        cursor: Option<GridPos>,
        /// `mode_change(name, idx)` — short editor mode name like
        /// `"normal"`, `"insert"`, `"visual"`, etc.
        mode: Option<String>,
    },
    /// The grid was resized. The client should reset its snapshot to a
    /// fresh `width x height` buffer of blank cells before consuming
    /// the next `GridUpdate`.
    GridResize {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        grid_id: u32,
        width: u32,
        height: u32,
    },
    /// Neovim cleared the full grid. The client should blank the
    /// current snapshot for this grid while preserving its dimensions.
    #[serde(alias = "NvimGridClear", alias = "Clear")]
    GridClear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        grid_id: u32,
    },
    /// Neovim scrolled a rectangular region of the grid by copying
    /// existing screen cells. The scrolled-in rows/columns are followed
    /// by normal `GridUpdate` cells in the same redraw stream.
    GridScroll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        grid_id: u32,
        top: u32,
        bot: u32,
        left: u32,
        right: u32,
        rows: i32,
        cols: i32,
    },
    /// Explicit cursor move from nvim's `grid_cursor_goto`. `GridUpdate`
    /// still carries a cursor snapshot for legacy consumers; this
    /// event lets thin renderers mirror nvim's redraw event stream
    /// without waiting for a batched cell update.
    #[serde(alias = "NvimCursorGoto")]
    CursorGoto {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        grid_id: u32,
        row: u32,
        col: u32,
    },
    /// Highlight id definition from nvim's `hl_attr_define`. Existing
    /// `GridCell`s carry resolved colors for simple clients; this
    /// palette event lets grid renderers cache nvim highlight ids and
    /// apply future protocol variants that carry `hl_id` directly.
    #[serde(alias = "NvimHighlightAttr")]
    HighlightDefined {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        hl_id: u64,
        attrs: HighlightAttrs,
    },
    /// Neovim viewport movement. `scroll_delta` is the canonical
    /// signal used by Neovide-style smooth scrolling.
    #[serde(alias = "NvimWinViewport")]
    WinViewport {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        grid_id: u32,
        topline: u64,
        botline: u64,
        line_count: u64,
        scroll_delta: f64,
        /// Buffer-coordinate cursor line (0-based) from nvim's
        /// `win_viewport` — the presence plane publishes THIS (not the
        /// screen row) so remote carets land on the right line on
        /// screens with different scroll positions. Defaulted for
        /// older daemons.
        #[serde(default)]
        curline: u64,
        /// Buffer-coordinate cursor column (0-based bytes).
        #[serde(default)]
        curcol: u64,
        /// Gutter width in grid cells (`getwininfo().textoff`) —
        /// renderers add this to buffer-column carets so they land in
        /// the text area, not inside the line numbers.
        #[serde(default)]
        textoff: u64,
    },
    /// Default colors changed (e.g. theme switched). The client uses
    /// these to fill blank cells. All three are packed `0x00RRGGBB`
    /// (high byte unused).
    DefaultColors {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        rgb_fg: u32,
        rgb_bg: u32,
        rgb_sp: u32,
    },
    /// Popup menu (completion / LSP) opened or selection moved.
    PopupMenu {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        items: Vec<PopupMenuItem>,
        /// Highlighted item index, or `None` when nvim sends
        /// `pum_select_item(-1)`.
        selected: Option<u32>,
        /// Anchor in the grid the popup is attached to.
        anchor: GridPos,
        /// Which grid the popup is anchored to.
        grid_id: u32,
    },
    /// Popup menu selection changed without nvim resending the item list.
    PopupMenuSelect {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        selected: Option<u32>,
    },
    /// Popup menu dismissed.
    PopupHide {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
    },
    /// Nvim toggled UI mouse capture (`mouse_on` / `mouse_off` redraw
    /// events). Clients use this to decide whether pointer events
    /// should be routed to nvim or local chrome affordances.
    #[serde(alias = "NvimMouseMode")]
    MouseMode {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        enabled: bool,
    },
    /// Diagnostics published via `vim.diagnostic.get` shim — pushed by
    /// the nvim-side bridge as a notification.
    Diagnostics {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        #[serde(default)]
        error: u64,
        #[serde(default)]
        warn: u64,
        #[serde(default)]
        info: u64,
        #[serde(default)]
        hint: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<PathBuf>,
        items: Vec<DiagnosticItem>,
    },
    /// One coarse LSP lifecycle update from the nvim-side bridge.
    LspStatus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filetype: Option<String>,
    },
    /// Per-buffer LSP snapshot: every configured candidate plus active
    /// clients, with runtime binary source and optional last-message
    /// text for error detail.
    LspSnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        /// File this snapshot was computed for. Older/nvim-originated
        /// snapshots may omit it; daemon-owned snapshots always include it so
        /// clients can reject a late result after a buffer switch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<PathBuf>,
        filetype: String,
        servers: Vec<LspSnapshotServer>,
    },
    /// Last nvim notification attributed to an LSP server.
    LspMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        server: String,
        text: String,
        level: String,
    },
    /// Result of a Rust-owned editor LSP action.
    LspActionResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        action: EditorLspAction,
        line: u32,
        character: u32,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hover: Option<String>,
        #[serde(default)]
        locations: Vec<EditorLspLocation>,
        #[serde(default)]
        symbol_count: usize,
        /// Flattened document-symbol outline (populated for
        /// `DocumentSymbols`; empty otherwise). Carries the display
        /// label + jump target for each row so the frontend can render a
        /// selectable symbol picker instead of a bare count.
        #[serde(default)]
        symbols: Vec<EditorLspSymbol>,
        /// Selectable quick fixes returned for `CodeActions`; empty for every
        /// other action and after one selection has been applied.
        #[serde(default)]
        code_actions: Vec<EditorLspCodeAction>,
    },
    /// Completion items for the active cursor, answering an
    /// `EditorClientMessage::LspComplete`. `seq` echoes the request so the
    /// client drops superseded responses. `replace_prefix` is the identifier
    /// already typed before the cursor — the client backspaces it before
    /// inserting the chosen item so member/prefix completion replaces cleanly.
    LspCompletions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        seq: u64,
        #[serde(default)]
        replace_prefix: String,
        #[serde(default)]
        items: Vec<EditorLspCompletionItem>,
    },
    /// Hover docs for an `LspHoverAt` request. `contents` is the rendered
    /// markdown (empty ⇒ nothing to show). `line`/`character` are the resolved
    /// zero-based buffer line and UTF-8 byte column; `seq` echoes the request
    /// so a superseded hover (mouse moved) is dropped.
    LspHoverResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        seq: u64,
        line: u32,
        character: u32,
        #[serde(default)]
        contents: String,
    },
    /// Editor mode changed independently of a grid update (`mode_change`
    /// arriving without an accompanying `grid_line`).
    #[serde(alias = "NvimModeChange")]
    ModeChange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        mode: String,
        mode_idx: u32,
    },
    /// Message/event text from nvim's external messages UI, primarily
    /// `msg_show`. For example `:lua print("hi")` arrives as kind
    /// `"lua_print"` with content `"hi"`. The editor envelope already
    /// scopes this to nvim, so the canonical wire tag is `Message`;
    /// the spec-style `NvimMessage` tag is accepted as a deserialize
    /// alias for compatibility.
    #[serde(alias = "NvimMessage")]
    Message {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        kind: String,
        content: String,
        replace_last: bool,
    },
    /// Toast-style notification from nvim-side glue (`rio_notify`,
    /// clipboard yanks, plugin messages).
    Notification {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        message: String,
        level: String,
    },
    /// Yank-flash region from `TextYankPost`, in grid cell rows.
    YankFlash {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        row_top: u32,
        row_bot: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        col_left: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        col_right: Option<u32>,
    },
    /// Buffer was opened and the embedded nvim is ready to render.
    BufferOpened {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        path: PathBuf,
        /// Total line count (handy for the chrome's status line).
        line_count: u64,
    },
    /// Cursor moved (nvim `rio_winbar`) — feeds the status line's
    /// `cur/total` lines pill for daemon-backed code panes, which the
    /// local-editor winbar drain can't see.
    CursorLine {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        /// 1-based cursor line.
        line: u64,
        /// Total buffer line count; 0 when unresolved.
        total_lines: u64,
    },
    /// Nvim's `modified` flag changed for a buffer. Desktop uses this
    /// to drive the yellow dirty dot in buffer tabs.
    BufferModified {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        path: PathBuf,
        modified: bool,
    },
    /// Embedded nvim exited or the daemon closed the session.
    Closed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        /// `None` if the session was closed cleanly, `Some(msg)` on
        /// error / unexpected exit.
        reason: Option<String>,
    },
    /// Error during request handling — protocol-level, not nvim's own.
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_id: Option<String>,
        message: String,
    },
}

impl EditorClientMessage {
    /// Surface id carried by editor commands that target a concrete
    /// web pane route. Legacy commands and `Close` return `None`.
    pub fn surface_id(&self) -> Option<&str> {
        match self {
            EditorClientMessage::OpenBuffer { surface_id, .. }
            | EditorClientMessage::SendKeys { surface_id, .. }
            | EditorClientMessage::Command { surface_id, .. }
            | EditorClientMessage::MouseInput { surface_id, .. }
            | EditorClientMessage::Resize { surface_id, .. }
            | EditorClientMessage::LspAction { surface_id, .. }
            | EditorClientMessage::ApplyLspCodeAction { surface_id, .. }
            | EditorClientMessage::LspComplete { surface_id, .. }
            | EditorClientMessage::ApplyLspCompletion { surface_id, .. }
            | EditorClientMessage::CancelLspCompletion { surface_id }
            | EditorClientMessage::LspHoverAt { surface_id, .. } => surface_id.as_deref(),
            EditorClientMessage::Close => None,
        }
    }
}

/// A single grid cell as published by nvim's `grid_line`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridCell {
    pub row: u32,
    pub col: u32,
    /// Cell text. Usually a single grapheme; nvim publishes `""` for
    /// the trailing half of a double-width glyph.
    pub ch: String,
    /// Resolved foreground color, `0x00RRGGBB`. The daemon resolves
    /// nvim's `hl_id` against the active highlight table so the wire
    /// stays palette-free.
    pub fg: u32,
    /// Resolved background color, `0x00RRGGBB`.
    pub bg: u32,
    /// Bitfield: bit 0 = bold, bit 1 = italic, bit 2 = underline,
    /// bit 3 = undercurl, bit 4 = strikethrough, bit 5 = reverse.
    pub attrs: u8,
}

/// Resolved attributes for an nvim highlight id. Colors are packed
/// `0x00RRGGBB`; absent channels inherit the current default color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightAttrs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sp: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub undercurl: bool,
    pub strikethrough: bool,
    pub reverse: bool,
}

/// 0-based cursor / anchor position within a grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridPos {
    pub row: u32,
    pub col: u32,
}

/// Popup-menu item shape. Mirrors nvim's `pum_show` 4-tuple
/// `[word, kind, menu, info]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PopupMenuItem {
    pub word: String,
    pub kind: String,
    pub menu: String,
    pub info: String,
}

/// Diagnostic severity, matching the `vim.diagnostic.severity` codes
/// (1=Error, 2=Warn, 3=Info, 4=Hint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warn,
    Info,
    Hint,
}

impl DiagnosticSeverity {
    /// Decode the `vim.diagnostic.severity` integer codes.
    pub fn from_u8(s: u8) -> Self {
        match s {
            1 => DiagnosticSeverity::Error,
            2 => DiagnosticSeverity::Warn,
            3 => DiagnosticSeverity::Info,
            _ => DiagnosticSeverity::Hint,
        }
    }
}

/// Single diagnostic published by the nvim-side bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticItem {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
    /// 0-based row in the buffer.
    pub line: u32,
    /// 0-based column.
    pub col: u32,
    /// Zero-based end position (exclusive) for the marked range.
    #[serde(default)]
    pub end_line: u32,
    #[serde(default)]
    pub end_col: u32,
    /// 1-based line number — convenient for `:<lnum>` jumps.
    pub lnum: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_information: Vec<crate::diagnostics::DiagnosticRelatedInformation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspSnapshotServer {
    pub name: String,
    pub binary: String,
    pub filetype: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_client(message: &EditorClientMessage) {
        let json = serde_json::to_string(message).expect("serialize client message");
        let back: EditorClientMessage =
            serde_json::from_str(&json).expect("deserialize client message");
        let json_back = serde_json::to_string(&back).expect("reserialize client message");
        assert_eq!(json, json_back, "roundtrip mismatch: {json}");
    }

    fn roundtrip_server(message: &EditorServerMessage) {
        let json = serde_json::to_string(message).expect("serialize server message");
        let back: EditorServerMessage =
            serde_json::from_str(&json).expect("deserialize server message");
        let json_back = serde_json::to_string(&back).expect("reserialize server message");
        assert_eq!(json, json_back, "roundtrip mismatch: {json}");
    }

    #[test]
    fn editor_client_input_resize_mouse_roundtrip() {
        roundtrip_client(&EditorClientMessage::OpenBuffer {
            path: "src/lib.rs".into(),
            line: Some(10),
            character: Some(2),
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::SendKeys {
            bytes: b"iHello<Esc>".to_vec(),
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::Command {
            command: "lua vim.cmd.edit('src/lib.rs')".into(),
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::Resize {
            width: 120,
            height: 40,
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::MouseInput {
            button: "left".into(),
            action: "press".into(),
            modifier: "".into(),
            grid: 1,
            row: 4,
            col: 8,
            count: 1,
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::LspHoverAt {
            seq: 42,
            grid: 0,
            row: 7,
            col: 19,
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::ApplyLspCodeAction {
            action: EditorLspCodeAction {
                server_id: "rust-analyzer".into(),
                file_path: "src/lib.rs".into(),
                document_revision: "revision-41".into(),
                title: "Import `HashMap`".into(),
                kind: Some("quickfix".into()),
                preferred: true,
                disabled_reason: None,
                payload: serde_json::json!({
                    "title": "Import `HashMap`",
                    "data": {"id": 7}
                }),
            },
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::ApplyLspCompletion {
            item: EditorLspCompletionItem {
                server_id: Some("typescript".into()),
                file_path: "src/main.ts".into(),
                document_revision: "revision-42".into(),
                label: "details".into(),
                kind: "property".into(),
                detail: Some("(property) details: string".into()),
                documentation: Some("Additional request details.".into()),
                insert_text: "details".into(),
                filter_text: Some("details".into()),
                sort_text: Some("11".into()),
                preselect: true,
                payload: Some(serde_json::json!({
                    "label": "details",
                    "data": {"id": 9}
                })),
            },
            replace_prefix: "det".into(),
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::CancelLspCompletion {
            surface_id: Some("pane:7".into()),
        });
        roundtrip_client(&EditorClientMessage::Close);
    }

    #[test]
    fn lsp_code_action_result_roundtrip_preserves_selection_payload() {
        let action = EditorLspCodeAction {
            server_id: "fixture-lsp".into(),
            file_path: "src/main.rs".into(),
            document_revision: "revision-99".into(),
            title: "Fix this".into(),
            kind: Some("quickfix".into()),
            preferred: true,
            disabled_reason: None,
            payload: serde_json::json!({"title": "Fix this", "data": {"id": 9}}),
        };
        roundtrip_server(&EditorServerMessage::LspActionResult {
            surface_id: Some("pane:7".into()),
            action: EditorLspAction::CodeActions,
            line: 3,
            character: 5,
            summary: "1 code action".into(),
            hover: None,
            locations: Vec::new(),
            symbol_count: 0,
            symbols: Vec::new(),
            code_actions: vec![action],
        });
    }

    #[test]
    fn editor_server_grid_cursor_highlight_roundtrip() {
        roundtrip_server(&EditorServerMessage::GridResize {
            surface_id: Some("pane:7".into()),
            grid_id: 1,
            width: 120,
            height: 40,
        });
        roundtrip_server(&EditorServerMessage::GridClear {
            surface_id: Some("pane:7".into()),
            grid_id: 1,
        });
        roundtrip_server(&EditorServerMessage::GridUpdate {
            surface_id: Some("pane:7".into()),
            grid_id: 1,
            width: 120,
            height: 40,
            cells: vec![GridCell {
                row: 0,
                col: 0,
                ch: "H".into(),
                fg: 0x00FF_FFFF,
                bg: 0x0000_0000,
                attrs: 0b0000_0001,
            }],
            cursor: Some(GridPos { row: 0, col: 1 }),
            mode: Some("insert".into()),
        });
        roundtrip_server(&EditorServerMessage::CursorGoto {
            surface_id: Some("pane:7".into()),
            grid_id: 1,
            row: 3,
            col: 9,
        });
        roundtrip_server(&EditorServerMessage::HighlightDefined {
            surface_id: Some("pane:7".into()),
            hl_id: 42,
            attrs: HighlightAttrs {
                fg: Some(0x00AA_BBCC),
                bg: Some(0x0001_0203),
                sp: Some(0x00CC_BBAA),
                bold: true,
                italic: true,
                underline: true,
                undercurl: false,
                strikethrough: false,
                reverse: true,
            },
        });
        roundtrip_server(&EditorServerMessage::MouseMode {
            surface_id: Some("pane:7".into()),
            enabled: true,
        });
        roundtrip_server(&EditorServerMessage::Message {
            surface_id: Some("pane:7".into()),
            kind: "lua_print".into(),
            content: "hi".into(),
            replace_last: false,
        });
    }
}
