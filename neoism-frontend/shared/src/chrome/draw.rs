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

        // 1. Background strips: buffer tabs (top), status line (bottom).
        let layout = self.layout.clone();
        let input_modal_active = self.command_palette.is_enabled()
            || self.finder.is_enabled()
            || self.git_diff.is_visible()
            || self.context_menu.is_visible();
        if input_modal_active {
            self.buffer_tabs.clear_hover_immediate();
            self.buffer_tabs.set_focused(false);
            self.blur(PanelKey::BufferTabs);
        }
        self.buffer_tabs.draw(
            sugarloaf,
            &PanelLayout {
                bounds: layout.buffer_tabs,
                scale: 1.0,
            },
            &ctx,
        );

        // Window-top chrome strip is rendered at the very end of this
        // function (search "TOP BAR LAST PASS") so its dropdown's
        // block-glyph fill emits AFTER every other panel's text and
        // properly overlays labels from the file tree, buffer tabs,
        // breadcrumbs, etc. Painting it here would let later panel
        // text bleed through the open menu.
        let status_palette = status_palette_from_theme(&theme);
        self.status_line.render(
            sugarloaf,
            layout.status_line.x,
            layout.status_line.y,
            layout.status_line.w,
            &status_palette,
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
        if terminal_rect.w > 0.0 && terminal_rect.h > 0.0 {
            if self.is_terminal_tab_active() {
                let wants_splash = !self.terminal_splash_dismissed;
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
                        &[],
                    );
                } else {
                    SplashOverlay::clear_image_overlays(sugarloaf);
                    agent_pane_view::clear_overlays(sugarloaf);
                    self.splash_overlay.reset();
                }
            } else if self.is_neoism_agent_tab_active() {
                SplashOverlay::clear_image_overlays(sugarloaf);
                self.splash_overlay.reset();
                if let Some(pane) = self.agent_pane.as_mut() {
                    agent_pane_view::render(
                        sugarloaf,
                        pane,
                        [
                            terminal_rect.x,
                            terminal_rect.y,
                            terminal_rect.w,
                            terminal_rect.h,
                        ],
                        &self.ide_theme,
                        true,
                        time.as_secs_f32(),
                        Some(self.last_pointer_pos),
                        self.chrome_scale,
                        // Stop the side panel at the TOP of the full-width
                        // status bar (the band bottom) so it doesn't paint
                        // over it — matches the tree / notes / git.
                        Some(layout.status_line.y),
                        // Start the side panel at the same band top as
                        // the file tree (below the full-width top bar +
                        // workspace strip), so the two line up. The tree
                        // sits at the buffer-tabs row.
                        Some(layout.buffer_tabs.y),
                        &[],
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
                sugarloaf.rect(
                    None,
                    terminal_rect.x,
                    terminal_rect.y,
                    terminal_rect.w,
                    terminal_rect.h,
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

                if self.tab_lang == crate::syntax::Lang::Markdown {
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
                        pane.scroll_cursor_into_view(terminal_rect.y, terminal_rect.h);
                        pane.tick_scroll();
                        let chrome_scale = self.chrome_scale;
                        crate::editor::markdown::render::render(
                            sugarloaf,
                            pane,
                            [
                                terminal_rect.x,
                                terminal_rect.y,
                                terminal_rect.w,
                                terminal_rect.h,
                            ],
                            &theme,
                            None,
                            &[],
                            chrome_scale,
                            self.animation_phase,
                        );
                    }
                } else if let Some(text) = self.tab_content.as_deref() {
                    let line_h = self.cell_h.max(14.0);
                    let pad_x = 16.0_f32;
                    let pad_y = 12.0_f32;
                    let max_w = terminal_rect.w - pad_x * 2.0;
                    let max_h = terminal_rect.h - pad_y * 2.0;
                    let opts = sugarloaf::text::DrawOpts {
                        font_size: 13.0,
                        color: theme.u8(theme.fg),
                        clip_rect: Some([
                            terminal_rect.x + pad_x,
                            terminal_rect.y + pad_y,
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
                            terminal_rect.y + pad_y + line_h * (i as f32) + line_h * 0.75
                                - effective_offset;
                        if y < terminal_rect.y - line_h
                            || y > terminal_rect.y + terminal_rect.h
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
                        let mut x_cursor = terminal_rect.x + pad_x;
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

        // 4. Sticky composer above the status line (still under modals).
        if let Some(rect) = layout.command_composer {
            let theme = self.ide_theme;
            let neutral = crate::panels::command_composer::InputClassification::neutral(
                theme.u8(theme.fg),
            );
            let trail_cursor_will_paint =
                self.is_terminal_tab_active() && self.command_composer.is_visible();
            let _ = self.command_composer.render(
                sugarloaf,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                &theme,
                &self.terminal_input,
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
        //     minimap rail, yank flash, cursor surfaces. These paint
        //     over the terminal column / tab bar through the standard
        //     `Panel`-shaped `draw` adapter shims in
        //     `panels::chrome_shim_more`. Data-driven panels (toasts
        //     without queue, popup without snapshot, minimap without
        //     route subscription) early-return without painting.
        if let Some(rect) = layout.breadcrumbs {
            self.breadcrumbs.render_with_options(
                sugarloaf,
                rect.x,
                rect.y,
                rect.w,
                &self.ide_theme,
                !input_modal_active,
            );
        }
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
        self.search_overlay.draw(
            sugarloaf,
            &PanelLayout {
                bounds: layout.terminal,
                scale: 1.0,
            },
            &ctx,
        );
        self.minimap.draw(
            sugarloaf,
            &PanelLayout {
                bounds: layout.terminal,
                scale: 1.0,
            },
            &ctx,
        );
        self.yank_flash.draw(
            sugarloaf,
            &PanelLayout {
                bounds: layout.terminal,
                scale: 1.0,
            },
            &ctx,
        );
        self.completion_menu.draw(
            sugarloaf,
            &PanelLayout {
                bounds: layout.terminal,
                scale: 1.0,
            },
            &ctx,
        );
        if self.context_menu.is_visible() {
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
        let agent_side_panel_focused = agent_tab_active
            && self
                .agent_pane
                .as_ref()
                .is_some_and(|pane| pane.side_panel().is_focused());
        let agent_input_cursor_available = agent_tab_active
            && self
                .agent_pane
                .as_ref()
                .and_then(|pane| pane.cursor_rect())
                .is_some();
        let markdown_cursor_available = !self.is_terminal_tab_active()
            && self.tab_lang == crate::syntax::Lang::Markdown
            && self
                .markdown_pane
                .as_ref()
                .and_then(|pane| pane.cursor_rect)
                .is_some();
        let markdown_active = !self.is_terminal_tab_active()
            && self.tab_lang == crate::syntax::Lang::Markdown;
        let terminal_block_input_active = self.is_terminal_tab_active()
            && self.command_composer.is_visible()
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
            modal_owns_editor_focus: false,
            agent_input_cursor_available,
            markdown_cursor_available,
            // Web has no native code pane yet; the desktop feeds this.
            code_cursor_available: false,
            terminal_block_input_active,
            trail_cursor_enabled: !markdown_active,
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
                    self.trail_cursor.snap_to_destination(w, h);
                    self.trail_cursor.draw_always(sugarloaf, 1.0, cursor_color);
                }
            }
            Some(TrailCursorOverlayTarget::Markdown) => {
                if let Some(pane) = self.markdown_pane.as_ref() {
                    if let Some(rect) = pane.cursor_rect {
                        self.draw_content_trail_cursor_rect(
                            sugarloaf,
                            rect,
                            pane.cursor_shape(),
                            dt,
                            cursor_color,
                        );
                    }
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
        let band_h = (band_bottom - band_top).max(0.0);

        // Notes sidebar — left column right of the file tree, scoped to
        // the middle band.
        if self.notes_sidebar.is_visible() {
            if let Some(viewport) = self.last_viewport {
                let ide_theme = self.ide_theme;
                let x_left = layout.file_tree.map(|ft| ft.x + ft.w).unwrap_or(viewport.x);
                let width = self.notes_sidebar.width().min(viewport.w * 0.8);
                self.notes_sidebar.render(
                    sugarloaf,
                    x_left,
                    band_top,
                    width,
                    band_h,
                    &ide_theme,
                    &[],
                    None,
                    0.0,
                );
            }
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
    }
}
