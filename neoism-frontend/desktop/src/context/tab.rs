use crate::app::ime::Ime;
use crate::app::messenger::Messenger;
use crate::context::renderable::{Cursor, RenderableContent};
use crate::context::splash::SplashInjection;
use crate::daemon_client::DaemonClientHandle;
use crate::editor::markdown::MarkdownPane;
use crate::editor::neodraw::DrawPane;
use crate::editor::notebook::NotebookPane;
use crate::event::sync::FairMutex;
use crate::event::Msg;
use crate::layout::ContextDimension;
use crate::neoism::agent::NeoismAgentPane;
use crate::performer::{self, Machine};
use crate::workspace::extensions::NeoismExtensionsPane;
use crate::workspace::tags_view::NeoismTagsPane;
use neoism_backend::event::EventListener;
use neoism_backend::performer::nvim::{
    BufEnterNotification, BufModifiedNotification, CwdNotification,
    DiagnosticsNotification, LspMessageNotification, LspSnapshotNotification,
    LspSnapshotServer, LspStatusNotification, MinimapNotification, ModalNotification,
    NvimEmbedMachine, NvimSpawnConfig, RedrawNotification, RioNotify,
    SearchMatchesNotification, TreesitterMissingNotification, WinbarNotification,
    YankFlashNotification,
};
use neoism_backend::performer::nvim_events::{
    apply_redraw_events, parse_redraw_batch, Colors as NvimColors, EditorMode,
    GridLineCell, HighlightTable, PackedColor, PopupMenu, PopupMenuItem, RedrawEvent,
    Style as NvimStyle, UnderlineStyle,
};
use neoism_protocol::editor::{
    DiagnosticSeverity as WireDiagnosticSeverity, EditorClientMessage,
    EditorServerMessage, HighlightAttrs,
};
use neoism_terminal_core::crosswords::Crosswords;
use neoism_terminal_core::selection::SelectionRange;
use neoism_ui::render_policy::editor_consume_pending_grid_scroll_animation;
use std::collections::VecDeque;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::{mpsc as tokio_mpsc, watch as tokio_watch};

const MAX_EDITOR_REDRAW_NOTIFICATIONS_PER_FRAME: usize = 96;
const MAX_EDITOR_REDRAW_EVENTS_PER_FRAME: usize = 4096;
const SCROLL_LOG_ENV: &str = "NEOISM_SCROLL_LOG";

pub struct Context<T: EventListener> {
    pub route_id: usize,
    pub terminal: Arc<FairMutex<Crosswords>>,
    pub terminal_input: crate::terminal::blocks::TerminalInputBuffer,
    pub terminal_shell_kind: crate::terminal::blocks::TerminalShellKind,
    pub renderable_content: RenderableContent,
    pub messenger: Messenger,
    #[cfg(not(target_os = "windows"))]
    pub main_fd: Arc<i32>,
    #[cfg(not(target_os = "windows"))]
    pub shell_pid: u32,
    pub rich_text_id: usize,
    pub dimension: ContextDimension,
    pub pending_terminal_resize: bool,
    /// True until the NEOISM splash banner has been written into this
    /// pane's scrollback. Deferred to first render so we know the
    /// real terminal width and can horizontally center the wordmark
    /// — the dimension at `create_context` time can be smaller than
    /// the eventual rendered width once layout settles. Editor-source
    /// panes are born with this `false` (no PTY scrollback to write
    /// to).
    pub pending_splash: bool,
    /// Counter of consecutive frames the pane's `(cols, rows)` has
    /// been stable. Used by the splash injector to wait for layout
    /// to settle before computing the centering pad — the
    /// dimension at the very first frame can lag the eventual
    /// rendered size.
    pub splash_dim_stable_frames: u8,
    /// `(cols, rows)` last seen by the splash injector. When this
    /// changes between frames the stable counter resets.
    pub splash_last_dim: (usize, usize),
    /// Cursor row observed on the previous render frame. Used by
    /// the splash dismiss trigger — when the live cursor row
    /// moves *down* between frames (`current > last`), the user
    /// has submitted a command (shell echoes `\r\n` on Enter)
    /// and we kick off the fade animation. Reset to the live
    /// cursor row whenever `history_size == 0`, so a `clear`
    /// that brings the splash back also resets the comparison.
    pub splash_last_cursor_row: i32,
    /// Pane geometry the splash was actually injected at — origin
    /// row in the live grid, column count, and the cell width/
    /// height in pixels at the moment of injection. The GPU
    /// overlay reads this so it can paint its pulse / ripple over
    /// the wordmark cells regardless of subsequent scroll state.
    pub splash_injection: Option<SplashInjection>,
    pub ime: Ime,
    /// Present when this pane renders a DAEMON-hosted shell (8A "one
    /// shell, many screens"): the feed pushes daemon `PtyOutput`
    /// frames into the machine's byte channel, and the shared slot
    /// carries the session id the pane's input sink resolves against.
    /// `None` for conventional local-PTY panes.
    pub remote_pty: Option<crate::context::remote_pty::RemotePtyBinding>,
    pub(super) _io_thread: Option<JoinHandle<(Machine<T>, performer::State)>>,
    /// Embedded `nvim --embed` runtime for editor-source contexts. Held so
    /// the tokio thread + child stay alive for the lifetime of the pane.
    /// `None` for conventional PTY panes.
    pub editor: Option<EditorBackend>,
    /// Drained on the renderer thread by `pump_editor_redraws`. Taken
    /// off `editor` exactly once at construction so we don't need to
    /// touch the inner `Mutex` on the hot path.
    pub(super) editor_redraw_rx: Option<std_mpsc::Receiver<RedrawNotification>>,
    /// Daemon-fed editor redraw/control messages queued by the desktop
    /// websocket pump. The normal redraw pump drains these into the same
    /// `RedrawEvent` application path used by local nvim.
    pub(super) editor_daemon_messages: VecDeque<EditorServerMessage>,
    /// nvim's highlight-id → Style table — populated as `hl_attr_define`
    /// events stream in. Phase 2e MVP doesn't paint per-cell colors yet,
    /// but the table is kept so 2f can wire it without re-architecting.
    pub editor_hl_table: HighlightTable,
    /// Default fg/bg/special set by `default_colors_set`.
    pub editor_default_colors: NvimColors,
    /// Last `mode_change` event from the embedded nvim. Tracked so the
    /// chrome can ask "is the editor in insert mode right now?" before
    /// hijacking keys like `:` (which open our ex-mode palette in
    /// normal/visual but must pass through as a literal char in insert).
    pub editor_mode: EditorMode,
    /// Sum of `grid_scroll.rows` applied since the renderer last drained
    /// it. Positive = nvim scrolled down through the buffer (content
    /// moved up in the grid), negative = scrolled up. The renderer
    /// converts this to pixels via `cell_height` and feeds it into the
    /// `EditorScroll` spring so keyboard nav (j/k/page-down/`:NN`) gets
    /// the same neovide-style slide as a mouse wheel scroll.
    pub editor_pending_scroll_lines: i32,
    /// NETCODE typing echo: cells we painted locally at keypress time
    /// (peer-link insert mode, blank tail-of-line only) that nvim has
    /// not confirmed yet. Confirmed by ANY authoritative repaint of
    /// the same row; reverted to blank after `PREDICTION_TTL` so a
    /// misprediction can never leave a permanent ghost.
    pub editor_predicted_cells: Vec<PredictedEditorCell>,
    /// `grid_scroll` rows that arrived before the matching
    /// `win_viewport.scroll_delta`. Ctrl-D/U can split this way:
    /// first nvim mutates the grid, then a later redraw reports the
    /// viewport delta. We seed the scrollback/spring from the early
    /// grid_scroll and consume the later viewport delta so the visual
    /// model does not lag or double-animate.
    pub editor_pending_grid_scroll_lines: i32,
    /// Set by the nvim redraw pump when the scrollback ring must be
    /// discarded before the next render. This prevents scroll animation
    /// from sampling offscreen rows from an old buffer/full redraw.
    pub editor_scroll_reset_pending: bool,
    /// Latest viewport state from `win_viewport` — used by the
    /// renderer to detect "at top of file" / "at bottom of file" so it
    /// can switch from the normal scroll spring to an elastic edge
    /// bounce when the user keeps scrolling past the boundary.
    /// `topline = 0` → at top; `botline >= line_count` → at bottom.
    pub editor_viewport_topline: u64,
    pub editor_viewport_botline: u64,
    pub editor_viewport_line_count: u64,
    /// Nvim grid id for the real editor viewport. Floating hover/docs
    /// use their own grids; applying their clear/resize/line events to
    /// this surface causes full-editor flashes and bogus line shifts.
    pub editor_grid_id: Option<u64>,
    /// Buffer-coordinate caret (0-based line/col) from win_viewport's
    /// curline/curcol — what the presence plane publishes so remote
    /// screens draw this pane's cursor at the true buffer position.
    pub editor_presence_line: u64,
    pub editor_presence_col: u64,
    /// Gutter width (cells) of this pane's nvim window — added to
    /// buffer-column carets so they land in the text area.
    pub editor_textoff: u64,
    /// Latest known cursor line (1-indexed) for the active buffer —
    /// updated from minimap notifications regardless of whether the
    /// minimap UI is visible. Consumed by the status line's "lines"
    /// pill so the user always sees `cur/total`.
    pub editor_cursor_line: u64,
    /// Latest known total line count for the active buffer. Same
    /// source as `editor_cursor_line`.
    pub editor_total_lines: u64,
    /// nvim's `msg_showcmd` tail — the half-typed normal-mode command
    /// ("2d", a bare count). Empty when nothing is pending. Shown by
    /// the status line next to the mode label.
    pub editor_pending_keys: String,
    /// Drained per frame — line-count of input the user attempted to
    /// scroll PAST a buffer edge (i.e., wheel/key arrived but
    /// win_viewport.scroll_delta was 0 OR opposite sign). Renderer
    /// pushes this into `editor_scroll.push_elastic`.
    #[allow(dead_code)]
    pub editor_pending_elastic_lines: i32,
    /// Latest external popupmenu model from nvim (`ext_popupmenu`).
    /// Behavior stays inside nvim; Rust owns only the visual surface.
    pub editor_popup_menu: Option<PopupMenu>,
    /// Latest managed nvim LSP lifecycle state for the status line.
    pub editor_lsp_status: Option<String>,
    /// Latest Rust-owned LSP action result. Kept structured so references,
    /// symbols, and hover UI can render from data instead of parsing status text.
    pub editor_lsp_action_result: Option<neoism_protocol::editor::EditorServerMessage>,
    pub editor_lsp_action_result_modal_seen: bool,
    /// Active Rust-engine completion popup (fed by `LspCompletions`), or
    /// `None` when no completion is showing. Owned entirely by the frontend:
    /// nav/accept/dismiss are handled here, insertion via `SendKeys`.
    pub editor_lsp_completion: Option<LspCompletionState>,
    /// Monotonic completion request id. Each keystroke-triggered request bumps
    /// it; a `LspCompletions` reply with an older `seq` is discarded so fast
    /// typing never shows stale candidates.
    pub editor_lsp_completion_seq: u64,
    /// Active VS Code-style hover popup (fed by `LspHoverResult`), or `None`.
    pub editor_lsp_hover: Option<LspHoverState>,
    /// Monotonic hover request id; a reply with an older `seq` is dropped so a
    /// hover the mouse already moved off never appears.
    pub editor_lsp_hover_seq: u64,
    /// Grid cell (row, col) the last hover request targeted — used to dedupe
    /// (don't re-request the same cell) and to anchor the popup on reply.
    pub editor_lsp_hover_cell: Option<(u32, u32)>,
    /// Daemon-side `BufModifiedSet` notifications waiting for the
    /// existing buffer-tab dirty-dot drain.
    pub editor_buf_modified: VecDeque<BufModifiedNotification>,
    /// Daemon-side `BufEnter` notifications waiting for the existing
    /// buffer-tab activation drain.
    pub editor_buf_enter: VecDeque<BufEnterNotification>,
    /// Daemon-side `rio_notify` / clipboard toasts waiting for the
    /// existing notifications drain.
    pub editor_notifications: VecDeque<RioNotify>,
    /// Daemon-side yank flashes waiting for the existing overlay drain.
    pub editor_yank_flashes: VecDeque<YankFlashNotification>,
    /// Latest `vim.diagnostic.get(0)` snapshot for the status line and
    /// the diagnostics popup. Replaced wholesale on each
    /// `rio_diagnostics` notify — items are already truncated and
    /// severity-sorted upstream.
    pub editor_diagnostics:
        Option<neoism_backend::performer::nvim::DiagnosticsNotification>,
    /// LSP servers currently attached to this buffer. Maintained by
    /// merging each `rio_lsp_status` notification (server name keys);
    /// "missing" / "none" states leave the list empty. Drives the
    /// status-line LSP popup and any future per-buffer LSP UI.
    pub attached_lsps: Vec<neoism_backend::performer::nvim::LspStatusNotification>,
    /// Latest comprehensive snapshot of every LSP server registered
    /// for the buffer's filetype — attached + candidate-not-yet-
    /// attached + errored. Replaces `attached_lsps` for the
    /// status-line popup (which uses this directly to render the
    /// Zed-style "all servers + state" list). Updated on BufEnter,
    /// LspAttach, and LspDetach by the embedded nvim runtime.
    pub lsp_snapshot: Option<neoism_backend::performer::nvim::LspSnapshotNotification>,
    /// Last `vim.notify` text we attributed to each LSP server name,
    /// keyed by server name. Lets the popup paint the most recent
    /// stderr / startup error per row.
    pub lsp_messages: std::collections::BTreeMap<
        String,
        neoism_backend::performer::nvim::LspMessageNotification,
    >,
    /// Active editor file path mirrored from nvim's `bufname` so the
    /// status_line + LSP popup can show "lib.rs" / "main.py" without
    /// re-resolving each frame.
    pub editor_path: Option<std::path::PathBuf>,
    /// Rust-rendered markdown document surface. Mutually exclusive with
    /// `editor` / real PTY content for workspace markdown tabs.
    pub markdown: Option<MarkdownPane>,
    /// Rust-rendered `.neodraw` sketch surface. Mutually exclusive with
    /// terminal/editor/markdown content, like the other Rust panes.
    pub draw: Option<DrawPane>,
    /// Rust-rendered `.ipynb` notebook surface. Owns notebook JSON while
    /// reusing the virtualized markdown renderer for cell presentation.
    pub notebook: Option<NotebookPane>,
    /// Rust-rendered Neoism agent chat surface. Mutually exclusive with
    /// terminal/editor/markdown content.
    pub neoism_agent: Option<NeoismAgentPane>,
    /// Rust-rendered Neoism workspace tags surface. Mutually exclusive
    /// with terminal/editor/markdown/agent content.
    pub neoism_tags: Option<NeoismTagsPane>,
    /// Rust-rendered Extensions browser surface. Mutually exclusive
    /// with terminal/editor/markdown/agent/tags content.
    pub neoism_extensions: Option<NeoismExtensionsPane>,
}

impl<T: neoism_backend::event::EventListener> Drop for Context<T> {
    fn drop(&mut self) {
        // Shutdown the terminal's PTY.
        let _ = self.messenger.channel.send(Msg::Shutdown);

        // Editor panes have no PTY and use a sentinel `shell_pid`; killing
        // a sentinel PID would target an unrelated process. Daemon-backed
        // panes (8A) report `shell_pid == 0` — `kill(0, SIGHUP)` would HUP
        // our OWN process group; their shell is owned by the daemon and
        // torn down via the `ClosePty` the machine's shutdown emits.
        #[cfg(not(target_os = "windows"))]
        if self.remote_pty.is_none()
            && self.editor.is_none()
            && self.markdown.is_none()
            && self.draw.is_none()
            && self.notebook.is_none()
            && self.neoism_agent.is_none()
            && self.neoism_tags.is_none()
            && self.neoism_extensions.is_none()
        {
            neoism_terminal_pty::kill_pid(self.shell_pid as i32);
        }
    }
}


mod context_pump;
mod context_editor;


/// One active Rust-engine completion popup. Owned by the frontend so
/// navigation/accept/dismiss stay snappy (no round trip), with insertion
/// applied via `SendKeys`.
#[derive(Clone, Debug)]
pub struct LspCompletionState {
    /// Identifier already typed before the cursor; backspaced on accept so
    /// prefix/member completion replaces cleanly.
    pub replace_prefix: String,
    /// Candidates, already sorted (preselect → sort_text → label).
    pub items: Vec<neoism_protocol::editor::EditorLspCompletionItem>,
    /// Selected row index into `items`.
    pub selected: usize,
}

/// Trim rust-analyzer's markdown hover for tooltip rendering. Keep code fences
/// so the hover popup can syntax-highlight signatures/snippets, but drop
/// section rules and cap the height so the popup stays a tooltip rather than a
/// wall of text.
fn hover_doc_lines(contents: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in contents.lines() {
        let trimmed = raw.trim_end();
        let lead = trimmed.trim_start();
        if lead == "---" || lead == "___" || lead == "***" {
            continue;
        }
        lines.push(trimmed.to_string());
    }
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.truncate(32);
    lines
}

/// One active VS Code-style hover popup. Owned by the frontend; shown while
/// the mouse rests over a symbol and dismissed when it moves off / a key is
/// pressed / the view scrolls.
#[derive(Clone, Debug)]
pub struct LspHoverState {
    /// Grid cell (row, col) the popup anchors under — the cell the mouse was
    /// over when the request fired.
    pub anchor_row: u32,
    pub anchor_col: u32,
    /// Rendered hover doc lines; markdown fences are preserved for syntax
    /// highlighting by the hover popup.
    pub lines: Vec<String>,
}

/// One locally-predicted (mosh-style) editor cell awaiting nvim's
/// authoritative confirmation.
#[derive(Clone, Copy, Debug)]
pub struct PredictedEditorCell {
    pub grid: u64,
    pub row: u64,
    pub col: u64,
    pub at: std::time::Instant,
}

/// How long a predicted cell may live unconfirmed before it is
/// reverted to blank. Long enough for a slow peer round trip, short
/// enough that a misprediction reads as a flicker, not a ghost.
const EDITOR_PREDICTION_TTL: std::time::Duration = std::time::Duration::from_millis(400);

pub enum EditorBackend {
    Local(NvimEmbedMachine),
    Daemon(DaemonEditorBackend),
}


mod backends;


pub struct DaemonEditorBackend {
    surface_id: String,
    handle: DaemonClientHandle,
    runtime: Option<tokio::runtime::Handle>,
    config: NvimSpawnConfig,
    send_tx: Option<tokio_mpsc::UnboundedSender<EditorClientMessage>>,
    /// Resize traffic is latest-wins and separate from input/commands. A
    /// window-edge drag must never fill the ordered command queue (or the
    /// websocket) with dimensions that are obsolete before they are sent.
    resize_tx: Option<tokio_watch::Sender<(u32, u32)>>,
}


fn editor_message_to_redraw_events(message: EditorServerMessage) -> Vec<RedrawEvent> {
    match message {
        EditorServerMessage::Batch { messages, .. } => messages
            .into_iter()
            .flat_map(editor_message_to_redraw_events)
            .collect(),
        EditorServerMessage::GridResize {
            grid_id,
            width,
            height,
            ..
        } => vec![RedrawEvent::Resize {
            grid: grid_id as u64,
            width: width as u64,
            height: height as u64,
        }],
        EditorServerMessage::GridClear { grid_id, .. } => {
            vec![RedrawEvent::Clear {
                grid: grid_id as u64,
            }]
        }
        EditorServerMessage::GridScroll {
            grid_id,
            top,
            bot,
            left,
            right,
            rows,
            cols,
            ..
        } => vec![RedrawEvent::Scroll {
            grid: grid_id as u64,
            top: top as u64,
            bottom: bot as u64,
            left: left as u64,
            right: right as u64,
            rows: rows as i64,
            columns: cols as i64,
        }],
        EditorServerMessage::CursorGoto {
            grid_id, row, col, ..
        } => vec![RedrawEvent::CursorGoto {
            grid: grid_id as u64,
            row: row as u64,
            column: col as u64,
        }],
        EditorServerMessage::HighlightDefined { hl_id, attrs, .. } => {
            vec![RedrawEvent::HighlightAttributesDefine {
                id: hl_id,
                style: style_from_highlight_attrs(attrs),
            }]
        }
        EditorServerMessage::DefaultColors {
            rgb_fg,
            rgb_bg,
            rgb_sp,
            ..
        } => vec![RedrawEvent::DefaultColorsSet {
            colors: NvimColors {
                foreground: Some(PackedColor(rgb_fg)),
                background: Some(PackedColor(rgb_bg)),
                special: Some(PackedColor(rgb_sp)),
            },
        }],
        EditorServerMessage::WinViewport {
            grid_id,
            topline,
            botline,
            line_count,
            scroll_delta,
            curline,
            curcol,
            textoff,
            ..
        } => vec![RedrawEvent::WinViewport {
            grid: grid_id as u64,
            topline,
            botline,
            line_count,
            scroll_delta,
            curline,
            curcol,
            textoff,
        }],
        EditorServerMessage::PopupMenu {
            items,
            selected,
            anchor,
            grid_id,
            ..
        } => {
            let max_word_chars = items
                .iter()
                .map(|item| item.word.chars().count())
                .max()
                .unwrap_or(0);
            vec![RedrawEvent::PopupMenuShow {
                menu: PopupMenu {
                    items: items
                        .into_iter()
                        .map(|item| PopupMenuItem {
                            word: item.word,
                            kind: item.kind,
                            menu: item.menu,
                            info: item.info,
                        })
                        .collect(),
                    selected: selected.map(i64::from).unwrap_or(-1),
                    row: anchor.row as u64,
                    col: anchor.col as u64,
                    grid: grid_id as u64,
                    max_word_chars,
                },
            }]
        }
        EditorServerMessage::PopupMenuSelect { selected, .. } => {
            vec![RedrawEvent::PopupMenuSelect {
                selected: selected.map(i64::from).unwrap_or(-1),
            }]
        }
        EditorServerMessage::PopupHide { .. } => vec![RedrawEvent::PopupMenuHide],
        EditorServerMessage::ModeChange { mode, mode_idx, .. } => {
            vec![RedrawEvent::ModeChange {
                mode: editor_mode_from_wire(&mode),
                mode_index: mode_idx as u64,
            }]
        }
        EditorServerMessage::GridUpdate {
            grid_id,
            cells,
            cursor,
            mode,
            ..
        } => {
            let mut events = Vec::with_capacity(cells.len().saturating_add(2));
            let mut defined_highlights = std::collections::HashSet::new();
            let mut pending_line: Option<(u64, u64, u64, Vec<GridLineCell>)> = None;
            for cell in cells {
                let hl_id = synthetic_highlight_id(cell.fg, cell.bg, cell.attrs);
                if defined_highlights.insert(hl_id) {
                    events.push(RedrawEvent::HighlightAttributesDefine {
                        id: hl_id,
                        style: style_from_wire_cell(cell.fg, cell.bg, cell.attrs),
                    });
                }
                let row = cell.row as u64;
                let col = cell.col as u64;
                let line_cell = GridLineCell {
                    text: cell.ch,
                    highlight_id: Some(hl_id),
                    repeat: None,
                };
                match pending_line.as_mut() {
                    Some((line_row, _start_col, next_col, line_cells))
                        if *line_row == row && *next_col == col =>
                    {
                        *next_col = next_col.saturating_add(1);
                        line_cells.push(line_cell);
                    }
                    _ => {
                        if let Some((line_row, start_col, _next_col, line_cells)) =
                            pending_line.take()
                        {
                            events.push(RedrawEvent::GridLine {
                                grid: grid_id as u64,
                                row: line_row,
                                column_start: start_col,
                                cells: line_cells,
                            });
                        }
                        pending_line =
                            Some((row, col, col.saturating_add(1), vec![line_cell]));
                    }
                }
            }
            if let Some((line_row, start_col, _next_col, line_cells)) =
                pending_line.take()
            {
                events.push(RedrawEvent::GridLine {
                    grid: grid_id as u64,
                    row: line_row,
                    column_start: start_col,
                    cells: line_cells,
                });
            }
            if let Some(cursor) = cursor {
                events.push(RedrawEvent::CursorGoto {
                    grid: grid_id as u64,
                    row: cursor.row as u64,
                    column: cursor.col as u64,
                });
            }
            if let Some(mode) = mode {
                events.push(RedrawEvent::ModeChange {
                    mode: editor_mode_from_wire(&mode),
                    mode_index: 0,
                });
            }
            events
        }
        EditorServerMessage::Closed { .. } => vec![RedrawEvent::Destroy { grid: 1 }],
        EditorServerMessage::BufferOpened { .. }
        | EditorServerMessage::BufferModified { .. }
        | EditorServerMessage::Notification { .. }
        | EditorServerMessage::YankFlash { .. }
        | EditorServerMessage::Diagnostics { .. }
        | EditorServerMessage::LspStatus { .. }
        | EditorServerMessage::LspSnapshot { .. }
        | EditorServerMessage::LspMessage { .. }
        | EditorServerMessage::LspActionResult { .. }
        | EditorServerMessage::LspCompletions { .. }
        | EditorServerMessage::LspHoverResult { .. }
        | EditorServerMessage::MouseMode { .. }
        | EditorServerMessage::Message { .. }
        | EditorServerMessage::Error { .. } => Vec::new(),
    }
}

fn editor_mode_from_wire(mode: &str) -> EditorMode {
    match mode {
        "normal" | "n" => EditorMode::Normal,
        "insert" | "i" => EditorMode::Insert,
        "visual" | "v" | "V" | "\u{16}" => EditorMode::Visual,
        "replace" | "r" | "R" => EditorMode::Replace,
        "cmdline" | "c" => EditorMode::CmdLine,
        other => EditorMode::Unknown(other.to_string()),
    }
}

fn notify_level_from_wire(level: &str) -> neoism_backend::performer::nvim::NotifyLevel {
    use neoism_backend::performer::nvim::NotifyLevel;
    match level.to_ascii_lowercase().as_str() {
        "warn" | "warning" => NotifyLevel::Warn,
        "error" | "err" => NotifyLevel::Error,
        _ => NotifyLevel::Info,
    }
}

fn lsp_locations_preview(
    locations: &[neoism_protocol::editor::EditorLspLocation],
) -> String {
    locations
        .iter()
        .take(5)
        .map(|location| {
            format!(
                "{}:{}:{}",
                location.uri,
                location.line.saturating_add(1),
                location.character.saturating_add(1)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn synthetic_highlight_id(fg: u32, bg: u32, attrs: u8) -> u64 {
    (1u64 << 56) | ((attrs as u64) << 48) | ((fg as u64) << 24) | (bg as u64)
}

fn style_from_wire_cell(fg: u32, bg: u32, attrs: u8) -> NvimStyle {
    NvimStyle {
        colors: NvimColors {
            foreground: Some(PackedColor(fg)),
            background: Some(PackedColor(bg)),
            special: None,
        },
        bold: attrs & 0b0000_0001 != 0,
        italic: attrs & 0b0000_0010 != 0,
        underline: if attrs & 0b0000_1000 != 0 {
            UnderlineStyle::UnderCurl
        } else if attrs & 0b0000_0100 != 0 {
            UnderlineStyle::Underline
        } else {
            UnderlineStyle::None
        },
        strikethrough: attrs & 0b0001_0000 != 0,
        reverse: attrs & 0b0010_0000 != 0,
        ..NvimStyle::default()
    }
}

fn style_from_highlight_attrs(attrs: HighlightAttrs) -> NvimStyle {
    NvimStyle {
        colors: NvimColors {
            foreground: attrs.fg.map(PackedColor),
            background: attrs.bg.map(PackedColor),
            special: attrs.sp.map(PackedColor),
        },
        bold: attrs.bold,
        italic: attrs.italic,
        underline: if attrs.undercurl {
            UnderlineStyle::UnderCurl
        } else if attrs.underline {
            UnderlineStyle::Underline
        } else {
            UnderlineStyle::None
        },
        strikethrough: attrs.strikethrough,
        reverse: attrs.reverse,
        ..NvimStyle::default()
    }
}


#[cfg(test)]
mod tests;
