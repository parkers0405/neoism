// Split from screen/misc.rs. Hosts the main `Screen::render` path; the
// welcome-screen renderer lives in the `welcome` submodule. Part of the
// `impl Screen<'_>` block — see `screen/mod.rs` for the struct itself.

pub mod welcome;

use super::*;
use neoism_terminal_core::crosswords::square::LineLength;
use neoism_ui::chrome_policy::{
    editor_chrome_mask_rects, grid_panel_chrome_geometry, trail_cursor_overlay_draw_kind,
    trail_cursor_overlay_target, EditorChromeMaskInput, GridPanelChromeGeometryInput,
    TrailCursorOverlayDrawKind, TrailCursorOverlayState, TrailCursorOverlayTarget,
};
use neoism_ui::render_policy::{
    block_cursor_uniforms, editor_cursor_grid_row, editor_edge_slot_actions,
    editor_edge_slot_source_y, editor_scroll_frame_plan, terminal_cursor_visible,
    terminal_edge_slot_actions, EditorScrollSourcePlan, TerminalCursorVisibilityInput,
    TerminalEdgeSlotAction,
};
use std::sync::OnceLock;
use std::time::Instant;

fn overlay_cursor_blink_visible(blinking_interval_ms: u64) -> bool {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    let interval = blinking_interval_ms.max(1) as u128;
    let phase = start.elapsed().as_millis() / interval;
    phase % 2 == 0
}

fn wayland_frame_callback_throttle_disabled() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("NEOISM_WAYLAND_NO_FRAME_CALLBACK_THROTTLE").is_some()
            || std::env::var("NEOISM_WAYLAND_FRAME_PACING")
                .map(|value| {
                    matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "off" | "none" | "immediate" | "unthrottled"
                    )
                })
                .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}
use std::path::{Path, PathBuf};

mod cell_emit;
mod overlays;
mod status_sync;
mod terminal_compose;

pub(crate) struct PanelFrame {
    route_id: usize,
    /// rich_text_id used to look up per-pane scroll offset
    /// (editor_scroll spring or terminal_scroll direct).
    /// Panel grids each carry one rich_text id; the scroll
    /// modules key everything off it.
    #[allow(dead_code)]
    rich_text_id: usize,
    /// Editor scroll spring position in signed rows. This
    /// feeds Neovide/Ghostty's floor(position)+row source
    /// lookup; the fractional remainder becomes the pixel
    /// offset applied uniformly to the grid.
    editor_scroll_position_lines: f32,
    /// Elastic rubber-band offset in physical pixels. Kept
    /// separate from `editor_scroll_position_lines` because
    /// only the real scrollback spring participates in
    /// Neovide's row/fraction source lookup.
    editor_elastic_offset_y: f32,
    /// Terminal smooth-scroll residual in physical pixels.
    /// Unlike the old whole-grid shift, this now feeds the
    /// per-grid uniform path so hidden edge rows can slide
    /// partially into view like the editor/tree.
    terminal_scroll_offset_y: f32,
    /// Rows reserved at the bottom for the off-grid command
    /// composer. Terminal output must clip above this band.
    terminal_reserved_bottom_rows: u32,
    /// `true` for nvim editor panes — drives the buffer-
    /// row allocation and snapshot-row write loop. Other
    /// panes treat this as `false` and skip the buffer.
    is_editor: bool,
    /// Neovide-style 2x viewport scrollback ring for nvim
    /// editor panes. During scroll animation, visible and
    /// edge rows are sampled from this single coherent
    /// buffer using signed floor-based row indices.
    editor_scrollback: Option<(
        Vec<
            Option<
                neoism_terminal_core::crosswords::grid::row::Row<
                    neoism_terminal_core::crosswords::square::Square,
                >,
            >,
        >,
        isize,
    )>,
    /// Logical origin of the editor scrollback ring. This
    /// is cheap to carry even when `editor_scrollback` is
    /// not cloned for drawing, and it must stay available
    /// so row-shift planning compares against the same
    /// persistent ring base Neovide uses.
    editor_scrollback_origin: Option<isize>,
    editor_viewport_topline: u64,
    editor_viewport_botline: u64,
    editor_viewport_line_count: u64,
    /// One hidden row above the visible terminal viewport.
    /// Used for fractional smooth scroll; `None` for
    /// editor panes and when no older row exists.
    terminal_snapshot_above: Option<
        neoism_terminal_core::crosswords::grid::row::Row<
            neoism_terminal_core::crosswords::square::Square,
        >,
    >,
    /// One hidden row below the visible terminal viewport.
    /// Used for fractional smooth scroll; `None` for
    /// editor panes and when no newer row exists.
    terminal_snapshot_below: Option<
        neoism_terminal_core::crosswords::grid::row::Row<
            neoism_terminal_core::crosswords::square::Square,
        >,
    >,
    layout_rect: [f32; 4],
    cols: u32,
    rows: u32,
    cell_w: f32,
    cell_h: f32,
    font_px: f32,
    visible_rows: Vec<
        neoism_terminal_core::crosswords::grid::row::Row<
            neoism_terminal_core::crosswords::square::Square,
        >,
    >,
    source_row_indices: Vec<Option<usize>>,
    style_set: neoism_terminal_core::crosswords::style::StyleSet,
    term_colors: neoism_terminal_core::colors::term::TermColors,
    cursor_col: u16,
    cursor_row: u16,
    cursor_visible: bool,
    /// Terminal-side cursor shape (block / underline /
    /// beam / hidden). Driven by DECSCUSR + the
    /// configured default. Mapped to a render style
    /// inside the rebuild loop.
    cursor_shape: neoism_terminal_core::ansi::CursorShape,
    /// `true` when the terminal has cursor blink
    /// enabled (DECTCEM blink mode or SGR cursor blink).
    cursor_blinking: bool,
    /// `true` for the visible half of the blink cycle.
    /// Always `true` when blink isn't enabled. Driven
    /// by `Renderer::run`'s blink toggler.
    cursor_blink_visible: bool,
    /// `true` while an IME pre-edit string is active —
    /// forces a block cursor regardless of the
    /// configured shape so the user can tell IME is
    /// taking input.
    cursor_preedit: bool,
    /// Resolved cursor color: OSC 12 wins, then config /
    /// theme `cursor`.
    /// `state.colors.cursor → config.cursor_color`
    /// resolution. Per-panel
    /// because each terminal can issue its own OSC 12.
    cursor_color: neoism_backend::config::colors::ColorArray,
    is_active: bool,
    damage: neoism_terminal_core::damage::TerminalDamage,
    /// Selection is per-context (`renderable_content`), not
    /// per-terminal. Grabbed alongside the grid snapshot so
    /// `build_row_bg`/`build_row_fg` can tint selected cells.
    selection: Option<neoism_terminal_core::selection::SelectionRange>,
    /// `i - display_offset = absolute Line` for the
    /// per-row selection interval check. Snapshotted at
    /// the same lock as `visible_rows` to stay consistent.
    display_offset: i32,
    /// Search-hint matches for this panel. `None` when
    /// search is inactive. Consumed alongside `selection`
    /// inside `build_row_bg` / `build_row_fg` to apply
    /// `search_match_background` / `_foreground`. Mirrors
    /// `row_data.highlights` at
    /// `ghostty/src/renderer/generic.zig:1317`.
    hint_matches: Option<Vec<neoism_terminal_core::crosswords::search::Match>>,
    /// Currently-focused search match (↑/↓ navigation).
    /// Rendered with `search_focused_match_background` /
    /// `_foreground` — `.search_selected`
    /// highlight tag.
    focused_match: Option<neoism_terminal_core::crosswords::search::Match>,
    /// (start, end) of the currently-hovered hyperlink /
    /// regex hint. Only populated for the active panel.
    /// Triggers the forced underline in `emit_underlines`;
    /// no bg / fg color change.
    hovered_hyperlink: Option<(
        neoism_terminal_core::crosswords::pos::Pos,
        neoism_terminal_core::crosswords::pos::Pos,
    )>,
}

/// Per-frame scratch that carries locals across the extracted
/// `render()` phase methods. Built once at the top of `render()`;
/// each phase reads/writes `ctx.field` in place of the original
/// shared local. Pure mechanical container.
pub(crate) struct FrameCtx {
    window_id: neoism_window::window::WindowId,
    render_started: std::time::Instant,
    editor_redraw_backlog_pending: bool,
    current_route: usize,
    editor_scroll_was_animating: bool,
    window_update: Option<crate::context::renderable::WindowUpdate>,
    any_panel_dirty: bool,
    has_animation: bool,
    initial_redraw_reason: Option<&'static str>,
    late_redraw_reason: Option<&'static str>,
    scale_factor: f32,
    trail_cursor_target: Option<neoism_ui::chrome_policy::TrailCursorOverlayTarget>,
    scaled_margin: neoism_backend::config::layout::Margin,
    panels: Vec<PanelFrame>,
}

impl Screen<'_> {
    fn draw_chrome_trail_cursor_rect(
        &mut self,
        [x, y, w, h]: [f32; 4],
        scale_factor: f32,
        animation_dt_secs: f32,
        cursor_blink_visible: bool,
    ) {
        self.renderer
            .trail_cursor
            .set_cursor_shape(neoism_terminal_core::ansi::CursorShape::Block);
        self.renderer.trail_cursor.set_destination(
            x * scale_factor,
            y * scale_factor,
            w * scale_factor,
            h * scale_factor,
        );
        if self.renderer.trail_cursor_enabled {
            self.renderer.trail_cursor.animate(
                w * scale_factor,
                h * scale_factor,
                animation_dt_secs,
            );
        } else {
            self.renderer
                .trail_cursor
                .snap_to_destination(w * scale_factor, h * scale_factor);
        }
        if self.renderer.trail_cursor.is_animating() || cursor_blink_visible {
            let cursor_color = self.renderer.live_cursor_color();
            self.renderer.trail_cursor.draw_always(
                &mut self.sugarloaf,
                scale_factor,
                cursor_color,
            );
        }
    }

    fn draw_agent_input_trail_cursor_rect(
        &mut self,
        [x, y, w, h]: [f32; 4],
        scale_factor: f32,
        _animation_dt_secs: f32,
        cursor_blink_visible: bool,
    ) {
        self.renderer
            .trail_cursor
            .set_cursor_shape(neoism_terminal_core::ansi::CursorShape::Block);
        self.renderer.trail_cursor.set_destination(
            x * scale_factor,
            y * scale_factor,
            w * scale_factor,
            h * scale_factor,
        );
        self.renderer
            .trail_cursor
            .snap_to_destination(w * scale_factor, h * scale_factor);
        if self.renderer.trail_cursor.is_animating() || cursor_blink_visible {
            let cursor_color = self.renderer.live_cursor_color();
            self.renderer.trail_cursor.draw_always(
                &mut self.sugarloaf,
                scale_factor,
                cursor_color,
            );
        }
    }

    fn chrome_trail_cursor_rect(
        &self,
        target: TrailCursorOverlayTarget,
        tab_cursor_rect: Option<[f32; 4]>,
    ) -> Option<[f32; 4]> {
        match target {
            TrailCursorOverlayTarget::Finder => {
                self.renderer.finder.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::CommandPalette => {
                self.renderer.command_palette.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::ContextMenu => {
                self.renderer.context_menu.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::FileTree => {
                self.renderer.file_tree.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::NotesSidebar => {
                self.renderer.notes_sidebar.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::AgentSidePanel => self
                .context_manager
                .current()
                .neoism_agent
                .as_ref()
                .and_then(|agent| agent.side_panel().selected_cursor_rect()),
            TrailCursorOverlayTarget::Tabs => tab_cursor_rect,
            TrailCursorOverlayTarget::GitDiffPanel => {
                self.renderer.git_diff_panel.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::AgentInput => self
                .context_manager
                .current()
                .neoism_agent
                .as_ref()
                .and_then(|agent| agent.cursor_rect()),
            _ => None,
        }
    }

    pub(crate) fn render(
        &mut self,
        animation_dt: std::time::Duration,
        is_fullscreen: bool,
        mut before_present: impl FnMut(),
    ) -> Option<crate::context::renderable::WindowUpdate> {
        let window_id = self.context_manager.window_id();
        crate::app::freeze_watchdog::mark_render_stage(window_id, "screen.render.enter");

        // First-run only: auto-open the notes sidebar with `Welcome/`
        // expanded (no note opened, focus left on the splash/terminal).
        // Runs on the first render tick — panes + workspace are wired by
        // now, unlike inside `Screen::new`'s inline struct return. The
        // method itself is further gated by the on-disk marker and deletes
        // it, so this only ever does work once per fresh install.
        if std::mem::take(&mut self.welcome_reveal_pending) {
            self.reveal_welcome_notes_first_run();
        }

        // Surface any background reMarkable auto-sync results (no-op unless
        // the optional `remarkable` extension is compiled in).
        self.poll_remarkable_autosync();
        // Frame-time tally for the editor scroll FPS log — wraps the
        // entire render path (cell emission + Vulkan submit + present)
        // so the log line can answer "where did the 13ms go" by
        // showing mean and worst single-frame duration alongside fps.
        let render_started = std::time::Instant::now();
        // Phase 2.0 smoke test: ensure the active panel has a
        // `GridRenderer`. This forces `MetalGridRenderer::new` /
        // `WgpuGridRenderer::new` to actually run on real hardware,
        // which is when the Metal shader compiler + wgpu pipeline
        // creator first see our shader source. Any shader syntax
        // error here becomes a startup panic rather than a silent
        // failure later. Nothing is rendered *through* the grid yet
        // — `sugarloaf.render()` is still called with no grids
        // slice below.
        // Drain editor-pane redraws BEFORE we read grid dimensions —
        // a `grid_resize` event may change them. Walk all editor
        // contexts so hidden buffers cannot accumulate an unbounded
        // redraw backlog and later stall the whole window.
        let (_, editor_redraw_backlog_pending) =
            self.context_manager.pump_editor_redraws();

        if self.context_manager.current().pending_terminal_resize {
            let current_editor_row_fit = {
                let current_grid = self.context_manager.current_grid();
                current_grid.current_item().and_then(|item| {
                    item.val.editor.as_ref().and_then(|_| {
                        Self::editor_rows_above_bottom_chrome(
                            item.layout_rect,
                            current_grid.get_scaled_margin(),
                            item.val.dimension,
                            self.sugarloaf.window_size().height as f32,
                            self.renderer.status_line_height()
                                * self.sugarloaf.scale_factor(),
                        )
                    })
                })
            };
            let current = self.context_manager.current_mut();
            if !Self::apply_context_resize(current, current_editor_row_fit) {
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "screen.render.skip_pending_resize_busy",
                );
                return None;
            }
        }

        let current_route = self.context_manager.current_route();
        let (grid_cols, grid_rows) = {
            let Some(terminal) =
                self.context_manager.current().terminal.try_lock_unfair()
            else {
                tracing::debug!(
                    target: "neoism::render",
                    route_id = current_route,
                    "skipping render frame because current terminal state is busy"
                );
                self.context_manager
                    .current_mut()
                    .renderable_content
                    .pending_update
                    .set_dirty();
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "screen.render.skip_terminal_busy",
                );
                return None;
            };
            (terminal.columns() as u32, terminal.screen_lines() as u32)
        };
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "screen.render.terminal_dimensions",
        );
        if grid_cols > 0 && grid_rows > 0 {
            let is_editor = self.context_manager.current().editor.is_some();
            self.ensure_grid(current_route, grid_cols, grid_rows, is_editor);
        }

        let mut frame_ctx = FrameCtx {
            window_id,
            render_started,
            editor_redraw_backlog_pending,
            current_route,
            editor_scroll_was_animating: false,
            window_update: None,
            any_panel_dirty: false,
            has_animation: false,
            initial_redraw_reason: None,
            late_redraw_reason: None,
            scale_factor: 0.0,
            trail_cursor_target: None,
            scaled_margin: neoism_backend::config::layout::Margin {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            },
            panels: Vec::new(),
        };

        self.sync_status_and_chrome(&frame_ctx);

        // Re-elect primary editor route every frame — handles
        // workspace switches (the prior workspace's pinned route
        // vanishes from the new grid) and split-pane closures (the
        // primary may have been the closed pane, leaving us pointing
        // at a dead route). Cheap: a single hash lookup when the
        // current pin is still valid.
        self.ensure_primary_editor_route();

        if self.poll_notebook_executions() {
            self.mark_dirty();
        }
        self.renderer.notebook_animating = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .is_some_and(|notebook| notebook.has_running_cells());
        if self.render_markdown_panels() {
            self.mark_dirty();
        }
        if self.render_draw_panels() {
            self.mark_dirty();
        }
        if self.render_neoism_tags_panels() {
            self.mark_dirty();
        }
        if self.render_neoism_extensions_panels() {
            self.mark_dirty();
        }
        self.render_neoism_agent_panels();

        frame_ctx.editor_scroll_was_animating =
            self.step_editor_scroll_for_frame(animation_dt);

        let (window_update, any_panel_dirty) = self.renderer.run(
            &mut self.sugarloaf,
            &mut self.context_manager,
            &self.search_state.focused_match,
        );
        frame_ctx.window_update = window_update;
        frame_ctx.any_panel_dirty = any_panel_dirty;
        if frame_ctx.editor_redraw_backlog_pending {
            self.mark_dirty();
        }

        self.draw_overlays(&mut frame_ctx, animation_dt);

        self.snapshot_panels(&mut frame_ctx);
        self.mask_editor_seams(&frame_ctx);
        self.emit_and_present_grids(
            &frame_ctx,
            animation_dt,
            is_fullscreen,
            &mut before_present,
        );

        // Mark as dirty if we need continuous rendering (e.g.,
        // indeterminate progress bar, trail cursor animation). UI-only
        // — terminal cells didn't change, but we want the next vsync
        // to fire a render so overlays/animations tick forward.
        if frame_ctx.has_animation {
            self.context_manager
                .current_mut()
                .renderable_content
                .pending_update
                .set_dirty();
        }

        // In case the configuration of blinking cursor is enabled
        // and the terminal also have instructions of blinking enabled
        // TODO: enable blinking for selection after adding debounce (https://github.com/raphamorim/rio/issues/437)
        if self.renderer.config_has_blinking_enabled
            && self.selection_is_empty()
            && (self
                .context_manager
                .current()
                .renderable_content
                .has_blinking_enabled
                || frame_ctx.trail_cursor_target.is_some())
        {
            self.context_manager
                .blink_cursor(self.renderer.config_blinking_interval);
        }

        // Stash this frame's full duration (CPU emission + Vulkan
        // submit + queue_present) so the next frame's FPS log can show
        // mean_full_ms / max_full_ms / wait_outside_render_ms. Doing
        // this here, AFTER `sugarloaf.render_with_grids` has run, means
        // the timing covers everything the FPS log fps number is paced
        // against — including the swapchain acquire fence wait and the
        // queue_present call that interacts with vsync.
        self.last_full_render_us = frame_ctx
            .render_started
            .elapsed()
            .as_micros()
            .min(u64::MAX as u128) as u64;

        crate::app::freeze_watchdog::mark_render_stage(
            frame_ctx.window_id,
            "screen.render.return",
        );
        frame_ctx.window_update
    }
}
