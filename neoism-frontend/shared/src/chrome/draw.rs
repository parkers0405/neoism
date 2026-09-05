use super::*;

use web_time::Duration;

use sugarloaf::Sugarloaf;

use crate::chrome_policy::{
    trail_cursor_overlay_draw_kind, trail_cursor_overlay_target,
    TrailCursorOverlayDrawKind, TrailCursorOverlayState, TrailCursorOverlayTarget,
};
use crate::input::InputBuffer;
use crate::layout::PanelLayout;
use crate::panels::agent_pane::view as agent_pane_view;
use crate::panels::splash_overlay::{SplashInjection, SplashOverlay};
use crate::panels::terminal_splash::adapt_layout;
use crate::panels::{Panel, PanelContext};
use crate::services::Services;

/// The shared `Chrome::draw` path is the web/WASM status-line host. Keep both
/// the strip and its interactive contents in window coordinates: side panels
/// are confined to the middle band and must not reflow this bottom band.
fn status_line_render_geometry(layout: &crate::layout::ChromeLayout) -> (Rect, Rect) {
    (layout.status_line, layout.status_line)
}

impl<A: Send + Copy + 'static> Chrome<A> {
    /// Paint every visible panel through `sugarloaf` in z-order.
    /// Background panels paint first; modal overlays paint last so
    /// they sit on top.
    ///
    /// The terminal canvas itself is drawn by the host outside of
    /// `Chrome` — the chrome only owns *chrome* surfaces. The host
    /// uses `Chrome::layout().terminal` as the canvas rect.
    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        services: Services<'_>,
        time: Duration,
    ) {
        let theme = self.theme.clone();
        let ctx = PanelContext {
            services,
            theme: &theme,
            time,
        };
        let dt = match self.last_draw_time {
            Some(prev) if time > prev => (time - prev).as_secs_f32().clamp(0.0, 0.1),
            // First-ever frame or non-monotonic clock — fall back to a
            // 60Hz budget so springs still advance toward their
            // destinations instead of stalling at zero.
            _ => 1.0 / 60.0,
        };
        self.last_draw_time = Some(time);
        // Re-derived below while a hosted editor pane paints; cleared
        // here so switching to a terminal/agent tab stops the pump.
        self.editor_pane_animating = false;

        // 1. Background strips: buffer tabs (top), status line (bottom).
        let layout = self.layout.clone();
        let input_modal_active = self.command_palette.is_enabled()
            || self.finder.is_enabled()
            || self.git_diff.is_visible()
            || self.context_menu.is_visible();
        let window_width = [
            layout.buffer_tabs.x + layout.buffer_tabs.w,
            layout.status_line.x + layout.status_line.w,
            layout.terminal.x + layout.terminal.w,
        ]
        .into_iter()
        .fold(0.0_f32, f32::max);
        let command_palette_occlusion =
            self.command_palette.active_visual_rect(window_width, 1.0);
        let mut active_text_occlusions: Vec<[f32; 4]> =
            command_palette_occlusion.into_iter().collect();
        if let Some(rect) = self.file_browser.occlusion_rect_for([
            0.0,
            0.0,
            window_width,
            layout.status_line.y + layout.status_line.h,
        ]) {
            active_text_occlusions.push(rect);
        }
        if self.share_sheet.is_visible() {
            active_text_occlusions.push([
                0.0,
                0.0,
                window_width,
                layout.status_line.y + layout.status_line.h,
            ]);
        }
        if input_modal_active {
            self.buffer_tabs.clear_hover_immediate();
            self.buffer_tabs.set_focused(false);
            self.blur(PanelKey::BufferTabs);
        }
        // Agent-logo overlays are immediate-mode: `push_image_overlay`
        // APPENDS to a per-panel Vec, so the strip must drop last frame's
        // pushes or every repaint would stack another copy forever. The
        // desktop host does the same through its own `clear_icon_overlays`
        // (it doesn't go through `Chrome`, so this clear is web-only).
        sugarloaf
            .clear_image_overlays_for(crate::panels::agent_pane::icon::ICON_PANEL_ID);
        // Splash images are retained by Sugarloaf, unlike frame-local quads
        // and text. Clear the previous frame before any eligibility branch so
        // a full-page Tree/Notes takeover cannot leave the old wordmark alive.
        SplashOverlay::clear_image_overlays(sugarloaf);
        if layout.buffer_tabs.w > 0.0 && layout.buffer_tabs.h > 0.0 {
            self.buffer_tabs.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: layout.buffer_tabs,
                    scale: 1.0,
                },
                &ctx,
            );
        }

        // Window-top chrome strip is rendered at the very end of this
        // function (search "TOP BAR LAST PASS") so its dropdown's
        // block-glyph fill emits AFTER every other panel's text and
        // properly overlays labels from the file tree, buffer tabs,
        // breadcrumbs, etc. Painting it here would let later panel
        // text bleed through the open menu.
        // Desktop parity: paint through the SAME entry point the native
        // host uses (`render_with_ide_theme_in_content_bounds`, see
        // `host/run.rs`) with the real `IdeTheme`.
        //
        // This used to build a `StatusPalette` out of `ChromeTheme` via
        // `status_palette_from_theme`, which remaps to DIFFERENT palette
        // slots than desktop's conversion does — `surface<-bg_elevated`,
        // `muted<-fg_dim`, `red<-error`, `green<-success` and notably
        // `blue<-accent`. Same widget, different numbers, so the web
        // status bar came out a different color than the desktop one.
        // The content-bounds variant also starts the pills at the editor
        // column instead of x=0, which is the other half of the mismatch.
        let (status_background, status_content) = status_line_render_geometry(&layout);
        self.status_line.render_with_ide_theme_in_content_bounds(
            sugarloaf,
            status_background.x,
            status_background.y,
            status_background.w,
            status_content.x,
            status_content.w,
            &self.ide_theme,
        );

        // 2. File tree sidebar.
        if let (Some(rect), Some(tree)) = (layout.file_tree, self.file_tree.as_ref()) {
            tree.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: rect,
                    scale: 1.0,
                },
                &ctx,
            );
        }

        // 3. Splash overlay — animated NEOISM wordmark + menu over the
        //    terminal pane. The host paints terminal cells outside of
        //    `Chrome::draw`, so by emitting splash overlays here we sit
        //    on top of the cells but under the composer / modals.
        //
        //    The host controls the `wants_visible` signal: web mirrors
        //    command submission into `dismiss_terminal_splash`, while
        //    desktop derives the same idea from terminal input state.
        let terminal_rect = layout.terminal;
        // While the pane grid is split the ACTIVE surface paints into
        // the focused pane's rect; unfocused panes render through
        // `draw_unfocused_pane_surfaces` below.
        let content_rect = self.focused_content_rect();
        let content_available = content_rect.w > 0.0 && content_rect.h > 0.0;
        if !content_available {
            agent_pane_view::clear_overlays(sugarloaf);
            self.splash_overlay.reset();
        }
        if content_available {
            if self.is_terminal_tab_active() {
                // The splash wordmark is a whole-content flourish — it
                // stands down while the grid is split so it can't paint
                // across pane boundaries.
                let wants_splash =
                    !self.terminal_splash_dismissed && !self.pane_grid.is_split();
                if wants_splash {
                    agent_pane_view::clear_overlays(sugarloaf);
                    // Terminal tab — paint the splash overlay on top of
                    // the host-rendered cells.
                    let cell_w = self.cell_w;
                    let cell_h = self.cell_h;
                    let rows = (terminal_rect.h / cell_h).floor().max(0.0) as usize;
                    let splash_layout = adapt_layout(rows);
                    let injection = match splash_layout {
                        Some(sl) => SplashInjection {
                            wordmark_row: sl.wordmark_row(),
                            wordmark_cells_h: sl.wordmark_rows,
                            gap_cells_h: sl.gap_rows,
                            menu_cells_h: sl.menu_rows,
                        },
                        None => SplashInjection::default(),
                    };
                    let ide_theme = self.ide_theme;
                    self.splash_overlay.render(
                        sugarloaf,
                        &injection,
                        (terminal_rect.x, terminal_rect.y),
                        (terminal_rect.w, terminal_rect.h),
                        cell_w,
                        cell_h,
                        &ide_theme,
                        1.0,
                        true,
                        &active_text_occlusions,
                    );
                } else {
                    SplashOverlay::clear_image_overlays(sugarloaf);
                    agent_pane_view::clear_overlays(sugarloaf);
                    self.splash_overlay.reset();
                }
            } else if self.is_neoism_agent_tab_active() {
                SplashOverlay::clear_image_overlays(sugarloaf);
                self.splash_overlay.reset();
                let narrow_takeover = self.agent_side_panel_takeover_active();
                if let Some(pane) = self.agent_pane.as_mut() {
                    if narrow_takeover {
                        // The composer is not rendered during takeover, so its
                        // previous frame's caret must not remain globally live.
                        pane.set_cursor_rect(None);
                    }
                    agent_pane_view::render_responsive(
                        sugarloaf,
                        pane,
                        [
                            content_rect.x,
                            content_rect.y,
                            content_rect.w,
                            content_rect.h,
                        ],
                        &self.ide_theme,
                        true,
                        time.as_secs_f32(),
                        Some(self.last_pointer_pos),
                        self.chrome_scale,
                        &active_text_occlusions,
                        narrow_takeover,
                    );
                }
            } else {
                // File-viewer tab — paint the cached text content over
                // a solid theme-bg rect. Clears the splash overlays so
                // the wordmark doesn't bleed through.
                SplashOverlay::clear_image_overlays(sugarloaf);
                agent_pane_view::clear_overlays(sugarloaf);
                self.splash_overlay.reset();
                let theme = self.ide_theme;
                // Backdrop for the ACTIVE surface only. While split the
                // fill stays inside the focused pane's content rect —
                // a full-rect fill at this order would paint over the
                // secondary pane terminals' cell backgrounds.
                sugarloaf.rect(
                    None,
                    content_rect.x,
                    content_rect.y,
                    content_rect.w,
                    content_rect.h,
                    theme.f32(theme.bg),
                    0.0,
                    1,
                );
                // Tick the rubber-band spring forward BEFORE borrowing
                // `tab_content` so the spring write doesn't fight the
                // text borrow. `dt` is a fixed-ish frame budget — we
                // don't get a real delta here without threading the
                // host clock through draw, but 16ms is close enough
                // for the settle and the spring clamps internally.
                // Match native editor_scroll's `ANIMATION_LENGTH =
                // 0.30` (time-to-target-within-2%). Using a shorter
                // value made the rubber-band feel jittery vs desktop.
                self.scroll_spring.update(1.0 / 60.0, 0.30);
                let effective_offset =
                    (self.scroll_offset_px + self.scroll_spring.position).max(0.0);

                if let Some(page) = self.active_chrome_page() {
                    // Chrome helper page (Extensions / NeoWorld) —
                    // paints over the theme-bg backdrop laid down
                    // above, the same way desktop's context-grid pages
                    // take over the pane body.
                    let _ = effective_offset; // pages own their scroll
                    self.draw_chrome_page_body(sugarloaf, page, content_rect);
                } else if self.tab_lang == crate::syntax::Lang::Markdown {
                    if let Some(pane) = self.markdown_pane.as_mut() {
                        // The REAL renderer — same virtualized path as the
                        // desktop (Live Preview, caret, remote carets,
                        // roster). The legacy draw_markdown_blocks painter
                        // showed raw markup and no cursor.
                        let _ = effective_offset; // pane owns its own scroll
                                                  // Follow-cursor, like the desktop's per-frame call:
                                                  // arrowing off-screen scrolls the doc to keep the
                                                  // caret visible (uses last frame's caret rect; the
                                                  // host's animation pump keeps frames flowing until
                                                  // the eased scroll settles).
                        pane.scroll_cursor_into_view(content_rect.y, content_rect.h);
                        pane.tick_scroll();
                        let chrome_scale = self.chrome_scale;
                        crate::editor::markdown::render::render(
                            sugarloaf,
                            pane,
                            [
                                content_rect.x,
                                content_rect.y,
                                content_rect.w,
                                content_rect.h,
                            ],
                            &theme,
                            None,
                            &[],
                            chrome_scale,
                            self.animation_phase,
                        );
                    }
                } else if let Some(kind) = self.active_editor_pane_kind() {
                    // Hosted native editor pane (code / notebook /
                    // draw) — the desktop-parity surfaces. Renders the
                    // SAME shared painters desktop uses; the legacy
                    // read-only text loop below is now only the
                    // fallback for tabs no pane claimed.
                    let chrome_scale = self.chrome_scale;
                    let rect = [
                        content_rect.x,
                        content_rect.y,
                        content_rect.w,
                        content_rect.h,
                    ];
                    match kind {
                        crate::chrome::EditorPaneKind::Code => {
                            let mouse =
                                Some([self.last_pointer_pos.0, self.last_pointer_pos.1]);
                            if let Some(pane) = self.code_pane.as_mut() {
                                // The chrome trail cursor draws the
                                // caret (desktop parity) — the pane
                                // only publishes `cursor_rect`.
                                pane.caret_drawn_by_host = true;
                                let animating = crate::editor::code::render::render(
                                    sugarloaf,
                                    pane,
                                    rect,
                                    &theme,
                                    &[],
                                    chrome_scale,
                                    mouse,
                                );
                                self.editor_pane_animating |= animating;
                            }
                        }
                        crate::chrome::EditorPaneKind::Notebook => {
                            let animation_phase = self.animation_phase;
                            if let Some(pane) = self.notebook_pane.as_mut() {
                                let markdown = &mut pane.markdown;
                                let follow = markdown.scroll_cursor_into_view(
                                    content_rect.y,
                                    content_rect.h,
                                );
                                let ticking = markdown.tick_scroll();
                                crate::editor::markdown::render::render(
                                    sugarloaf,
                                    markdown,
                                    rect,
                                    &theme,
                                    None,
                                    &[],
                                    chrome_scale,
                                    animation_phase,
                                );
                                self.editor_pane_animating |= follow || ticking;
                            }
                        }
                        crate::chrome::EditorPaneKind::Draw => {
                            if let Some(pane) = self.draw_pane.as_mut() {
                                crate::editor::neodraw::render_pane(
                                    sugarloaf, pane, rect, &theme,
                                );
                                let graph_animating = pane
                                    .graph
                                    .as_ref()
                                    .is_some_and(|graph| graph.is_animating());
                                self.editor_pane_animating |= graph_animating;
                            }
                        }
                    }
                } else if let Some(text) = self.tab_content.as_deref() {
                    let line_h = self.cell_h.max(14.0);
                    let pad_x = 16.0_f32;
                    let pad_y = 12.0_f32;
                    let max_w = content_rect.w - pad_x * 2.0;
                    let max_h = content_rect.h - pad_y * 2.0;
                    let opts = sugarloaf::text::DrawOpts {
                        font_size: 13.0,
                        color: theme.u8(theme.fg),
                        clip_rect: Some([
                            content_rect.x + pad_x,
                            content_rect.y + pad_y,
                            max_w.max(0.0),
                            max_h.max(0.0),
                        ]),
                        ..sugarloaf::text::DrawOpts::default()
                    };
                    // Cull lines that fall outside the visible band
                    // before/after the offset. `i` indexes into the
                    // full text; only paint the slice the viewport
                    // covers (with one row of slop on each side so
                    // partial rows still render during a scroll
                    // animation).
                    let first_visible = ((effective_offset / line_h).floor() as isize - 1)
                        .max(0) as usize;
                    let last_visible_excl =
                        (((effective_offset + max_h) / line_h).ceil() as usize + 1)
                            .min(text.lines().count());
                    let lang = self.tab_lang;
                    for (i, line) in text
                        .lines()
                        .enumerate()
                        .skip(first_visible)
                        .take(last_visible_excl.saturating_sub(first_visible))
                    {
                        let y =
                            content_rect.y + pad_y + line_h * (i as f32) + line_h * 0.75
                                - effective_offset;
                        if y < content_rect.y - line_h
                            || y > content_rect.y + content_rect.h
                        {
                            continue;
                        }
                        // Emit one DrawOpts per syntax span so each
                        // gets its own foreground color. The x-cursor
                        // walks left-to-right; measure each span to
                        // advance. Lang::Other (and json/toml) produce
                        // a single Plain span so this still degrades
                        // to one draw per line for unknown filetypes.
                        let spans = crate::syntax::highlight_line(line, lang);
                        let mut x_cursor = content_rect.x + pad_x;
                        for (tok, slice) in spans {
                            if slice.is_empty() {
                                continue;
                            }
                            let mut span_opts = opts;
                            span_opts.color =
                                crate::syntax::syn_color(tok, &theme, false);
                            let w =
                                sugarloaf.text_mut().draw(x_cursor, y, slice, &span_opts);
                            x_cursor += w;
                        }
                    }
                }
            }
        }

        // 3a. Unfocused pane surfaces: parked editor panes render live
        //     into their pane rects; panes the host painted (secondary
        //     terminal grids) are skipped; anything else gets an honest
        //     labeled placeholder.
        if content_available {
            self.draw_unfocused_pane_surfaces(sugarloaf);
        }

        // 3a'. Per-pane tab strips + breadcrumbs on stacked panes
        //      (desktop `pane_tabs` / `pane_breadcrumbs` parity).
        if content_available {
            self.draw_pane_tab_strips(sugarloaf);
        }

        // 3b. Pane-grid chrome: divider bands between split panes, the
        //     focused-pane outline, and the live drag-to-split drop
        //     preview. Painted over the pane surfaces but under the
        //     composer / modals — the Rust twin of the deleted DOM
        //     `.terminal-pane-layout-cell` overlay.
        if content_available {
            self.draw_pane_grid_overlay(sugarloaf);
        }

        // 4. Sticky composer above the status line (still under modals).
        if let Some(rect) = layout.command_composer {
            let theme = self.ide_theme;
            let neutral = crate::panels::command_composer::InputClassification::neutral(
                theme.u8(theme.fg),
            );
            let trail_cursor_will_paint = self.terminal_composer_eligible();
            let _ = self.command_composer.render(
                sugarloaf,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                &theme,
                &self.terminal_input,
                None,
                None,
                self.animation_phase,
                true,
                self.cell_w.max(1.0),
                self.cell_h.max(1.0),
                trail_cursor_will_paint,
                530,
                neutral,
                self.terminal_input.shell_kind(),
            );
        }

        // 4b. Slim panels lifted in Wave 6F: breadcrumbs strip, toast
        //     notifications, completion popup, in-buffer search bar,
        //     minimap rail, yank flash, cursor surfaces. Most paint
        //     over the terminal column / tab bar through the
        //     `Panel`-shaped `draw` adapter shims in
        //     `panels::chrome_shim_more`; the minimap renders directly
        //     through its per-route `render_pane` (desktop parity).
        //     Data-driven panels (toasts without queue, popup without
        //     snapshot, minimap without a fed snapshot) early-return
        //     without painting.
        if let Some(rect) = layout
            .breadcrumbs
            .filter(|rect| rect.w > 0.0 && rect.h > 0.0)
        {
            self.breadcrumbs.render_with_options(
                sugarloaf,
                rect.x,
                rect.y,
                rect.w,
                &self.ide_theme,
                !input_modal_active,
            );
        }
        if !self.connection_gate_active() {
            self.notifications.draw(
                sugarloaf,
                &PanelLayout {
                // Full-width band: buffer_tabs spans the whole viewport,
                // so toasts anchor at the real WINDOW right edge instead
                // of the terminal pane's (which reserves right-side
                // space and starts after the file tree).
                bounds: crate::layout::Rect {
                    x: layout.buffer_tabs.x,
                    y: layout.terminal.y,
                    w: layout.buffer_tabs.w,
                    h: layout.terminal.h,
                },
                scale: 1.0,
                },
                &ctx,
            );
        }
        if content_available {
            self.search_overlay.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: layout.terminal,
                    scale: 1.0,
                },
                &ctx,
            );
        }
        // Minimap rail — desktop parity with the per-route loop in
        // `desktop/src/screen/render/cell_emit.rs`: `begin_frame()`
        // resets the hit rects, then each visible pane paints the
        // snapshot pushed for its route (`Minimap::apply_update`).
        // Route ids come from the pane grid's host-bound external ids;
        // a surface with no bound id (the common single-pane web case)
        // falls back to the terminal rect + route 0, which is the route
        // the web host feeds for the active editor surface.
        self.minimap.begin_frame();
        if content_available && self.minimap.is_enabled() {
            let ide_theme = self.ide_theme;
            let minimap_cell_h = self.cell_h.max(1.0);
            let pane_routes: Vec<(u64, crate::layout::Rect)> = self
                .pane_grid
                .panes()
                .iter()
                .filter_map(|pane| pane.external_id.map(|id| (id, pane.rect)))
                .collect();
            if pane_routes.is_empty() {
                let rows = (layout.terminal.h / minimap_cell_h).floor().max(1.0) as u32;
                self.minimap.render_pane(
                    sugarloaf,
                    0,
                    layout.terminal.x,
                    layout.terminal.y,
                    layout.terminal.w,
                    layout.terminal.h,
                    rows,
                    0.0,
                    ide_theme,
                );
            } else {
                for (route_id, rect) in pane_routes {
                    let rows = (rect.h / minimap_cell_h).floor().max(1.0) as u32;
                    self.minimap.render_pane(
                        sugarloaf,
                        route_id as usize,
                        rect.x,
                        rect.y,
                        rect.w,
                        rect.h,
                        rows,
                        0.0,
                        ide_theme,
                    );
                }
            }
        }
        if content_available {
            self.yank_flash.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: layout.terminal,
                    scale: 1.0,
                },
                &ctx,
                self.cell_w,
                self.cell_h,
            );
        }
        // Code-pane LSP session hosting: pump the shared session layer
        // (buffer sync, mouse-rest hover, diagnostics refold, popup
        // dismissal) and feed its completion / code-action menu into
        // the stored-popup slot the shim below paints from.
        if content_available {
            self.pump_code_lsp_layer(input_modal_active);
            self.completion_menu.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: layout.terminal,
                    scale: 1.0,
                },
                &ctx,
                self.cell_w,
                self.cell_h,
            );
        }
        if content_available && self.context_menu.is_visible() {
            let window_w = [
                layout.buffer_tabs.x + layout.buffer_tabs.w,
                layout.status_line.x + layout.status_line.w,
                layout.terminal.x + layout.terminal.w,
                layout.file_tree.map(|rect| rect.x + rect.w).unwrap_or(0.0),
            ]
            .into_iter()
            .fold(0.0_f32, f32::max);
            let window_h = layout.status_line.y + layout.status_line.h;
            self.context_menu.render(
                sugarloaf,
                (window_w, window_h, 1.0),
                &self.ide_theme,
            );
        }

        // Modal overlays must draw before the shared trail cursor so
        // their selected cursor rects are from this frame. Desktop
        // renders palette/finder first, then drives the cursor trail
        // from the freshly computed rect; doing this after the trail
        // leaves the web cursor one animated frame behind.
        if let Some(rect) = layout.git_diff {
            self.git_diff.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: rect,
                    scale: 1.0,
                },
                &ctx,
            );
        }
        if let Some(rect) = layout.finder {
            self.finder.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: rect,
                    scale: 1.0,
                },
                &ctx,
            );
        }
        if let Some(rect) = layout.command_palette {
            self.command_palette.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: rect,
                    scale: 1.0,
                },
                &ctx,
            );
        }
        // TrailCursor drive: mirror the native priority chain from
        // `frontends/neoism/src/screen/render/mod.rs` (lines 1034-1257)
        // so a single cursor glides between surfaces in the same
        // order as the native renderer. Each active branch performs
        // the same four operations native does: set Block shape, set
        // destination, animate, then `draw_always` with the cursor
        // color.
        //
        // Web works entirely in CSS pixels — there's no separate
        // physical-pixel scale to multiply by — so rects are passed
        // through unmodified and `cell_w` / `cell_h` (already in CSS
        // px) drive `animate`.
        let cell_w = self.cell_w.max(1.0);
        let cell_h = self.cell_h.max(1.0);
        let cursor_color = self.live_cursor_color();

        let tab_cursor_rect = self.buffer_tabs.focused_cursor_rect();
        let agent_tab_active = self.is_neoism_agent_tab_active();
        let agent_side_panel_takeover = self.agent_side_panel_takeover_active();
        let agent_side_panel_focused = content_available
            && agent_tab_active
            && self
                .agent_pane
                .as_ref()
                .is_some_and(|pane| pane.side_panel().is_focused());
        let agent_input_cursor_available = content_available
            && agent_tab_active
            && !agent_side_panel_takeover
            && self
                .agent_pane
                .as_ref()
                .and_then(|pane| pane.cursor_rect())
                .is_some();
        // Hosted editor pane (code / notebook / draw) claim for the
        // trail-cursor owner. Only counts while a non-terminal,
        // non-agent tab is active — the same gate the render branch
        // above uses.
        let editor_pane_kind =
            if content_available && !self.is_terminal_tab_active() && !agent_tab_active {
                self.active_editor_pane_kind()
            } else {
                None
            };
        let code_cursor_available = editor_pane_kind
            == Some(crate::chrome::EditorPaneKind::Code)
            && self
                .code_pane
                .as_ref()
                .and_then(|pane| pane.cursor_rect)
                .is_some();
        let notebook_cursor_available = editor_pane_kind
            == Some(crate::chrome::EditorPaneKind::Notebook)
            && self
                .notebook_pane
                .as_ref()
                .and_then(|pane| pane.markdown.cursor_rect)
                .is_some();
        let markdown_cursor_available = (!self.is_terminal_tab_active()
            && self.tab_lang == crate::syntax::Lang::Markdown
            && self
                .markdown_pane
                .as_ref()
                .and_then(|pane| pane.cursor_rect)
                .is_some())
            || notebook_cursor_available;
        let markdown_active = content_available
            && !self.is_terminal_tab_active()
            && self.tab_lang == crate::syntax::Lang::Markdown;
        let terminal_block_input_active = content_available
            && self.terminal_composer_eligible()
            && self.command_composer.last_frame().caret_rect.is_some();

        match trail_cursor_overlay_target(TrailCursorOverlayState {
            finder_enabled: self.finder.is_enabled(),
            command_palette_enabled: self.command_palette.is_enabled(),
            // Markdown completion popups (the `/` block menu and `[[` link
            // menu) are typing aids — the caret stays on the text being
            // typed instead of jumping into the popup rows.
            context_menu_visible: self.context_menu.is_visible()
                && !self.context_menu.is_markdown_link_completion()
                && !self.context_menu.is_markdown_block_completion(),
            file_tree_focused: self
                .file_tree
                .as_ref()
                .is_some_and(|tree| tree.is_focused()),
            notes_sidebar_focused: self.notes_sidebar.is_focused(),
            agent_side_panel_focused,
            tab_cursor_available: tab_cursor_rect.is_some(),
            // Either git surface claims the overlay: the slim modal while
            // visible, or the rich side panel while focused (desktop
            // parity — the caret jumps to its selected file row on open).
            git_diff_panel_focused: self.git_diff.is_visible()
                || self.git_diff_panel.is_focused(),
            search_active: self.search_overlay.is_active(),
            // Settings/file modals and every other generic keyboard owner
            // suppress the unrelated content caret just like palette/finder.
            modal_owns_editor_focus: self.generic_keyboard_overlay_active(),
            agent_surface_active: content_available && agent_tab_active,
            agent_input_cursor_available,
            markdown_cursor_available,
            code_cursor_available,
            // Every hosted editor owns cursor focus even when its document
            // caret is scrolled fully offscreen. Otherwise Ctrl+D/U can fall
            // through to the parked terminal cursor at the top-left corner.
            cursorless_surface_active: editor_pane_kind.is_some()
                || markdown_active
                || self.active_chrome_page().is_some(),
            terminal_block_input_active,
            // Hosted editor panes draw content carets (or none) — the
            // terminal-grid fallback must not park a cursor over them
            // while the pane's first `cursor_rect` is still pending.
            trail_cursor_enabled: content_available
                && !markdown_active
                && editor_pane_kind.is_none(),
        }) {
            Some(target)
                if trail_cursor_overlay_draw_kind(target)
                    == TrailCursorOverlayDrawKind::ChromeRect =>
            {
                if let Some(rect) = self.chrome_trail_cursor_rect(target, tab_cursor_rect)
                {
                    self.draw_block_trail_cursor_rect(
                        sugarloaf,
                        rect,
                        cell_w,
                        cell_h,
                        dt,
                        cursor_color,
                    );
                }
            }
            Some(TrailCursorOverlayTarget::SuppressedByInputOverlay) | None => {}
            Some(TrailCursorOverlayTarget::AgentInput) => {
                if let Some(rect) = self.chrome_trail_cursor_rect(
                    TrailCursorOverlayTarget::AgentInput,
                    tab_cursor_rect,
                ) {
                    let [x, y, w, h] = rect;
                    self.trail_cursor
                        .set_cursor_shape(neoism_terminal_core::ansi::CursorShape::Block);
                    self.trail_cursor.set_destination(x, y, w, h);
                    // `set_destination` only records where the caret is
                    // headed; `draw_quad` builds its triangles from the
                    // spring-animated `corners[i].x/.y`, which ONLY
                    // `animate`/`snap_to_destination` ever write. Without
                    // this advance the quad stayed at its initial (0,0)
                    // zero-area state, so the agent composer's caret was
                    // never visible even though focus and typing worked -
                    // and since this is the only arm taken while the agent
                    // tab is active, nothing else advanced it either. Every
                    // other target animates (see
                    // `draw_block_trail_cursor_rect` /
                    // `draw_content_trail_cursor_rect`); this arm was the
                    // lone outlier. It also re-arms `is_animating()`, so the
                    // caret counts as an animation owner in
                    // `animations_active()` and the host keeps pumping
                    // frames for it.
                    self.trail_cursor.animate(w, h, dt);
                    sugarloaf.set_late_overlay_mode(true);
                    self.trail_cursor.draw_always(sugarloaf, 1.0, cursor_color);
                    sugarloaf.set_late_overlay_mode(false);
                }
            }
            Some(TrailCursorOverlayTarget::Markdown) => {
                // A `.md` tab's own pane wins; otherwise the notebook's
                // inner markdown pane owns the caret (its
                // `notebook_cursor_available` claim got us here).
                let cursor = self
                    .markdown_pane
                    .as_ref()
                    .filter(|_| markdown_active)
                    .or_else(|| self.notebook_pane.as_ref().map(|pane| &pane.markdown))
                    .and_then(|pane| {
                        pane.cursor_rect.map(|rect| (rect, pane.cursor_shape()))
                    });
                if let Some((rect, shape)) = cursor {
                    self.draw_content_trail_cursor_rect(
                        sugarloaf,
                        rect,
                        shape,
                        dt,
                        cursor_color,
                    );
                }
            }
            Some(TrailCursorOverlayTarget::Code) => {
                let cursor = self.code_pane.as_ref().and_then(|pane| {
                    pane.cursor_rect.map(|rect| (rect, pane.cursor_shape()))
                });
                if let Some((rect, shape)) = cursor {
                    self.draw_content_trail_cursor_rect(
                        sugarloaf,
                        rect,
                        shape,
                        dt,
                        cursor_color,
                    );
                }
            }
            Some(TrailCursorOverlayTarget::TerminalBlockInput) => {
                if let Some(rect) = self.chrome_trail_cursor_rect(
                    TrailCursorOverlayTarget::TerminalBlockInput,
                    tab_cursor_rect,
                ) {
                    self.draw_content_trail_cursor_rect(
                        sugarloaf,
                        rect,
                        neoism_terminal_core::ansi::CursorShape::Block,
                        dt,
                        cursor_color,
                    );
                }
            }
            Some(TrailCursorOverlayTarget::TerminalGrid) => {
                // Default terminal tabs have a cell cursor underneath
                // the trail, so they only need the in-flight afterimage.
                self.trail_cursor.animate(cell_w, cell_h, dt);
                self.trail_cursor.draw_slim(
                    sugarloaf,
                    &PanelLayout {
                        bounds: layout.terminal,
                        scale: 1.0,
                    },
                    &ctx,
                );
            }
            Some(_) => {}
        }
        // 6. Custom mouse-cursor sprite — paints on top of everything
        //    so the pointer sits above modal overlays. The desktop
        //    renderer drives this from its live `Mouse` struct; on the
        //    web bridge the position is pushed in through
        //    `Chrome.custom_cursor.set_position(...)` from JS. When
        //    `visible` is false (the default until the host pushes a
        //    position) the sprite is suppressed.
        if self.custom_cursor.visible {
            // The free draw fn divides x/y by `scale` internally to
            // land in logical pixels. The web bridge already passes
            // physical-pixel coordinates (matching the desktop's
            // `Mouse.x` / `Mouse.y` convention), so we forward an
            // identity scale of 1.0 here. Hosts that want a different
            // sprite scaling can wrap this call themselves.
            crate::panels::custom_cursor::draw(
                sugarloaf,
                self.custom_cursor.x,
                self.custom_cursor.y,
                1.0,
            );
        }

        // The side panels are confined to the middle band — the strip
        // between the bottom of the full-width top chrome (top bar +
        // workspace strip, i.e. the top of the buffer tabs) and the top
        // of the full-width status bar — so they no longer run the
        // whole window height. The tabs sit at the band's top edge in
        // the content column, so the band starts at `buffer_tabs.y`.
        let band_top = layout.buffer_tabs.y;
        let band_bottom = layout.status_line.y;

        // Notes sidebar — left column right of the file tree, scoped to
        // the middle band.
        if let Some(rect) = layout.notes_sidebar {
            let ide_theme = self.ide_theme;
            self.notes_sidebar.render(
                sugarloaf,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                &ide_theme,
                &[],
                None,
                0.0,
            );
        }

        // Rich git side panel — right column scoped to the middle band,
        // mirrors the desktop fork's late paint so it sits above the
        // content column it just carved space from.
        if self.git_diff_panel.is_visible() {
            if let Some(viewport) = self.last_viewport {
                let ide_theme = self.ide_theme;
                self.git_diff_panel.render(
                    sugarloaf,
                    viewport.x + viewport.w,
                    band_top,
                    band_bottom,
                    &ide_theme,
                );
            }
        }

        // Diagnostics popup — anchored to a status line pill. Mirrors
        // the desktop layering in `desktop/src/host/run.rs` (~:1425):
        // painted after the side panels / modal surfaces so a click on
        // its pill can never open a popup that ducks behind them.
        // `render` ticks the popover fade itself and early-returns
        // while closed, so the unconditional call matches desktop.
        // Web works in CSS px (no separate physical scale), so the
        // scale_factor argument — which the shared render ignores in
        // favor of its own `set_scale` chrome scale — is 1.0.
        {
            let ide_theme = self.ide_theme;
            self.diagnostics_popup
                .render(sugarloaf, window_width, 1.0, &ide_theme);
        }

        // Code-pane LSP overlays: the hover/signature card pinned to
        // its buffer cell, and the LSP status-pill "Server Details"
        // popup. Painted late (with the diagnostics popup) so editor
        // content and side panels never cover them.
        self.draw_code_lsp_overlays(
            sugarloaf,
            window_width,
            layout.status_line.y + layout.status_line.h,
            input_modal_active,
        );

        // TOP BAR LAST PASS — render after every other chrome panel so
        // hit rects and late overlay menu draws use the final tab /
        // breadcrumb geometry for this frame.
        if let Some(rect) = layout.top_bar {
            let ide_theme = self.ide_theme;
            // Reflect which panels are open so the toggle buttons paint
            // in their active accent style.
            let tree_open = self.file_tree.as_ref().is_some_and(|t| t.is_visible());
            let agent_panel_open = self
                .agent_pane
                .as_ref()
                .is_some_and(|p| !p.side_panel().user_hidden());
            self.top_bar.set_panel_open(tree_open);
            self.top_bar.set_right_panel_open(agent_panel_open);
            // The top bar spans the full viewport width and sits above
            // every side panel (the agent side panel now docks in the
            // band below it), so it no longer shrinks to dodge them.
            self.top_bar
                .render(sugarloaf, rect.x, rect.y, rect.w, &ide_theme);
        }

        // FULL-SCREEN CHROME OVERLAYS — settings page + About modal —
        // paint after every other panel, through sugarloaf's late
        // overlay pass, so no earlier text can bleed through (the
        // same layering desktop gives its settings overlay + modal).
        self.draw_chrome_overlays(sugarloaf);
        // Connection notifications must remain readable above the blocking
        // late-overlay gate (ordinary modals intentionally cover toasts).
        if self.connection_gate_active() {
            sugarloaf.set_late_overlay_mode(true);
            self.notifications.draw(
                sugarloaf,
                &PanelLayout {
                    bounds: crate::layout::Rect {
                        x: layout.buffer_tabs.x,
                        y: layout.terminal.y,
                        w: layout.buffer_tabs.w,
                        h: layout.terminal.h,
                    },
                    scale: 1.0,
                },
                &ctx,
            );
            sugarloaf.set_late_overlay_mode(false);
        }

        // Reusable file browser: a late material pass prevents text/image
        // bleed, while its translucent backdrop leaves the themed workspace
        // visibly present beneath the modal.
        if self.file_browser.is_active() {
            if let Some(viewport) = self.last_viewport {
                sugarloaf.set_late_overlay_mode(true);
                self.file_browser.set_font_scale(self.chrome_scale);
                self.file_browser.render(
                    sugarloaf,
                    [viewport.x, viewport.y, viewport.w, viewport.h],
                    &self.ide_theme,
                );
                sugarloaf.set_late_overlay_mode(false);
            }
        }

        // "Share with phone" QR — genuinely last, above even the chrome
        // overlays, because it is a modal the user is pointing a camera at.
        if self.share_sheet.is_visible() {
            let viewport = [
                0.0,
                0.0,
                layout.terminal.x + layout.terminal.w,
                layout.status_line.y + layout.status_line.h,
            ];
            let ide_theme = self.ide_theme;
            // MUST go through the late-overlay pass, same as the settings
            // page and the About modal. Text is emitted in its own earlier
            // pass, so a plain rect drawn here does NOT cover it — the
            // sheet looked transparent with the timeline showing through.
            sugarloaf.set_late_overlay_mode(true);
            self.share_sheet
                .render(sugarloaf, viewport, &ide_theme, self.chrome_scale);
            sugarloaf.set_late_overlay_mode(false);
        }
    }

    /// Render every UNFOCUSED visible pane's surface while the grid is
    /// split. Editor-like panes resolve through the host-pushed
    /// [`crate::chrome::PaneSurfaceInfo`] descriptors onto the parked
    /// pane maps (cursor/undo/edits intact) — or onto the live hosted
    /// slot when the focused pane doesn't claim it. Panes the host
    /// painted itself (live terminal grids, listed via
    /// [`Chrome::set_host_drawn_panes`]) are skipped. Anything else
    /// gets a theme-bg fill with a title label so the split never
    /// shows another pane's bleed-through.
    fn draw_unfocused_pane_surfaces(&mut self, sugarloaf: &mut Sugarloaf) {
        if !self.pane_grid.is_split()
            || self.chrome_overlay_active()
            || self.is_neoism_agent_tab_active()
        {
            return;
        }
        let theme = self.ide_theme;
        let chrome_scale = self.chrome_scale;
        let animation_phase = self.animation_phase;
        let panes: Vec<(u64, crate::layout::Rect, crate::layout::Rect)> = self
            .pane_grid
            .panes()
            .iter()
            .filter(|p| !p.focused)
            .filter_map(|p| {
                p.external_id
                    .map(|id| (id, p.rect, self.pane_content_rect(id).unwrap_or(p.rect)))
            })
            .collect();
        if panes.is_empty() {
            return;
        }
        // Whether the focused pane's surface is currently hosted in the
        // live editor slots — if not (the focused pane is a terminal),
        // an unfocused pane may borrow the live slot for rendering.
        let focused_claims_live_slots = !self.is_terminal_tab_active();
        for (id, rect, content) in panes {
            if self.host_drawn_panes.contains(&id) {
                continue;
            }
            let info = self
                .pane_surfaces
                .iter()
                .find(|s| s.external_id == id)
                .cloned();
            // Base fill so the previous surface can't bleed through.
            sugarloaf.rect(
                None,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                theme.f32(theme.bg),
                0.0,
                1,
            );
            let rect_arr = [content.x, content.y, content.w, content.h];
            let mut rendered = false;
            let mut animating = false;
            if let Some(path) = info.as_ref().and_then(|i| i.path.clone()) {
                if let Some(pane) = self.parked_code_panes.get_mut(&path) {
                    pane.caret_drawn_by_host = true;
                    animating |= crate::editor::code::render::render(
                        sugarloaf,
                        pane,
                        rect_arr,
                        &theme,
                        &[],
                        chrome_scale,
                        None,
                    );
                    rendered = true;
                } else if let Some(pane) = self.parked_notebook_panes.get_mut(&path) {
                    crate::editor::markdown::render::render(
                        sugarloaf,
                        &mut pane.markdown,
                        rect_arr,
                        &theme,
                        None,
                        &[],
                        chrome_scale,
                        animation_phase,
                    );
                    rendered = true;
                } else if let Some(pane) = self.parked_draw_panes.get_mut(&path) {
                    crate::editor::neodraw::render_pane(
                        sugarloaf, pane, rect_arr, &theme,
                    );
                    rendered = true;
                } else if !focused_claims_live_slots {
                    // Focused pane is a terminal — the live hosted slot
                    // (if it matches this pane's file) is free to paint
                    // here so switching focus doesn't blank the editor.
                    if self
                        .code_pane
                        .as_ref()
                        .is_some_and(|pane| pane.path == path)
                    {
                        let pane = self.code_pane.as_mut().expect("checked above");
                        pane.caret_drawn_by_host = true;
                        animating |= crate::editor::code::render::render(
                            sugarloaf,
                            pane,
                            rect_arr,
                            &theme,
                            &[],
                            chrome_scale,
                            None,
                        );
                        rendered = true;
                    } else if self
                        .notebook_pane
                        .as_ref()
                        .is_some_and(|pane| pane.path == path)
                    {
                        let pane = self.notebook_pane.as_mut().expect("checked above");
                        crate::editor::markdown::render::render(
                            sugarloaf,
                            &mut pane.markdown,
                            rect_arr,
                            &theme,
                            None,
                            &[],
                            chrome_scale,
                            animation_phase,
                        );
                        rendered = true;
                    } else if self
                        .draw_pane
                        .as_ref()
                        .is_some_and(|pane| pane.path == path)
                    {
                        let pane = self.draw_pane.as_mut().expect("checked above");
                        crate::editor::neodraw::render_pane(
                            sugarloaf, pane, rect_arr, &theme,
                        );
                        rendered = true;
                    } else if self
                        .markdown_pane
                        .as_ref()
                        .is_some_and(|pane| pane.path == path)
                    {
                        let pane = self.markdown_pane.as_mut().expect("checked above");
                        crate::editor::markdown::render::render(
                            sugarloaf,
                            pane,
                            rect_arr,
                            &theme,
                            None,
                            &[],
                            chrome_scale,
                            animation_phase,
                        );
                        rendered = true;
                    }
                }
            }
            self.editor_pane_animating |= animating;
            if !rendered {
                // Labeled placeholder: pane title (or file name /
                // surface kind) in the pane's top-left corner.
                let title = info
                    .as_ref()
                    .and_then(|i| i.title.clone())
                    .or_else(|| {
                        info.as_ref().and_then(|i| {
                            i.path.as_ref().and_then(|p| {
                                p.file_name().map(|n| n.to_string_lossy().into_owned())
                            })
                        })
                    })
                    .or_else(|| info.as_ref().map(|i| i.kind.clone()))
                    .unwrap_or_else(|| "pane".to_string());
                let opts = sugarloaf::text::DrawOpts {
                    font_size: 12.0 * chrome_scale.max(0.5),
                    color: theme.u8(theme.muted),
                    clip_rect: Some(rect_arr),
                    ..sugarloaf::text::DrawOpts::default()
                };
                sugarloaf.text_mut().draw(
                    content.x + 12.0,
                    content.y + 10.0 + 12.0 * chrome_scale.max(0.5) * 0.75,
                    &title,
                    &opts,
                );
            }
        }
    }

    /// Paint each stacked pane's local tab strip + breadcrumbs row in
    /// the rects `ChromeLayout::panes` reserved. Top-aligned panes
    /// carry no local strip (the workspace strip serves them), so
    /// their entries have `tabs: None` and are skipped here.
    fn draw_pane_tab_strips(&mut self, sugarloaf: &mut Sugarloaf) {
        if !self.pane_grid.is_split()
            || self.chrome_overlay_active()
            || self.is_neoism_agent_tab_active()
        {
            return;
        }
        let theme = self.ide_theme;
        let pane_layouts = self.layout.panes.clone();
        for pane in pane_layouts {
            if let Some(tabs_rect) = pane.tabs {
                if let Some(strip) = self.pane_tabs.get_mut(&pane.external_id) {
                    strip.render(
                        sugarloaf,
                        tabs_rect.x,
                        tabs_rect.y,
                        tabs_rect.w,
                        &theme,
                        None,
                        &[],
                    );
                }
            }
            if let Some(crumbs_rect) = pane.breadcrumbs {
                if let Some(crumbs) = self.pane_breadcrumbs.get(&pane.external_id) {
                    crumbs.render(
                        sugarloaf,
                        crumbs_rect.x,
                        crumbs_rect.y,
                        crumbs_rect.w,
                        &theme,
                    );
                }
            }
        }
    }

    /// Paint the shared pane-grid chrome: one thin divider line per
    /// resizable gap (accent-highlighted under the pointer / while
    /// dragging), a subtle outline around the focused pane while
    /// split, and the translucent drop-zone preview during a
    /// drag-to-split. All geometry comes straight from the solved
    /// [`crate::panels::pane_grid::PaneGrid`] so it can never drift
    /// from the hit-test rects the pointer path uses.
    fn draw_pane_grid_overlay(&mut self, sugarloaf: &mut Sugarloaf) {
        if self.chrome_overlay_active() || self.is_neoism_agent_tab_active() {
            return;
        }
        let theme = self.ide_theme;
        let (px, py) = self.last_pointer_pos;
        let accent = theme.f32(theme.accent);

        if self.pane_grid.is_split() {
            let active_divider = self.pane_grid.active_divider_rect();
            let divider_dragging = self.pane_grid.is_divider_dragging();
            let dividers: Vec<_> = self.pane_grid.solved().dividers.to_vec();
            for div in &dividers {
                let r = div.rect;
                let is_active = active_divider.map_or(false, |a| a == r)
                    || (!divider_dragging && r.contains(px, py));
                let horizontal =
                    matches!(div.axis, crate::session_layout::SplitAxis::Horizontal);
                let (line_w, color) = if is_active {
                    (3.0, accent)
                } else {
                    (1.0, theme.f32(theme.border))
                };
                let (x, y, w, h) = if horizontal {
                    // Horizontal split → left/right panes → vertical band.
                    (r.x + (r.w - line_w) * 0.5, r.y, line_w, r.h)
                } else {
                    (r.x, r.y + (r.h - line_w) * 0.5, r.w, line_w)
                };
                sugarloaf.rect(None, x, y, w, h, color, 0.0, 1);
            }

            // Focused-pane outline (desktop's active-pane cue).
            if let Some(pane) = self.pane_grid.panes().iter().find(|p| p.focused) {
                let r = pane.rect;
                let edge = [accent[0], accent[1], accent[2], 0.55];
                let bw = 1.0;
                sugarloaf.rect(None, r.x, r.y, r.w, bw, edge, 0.0, 1);
                sugarloaf.rect(None, r.x, r.y + r.h - bw, r.w, bw, edge, 0.0, 1);
                sugarloaf.rect(None, r.x, r.y, bw, r.h, edge, 0.0, 1);
                sugarloaf.rect(None, r.x + r.w - bw, r.y, bw, r.h, edge, 0.0, 1);
            }
        }

        // Live drag-to-split preview: the region the dragged surface
        // would occupy if released now.
        if let Some(zone) = self.pane_grid.hover_drop_zone() {
            let hl = zone.highlight;
            let fill = [accent[0], accent[1], accent[2], 0.16];
            sugarloaf.rect(None, hl.x, hl.y, hl.w, hl.h, fill, 0.0, 2);
            let edge = [accent[0], accent[1], accent[2], 0.85];
            let bw = 2.0;
            sugarloaf.rect(None, hl.x, hl.y, hl.w, bw, edge, 0.0, 2);
            sugarloaf.rect(None, hl.x, hl.y + hl.h - bw, hl.w, bw, edge, 0.0, 2);
            sugarloaf.rect(None, hl.x, hl.y, bw, hl.h, edge, 0.0, 2);
            sugarloaf.rect(None, hl.x + hl.w - bw, hl.y, bw, hl.h, edge, 0.0, 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::FileTree;
    use std::path::PathBuf;

    #[test]
    fn web_status_line_stays_full_width_when_file_tree_is_open() {
        let viewport = Rect::new(0.0, 0.0, 1024.0, 768.0);
        let mut chrome = Chrome::<()>::new();
        let mut tree = FileTree::new(PathBuf::from("/workspace"));
        tree.set_visible(true);
        chrome.install_file_tree(tree);
        chrome.set_layout(viewport);

        let layout = chrome.layout();
        assert!(
            layout.file_tree.is_some(),
            "test must exercise an open tree"
        );
        assert!(layout.terminal.x > viewport.x, "tree must reflow content");

        let (background, content) = status_line_render_geometry(layout);
        assert_eq!(background.x, viewport.x);
        assert_eq!(background.w, viewport.w);
        assert_eq!(content.x, viewport.x);
        assert_eq!(content.w, viewport.w);
    }
}

// ---------------------------------------------------------------------
// Code-pane LSP hosting (shared `editor::code::lsp_session` layer):
// desktop drives the same session shapes from `host/run.rs`; on web the
// chrome pumps the session per frame and feeds/paints its popups here.
// ---------------------------------------------------------------------
impl<A: Send + Copy + 'static> Chrome<A> {
    /// Whether the hosted CODE pane is the active content surface.
    fn code_lsp_pane_active(&self) -> bool {
        !self.is_terminal_tab_active()
            && !self.is_neoism_agent_tab_active()
            && self.active_editor_pane_kind() == Some(EditorPaneKind::Code)
    }

    /// Per-frame session pump + completion/action menu feed. Runs just
    /// before the completion-menu shim paints so the stored popup is
    /// this frame's snapshot.
    fn pump_code_lsp_layer(&mut self, input_modal_active: bool) {
        if !self.code_lsp_pane_active() || self.code_pane.is_none() {
            if self.code_lsp.has_session_state() {
                self.code_lsp.clear_sessions();
            }
            if self.code_lsp.owns_menu_popup {
                self.completion_menu.set_popup(None);
                self.code_lsp.owns_menu_popup = false;
            }
            return;
        }
        let pointer = self.last_pointer_pos;
        if let Some(pane) = self.code_pane.as_mut() {
            self.code_lsp.note_mouse_move(pane, Some(pointer));
            self.code_lsp.pump(pane);
        }

        // Feed the shared completion-menu panel: the code-action menu
        // takes precedence over completion (desktop host parity — at
        // most one is open; opening one dismisses the other).
        let popup_and_anchor = if input_modal_active {
            None
        } else {
            let pane = self.code_pane.as_ref();
            let actions = self.code_lsp.actions.as_ref().and_then(|session| {
                let pane = pane?;
                Self::code_lsp_menu_anchor(
                    pane,
                    &session.path,
                    session.line,
                    session.col,
                    &session.display,
                )
            });
            let completion = self.code_lsp.completion.as_ref().and_then(|session| {
                let pane = pane?;
                Self::code_lsp_menu_anchor(
                    pane,
                    &session.path,
                    session.line,
                    session.anchor_col,
                    &session.display,
                )
            });
            actions.or(completion)
        };
        match popup_and_anchor {
            Some((popup, anchor)) => {
                self.completion_menu.set_anchor(anchor);
                self.completion_menu.set_popup(Some(popup));
                self.code_lsp.owns_menu_popup = true;
            }
            None => {
                if self.code_lsp.owns_menu_popup {
                    self.completion_menu.set_popup(None);
                    self.code_lsp.owns_menu_popup = false;
                }
            }
        }
    }

    /// Build the wrap-aware popup anchor for a session pinned at
    /// `(line, col)` on the hosted code pane. Port of the desktop
    /// anchor build in `desktop/src/host/run.rs` (web works in CSS px,
    /// so scale_factor is 1). Returns `None` while the session has no
    /// visible rows yet or the pane moved to another file.
    fn code_lsp_menu_anchor(
        pane: &crate::editor::code::CodePane,
        session_path: &std::path::Path,
        line: usize,
        col: usize,
        display: &crate::editor_snapshot::PopupMenu,
    ) -> Option<(
        crate::editor_snapshot::PopupMenu,
        crate::panels::completion_menu::EditorAnchor,
    )> {
        if pane.path != session_path || display.items.is_empty() {
            return None;
        }
        let geometry = &pane.geometry;
        if geometry.cell_w <= 0.0 || geometry.row_h <= 0.0 {
            return None;
        }
        let line_text = pane.buffer.lines.get(line)?;
        let (segment, col_cells) = geometry.wrap.visual_position(
            line,
            line_text,
            col,
            crate::editor::code::layout::TAB_DISPLAY_WIDTH,
        );
        let visual_row = geometry.wrap.first_row_of_line(line) + segment;
        let anchor_x =
            geometry.text_x + col_cells as f32 * geometry.cell_w - geometry.scroll_x;
        let anchor_y =
            geometry.rect[1] + visual_row as f32 * geometry.row_h - geometry.scroll_y;
        let pane_bottom = geometry.rect[1] + geometry.rect[3];
        let lines_below =
            (((pane_bottom - anchor_y) / geometry.row_h).floor()).max(1.0) as u32;
        Some((
            display.clone(),
            crate::panels::completion_menu::EditorAnchor {
                cell_w: geometry.cell_w,
                cell_h: geometry.row_h,
                panel_left_phys: anchor_x,
                panel_top_phys: anchor_y,
                panel_lines: lines_below,
                editor_focused: true,
            },
        ))
    }

    /// Late-pass code LSP overlays: the hover/signature card pinned to
    /// its buffer cell (desktop `host/run.rs` code_hover port) and the
    /// LSP status-pill "Server Details" popup.
    fn draw_code_lsp_overlays(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        window_width: f32,
        window_bottom: f32,
        input_modal_active: bool,
    ) {
        let ide_theme = self.ide_theme;
        // The pill popup ticks its own fade and early-returns while
        // closed, so the unconditional call matches the desktop layer.
        self.lsp_popup
            .render(sugarloaf, &ide_theme, self.chrome_scale);

        if input_modal_active || !self.code_lsp_pane_active() {
            return;
        }
        // The completion / action menu wins the surface (desktop
        // suppresses the hover card while a popup is up).
        if self.code_lsp.owns_menu_popup {
            return;
        }
        let Some(pane) = self.code_pane.as_ref() else {
            return;
        };
        let Some(card) = self.code_lsp.hover.as_ref() else {
            return;
        };
        if card.lines.is_empty() || card.path != pane.path {
            return;
        }
        // Keyboard cards pin to the cursor; mouse cards pin to the
        // hovered cell (dismissed by pointer-cell change instead).
        let cursor = pane.buffer.cursor();
        if !card.from_mouse && (cursor.line != card.line || cursor.col != card.col) {
            return;
        }
        let geometry = &pane.geometry;
        if geometry.cell_w <= 0.0 || geometry.row_h <= 0.0 {
            return;
        }
        let Some(line_text) = pane.buffer.lines.get(card.line) else {
            return;
        };
        // Wrap-aware anchor: the card position maps through the wrap
        // index to a VISUAL row + column-within-segment (identity when
        // wrap is off, honoring scroll_x).
        let (segment, local_col) = geometry.wrap.visual_position(
            card.line,
            line_text,
            card.col,
            crate::editor::code::layout::TAB_DISPLAY_WIDTH,
        );
        let visual_row = geometry.wrap.first_row_of_line(card.line) + segment;
        let anchor_x =
            geometry.text_x + local_col as f32 * geometry.cell_w - geometry.scroll_x;
        let anchor_y =
            geometry.rect[1] + visual_row as f32 * geometry.row_h - geometry.scroll_y;
        crate::panels::hover_popup::render(
            sugarloaf,
            &card.lines,
            crate::panels::hover_popup::HoverPopupLayout {
                anchor_x,
                anchor_y,
                cell_h: geometry.row_h,
                window_w: window_width,
                window_h: window_bottom,
                scale: self.chrome_scale,
            },
            &ide_theme,
        );
    }
}
