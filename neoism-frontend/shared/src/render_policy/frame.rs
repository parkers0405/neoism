// ---------------------------------------------------------------------------
// Per-frame present / pacing / pane-projection policy.
//
// The native renderer historically inlined three pure decisions inside
// the giant `Screen::render` loop:
//   - "Should we call `sugarloaf.render()` this frame?" — folded
//     `any_panel_dirty || has_animation` together with the implicit
//     "RedrawRequested is a contract" branch; both web and native need
//     the same yes/no answer so they can choose between dirty-present,
//     clean-present, or skip.
//   - "Translate physical pane rect → logical pane rect" for the
//     minimap (and any other overlay that paints in logical pixels).
//   - "Aggregate per-frame timing counters into FPS / frame-budget /
//     pacing-jitter for the FPS log." — pure scalar math over `u64`
//     micros + a frame count.
//
// All three are POD-in / POD-out and have no Sugarloaf dependencies, so
// they belong here next to the other render-loop policies.
// ---------------------------------------------------------------------------

/// Pure decision: should this frame trigger a present, or is it
/// idle enough that the host may rely on the next redraw tick?
///
/// `any_panel_dirty` is the cell-buffer dirty bit aggregated across
/// every grid this frame; `has_animation` covers per-frame animation
/// owners (trail cursor, cursorline overlay, splash ripple, indeterminate
/// progress, etc.) that want the next vsync to fire.
///
/// The native fork always presents on `RedrawRequested` regardless of
/// this value (the OS asked, so we ship a clean frame to satisfy the
/// frame callback). Hosts that drive their own loop can use the policy
/// to skip cleanly.
pub fn should_present_frame(any_panel_dirty: bool, has_animation: bool) -> bool {
    any_panel_dirty || has_animation
}

/// Inputs to translate a physical-pixel pane layout into the logical
/// pane rect minimap / overlay drawers paint into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneLogicalRectInput {
    /// Physical-pixel scaled margin already applied to the pane host.
    pub scaled_margin_left: f32,
    pub scaled_margin_top: f32,
    /// Pane layout rect in physical pixels (`[x, y, _w, _h]`); only the
    /// origin is read — width/height come from the cell grid.
    pub layout_rect: [f32; 4],
    /// Per-cell physical metrics, already rounded to whole pixels.
    pub cell_width_phys: f32,
    pub cell_height_phys: f32,
    /// Grid dimensions in cells.
    pub columns: u32,
    pub rows: u32,
    /// HiDPI scale factor (physical → logical divisor).
    pub scale_factor: f32,
}

/// Logical-pixel pane rectangle ready for sugarloaf primitives / web
/// canvas overlays. All four values are pre-divided by the scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneLogicalRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Project a pane's physical layout into its logical drawing rect.
///
/// Mirrors the minimap loop's per-pane math
/// (`(scaled_margin.left + layout_rect[0]) / s`, etc.) so the same code
/// path can be reused by any overlay that draws on a logical canvas
/// (minimap, scrollbar, decorations). Guards a zero/non-finite scale by
/// returning logical 1.0 so a degenerate display config can't blow up
/// the host's primitives.
pub fn pane_logical_rect(input: PaneLogicalRectInput) -> PaneLogicalRect {
    let scale = if input.scale_factor.is_finite() && input.scale_factor > 0.0 {
        input.scale_factor
    } else {
        1.0
    };
    let cell_w = if input.cell_width_phys.is_finite() && input.cell_width_phys > 0.0 {
        input.cell_width_phys
    } else {
        0.0
    };
    let cell_h = if input.cell_height_phys.is_finite() && input.cell_height_phys > 0.0 {
        input.cell_height_phys
    } else {
        0.0
    };
    PaneLogicalRect {
        x: (input.scaled_margin_left + input.layout_rect[0]) / scale,
        y: (input.scaled_margin_top + input.layout_rect[1]) / scale,
        width: (input.columns as f32 * cell_w) / scale,
        height: (input.rows as f32 * cell_h) / scale,
    }
}

/// Off-screen / out-of-viewport skip rule for per-pane overlays
/// (minimap, scrollbar, decoration painters). Returns `true` when the
/// pane has positive cell metrics, a non-empty grid, and an editor role
/// (the only role the minimap/cursorline overlays currently target).
///
/// Hosts pass `is_editor=true` for editor-grid panes; terminal panes
/// always skip. Web hosts will use the same gate when the minimap port
/// lands so the two stay in lockstep.
pub fn pane_overlay_is_paintable(
    is_editor: bool,
    columns: u32,
    rows: u32,
    cell_width_phys: f32,
    cell_height_phys: f32,
) -> bool {
    if !is_editor {
        return false;
    }
    if columns == 0 || rows == 0 {
        return false;
    }
    if !cell_width_phys.is_finite() || cell_width_phys <= 0.0 {
        return false;
    }
    if !cell_height_phys.is_finite() || cell_height_phys <= 0.0 {
        return false;
    }
    true
}

/// Accumulated per-pane timing counters fed into the FPS log. All
/// values are raw micros / frame counts as captured by the render loop;
/// the policy turns them into the derived metrics
/// (`frame_budget_ms`, `wait_outside_render_ms`, `pacing_jitter_ms`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FramePacingCounters {
    pub frames: u32,
    pub elapsed_secs: f32,
    pub render_us_sum: u64,
    pub render_us_max: u64,
    pub full_render_us_sum: u64,
    pub full_render_us_max: u64,
    pub animation_dt_us_sum: u64,
    pub animation_dt_us_max: u64,
}

/// Derived per-pane FPS / pacing statistics ready to feed the
/// `neoism::scroll_fps` log fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FramePacingStats {
    pub fps: f32,
    pub mean_render_ms: f32,
    pub max_render_ms: f32,
    pub mean_full_ms: f32,
    pub max_full_ms: f32,
    pub mean_animation_dt_ms: f32,
    pub max_animation_dt_ms: f32,
    /// `1000 / fps` if `fps > 0`, else `0`.
    pub frame_budget_ms: f32,
    /// Best-effort estimate of how much of the frame budget is spent
    /// outside `render_with_grids` (vsync / acquire wait / sleeping).
    /// Clamped at zero so a pacing overshoot doesn't show as negative.
    pub wait_outside_render_ms: f32,
    /// `max_animation_dt - mean_animation_dt`. A proxy for the pacing
    /// spread; large values mean some frames took noticeably longer
    /// than the mean (jank / scheduler stalls).
    pub pacing_jitter_ms: f32,
}

/// Roll a frame-window's accumulated counters into the per-pane FPS
/// log stats. The host decides when to flush (currently every 0.5s of
/// elapsed wall time); the policy only does scalar math.
///
/// Mirrors the inline computation around line 3730 of the render loop.
/// Guards `elapsed_secs <= 0` and `frames == 0` so a flush triggered
/// on the very first sample doesn't divide by zero.
pub fn frame_pacing_stats(counters: FramePacingCounters) -> FramePacingStats {
    let frames = counters.frames.max(1) as f32;
    let elapsed = counters.elapsed_secs.max(0.001);
    let fps = counters.frames as f32 / elapsed;
    let mean_render_ms = (counters.render_us_sum as f32 / frames) / 1000.0;
    let max_render_ms = counters.render_us_max as f32 / 1000.0;
    let mean_full_ms = (counters.full_render_us_sum as f32 / frames) / 1000.0;
    let max_full_ms = counters.full_render_us_max as f32 / 1000.0;
    let mean_animation_dt_ms = (counters.animation_dt_us_sum as f32 / frames) / 1000.0;
    let max_animation_dt_ms = counters.animation_dt_us_max as f32 / 1000.0;
    let frame_budget_ms = if fps > 0.0 && fps.is_finite() {
        1000.0 / fps
    } else {
        0.0
    };
    let wait_outside_render_ms = (frame_budget_ms - mean_full_ms).max(0.0);
    let pacing_jitter_ms = (max_animation_dt_ms - mean_animation_dt_ms).max(0.0);
    FramePacingStats {
        fps,
        mean_render_ms,
        max_render_ms,
        mean_full_ms,
        max_full_ms,
        mean_animation_dt_ms,
        max_animation_dt_ms,
        frame_budget_ms,
        wait_outside_render_ms,
        pacing_jitter_ms,
    }
}

// ---------------------------------------------------------------------------
// Final-pass extractions (B2).
//
// These small helpers collapse the last set of repeated decisions / shape
// computations that still lived inline in `screen/render/mod.rs`. None of
// them touch sugarloaf, the terminal lock, or app state — they take POD
// inputs and return POD decisions so the same logic can be exercised by
// the web frontend's renderer once it reaches feature parity, and so the
// native fork stays readable as one orchestration layer over named
// policies.
// ---------------------------------------------------------------------------

/// Total physical-grid row count for one pane (visible rows + the
/// hidden buffer rows that hold the fractional edge slot above/below).
///
/// Matches the GPU-side `grid_size.y` the renderer emits. Centralised
/// here so the `EDITOR_BUFFER_ABOVE + rows + EDITOR_BUFFER_BELOW`
/// shape can't drift out of sync between the GPU-grid sizing call and
/// the cursor-clamp call in `mod.rs`.
pub fn grid_total_row_count(rows: u32, buffer_above: u32, buffer_below: u32) -> u32 {
    rows.saturating_add(buffer_above)
        .saturating_add(buffer_below)
}

/// Source-Y of the hidden edge slot that wraps the visible area, given
/// the current fractional pixel offset and the integer source-line
/// stride.
///
/// The renderer threads two named slots into the GPU grid: one row
/// above the visible band (`buffer_above - 1`) and one row below it
/// (`buffer_above + visible_rows`). Whether each slot needs an actual
/// source row depends entirely on the sign of the fractional scroll
/// residual:
///
/// * `pixel_offset_y > 0` — content is gliding DOWN; the row entering
///   from above must be sampled.
/// * `pixel_offset_y < 0` — content is gliding UP; the row entering
///   from below must be sampled.
/// * `pixel_offset_y == 0` — no fractional residual; both slots stay
///   empty (the integer SHIFT plan handled the whole motion).
///
/// `visible_rows` is the on-screen row count (`p.visible_rows.len()`),
/// `source_line_offset` is the integer-line stride already applied to
/// the source rows. The returned `(above, below)` are source-row
/// indices (signed because the above slot is below source-y=0 when the
/// editor sits at the top of the buffer).
pub fn editor_edge_slot_source_y(
    pixel_offset_y: f32,
    source_line_offset: i32,
    visible_rows: i32,
) -> (Option<i32>, Option<i32>) {
    let above = if pixel_offset_y > 0.0 {
        Some(-1 + source_line_offset)
    } else {
        None
    };
    let below = if pixel_offset_y < 0.0 {
        Some(visible_rows + source_line_offset)
    } else {
        None
    };
    (above, below)
}

/// Editor-pane edge slot decision. Unlike raw terminal panes, editor
/// panes split smooth scroll into an integer `source_line_offset` plus
/// a fractional pixel residual, so the top/bottom hidden slots sample
/// source rows relative to that integer stride.
///
/// Returns the action for the above and below slots plus the desired
/// source rows that should be retained by the host for the next frame.
pub fn editor_edge_slot_actions(
    pixel_offset_y: f32,
    source_line_offset: i32,
    visible_rows: i32,
    previous_above_source_y: Option<i32>,
    previous_below_source_y: Option<i32>,
    above_damaged: bool,
    below_damaged: bool,
    force_refresh: bool,
) -> (
    TerminalEdgeSlotAction,
    TerminalEdgeSlotAction,
    Option<i32>,
    Option<i32>,
) {
    let (desired_above, desired_below) =
        editor_edge_slot_source_y(pixel_offset_y, source_line_offset, visible_rows);

    let above = editor_edge_slot_action(
        desired_above,
        previous_above_source_y,
        above_damaged,
        force_refresh,
    );
    let below = editor_edge_slot_action(
        desired_below,
        previous_below_source_y,
        below_damaged,
        force_refresh,
    );

    (above, below, desired_above, desired_below)
}

fn editor_edge_slot_action(
    desired_source_y: Option<i32>,
    previous_source_y: Option<i32>,
    damaged: bool,
    force_refresh: bool,
) -> TerminalEdgeSlotAction {
    if force_refresh || previous_source_y != desired_source_y || damaged {
        match desired_source_y {
            Some(source_y) => TerminalEdgeSlotAction::Emit { source_y },
            None => {
                if previous_source_y.is_some() || force_refresh {
                    TerminalEdgeSlotAction::Clear
                } else {
                    TerminalEdgeSlotAction::Leave
                }
            }
        }
    } else {
        TerminalEdgeSlotAction::Leave
    }
}

/// Terminal-pane edge slot decision. Terminal scroll has no
/// integer-line stride (no editor spring lifts the source base), so
/// the above/below pick collapses to "sample the row immediately
/// outside the visible band when there's any fractional pixel
/// residual" — or, when the renderer asked for a force refresh, clear
/// the slot back to empty.
///
/// Returns a small POD describing what the host should do with each
/// edge slot. The host is responsible for emitting the row / clearing
/// the slot; this policy only decides which path to take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalEdgeSlotAction {
    /// Sample `source_y` and write it into the slot.
    Emit { source_y: i32 },
    /// Clear the slot (no row is needed this frame).
    Clear,
    /// Leave the slot alone (no fractional motion, no force refresh).
    Leave,
}

/// Decide above + below edge-slot actions for a terminal pane.
///
/// Mirrors the historical `if terminal_pixel_offset_y > 0.0 { ... }
/// else if force_refresh || terminal_pixel_offset_y <= 0.0 { ... }`
/// pair (and the symmetric bottom variant) so the renderer can replace
/// two manually-mirrored if/else blocks with a single policy call.
pub fn terminal_edge_slot_actions(
    pixel_offset_y: f32,
    visible_rows: i32,
    force_refresh: bool,
) -> (TerminalEdgeSlotAction, TerminalEdgeSlotAction) {
    let above = if pixel_offset_y > 0.0 {
        TerminalEdgeSlotAction::Emit { source_y: -1 }
    } else if force_refresh || pixel_offset_y <= 0.0 {
        TerminalEdgeSlotAction::Clear
    } else {
        TerminalEdgeSlotAction::Leave
    };
    let below = if pixel_offset_y < 0.0 {
        TerminalEdgeSlotAction::Emit {
            source_y: visible_rows,
        }
    } else if force_refresh || pixel_offset_y >= 0.0 {
        TerminalEdgeSlotAction::Clear
    } else {
        TerminalEdgeSlotAction::Leave
    };
    (above, below)
}

/// Visibility policy for the raw PTY cursor on a terminal pane.
///
/// Three-arm decision the native renderer was making inline in the
/// per-pane snapshot loop:
///
/// 1. If the block UI owns the active terminal (footer composer is
///    focused, or a running command's prompt is hidden), the PTY
///    cursor must hide so only the composer caret / block chrome
///    reads as interactive.
/// 2. If a terminal-block input cursor is composing in the footer,
///    the PTY cursor is allowed to show whenever the file tree
///    isn't focused AND the trail-cursor overlay isn't claiming the
///    caret.
/// 3. Otherwise it follows the raw `cursor.state.is_visible()` flag,
///    again gated by tree focus and trail overlay ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursorVisibilityInput {
    pub block_footer_active: bool,
    pub is_active: bool,
    pub hide_running_command_cursor: bool,
    pub block_input_cursor_present: bool,
    pub cursor_state_visible: bool,
    pub tree_focused: bool,
    pub trail_cursor_enabled: bool,
}

pub fn terminal_cursor_visible(input: TerminalCursorVisibilityInput) -> bool {
    if (input.block_footer_active && input.is_active) || input.hide_running_command_cursor
    {
        return false;
    }
    let base = !input.tree_focused && !input.trail_cursor_enabled;
    if input.block_input_cursor_present {
        base
    } else {
        input.cursor_state_visible && base
    }
}

/// Block-cursor uniform tuple. The block style paints the cursor cell
/// through the bg fragment + glyph fg swap (the sprite path is unused
/// for `Block`). All other styles emit a sprite and leave these zeroed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockCursorUniforms {
    pub cursor_pos: [u32; 2],
    pub cursor_color_u: [f32; 4],
    pub cursor_bg_u: [f32; 4],
}

impl BlockCursorUniforms {
    /// Off / hidden uniforms — `cursor_pos = [u32::MAX; 2]` sentinel
    /// the bg shader reads as "no cursor", colors zeroed.
    pub const HIDDEN: BlockCursorUniforms = BlockCursorUniforms {
        cursor_pos: [u32::MAX; 2],
        cursor_color_u: [0.0; 4],
        cursor_bg_u: [0.0; 4],
    };
}

/// Compute the block-cursor uniform values.
///
/// `block_cursor` is the `matches!(render_style, Some(Block))` decision
/// already resolved by the host. `suppress_static_cursor_bg` collapses
/// the smooth-scroll guard: during a fractional scroll frame the bg
/// + fg swap is paused so the cursor doesn't ghost a stale glyph at
/// the integer slot for one frame.
///
/// FG/BG swap convention: the GPU `GridUniforms` slot named
/// `cursor_color` is fed `cursor_color_u`, but the bg-fragment shader
/// uses that slot to INVERT the glyph under the block cursor — i.e. it
/// paints the cell's bg color through the cursor's foreground channel
/// so the glyph reads against the cursor's solid background. Therefore
/// `cursor_color_u` here holds `bg_color` and `cursor_bg_u` holds the
/// actual `cursor_color_rgb`. Mirrors the inline swap that previously
/// lived in `screen/render/mod.rs`.
pub fn block_cursor_uniforms(
    block_cursor: bool,
    suppress_static_cursor_bg: bool,
    cursor_col: u32,
    cursor_grid_row: u32,
    bg_color: [f32; 4],
    cursor_color_rgb: [f32; 3],
) -> BlockCursorUniforms {
    if !block_cursor {
        return BlockCursorUniforms::HIDDEN;
    }
    let cursor_pos = [cursor_col, cursor_grid_row];
    let (cursor_color_u, cursor_bg_u) = if suppress_static_cursor_bg {
        ([0.0; 4], [0.0; 4])
    } else {
        (
            bg_color,
            [
                cursor_color_rgb[0],
                cursor_color_rgb[1],
                cursor_color_rgb[2],
                1.0,
            ],
        )
    };
    BlockCursorUniforms {
        cursor_pos,
        cursor_color_u,
        cursor_bg_u,
    }
}

/// LSP status token used by the status-line chrome. Matches the
/// `panels::status_line::LspStatus` enum but lives here so policy code
/// doesn't reach into the panel module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LspStatusToken {
    Initializing,
    Active,
    Missing,
}

/// Pick the status-line LSP token from the editor's stringly-typed
/// `editor_lsp_status` snapshot:
///
/// * `"none"` -> `None` (status pill hidden)
/// * `"active"` -> `Some(Active)`
/// * `"missing"` -> `Some(Missing)`
/// * any other value (including `None`) -> `Some(Initializing)`
///
/// The fallback mirrors the historical inline match: any unrecognised
/// state token (or a missing snapshot) reads as "still spinning up" so
/// the pill stays visible while the LSP boots.
pub fn lsp_status_from_state(state: Option<&str>) -> Option<LspStatusToken> {
    match state {
        Some("none") => None,
        Some("active") => Some(LspStatusToken::Active),
        Some("missing") => Some(LspStatusToken::Missing),
        _ => Some(LspStatusToken::Initializing),
    }
}

/// Render a filesystem path as a zsh-style "home tilde" string.
///
/// * `path == home` -> `"~"`
/// * `path` under `home` -> `"~/<relative>"`
/// * `path` outside `home` (or `home == None`) -> the path verbatim.
///
/// All inputs are accepted as `&str` so the policy stays target-free
/// (no `std::path::Path` dependency on hosts that pass JS strings).
/// The native renderer feeds it `path.to_string_lossy()`; the web
/// renderer can pass a literal `string` slice once the workspace-cwd
/// pill ports.
pub fn home_tilde_display(path: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    if path == home {
        return "~".to_string();
    }
    // Match `Path::strip_prefix` semantics: only strip a full path
    // segment. Without the trailing-slash check, `/home/parker2` under
    // `home=/home/parker` would collapse to `~2`. The native helper
    // used `strip_prefix` on `Path`, which inserts its own separator
    // boundary check — emulate that here by requiring the next byte
    // to be a path separator.
    let trimmed_home = home.trim_end_matches('/');
    if let Some(rest) = path.strip_prefix(trimmed_home) {
        if let Some(rest) = rest.strip_prefix('/') {
            return format!("~/{}", rest);
        }
    }
    path.to_string()
}
