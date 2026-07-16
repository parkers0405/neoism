// MIT License
// Copyright 2022-present Raphael Amorim
//
// The functions (including comments) and logic of process_key_event, build_key_sequence, process_mouse_bindings, copy_selection, start_selection, update_selection_scrolling,
// side_by_pos, on_left_click, paste, sgr_mouse_report, mouse_report, normal_mouse_report, scroll,
// were retired from https://github.com/alacritty/alacritty/blob/c39c3c97f1a1213418c3629cc59a1d46e34070e0/alacritty/src/input.rs
// which is licensed under Apache 2.0 license.

/// One hidden grid row above each editor pane's visible area.
///
/// Older versions reserved 64 rows above and below because the scroll
/// spring itself could lag by that many rows. The current renderer
/// samples large integer lag directly into the visible rows via
/// `source_line_offset`; only the fractional edge needs a real offscreen
/// row. Keeping the buffer at one row avoids drawing/allocating 126
/// permanently hidden rows on every nvim frame.
pub const EDITOR_BUFFER_ABOVE: u32 = 1;
pub const TERMINAL_BUFFER_ABOVE: u32 = 1;
pub(crate) const BUILTIN_SHADER_OVERLAY_CHOICES: &[&str] = &["builtin:ctv_round"];
// "builtin:hypno_crt" remains shipped in Sugarloaf, but is hidden from the
// picker for now.

/// Symmetric to `EDITOR_BUFFER_ABOVE`: one hidden row below the visible
/// viewport for the fractional row entering from the bottom.
pub const EDITOR_BUFFER_BELOW: u32 = 1;
pub const TERMINAL_BUFFER_BELOW: u32 = 1;
const TERMINAL_BOTTOM_CLIP_BLEED_PX: f32 = 2.0;
const TERMINAL_TOP_BREATHING_PX: f32 = 6.0;

/// No visible gap between Rust chrome and the editor grid. Edge bleed is
/// handled by clearing/masking the scroll-buffer rows, not by reserving
/// a black safety band the user can see.
const CHROME_SAFETY_PAD: f32 = 0.0;
const SCROLL_LOG_ENV: &str = "NEOISM_SCROLL_LOG";
const EDITOR_GEOMETRY_LOG_ENV: &str = "NEOISM_EDITOR_GEOMETRY_LOG";
const LSP_LOG_ENV: &str = "NEOISM_LSP_LOG";
const BLOCK_LOG_ENV: &str = "NEOISM_BLOCK_LOG";

pub(crate) type WorkspaceKey = String;

fn block_log_enabled() -> bool {
    std::env::var_os(BLOCK_LOG_ENV).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EditorGeometryLogState {
    class: u8,
    route_id: usize,
    current_route: usize,
    rows: u32,
    visible_rows: u32,
    cols: u32,
    status_top_px: i32,
    clip_bottom_px: i32,
    last_row_bottom_px: i32,
    row_status_delta_px: i32,
    layout_bottom_px: i32,
    layout_status_delta_px: i32,
    bottom_row_clear: bool,
    penultimate_row_clear: bool,
    buffer_tab_count: u32,
    buffer_tabs_visible: bool,
    breadcrumbs_visible: bool,
    split_count: u32,
}

fn block_snapshot_debug(
    snapshots: &[crate::terminal::blocks::CommandBlockSnapshot],
) -> String {
    snapshots
        .iter()
        .enumerate()
        .map(|(idx, block)| {
            format!(
                "{}:{}@{:?}",
                idx,
                block.command.replace('\n', "\\n"),
                block.output_start_row
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn block_span_debug(spans: &[crate::terminal::blocks::BlockHeaderSpan]) -> String {
    spans
        .iter()
        .map(|span| {
            format!(
                "{}:{}..{} first={} count={}",
                span.block_idx,
                span.start_display_row,
                span.end_display_row,
                span.first_chrome_row,
                span.chrome_row_count
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(not(target_arch = "wasm32"))]
fn acp_update_primary_path(update: &serde_json::Value) -> Option<PathBuf> {
    update
        .get("locations")
        .and_then(serde_json::Value::as_array)
        .and_then(|locations| locations.first())
        .and_then(|location| location.get("path"))
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .or_else(|| {
            update
                .get("content")
                .and_then(serde_json::Value::as_array)
                .and_then(|content| {
                    content.iter().find_map(|item| {
                        item.get("path")
                            .and_then(serde_json::Value::as_str)
                            .map(PathBuf::from)
                    })
                })
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn acp_permission_option_id(option: &serde_json::Value) -> Option<String> {
    option
        .get("optionId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[cfg(not(target_arch = "wasm32"))]
fn acp_permission_option_label(option: &serde_json::Value) -> String {
    option
        .get("name")
        .and_then(serde_json::Value::as_str)
        .or_else(|| option.get("kind").and_then(serde_json::Value::as_str))
        .or_else(|| option.get("optionId").and_then(serde_json::Value::as_str))
        .unwrap_or("Choose")
        .to_string()
}

#[cfg(not(target_arch = "wasm32"))]
fn acp_pretty_json(value: &serde_json::Value, max_chars: usize) -> String {
    let mut text =
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    if text.len() > max_chars {
        let mut cut = max_chars.min(text.len());
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n...");
    }
    text
}

// Pure scroll-policy math now lives in `neoism_ui::render_policy`.
// Re-export the public types/functions under their historical local
// paths so call sites inside this fork don't churn.
use neoism_ui::render_policy::{
    animation_phase_from_unix_secs, block_header_panel_geometry,
    block_header_row_metrics, block_hover_icon_anchor_row, block_hover_icon_layout,
    block_status_color_token, block_status_glyph,
    editor_scroll_render_offset_for_mutated_snapshot as ui_editor_scroll_render_offset,
    editor_scroll_shifted_row_count, loader_animation_frame, loader_orbit_position,
    loader_pastel_color, BlockHeaderPanelGeometryInput, BlockHoverIconLayoutInput,
    BlockStatusColorToken, EditorScrollGridRenderState, EditorScrollRenderOffset,
};
#[cfg(test)]
use neoism_ui::render_policy::{
    editor_cursor_output_row, editor_scroll_effective_source_base,
    editor_scroll_render_state_changes as ui_editor_scroll_render_state_changes,
    editor_scroll_source_plan as ui_editor_scroll_source_plan, EditorScrollSourcePlan,
};

#[derive(Debug, Default)]
struct EditorScrollGridState {
    render: EditorScrollGridRenderState,
    /// `source_y` whose row content is currently emitted at the
    /// top edge slot (`EDITOR_BUFFER_ABOVE - 1`). `None` means the
    /// slot is currently cleared. We only rewrite the slot when this
    /// value would change between frames — re-emitting the same source
    /// row every frame for a smooth fractional scroll was costing
    /// hundreds of redundant cell writes plus a full bg/fg re-upload.
    edge_above_source_y: Option<i32>,
    /// Same idea for the bottom edge slot (`EDITOR_BUFFER_ABOVE +
    /// visible_rows`).
    edge_below_source_y: Option<i32>,
    log_started_at: Option<std::time::Instant>,
    log_frames: u32,
    log_rebuilt_rows: u32,
    log_exposed_rows: u32,
    log_damage_rows: u32,
    log_full_rows: u32,
    log_shifted_rows: u32,
    log_source_changes: u32,
    log_offset_updates: u32,
    /// Microseconds spent inside `Screen::render` summed across the
    /// scroll log window. Divide by `log_frames` for mean ms/frame.
    log_render_us: u64,
    /// Worst single-frame render duration in microseconds inside the
    /// scroll log window. Surfaces stalls that the mean would hide.
    log_render_us_max: u64,
    /// Sum of *full* render durations (the previous frame's complete
    /// CPU+Vulkan-present time) over the log window.
    log_full_render_us: u64,
    /// Worst full-frame duration over the log window.
    log_full_render_us_max: u64,
    /// Sum/worst animation deltas fed into the editor scroll spring.
    log_animation_dt_us: u64,
    log_animation_dt_us_max: u64,
}

/// Which splash menu option is being opened — used by
/// `dismiss_other_modals` to know which floating panel NOT to
/// close (we're about to open it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplashModalKind {
    None,
    Finder,
    CommandPalette,
}

// Thin shim: forward to the shared pure-policy helper in neoism-ui.
// The signature/return type are unchanged so call sites compile as-is.
fn editor_scroll_render_offset(
    scroll_position_lines: f32,
    elastic_offset_y: f32,
    cell_h: f32,
    previous_source_line_offset: Option<i32>,
) -> EditorScrollRenderOffset {
    ui_editor_scroll_render_offset(
        scroll_position_lines,
        elastic_offset_y,
        cell_h,
        previous_source_line_offset,
    )
}

// Native-side wrapper that adapts the shared 3-value previous-state
// signature to the `EditorScrollGridState` reference we carry per
// grid. Pure forwarding logic — no state mutation.
#[cfg(test)]
fn editor_scroll_state_changes(
    previous: Option<&EditorScrollGridState>,
    current: EditorScrollRenderOffset,
    current_scrollback_origin: Option<isize>,
) -> (bool, bool) {
    ui_editor_scroll_render_state_changes(
        previous.map(|s| s.render),
        current,
        current_scrollback_origin,
    )
}

// `editor_scroll_source_plan` is re-exported from `neoism_ui::render_policy`
// at the top of the file (aliased to `ui_editor_scroll_source_plan` so we
// can wrap it here without name clash).
#[cfg(test)]
fn editor_scroll_source_plan(
    previous_source_base: Option<i64>,
    current_source_base: i64,
    visible_rows: usize,
) -> EditorScrollSourcePlan {
    ui_editor_scroll_source_plan(previous_source_base, current_source_base, visible_rows)
}

fn line_for_absolute_row(abs_row: usize, history_size: usize) -> Line {
    let line = abs_row as i64 - history_size as i64;
    Line(line.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

fn visible_index_for_absolute_row(
    abs_row: usize,
    history_size: usize,
    display_offset: i32,
) -> Option<usize> {
    let idx = abs_row as i64 - history_size as i64 + display_offset as i64;
    (idx >= 0).then_some(idx as usize)
}

fn composed_display_row_for_abs(
    source_row_indices: &[Option<usize>],
    abs_row: usize,
) -> Option<u16> {
    source_row_indices
        .iter()
        .position(|source| *source == Some(abs_row))
        .map(|row| row.min(u16::MAX as usize) as u16)
}

fn terminal_row_is_empty(row: &Row<Square>) -> bool {
    row.inner.iter().all(|cell| {
        if cell.is_bg_only() || cell.has_graphics() {
            return false;
        }
        let c = cell.c();
        (c == ' ' || c == '\0' || c == '\t')
            && cell.style_id()
                == neoism_terminal_core::crosswords::style::DEFAULT_STYLE_ID
            && cell.extras_id().is_none()
    })
}

fn block_row_visual_height(
    abs_row: usize,
    snapshots: &[crate::terminal::blocks::CommandBlockSnapshot],
    echo_rows: Option<&BTreeSet<usize>>,
) -> usize {
    if echo_rows.is_some_and(|rows| rows.contains(&abs_row))
        || snapshots
            .iter()
            .any(|block| block.output_start_row == Some(abs_row))
    {
        crate::terminal::blocks::COMMAND_BLOCK_CHROME_ROWS
    } else {
        1
    }
}

fn block_scroll_cursor_or_anchor(
    existing: Option<crate::terminal::scroll::BlockScrollCursor>,
    anchor_abs: usize,
) -> crate::terminal::scroll::BlockScrollCursor {
    // The block cursor is a VIRTUAL stream cursor. Its raw row is not
    // expected to equal the raw terminal viewport top once two-row
    // chrome has expanded any command echoes above it. Raw
    // display_offset is only the backing-grid carrier.
    existing.unwrap_or(crate::terminal::scroll::BlockScrollCursor {
        raw_top_abs: anchor_abs,
        chrome_row: 0,
    })
}

fn drop_composer_owned_prompt_row(
    rows: &mut Vec<Row<Square>>,
    sources: &mut Vec<usize>,
    prompt_abs_row: Option<usize>,
) {
    let Some(prompt_abs_row) = prompt_abs_row else {
        return;
    };

    let Some(index) = sources.iter().position(|&source| source == prompt_abs_row) else {
        return;
    };

    // Once the command composer owns input, this live prompt row is
    // chrome, not terminal output. Shells can still paint cwd/git
    // glyphs into it, so content cannot be the condition for keeping
    // it in the composed terminal stream.
    rows.remove(index);
    sources.remove(index);
}

#[allow(clippy::too_many_arguments)]
fn sync_composed_terminal_image_overlays(
    sugarloaf: &mut Sugarloaf<'_>,
    terminal: &neoism_terminal_core::crosswords::Crosswords,
    rich_text_id: usize,
    frame_rows: &[Row<Square>],
    frame_source_rows: &[Option<usize>],
    style_set: &neoism_terminal_core::crosswords::style::StyleSet,
    origin_x: f32,
    origin_y: f32,
    cell_width: f32,
    cell_height: f32,
) {
    let has_direct = !terminal.graphics.kitty_placements.is_empty();
    let has_virtual = !terminal.graphics.kitty_virtual_placements.is_empty();
    if !has_direct && !has_virtual {
        sugarloaf.clear_image_overlays_for(rich_text_id);
        return;
    }

    let images = terminal.graphics.kitty_images.clone();
    if images.is_empty() {
        sugarloaf.clear_image_overlays_for(rich_text_id);
        return;
    }

    let overlays = sugarloaf.image_overlays.entry(rich_text_id).or_default();
    overlays.clear();

    if has_direct {
        let mut placements = terminal
            .graphics
            .kitty_placements
            .values()
            .filter(|placement| images.contains_key(&placement.image_id))
            .cloned()
            .collect::<Vec<_>>();
        placements.sort_by_key(|placement| placement.z_index);

        for placement in placements {
            let image_start = placement.dest_row;
            let image_end = image_start + placement.rows as i64;
            let Some((display_row, source_row)) = frame_source_rows
                .iter()
                .enumerate()
                .find_map(|(display_row, source)| {
                    let source_row = (*source)? as i64;
                    (source_row >= image_start && source_row < image_end)
                        .then_some((display_row, source_row))
                })
            else {
                continue;
            };
            let hidden_rows_above = source_row - image_start;
            overlays.push(neoism_backend::sugarloaf::GraphicOverlay {
                image_id: placement.image_id,
                x: origin_x
                    + placement.dest_col as f32 * cell_width
                    + placement.cell_x_offset as f32,
                y: origin_y
                    + (display_row as f32 - hidden_rows_above as f32) * cell_height
                    + placement.cell_y_offset as f32,
                width: placement.pixel_width as f32,
                height: placement.pixel_height as f32,
                z_index: placement.z_index,
                source_rect: neoism_backend::sugarloaf::GraphicOverlay::FULL_SOURCE_RECT,
            });
        }
    }

    if has_virtual {
        let snapshot = crate::context::renderable::TerminalSnapshot {
            colors: terminal.colors,
            display_offset: terminal.display_offset(),
            blinking_cursor: terminal.blinking_cursor,
            visible_rows: frame_rows.to_vec(),
            style_set: style_set.clone(),
            extras_table: terminal.grid.extras_table.clone(),
            cursor: terminal.cursor(),
            damage: neoism_terminal_core::damage::TerminalDamage::Noop,
            columns: terminal.columns(),
            screen_lines: frame_rows.len(),
            history_size: terminal.history_size(),
            kitty_virtual_placements: terminal.graphics.kitty_virtual_placements.clone(),
            kitty_images: images,
            kitty_placements: Vec::new(),
            kitty_graphics_dirty: terminal.graphics.kitty_graphics_dirty,
        };
        crate::host::Renderer::push_virtual_placeholder_overlays(
            overlays,
            &snapshot,
            origin_x,
            origin_y,
            cell_width,
            cell_height,
        );
    }
}

fn starts_passthrough_session(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else {
        return false;
    };
    let program = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    match program {
        // Bare POSIX sh has no injected Neoism command-block hooks.
        "sh" => parts.next().is_none(),
        // Remote shells need their own bootstrap to be block-aware.
        // Until then, keep the PTY raw so the remote cursor/prompt is
        // not fighting the local command composer.
        "ssh" | "mosh" => true,
        _ => false,
    }
}

fn ends_passthrough_session(command: &str) -> bool {
    matches!(command.trim(), "exit" | "logout")
}

use crate::app::window_event::touch::TouchPurpose;
use crate::bindings::{
    Action as Act, BindingKey, BindingMode, FontSizeAction, MouseBinding, SearchAction,
    ViAction,
};
use crate::context;
use crate::context::renderable::{Cursor, RenderableContent};
use crate::context::{next_rich_text_id, process_open_url, ContextManager};
use crate::host::Renderer;
use crate::input::mouse::{calculate_mouse_position, Mouse};
use crate::layout::ContextDimension;
use crate::terminal::hints::HintState;
use core::fmt::Debug;
use neoism_backend::config::layout::Margin;
use neoism_backend::config::renderer::Backend;
use neoism_backend::error::{RioError, RioErrorLevel, RioErrorType};
use neoism_backend::event::{ClickState, EventProxy, SearchState};
use neoism_backend::sugarloaf::{
    layout::RootStyle, Sugarloaf, SugarloafBackend, SugarloafErrors, SugarloafRenderer,
    SugarloafWindow, SugarloafWindowSize,
};
use neoism_terminal_core::crosswords::pos::{CursorState, Line};
use neoism_terminal_core::crosswords::search::RegexSearch;
use neoism_terminal_core::crosswords::{
    grid::{row::Row, Dimensions, Scroll},
    pos::{Column, Pos, Side},
    square::Square,
    vi_mode::ViMotion,
    Mode,
};
use neoism_ui::terminal_blocks::hint::HintMatches;
use neoism_ui::utils::padding_top_from_config;
use neoism_window::event::Modifiers;
use neoism_window::event::MouseButton;
#[cfg(target_os = "macos")]
use neoism_window::keyboard::ModifiersKeyState;
use neoism_window::keyboard::{KeyCode, KeyLocation, ModifiersState, PhysicalKey};
use neoism_window::platform::modifier_supplement::KeyEventExtModifierSupplement;
use notify::Watcher as _;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Maximum number of lines for the blocking search while still typing the search regex.
const MAX_SEARCH_WHILE_TYPING: Option<usize> = Some(1000);
const BLOCK_ICON_CLICK_MS: f32 = 220.0;
const TERMINAL_BLOCK_CHROME_ORDER: u8 = 10;
const TERMINAL_BLOCK_CHROME_ACTIVE_ORDER: u8 = 11;
// Git status can echo through the `.git` watcher; keep those self-events
// from recursively spawning another git status process.
const FILE_TREE_GIT_SELF_EVENT_SUPPRESS: Duration = Duration::from_millis(1200);

/// Maximum number of search terms stored in the history.
const MAX_SEARCH_HISTORY_SIZE: usize = 255;
static SCRATCH_BUFFER_ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// A decoded `cover:` banner registered with sugarloaf — see
/// `markdown_cover_cache`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MarkdownCoverImage {
    pub(crate) image_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Copy, Debug)]
struct FileTreeResizeState {
    start_x: f32,
    original_width: f32,
}

#[derive(Clone, Copy, Debug)]
struct NotesSidebarResizeState {
    start_x: f32,
    original_width: f32,
}

/// Live drag state for resizing the git diff panel via its leading
/// edge. Symmetric with `FileTreeResizeState`; `start_x` is the mouse
/// x where the drag began so deltas land on the original width.
#[derive(Clone, Copy, Debug)]
struct GitDiffPanelResizeState {
    start_x: f32,
    original_width: f32,
}

/// Live drag state for grabbing one of the panel's scrollbar thumbs.
#[derive(Clone, Copy, Debug)]
struct GitDiffPanelScrollbarDragState {
    kind: crate::editor::git_diff_panel::ScrollbarKind,
}

/// A workspace lifted out of a window by a detach gesture, carrying its
/// live grid plus the per-workspace chrome state keyed by the grid's
/// stable daemon workspace id so the destination window can restore the
/// right buffer-tab strip / file-tree root.
pub(crate) struct DetachedWorkspace {
    pub grid: crate::layout::grid::ContextGrid<EventProxy>,
    pub workspace_id: Option<WorkspaceKey>,
    pub root: Option<std::path::PathBuf>,
    pub buffer_tabs: Option<
        neoism_ui::panels::buffer_tabs::BufferTabs<crate::neoism::icon::AgentKind>,
    >,
    pub buf_enter_target: Option<Option<std::path::PathBuf>>,
    pub editor_active_path: Option<std::path::PathBuf>,
}

/// A workspace in some window, surfaced to another window's right-click
/// menu so a tab can be moved into it even across OS windows (e.g. a
/// detached workspace). The app rebuilds this list from the router.
pub(crate) struct CrossWindowWorkspace {
    pub window_id: u64,
    pub workspace: usize,
    pub title: String,
}

/// Transient payload handed from a source window's `Screen` to a target
/// window's `Screen` during a cross-window buffer-tab move.
pub(crate) enum CrossWindowTabPayload {
    /// A live terminal context (PTY intact) plus its route id.
    Terminal {
        context: crate::context::Context<EventProxy>,
        route_id: usize,
    },
    /// A path-backed tab (nvim / markdown / file) re-opened in the target.
    Path(std::path::PathBuf),
}

/// State for an in-place "Draw on Note" session — a real [`DrawPane`]
/// (toolbar, tools, undo/redo, dirty tracking) composited over the rendered
/// markdown, with its camera locked to the note's scroll.
///
/// [`DrawPane`]: crate::editor::neodraw::DrawPane
pub(crate) struct DrawOverNote {
    /// The `.md` note being drawn on.
    pub note: std::path::PathBuf,
    /// The ink layer as a full draw pane (strokes in content coords).
    pub pane: crate::editor::neodraw::DrawPane,
}

/// Inputs whose transition requires a full chrome/grid reflow. Keeping this
/// signature at the screen boundary repairs async pane-type changes (terminal
/// -> editor) even when the code path that completed the transition did not
/// originate from a resize event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChromeLayoutSignature {
    route_id: usize,
    reserves_editor_chrome: bool,
    editor_top_bits: u32,
    terminal_top_bits: u32,
    bottom_bits: u32,
    buffer_tabs_present: bool,
    pane_tab_strip_count: usize,
    pane_breadcrumb_count: usize,
}

pub struct Screen<'screen> {
    bindings: crate::bindings::KeyBindings,
    mouse_bindings: Vec<MouseBinding>,
    pub modifiers: Modifiers,
    pub mouse: Mouse,
    pub touchpurpose: TouchPurpose,
    pub search_state: SearchState,
    pub hint_state: HintState,
    pub renderer: Renderer,
    /// Last chrome state that completed a full grid reflow. Breadcrumb and
    /// editor activation can finish asynchronously; a mismatch is repaired
    /// in status sync before the grid is painted.
    last_chrome_layout_signature: Option<ChromeLayoutSignature>,
    /// One-shot guard for the first-run welcome reveal. `Screen::new`
    /// returns an inline `Ok(Screen { .. })` so there is no `self` to call
    /// a startup method on; instead we flip this on the first `render`
    /// tick (panes/workspace fully wired by then) to auto-open the notes
    /// sidebar with `Welcome/` expanded, gated further by the on-disk
    /// first-run marker. See [`Self::reveal_welcome_notes_first_run`].
    welcome_reveal_pending: bool,
    /// Live-`/`-search redraw pump. `dispatch_palette_search_query` sets
    /// this to a short deadline when it fires an async `rio_search_matches`
    /// query at nvim; the per-frame status-sync loop keeps marking dirty
    /// until then so the async reply is drained + previewed within a frame
    /// or two of each keystroke instead of only on the next input event
    /// (the "search only updates on Enter" bug). Pure redraw scheduling —
    /// it issues NO nvim RPC per frame, so it can't wedge input on a
    /// pending count.
    search_reply_pump_until: Option<std::time::Instant>,
    pending_notebook_executions: Vec<
        std::sync::mpsc::Receiver<(
            std::path::PathBuf,
            neoism_ui::editor::notebook::NotebookExecutionEvent,
        )>,
    >,
    pending_python_kernel_retry: Option<(std::path::PathBuf, usize)>,
    notebook_runtime: crate::notebook_runtime::NotebookRuntimeManager,
    /// Background reMarkable auto-sync (optional `remarkable` extension).
    /// Off by default; opt in with `NEOISM_RM_AUTOSYNC=1`.
    #[cfg(feature = "remarkable")]
    remarkable_autosync: Option<neoism_remarkable::RemarkableAutoSync>,
    /// Whether to paint the pulled reMarkable handwriting over the
    /// markdown note (toggle with [`Self::toggle_ink_overlay`]).
    show_ink_overlay: bool,
    /// Active in-place "Draw on Note" session: draw ink directly over the
    /// rendered markdown (content coords). `None` = not drawing.
    pub(crate) draw_over_note: Option<DrawOverNote>,
    /// Cache of a note's ink overlay (drawn over the rendered markdown, in
    /// content coordinates so it scrolls with the text), keyed by the `.md`
    /// path and refreshed when the sidecar `.neodraw` changes on disk.
    ink_overlay_cache: std::collections::HashMap<
        std::path::PathBuf,
        (std::time::SystemTime, Option<crate::editor::neodraw::Scene>),
    >,
    /// Per-panel `GridRenderer`, keyed by `route_id`. Lazily created
    /// on first render of each panel so construction (which compiles
    /// the Metal/WGSL shaders and builds pipeline states) runs once
    /// per panel lifetime. Removed when the panel closes.
    ///
    /// Phase 2.0: the grids are constructed and kept in sync with
    /// panel layout, but `sugarloaf.render_with_grids` is still
    /// called with an empty slice — so behavior is unchanged and
    /// this only validates that the shaders compile on real
    /// hardware. Phase 2.1/2.2 flip the switch.
    pub grids:
        rustc_hash::FxHashMap<usize, neoism_backend::sugarloaf::grid::GridRenderer>,
    // Drop order is declaration order. Vulkan grid renderers clone the
    // Sugarloaf device, so they must be dropped before `sugarloaf` tears
    // down the Vulkan context.
    pub sugarloaf: Sugarloaf<'screen>,
    pub context_manager: context::ContextManager<EventProxy>,
    /// A workspace lifted out by a detach gesture, parked here until the
    /// app loop (which owns `event_loop` + the router) spawns a new OS
    /// window and adopts it. See `detach_workspace_at`.
    pending_detached_workspace: Option<DetachedWorkspace>,
    /// A cross-window buffer-tab move parked by the right-click menu:
    /// `(tab_index, target_window_u64, target_workspace_index)`. The app
    /// loop (which can borrow both windows' routes) completes it.
    pending_cross_window_tab_move: Option<(usize, u64, usize)>,
    pub daemon_pane_layout: daemon_layout::ScreenPaneLayoutCache,
    /// Wave 7A multiplayer presence: remote peer cursors per buffer id,
    /// fed from daemon `CrdtReply` pushes. Renderer reads it per frame
    /// through `Screen::remote_cursors_for_path` (see screen/presence.rs).
    pub remote_presence: neoism_ui::editor::crdt::RemotePresenceStore,
    /// Coalescing publisher for the LOCAL cursor (lazily initialized
    /// with the device identity on first daemon presence pump).
    pub(crate) presence_publisher: Option<neoism_ui::editor::crdt::PresencePublisher>,
    /// Wave 7G: `[neoism] display-name` from the config file, captured
    /// at window construction. Feeds `local_presence_identity` when
    /// the presence publisher initializes (the `NEOISM_DISPLAY_NAME`
    /// env var still wins over this).
    pub(crate) presence_display_name_override: Option<String>,
    /// Wave 7B: per-document CRDT bindings for daemon-backed markdown
    /// panes (local edits → minimal ops, remote ops → incremental
    /// splices with caret transform). See `screen/markdown_crdt.rs`.
    pub markdown_crdt: markdown_crdt::MarkdownCrdtState,
    last_ime_cursor_pos: Option<(f32, f32)>,
    last_editor_trail_cursor_cell: Option<(usize, usize, usize)>,
    last_editor_key_log_at: Option<std::time::Instant>,
    last_editor_key_log_notation: Option<String>,
    hints_config: Vec<std::rc::Rc<neoism_backend::config::hints::Hint>>,
    pub resize_state: Option<crate::layout::ResizeState>,
    file_tree_resize_state: Option<FileTreeResizeState>,
    notes_sidebar_resize_state: Option<NotesSidebarResizeState>,
    git_diff_panel_resize_state: Option<GitDiffPanelResizeState>,
    git_diff_panel_scrollbar_drag: Option<GitDiffPanelScrollbarDragState>,
    // (constant lives just above the field's first use; declared as a
    // const item so the rest of the screen module — uniform calc,
    // write loop, snapshot writer — can reference the same value.)
    /// Per-window glyph rasterizer shared across panels. Owns a
    /// char → font resolution cache; the per-panel atlas lives on
    /// each `GridRenderer`.
    pub grid_rasterizer: crate::terminal::grid_emit::GridGlyphRasterizer,
    /// Per editor grid render state used to avoid reshaping/rebuilding
    /// every row for pure fractional scroll frames. Keyed by route_id,
    /// same as `grids`.
    editor_scroll_grid_states: rustc_hash::FxHashMap<usize, EditorScrollGridState>,
    /// Last bottom-boundary geometry signature logged for each editor
    /// pane. Keeps `NEOISM_EDITOR_GEOMETRY_LOG` useful without emitting
    /// every frame.
    editor_geometry_log_last: rustc_hash::FxHashMap<usize, EditorGeometryLogState>,
    shader_overlay_paths: Vec<String>,
    active_shader_overlay: Option<String>,
    /// Full render duration of the *previous* frame in microseconds —
    /// covers everything inside `Screen::render` including the
    /// trailing `sugarloaf.render_with_grids` (Vulkan submit + queue
    /// present, swapchain acquire fence wait, etc.). Surfaced through
    /// the editor scroll FPS log so we can see whether the missing
    /// budget at e.g. 145fps on a 165Hz display is in CPU emission
    /// (mean_render_ms above this) or in present pacing (the
    /// difference `1000/fps - full_render_ms` ≈ time spent waiting
    /// for the next vsync / RedrawRequested).
    last_full_render_us: u64,
    /// Leader-sequence buffer for editor panes. When the user presses
    /// `<space>` inside an editor, we hold it briefly to see if `e`
    /// follows (→ toggle file tree, nvim-tree style). Anything else
    /// flushes the buffered space and the new key through to nvim.
    leader_pending: Option<std::time::Instant>,
    /// Markdown normal-mode leader buffer. Mirrors the editor
    /// `<space>x` close-tab shortcut while keeping plain Space as the
    /// page-down action when the sequence does not match.
    markdown_leader_pending: Option<std::time::Instant>,
    /// Second-stage leader for the `<space>f` finder prefix. Set when
    /// the user presses `<space>` then `f` and cleared on the next
    /// key (`f` → files finder, `w` → grep finder, anything else →
    /// flush `<Space>f<key>` through to nvim).
    finder_leader_pending: Option<std::time::Instant>,
    editor_mouse_dragging: bool,
    mouse_hidden_by_typing: bool,
    /// Editor route selected when finder opens, so accepting a result
    /// edits the pane that launched it instead of whichever split is
    /// currently first in the grid map.
    finder_target_route: Option<usize>,
    /// LSP install prompts already shown this session. Prevents a
    /// missing server from reopening the modal on every render/FileType
    /// event after the user dismisses it.
    lsp_missing_prompts: BTreeSet<String>,
    /// Treesitter parsers Rio is already installing/has attempted this
    /// session. Missing parser notifications can repeat on FileType and
    /// BufEnter, but installs should run once per language.
    treesitter_installing: BTreeSet<String>,
    /// Active workspace root for Rust-owned IDE chrome. Terminal panes
    /// update this from OSC 7 cwd, editor panes from their embedded
    /// nvim cwd. The file tree mirrors this root when visible.
    active_workspace_root: Option<PathBuf>,
    /// A Workspaces-modal pick of a workspace that lives on a tailnet
    /// PEER's daemon: `(workspace_id, daemon_url)`. The app layer
    /// drains this each pump, re-dials the daemon connection to the
    /// owning host (the host owns the daemon — joining means following
    /// it), and adopts the workspace once the fresh tree lands.
    pending_peer_workspace_join: Option<(String, String)>,
    /// Set when the LAST joined workspace was left — the app layer
    /// re-dials the daemon connection back to this desktop's home
    /// daemon on the next pump.
    pending_daemon_go_home: bool,
    /// Host-level server switch selected from the shared server picker.
    pending_server_connect: Option<String>,
    pending_server_manager_open: bool,
    pending_server_add: Option<(String, Option<String>, Option<String>)>,
    pending_server_edit: Option<String>,
    pending_server_edit_submit: Option<(String, String, Option<String>, Option<String>)>,
    pending_server_remove: Option<String>,
    pending_workspace_subscription: Option<String>,
    /// In-flight files-plane MUTATIONS (create/rename/delete) this
    /// screen issued for the remote tree, by request id. Replies with
    /// these ids drive toasts + re-lists; unknown ids are listing
    /// traffic and stay quiet.
    pending_remote_file_ops: std::collections::HashSet<u64>,
    /// In-flight daemon `ReadFile` fetches for markdown panes opened in
    /// a joined workspace (the bytes only exist on the host), by request
    /// id → pane path. The correlated `FileContent` reply fills the pane.
    pending_remote_markdown_opens: HashMap<u64, PathBuf>,
    /// Decoded `cover:` banner images by resolved file path. `None`
    /// records a failed decode so a broken cover doesn't re-decode every
    /// frame. Entries re-load lazily if sugarloaf drops the texture.
    pub(crate) markdown_cover_cache:
        HashMap<PathBuf, Option<crate::screen::MarkdownCoverImage>>,
    /// rich_text_ids that received md/notebook image overlays LAST
    /// frame. Any id not re-pushed this frame gets its overlays
    /// cleared — a pane that stops rendering (tab stashed to another
    /// grid, content swapped to a terminal) must not leave its cover
    /// glued to the screen.
    markdown_image_overlay_ids: std::collections::HashSet<usize>,
    /// In-flight remote git-status request → the root it was asked
    /// for, so a stale reply for a workspace we've left is dropped.
    pending_remote_git_status: HashMap<u64, PathBuf>,
    /// In-flight WalkTree listings of a joined workspace's `Notes/`
    /// folder — replies feed the notes sidebar, not the file tree.
    pending_remote_notes_listing: std::collections::HashSet<u64>,
    workspace_roots: HashMap<WorkspaceKey, PathBuf>,
    workspace_buffer_tabs: HashMap<
        WorkspaceKey,
        neoism_ui::panels::buffer_tabs::BufferTabs<crate::neoism::icon::AgentKind>,
    >,
    workspace_buf_enter_targets: HashMap<WorkspaceKey, Option<PathBuf>>,
    /// Per-workspace FILE TREE state (root, entries, open dirs,
    /// selection, scroll, remote wiring). The live tree in
    /// `renderer.file_tree` always belongs to exactly one workspace —
    /// `file_tree_workspace` — and workspace switches SWAP whole trees
    /// instead of re-rooting a shared one, so every workspace keeps
    /// its own tree exactly as the user left it (local and joined
    /// alike). Mirrors `workspace_buffer_tabs`.
    workspace_file_trees: HashMap<WorkspaceKey, crate::editor::file_tree::FileTree>,
    file_tree_workspace: Option<WorkspaceKey>,
    /// Per-workspace NOTES panel state (viewed vault, entries, open
    /// dirs, selection) — workspace switches SWAP whole panels exactly
    /// like `workspace_file_trees`, so a joined workspace never shows
    /// this machine's personal vault.
    workspace_notes_sidebars: HashMap<WorkspaceKey, neoism_ui::panels::NotesSidebar>,
    notes_sidebar_workspace: Option<WorkspaceKey>,
    workspace_editor_active_paths: HashMap<WorkspaceKey, PathBuf>,
    workspace_note_indexes: HashMap<PathBuf, crate::workspace::notes::WorkspaceNoteIndex>,
    workspace_note_index_rx: Option<
        std_mpsc::Receiver<crate::screen::bridges::workspace::WorkspaceNoteIndexUpdate>,
    >,
    file_tree_clipboard: Option<PathBuf>,
    file_tree_fs_watch_root: Option<PathBuf>,
    file_tree_fs_watcher:
        Option<crate::screen::bridges::file_tree::FileTreeFsWatcherHandle>,
    markdown_fs_reload_fingerprints: HashMap<PathBuf, (u64, std::time::SystemTime)>,
    file_tree_git_watch_root: Option<PathBuf>,
    file_tree_git_watcher: Option<notify::RecommendedWatcher>,
    file_tree_git_refresh_rx:
        Option<std_mpsc::Receiver<crate::editor::file_tree::FileTreeGitRefreshResult>>,
    file_tree_git_refresh_inflight: bool,
    file_tree_git_refresh_pending: bool,
    file_tree_git_self_event_suppressed_until: Option<Instant>,
    #[cfg(not(target_arch = "wasm32"))]
    acp_events_tx: std_mpsc::Sender<crate::neoism::acp::AcpUiEvent>,
    #[cfg(not(target_arch = "wasm32"))]
    acp_events_rx: std_mpsc::Receiver<crate::neoism::acp::AcpUiEvent>,
    #[cfg(not(target_arch = "wasm32"))]
    acp_handles: Vec<crate::neoism::acp::AcpClientHandle>,
    /// Per-frame cache of block-header hover icon hit-test rects
    /// (logical pixels). Populated during render when the mouse is
    /// over a block's header row; consumed by `on_left_click` to
    /// dispatch copy / filter actions. Cleared at the start of every
    /// render and at every cursor_moved that doesn't land on a block.
    block_hover_icons: Vec<BlockHoverIcon>,
    /// Last terminal file-link hover uploaded into the grid. Link hover
    /// changes mutate cloned cell styles, so the active terminal row has
    /// to be rebuilt when this changes or stale blue text stays resident.
    terminal_file_link_hover: Option<(usize, usize, usize)>,
    block_hover_icon_visual: Option<BlockHoverIconVisualState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockHoverAction {
    Copy,
    Favorite,
    Filter,
}

#[derive(Debug, Clone, Copy)]
struct BlockHoverIconVisualState {
    block_idx: usize,
    action: BlockHoverAction,
    hover_started: Instant,
    clicked_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
pub struct BlockHoverIcon {
    pub block_idx: usize,
    pub action: BlockHoverAction,
    /// Logical-pixel rect: [x, y, w, h].
    pub rect: [f32; 4],
}

/// Per-frame snapshot captured during the active terminal pane's
/// compose pass, used after the per-pane loop to render block-hover
/// icons + populate `Screen::block_hover_icons` for click hit-test.
struct ActiveBlockHeaders {
    spans: Vec<crate::terminal::blocks::BlockHeaderSpan>,
    snapshots: Vec<crate::terminal::blocks::CommandBlockSnapshot>,
    /// Logical-pixel y of the cell grid's top row (already shifted
    /// by composer overlay).
    panel_top_logical: f32,
    panel_left_logical: f32,
    panel_right_logical: f32,
    cell_w_logical: f32,
    cell_h_logical: f32,
    content_clip_logical: [f32; 4],
    font_size_logical: f32,
    animation_phase: f32,
}

// Native shim: resolve the shared color token through `IdeTheme`. The
// pure policy returns the token slot; only the native host knows the
// concrete RGBA the GPU sampler wants.
fn block_status_color(
    theme: neoism_ui::primitives::ide_theme::IdeTheme,
    status: crate::terminal::blocks::BlockStatusKind,
) -> [u8; 4] {
    match block_status_color_token(status) {
        BlockStatusColorToken::Yellow => theme.u8(theme.yellow),
        BlockStatusColorToken::Green => theme.u8(theme.green),
        BlockStatusColorToken::Red => theme.u8(theme.red),
    }
}

fn draw_running_block_loader(
    sugarloaf: &mut Sugarloaf,
    _theme: neoism_ui::primitives::ide_theme::IdeTheme,
    left: f32,
    row_top: f32,
    cell_h: f32,
    font_size: f32,
    animation_phase: f32,
    clip_rect: [f32; 4],
) -> f32 {
    let slot_w = (font_size * 1.08).max(12.0);
    let side = (font_size * 0.86).min(cell_h * 0.74).max(10.0);
    let half = side * 0.5;
    let dot = (side * 0.34).clamp(3.5, 5.8);
    let center_x = left + slot_w * 0.5;
    let center_y = row_top + cell_h * 0.5;
    // Phase + tick selection lives in render_policy so the web host can
    // run the same loader cadence (1.35x phase, 12 Hz palette tick).
    let loader_frame = loader_animation_frame(animation_phase);
    let phase = loader_frame.phase;
    let tick = loader_frame.tick;

    for (trail, alpha) in [1.0, 0.58, 0.32, 0.16].into_iter().enumerate() {
        let (dx, dy) = loader_orbit_position(phase - trail as f32 * 0.075, half);
        let x = center_x + dx - dot * 0.5;
        let y = center_y + dy - dot * 0.5;
        let dot_rect = [x, y, dot, dot];
        if intersect_rect(dot_rect, clip_rect).is_none() {
            continue;
        }
        if trail <= 1 {
            let glow = dot * 1.85;
            sugarloaf.rounded_rect(
                None,
                center_x + dx - glow * 0.5,
                center_y + dy - glow * 0.5,
                glow,
                glow,
                loader_pastel_color(tick, trail, alpha * 0.24),
                0.0,
                glow * 0.5,
                TERMINAL_BLOCK_CHROME_ORDER,
            );
        }
        sugarloaf.rounded_rect(
            None,
            x,
            y,
            dot,
            dot,
            loader_pastel_color(tick, trail, alpha),
            0.0,
            dot * 0.42,
            TERMINAL_BLOCK_CHROME_ACTIVE_ORDER,
        );
    }

    slot_w
}

fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]);
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    intersect_rect(a, b).is_some()
}

fn draw_text_with_occlusion(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &neoism_backend::sugarloaf::text::DrawOpts,
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    if occlusion_rects.is_empty() {
        return sugarloaf.text_mut().draw(x, y, text, opts);
    }

    let width = sugarloaf.text_mut().measure(text, opts);
    if width <= 0.0 {
        return 0.0;
    }

    let Some(base_clip) = opts.clip_rect else {
        return sugarloaf.text_mut().draw(x, y, text, opts);
    };
    let text_h = (opts.font_size * 1.8).max(opts.font_size + 8.0);
    let text_rect = [x, y - 4.0, width, text_h];
    let mut intervals = vec![(base_clip[0], base_clip[0] + base_clip[2])];

    for rect in occlusion_rects {
        if !rects_intersect(text_rect, *rect) {
            continue;
        }
        let cut_start = rect[0].max(base_clip[0]);
        let cut_end = (rect[0] + rect[2]).min(base_clip[0] + base_clip[2]);
        if cut_end <= cut_start {
            continue;
        }

        let mut next = Vec::with_capacity(intervals.len() + 1);
        for (start, end) in intervals {
            if cut_end <= start || cut_start >= end {
                next.push((start, end));
                continue;
            }
            if cut_start > start {
                next.push((start, cut_start));
            }
            if cut_end < end {
                next.push((cut_end, end));
            }
        }
        intervals = next;
        if intervals.is_empty() {
            return width;
        }
    }

    for (start, end) in intervals {
        let clip_w = end - start;
        if clip_w <= 0.0 {
            continue;
        }
        let mut clipped = *opts;
        clipped.clip_rect = Some([start, base_clip[1], clip_w, base_clip[3]]);
        sugarloaf.text_mut().draw(x, y, text, &clipped);
    }

    width
}

const LEADER_TIMEOUT_MS: u128 = 800;

fn child_path_for_input(base_dir: &Path, input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter a name first.".to_string());
    }
    let rel = Path::new(trimmed);
    if rel.is_absolute() {
        return Err("Use a relative name, not an absolute path.".to_string());
    }
    let mut has_name = false;
    for component in rel.components() {
        match component {
            Component::Normal(_) => has_name = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Names cannot escape the selected folder.".to_string());
            }
        }
    }
    if !has_name {
        return Err("Enter a name first.".to_string());
    }
    Ok(base_dir.join(rel))
}

fn unique_copy_target(dest_dir: &Path, file_name: &OsStr) -> PathBuf {
    let first = dest_dir.join(file_name);
    if !first.exists() {
        return first;
    }

    let name = file_name.to_string_lossy();
    let path = Path::new(name.as_ref());
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(name.as_ref());
    let ext = path.extension().and_then(|s| s.to_str());
    for ix in 1..10_000 {
        let suffix = if ix == 1 {
            " copy".to_string()
        } else {
            format!(" copy {ix}")
        };
        let candidate = match ext {
            Some(ext) if !ext.is_empty() => format!("{stem}{suffix}.{ext}"),
            _ => format!("{stem}{suffix}"),
        };
        let target = dest_dir.join(candidate);
        if !target.exists() {
            return target;
        }
    }
    dest_dir.join(format!("{name} copy"))
}

fn copy_dir_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn nvim_mouse_button(button: MouseButton) -> Option<&'static str> {
    match button {
        MouseButton::Left => Some("left"),
        MouseButton::Middle => Some("middle"),
        MouseButton::Right => Some("right"),
        _ => None,
    }
}

fn nvim_mouse_modifier(mods: ModifiersState) -> String {
    let mut modifier = String::new();
    if mods.shift_key() {
        modifier.push_str("S-");
    }
    if mods.control_key() {
        modifier.push_str("C-");
    }
    if mods.alt_key() {
        modifier.push_str("M-");
    }
    if mods.super_key() {
        modifier.push_str("D-");
    }
    modifier
}

fn git_state_event_kind(kind: &notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Any
            | notify::EventKind::Access(notify::event::AccessKind::Close(
                notify::event::AccessMode::Write,
            ))
            | notify::EventKind::Create(_)
            | notify::EventKind::Modify(_)
            | notify::EventKind::Remove(_)
            | notify::EventKind::Other
    )
}

fn git_state_event_relevant(event: &notify::Event) -> bool {
    git_state_event_kind(&event.kind)
        && (event.need_rescan()
            || event.paths.iter().any(|path| git_state_path_relevant(path)))
}

fn git_state_path_relevant(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };

    if name.ends_with(".lock") {
        return false;
    }

    matches!(
        name,
        "HEAD"
            | "ORIG_HEAD"
            | "FETCH_HEAD"
            | "MERGE_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD"
            | "BISECT_LOG"
            | "index"
            | "packed-refs"
            | "config"
    ) || path.components().any(|component| {
        matches!(component, Component::Normal(part) if part == OsStr::new("refs"))
    })
}

fn file_tree_fs_event_kind(kind: &notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Any
            | notify::EventKind::Access(notify::event::AccessKind::Close(
                notify::event::AccessMode::Write,
            ))
            | notify::EventKind::Create(_)
            | notify::EventKind::Modify(_)
            | notify::EventKind::Remove(_)
            | notify::EventKind::Other
    )
}

fn file_tree_fs_event_relevant(root: &Path, event: &notify::Event) -> bool {
    file_tree_fs_event_kind(&event.kind)
        && (event.need_rescan()
            || event
                .paths
                .iter()
                .any(|path| file_tree_fs_path_relevant(root, path)))
}

fn file_tree_fs_path_relevant(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    for component in relative.components() {
        if matches!(component, Component::Normal(part) if file_tree_fs_ignored_component(part))
        {
            return false;
        }
    }

    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return true;
    };
    !file_tree_fs_ignored_leaf(name)
}

fn file_tree_fs_ignored_component(part: &OsStr) -> bool {
    matches!(
        part.to_str(),
        Some(".git" | ".claude" | "target" | "node_modules" | ".direnv" | ".cache")
    )
}

fn file_tree_fs_ignored_leaf(name: &str) -> bool {
    name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swo")
        || name.ends_with(".tmp")
        || name.starts_with(".#")
}

fn status_line_height_for_font_size(font_size: f32) -> f32 {
    neoism_ui::panels::status_line::STATUS_LINE_HEIGHT
        * (font_size / crate::host::CHROME_BASELINE_FONT_SIZE).clamp(0.5, 3.0)
}

fn terminal_top_padding_for_chrome_scale(chrome_scale: f32) -> f32 {
    TERMINAL_TOP_BREATHING_PX * chrome_scale.clamp(0.5, 3.0)
}

fn terminal_top_padding_for_font_size(font_size: f32) -> f32 {
    terminal_top_padding_for_chrome_scale(
        font_size / crate::host::CHROME_BASELINE_FONT_SIZE,
    )
}

fn sugarloaf_backend_name(backend: &SugarloafBackend) -> &'static str {
    match backend {
        #[cfg(feature = "wgpu")]
        SugarloafBackend::Wgpu(_) => "wgpu",
        #[cfg(target_os = "macos")]
        SugarloafBackend::Metal => "metal",
        #[cfg(target_os = "linux")]
        SugarloafBackend::Vulkan => "vulkan",
        SugarloafBackend::Cpu => "cpu",
    }
}

#[cfg(not(target_os = "windows"))]
fn shell_pid_is_alive(shell_pid: u32) -> bool {
    if shell_pid == 0 {
        return false;
    }

    // `kill(pid, 0)` returns 0 if the process exists and we
    // can signal it. If it returns -1, errno tells us why.
    // `EPERM` means the process exists but we lack permission
    // to signal it — still alive, so treat as "yes".
    //
    // We read errno via `std::io::Error::last_os_error()`
    // instead of `libc::__errno_location()` because the
    // glibc symbol doesn't exist on macOS (which uses
    // `__error()` instead). `last_os_error()` picks the
    // right symbol per platform.
    let alive = unsafe { libc::kill(shell_pid as i32, 0) } == 0;
    if alive {
        return true;
    }
    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(code) if code == libc::EPERM,
    )
}

#[cfg(target_os = "windows")]
fn shell_pid_is_alive(_shell_pid: u32) -> bool {
    false
}

pub struct ScreenWindowProperties {
    pub size: neoism_window::dpi::PhysicalSize<u32>,
    pub scale: f64,
    pub raw_window_handle: RawWindowHandle,
    pub raw_display_handle: RawDisplayHandle,
    pub window_id: neoism_window::window::WindowId,
}

// Method definitions are split across sibling files for readability.
// Each declares its own `impl Screen<'_>` block.
pub mod bridges;
pub mod chrome_geom;
pub mod daemon_layout;
pub mod editor_scroll;
pub mod lifecycle;
pub mod markdown_crdt;
pub mod panes;
pub mod presence;
pub mod render;
pub mod selection;

#[cfg(test)]
mod tests;

impl Screen<'_> {
    pub fn new<'screen>(
        window_properties: ScreenWindowProperties,
        config: &neoism_backend::config::Config,
        event_proxy: EventProxy,
        font_library: &neoism_backend::sugarloaf::font::FontLibrary,
        open_url: Option<String>,
    ) -> Result<Screen<'screen>, Box<dyn Error>> {
        let size = window_properties.size;
        let scale = window_properties.scale;
        let raw_window_handle = window_properties.raw_window_handle;
        let raw_display_handle = window_properties.raw_display_handle;
        let window_id = window_properties.window_id;

        let padding_y_top = padding_top_from_config(
            &crate::bridges::utils::nav_shim(&config.navigation),
            config.margin.top,
            1,
            config.window.macos_use_unified_titlebar,
        ) + terminal_top_padding_for_font_size(config.fonts.size);

        // Reserve room for the bottom status line so terminal and nvim
        // both lay out within visible bounds (otherwise the last row is
        // hidden behind the chrome strip — same idea as the top buffer
        // tabs / breadcrumbs reservation).
        let padding_y_bottom = status_line_height_for_font_size(config.fonts.size);
        let sugarloaf_layout =
            RootStyle::new(scale as f32, config.fonts.size, config.line_height);

        let mut sugarloaf_errors: Option<SugarloafErrors> = None;

        let sugarloaf_window = SugarloafWindow {
            handle: raw_window_handle,
            display: raw_display_handle,
            scale: scale as f32,
            size: SugarloafWindowSize {
                width: size.width as f32,
                height: size.height as f32,
            },
        };

        let backend = if config.renderer.use_cpu {
            SugarloafBackend::Cpu
        } else {
            match config.renderer.backend {
                Backend::Automatic => {
                    // Linux + macOS pick their native GPU backend (ash
                    // / Metal). Other targets fall back to the wgpu
                    // umbrella (only available with the `wgpu`
                    // feature; otherwise we degrade to CPU rasterizer).
                    #[cfg(target_os = "linux")]
                    {
                        SugarloafBackend::Vulkan
                    }
                    #[cfg(target_os = "macos")]
                    {
                        SugarloafBackend::Metal
                    }
                    #[cfg(all(
                        not(any(target_os = "linux", target_os = "macos")),
                        feature = "wgpu",
                    ))]
                    {
                        #[cfg(target_arch = "wasm32")]
                        let default_backend =
                            wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
                        #[cfg(not(target_arch = "wasm32"))]
                        let default_backend = wgpu::Backends::all();

                        SugarloafBackend::Wgpu(default_backend)
                    }
                    #[cfg(all(
                        not(any(target_os = "linux", target_os = "macos")),
                        not(feature = "wgpu"),
                    ))]
                    {
                        SugarloafBackend::Cpu
                    }
                }
                // `Backend::Vulkan` from the user config now means the
                // native ash backend on Linux. Other OSes fall through
                // to the wgpu Vulkan path when the `wgpu` feature is
                // on; otherwise we degrade to CPU rasterizer.
                #[cfg(target_os = "linux")]
                Backend::Vulkan => SugarloafBackend::Vulkan,
                #[cfg(all(not(target_os = "linux"), feature = "wgpu"))]
                Backend::Vulkan => SugarloafBackend::Wgpu(wgpu::Backends::VULKAN),
                #[cfg(all(not(target_os = "linux"), not(feature = "wgpu")))]
                Backend::Vulkan => SugarloafBackend::Cpu,
                #[cfg(feature = "wgpu")]
                Backend::GL => SugarloafBackend::Wgpu(wgpu::Backends::GL),
                #[cfg(not(feature = "wgpu"))]
                Backend::GL => SugarloafBackend::Cpu,
                #[cfg(feature = "wgpu")]
                Backend::WgpuMetal => SugarloafBackend::Wgpu(wgpu::Backends::METAL),
                #[cfg(not(feature = "wgpu"))]
                Backend::WgpuMetal => SugarloafBackend::Cpu,
                #[cfg(target_os = "macos")]
                Backend::Metal => SugarloafBackend::Metal,
                #[cfg(feature = "wgpu")]
                Backend::DX12 => SugarloafBackend::Wgpu(wgpu::Backends::DX12),
                #[cfg(not(feature = "wgpu"))]
                Backend::DX12 => SugarloafBackend::Cpu,
            }
        };
        let backend_name = sugarloaf_backend_name(&backend);
        crate::app::freeze_watchdog::mark_window_event(
            window_id,
            "renderer_backend_selected",
            format!(
                "selected={} config_backend={:?} use_cpu={}",
                backend_name, config.renderer.backend, config.renderer.use_cpu
            ),
        );
        tracing::info!(
            target: "neoism::renderer",
            ?window_id,
            selected_backend = backend_name,
            config_backend = ?config.renderer.backend,
            use_cpu = config.renderer.use_cpu,
            raw_display_handle = ?raw_display_handle,
            raw_window_handle = ?raw_window_handle,
            xdg_session_type = ?std::env::var("XDG_SESSION_TYPE").ok(),
            wayland_display = std::env::var_os("WAYLAND_DISPLAY").is_some(),
            "selected sugarloaf renderer backend"
        );

        let sugarloaf_renderer = SugarloafRenderer {
            backend,
            font_features: config.fonts.features.clone(),
            colorspace: config.window.colorspace.to_sugarloaf_colorspace(),
        };

        crate::app::freeze_watchdog::mark_window_event(
            window_id,
            "sugarloaf.new.begin",
            format!("backend={backend_name}"),
        );
        let mut sugarloaf: Sugarloaf = match Sugarloaf::new(
            sugarloaf_window,
            sugarloaf_renderer,
            font_library,
            sugarloaf_layout,
        ) {
            Ok(instance) => instance,
            Err(instance_with_errors) => {
                sugarloaf_errors = Some(instance_with_errors.errors);
                instance_with_errors.instance
            }
        };
        crate::app::freeze_watchdog::mark_window_event(
            window_id,
            "sugarloaf.new.end",
            format!(
                "backend={backend_name} errors={}",
                sugarloaf_errors.is_some()
            ),
        );

        // Mash Up Packs: seed the examples on first run, make runtime
        // themes resolvable before Renderer::new reads `[neoism] theme`,
        // and re-apply the active pack's shader slots (the theme slot is
        // persisted into `[neoism] theme` at apply time, so it needs no
        // startup handling — and an individually overridden theme wins).
        crate::mashup::seed_example_packs();
        crate::mashup::sync_custom_ide_themes();
        crate::mashup::publish_active_look(
            &config.look,
            config.neoism.mashup_pack.as_deref(),
        );
        let startup_pack = config
            .neoism
            .mashup_pack
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .and_then(neoism_backend::config::mashup::find_mashup_pack);

        #[cfg(feature = "wgpu")]
        {
            let pack_filters = startup_pack
                .as_ref()
                .map(|pack| pack.filters.as_slice())
                .filter(|filters| !filters.is_empty());
            sugarloaf.update_filters(
                pack_filters.unwrap_or(config.renderer.filters.as_slice()),
            );
        }

        let mut startup_shader_overlay: Option<String> = None;
        if let Some(overlay) = startup_pack
            .as_ref()
            .and_then(|pack| pack.shader_overlay.clone())
        {
            let overlay_config =
                neoism_backend::sugarloaf::ShaderOverlayConfig::new([overlay.clone()]);
            match sugarloaf.set_shader_overlay(overlay_config) {
                Ok(()) => startup_shader_overlay = Some(overlay),
                Err(err) => tracing::warn!(
                    target: "neoism::mashup",
                    "failed to apply pack shader overlay at startup: {err}"
                ),
            }
        }

        let mut shader_overlay_paths: Vec<String> = BUILTIN_SHADER_OVERLAY_CHOICES
            .iter()
            .map(|shader| (*shader).to_string())
            .collect();
        shader_overlay_paths.extend(
            config
                .renderer
                .shader_overlays
                .iter()
                .map(|path| path.display().to_string()),
        );

        let mut renderer = Renderer::new(config);
        renderer.shader_overlay_active = startup_shader_overlay.is_some();

        let bindings = crate::bindings::default_key_bindings(config);

        let is_native = config.navigation.is_native();

        let (shell, working_dir) = process_open_url(
            config.shell.to_owned(),
            config.working_dir.to_owned(),
            config.editor.to_owned(),
            open_url.as_deref(),
        );

        let context_manager_config = context::ContextManagerConfig {
            cwd: config.navigation.current_working_directory,
            shell,
            working_dir,
            spawn_performer: true,
            #[cfg(not(target_os = "windows"))]
            use_fork: config.use_fork,
            is_native,
            // When navigation does not contain any color rule
            // does not make sense fetch for foreground process names/path
            should_update_title_extra: !config.navigation.color_automation.is_empty(),
            split_color: config.colors.split,
            split_active_color: config.colors.split_active,
            panel: config.panel,
            title: config.title.clone(),
            keyboard: config.keyboard,
            scrollback_history_limit: config.scrollback_history_limit,
            ide_theme: config.neoism.theme.clone(),
            cursor_blinking: config.cursor.blinking,
            source: neoism_backend::performer::nvim::ContextSource::Pty,
        };

        // Create rich text with initial position accounting for island
        let rich_text_id = next_rich_text_id();
        let _ = sugarloaf.text(Some(rich_text_id));
        sugarloaf.set_position(rich_text_id, config.margin.left, padding_y_top);

        // Create unscaled margin for ContextDimension (compute() will scale it)
        let margin = Margin::new(
            padding_y_top,
            config.margin.right,
            padding_y_bottom,
            config.margin.left,
        );
        // Create scaled margin for ContextGrid (already in physical pixels)
        let scaled_margin = Margin::new(
            padding_y_top * scale as f32,
            config.margin.right * scale as f32,
            padding_y_bottom * scale as f32,
            config.margin.left * scale as f32,
        );
        let context_dimension = ContextDimension::build(
            size.width as f32,
            size.height as f32,
            sugarloaf
                .get_text_dimensions(&rich_text_id)
                .unwrap_or_default(),
            config.line_height,
            margin,
        );

        let cursor = Cursor {
            content: config.cursor.shape.into(),
            content_ref: config.cursor.shape.into(),
            state: CursorState::new(config.cursor.shape.into()),
            is_ime_enabled: false,
        };

        let context_manager = context::ContextManager::start(
            // config.cursor.blinking
            (&cursor, config.cursor.blinking),
            event_proxy,
            window_id,
            0,
            rich_text_id,
            context_manager_config,
            context_dimension,
            scaled_margin,
            sugarloaf_errors,
        )?;

        sugarloaf.set_background_color(Some(renderer.dynamic_background.1));

        // Precedence: an explicit `[window] background-image` in config
        // is the user's individual override and beats the active Mash
        // Up Pack's wallpaper slot.
        let pack_wallpaper = startup_pack
            .as_ref()
            .and_then(|pack| pack.wallpaper.as_ref());
        if let Some(image) = config.window.background_image.as_ref().or(pack_wallpaper) {
            if let Err(message) = sugarloaf.set_background_image(image) {
                renderer.assistant.set_error(RioError {
                    level: RioErrorLevel::Warning,
                    report: RioErrorType::BackgroundImageLoadFailure(message),
                });
            }
        } else {
            sugarloaf.clear_background_image();
        }

        #[cfg(not(target_arch = "wasm32"))]
        let (acp_events_tx, acp_events_rx) = std_mpsc::channel();

        // We always launch with a terminal, so seed its tab now — otherwise
        // the buffer-tab strip (and the trailing "+" button) start empty until
        // the first tab op. `ensure_terminal_tab` is a no-op once a terminal
        // tab exists, so later workspace loads stay correct.
        renderer.buffer_tabs.ensure_terminal_tab();

        Ok(Screen {
            search_state: SearchState::default(),
            hint_state: HintState::new(config.hints.alphabet.clone()),
            hints_config: config
                .hints
                .rules
                .iter()
                .map(|h| std::rc::Rc::new(h.clone()))
                .collect(),
            mouse_bindings: crate::bindings::default_mouse_bindings(),
            // OFF by default — opt in with `NEOISM_RM_AUTOSYNC=1`. Only
            // present when the optional `remarkable` extension is built in.
            #[cfg(feature = "remarkable")]
            remarkable_autosync: (std::env::var("NEOISM_RM_AUTOSYNC").as_deref()
                == Ok("1"))
            .then(neoism_remarkable::RemarkableAutoSync::start),
            show_ink_overlay: std::env::var("NEOISM_RM_OVERLAY").as_deref() != Ok("0"),
            draw_over_note: None,
            ink_overlay_cache: std::collections::HashMap::new(),
            modifiers: Modifiers::default(),
            context_manager,
            pending_detached_workspace: None,
            pending_cross_window_tab_move: None,
            daemon_pane_layout: daemon_layout::ScreenPaneLayoutCache::default(),
            remote_presence: neoism_ui::editor::crdt::RemotePresenceStore::new(),
            presence_publisher: None,
            presence_display_name_override: config.neoism.display_name.clone(),
            markdown_crdt: markdown_crdt::MarkdownCrdtState::default(),
            sugarloaf,
            mouse: Mouse::new(config.scroll.multiplier, config.scroll.divider),
            touchpurpose: TouchPurpose::default(),
            renderer,
            last_chrome_layout_signature: None,
            welcome_reveal_pending: true,
            search_reply_pump_until: None,
            pending_notebook_executions: Vec::new(),
            pending_python_kernel_retry: None,
            notebook_runtime: crate::notebook_runtime::NotebookRuntimeManager::new(),
            bindings,
            last_ime_cursor_pos: None,
            last_editor_trail_cursor_cell: None,
            last_editor_key_log_at: None,
            last_editor_key_log_notation: None,
            resize_state: None,
            file_tree_resize_state: None,
            notes_sidebar_resize_state: None,
            git_diff_panel_resize_state: None,
            git_diff_panel_scrollbar_drag: None,
            grids: rustc_hash::FxHashMap::default(),
            grid_rasterizer: crate::terminal::grid_emit::GridGlyphRasterizer::new(),
            editor_scroll_grid_states: rustc_hash::FxHashMap::default(),
            editor_geometry_log_last: rustc_hash::FxHashMap::default(),
            shader_overlay_paths,
            active_shader_overlay: startup_shader_overlay,
            last_full_render_us: 0,
            leader_pending: None,
            markdown_leader_pending: None,
            finder_leader_pending: None,
            editor_mouse_dragging: false,
            mouse_hidden_by_typing: false,
            finder_target_route: None,
            lsp_missing_prompts: BTreeSet::new(),
            treesitter_installing: BTreeSet::new(),
            active_workspace_root: config
                .working_dir
                .clone()
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .map(Self::normalize_workspace_root),
            pending_peer_workspace_join: None,
            pending_daemon_go_home: false,
            pending_server_connect: None,
            pending_server_manager_open: false,
            pending_server_add: None,
            pending_server_edit: None,
            pending_server_edit_submit: None,
            pending_server_remove: None,
            pending_workspace_subscription: None,
            pending_remote_file_ops: std::collections::HashSet::new(),
            pending_remote_markdown_opens: HashMap::new(),
            markdown_cover_cache: HashMap::new(),
            markdown_image_overlay_ids: std::collections::HashSet::new(),
            pending_remote_git_status: HashMap::new(),
            pending_remote_notes_listing: std::collections::HashSet::new(),
            workspace_roots: HashMap::new(),
            workspace_buffer_tabs: HashMap::new(),
            workspace_notes_sidebars: HashMap::new(),
            notes_sidebar_workspace: None,
            workspace_file_trees: HashMap::new(),
            file_tree_workspace: None,
            workspace_buf_enter_targets: HashMap::new(),
            workspace_editor_active_paths: HashMap::new(),
            workspace_note_indexes: HashMap::new(),
            workspace_note_index_rx: None,
            file_tree_clipboard: None,
            file_tree_fs_watch_root: None,
            file_tree_fs_watcher: None,
            markdown_fs_reload_fingerprints: HashMap::new(),
            file_tree_git_watch_root: None,
            file_tree_git_watcher: None,
            file_tree_git_refresh_rx: None,
            file_tree_git_refresh_inflight: false,
            file_tree_git_refresh_pending: false,
            file_tree_git_self_event_suppressed_until: None,
            #[cfg(not(target_arch = "wasm32"))]
            acp_events_tx,
            #[cfg(not(target_arch = "wasm32"))]
            acp_events_rx,
            #[cfg(not(target_arch = "wasm32"))]
            acp_handles: Vec::new(),
            block_hover_icons: Vec::new(),
            terminal_file_link_hover: None,
            block_hover_icon_visual: None,
        })
    }

    /// Ensure a `GridRenderer` exists for `route_id` with the given
    /// dimensions. Lazily constructs on first call, resizes on
    /// subsequent calls when `(cols, rows)` change. Phase 2.0: the
    /// returned grid isn't yet bound into `render_with_grids`, so
    /// this is a smoke-test for shader compilation and pipeline
    /// creation on real hardware.

    /// Discard the grid for a panel that has closed. Frees the GPU
    /// buffers + pipeline state. Wired into the context-close path
    /// in Phase 2.1; kept `#[allow(dead_code)]` for Phase 2.0 so the
    /// method is available without failing the warnings-as-errors
    /// build.
    #[allow(dead_code)]
    #[inline]
    pub fn ctx(&self) -> &ContextManager<EventProxy> {
        &self.context_manager
    }

    #[inline]
    pub fn ctx_mut(&mut self) -> &mut ContextManager<EventProxy> {
        &mut self.context_manager
    }

    pub(crate) fn mouse_logical_for_hit_test(&self) -> (f32, f32) {
        let scale_factor = self.sugarloaf.scale_factor();
        let mouse_x = self.mouse.x as f32 / scale_factor;
        let mouse_y = self.mouse.y as f32 / scale_factor;
        self.unwarp_shader_overlay_point(mouse_x, mouse_y)
    }

    pub(crate) fn unwarp_shader_overlay_point(&self, x: f32, y: f32) -> (f32, f32) {
        let Some(shader) = self.active_shader_overlay.as_deref() else {
            return (x, y);
        };
        if shader != "builtin:ctv_round" {
            return (x, y);
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let width = (self.sugarloaf.window_size().width as f32 / scale_factor).max(1.0);
        let height = (self.sugarloaf.window_size().height as f32 / scale_factor).max(1.0);
        let uv_x = (x / width).clamp(0.0, 1.0);
        let uv_y = (y / height).clamp(0.0, 1.0);
        let p_x = uv_x * 2.0 - 1.0;
        let p_y = uv_y * 2.0 - 1.0;
        let r2 = p_x * p_x + p_y * p_y;
        let factor = 1.0 + r2 * 0.045;
        let unwarped_x = ((p_x * factor) * 0.5 + 0.5) * width;
        let unwarped_y = ((p_y * factor) * 0.5 + 0.5) * height;
        (unwarped_x, unwarped_y)
    }
}

/// Translate a winit named key to the shared
/// `neoism_ui::lifecycle_policy::NvimNamedKey` POD enum that the nvim
/// key formatter consumes. Returns `None` for keys nvim has no special
/// token for (letters, digits, modifier-only events).
fn named_key_to_nvim_kind(
    key: &neoism_window::keyboard::Key,
) -> Option<neoism_ui::lifecycle_policy::NvimNamedKey> {
    use neoism_ui::lifecycle_policy::NvimNamedKey;
    use neoism_window::keyboard::{Key, NamedKey};
    let Key::Named(named) = key else { return None };
    Some(match named {
        NamedKey::ArrowDown => NvimNamedKey::ArrowDown,
        NamedKey::ArrowLeft => NvimNamedKey::ArrowLeft,
        NamedKey::ArrowRight => NvimNamedKey::ArrowRight,
        NamedKey::ArrowUp => NvimNamedKey::ArrowUp,
        NamedKey::Backspace => NvimNamedKey::Backspace,
        NamedKey::Delete => NvimNamedKey::Delete,
        NamedKey::End => NvimNamedKey::End,
        NamedKey::Enter => NvimNamedKey::Enter,
        NamedKey::Escape => NvimNamedKey::Escape,
        NamedKey::Home => NvimNamedKey::Home,
        NamedKey::Insert => NvimNamedKey::Insert,
        NamedKey::PageDown => NvimNamedKey::PageDown,
        NamedKey::PageUp => NvimNamedKey::PageUp,
        NamedKey::Space => NvimNamedKey::Space,
        NamedKey::Tab => NvimNamedKey::Tab,
        NamedKey::F1 => NvimNamedKey::F1,
        NamedKey::F2 => NvimNamedKey::F2,
        NamedKey::F3 => NvimNamedKey::F3,
        NamedKey::F4 => NvimNamedKey::F4,
        NamedKey::F5 => NvimNamedKey::F5,
        NamedKey::F6 => NvimNamedKey::F6,
        NamedKey::F7 => NvimNamedKey::F7,
        NamedKey::F8 => NvimNamedKey::F8,
        NamedKey::F9 => NvimNamedKey::F9,
        NamedKey::F10 => NvimNamedKey::F10,
        NamedKey::F11 => NvimNamedKey::F11,
        NamedKey::F12 => NvimNamedKey::F12,
        _ => return None,
    })
}
