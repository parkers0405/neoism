use super::*;

impl Renderer {
    /// Scale factor applied to all chrome rows. `1.0` matches the
    /// base constants when the user's config font.size = 14pt.
    /// Callers (e.g. `change_font_size`) should use `set_chrome_scale`
    /// to mutate it so submodules stay in sync.
    pub fn chrome_scale(&self) -> f32 {
        self.chrome_scale
    }

    /// Window-wide live terminal/editor font size.
    pub fn zoom_font_size(&self) -> f32 {
        self.zoom_font_size
    }

    /// Update the canonical text zoom and its proportional chrome scale.
    pub fn set_zoom_font_size(&mut self, font_size: f32) {
        let font_size = if font_size.is_finite() {
            font_size.clamp(6.0, 100.0)
        } else {
            CHROME_BASELINE_FONT_SIZE
        };
        self.zoom_font_size = font_size;
        self.set_chrome_scale(font_size / CHROME_BASELINE_FONT_SIZE);
    }

    pub fn set_ide_theme(&mut self, theme: IdeTheme) {
        // Keep the process-wide cell in sync — shared code without a
        // theme reference (chrome shims, the agent wordmark tint)
        // reads it via `active_ide_theme()`.
        neoism_ui::chrome::publish_active_ide_theme(theme);
        let previous_alpha = self.dynamic_background.1.a;
        let mut window_bg = theme.sugar(theme.bg);
        if self.dynamic_background.2 && previous_alpha < 1.0 {
            window_bg.a = previous_alpha;
        }

        self.theme = theme;
        self.dynamic_background.0 = theme.f32(theme.bg);
        self.dynamic_background.1 = window_bg;
        self.named_colors.background = (theme.f32(theme.bg), window_bg);
        self.named_colors.foreground = theme.f32(theme.fg);
        self.named_colors.black = theme.f32(theme.bg);
        self.named_colors.red = theme.f32(theme.red);
        self.named_colors.green = theme.f32(theme.green);
        self.named_colors.yellow = theme.f32(theme.yellow);
        self.named_colors.blue = theme.f32(theme.blue);
        self.named_colors.magenta = theme.f32(theme.magenta);
        self.named_colors.cyan = theme.f32(theme.cyan);
        self.named_colors.white = theme.f32(theme.white);
        self.named_colors.light_black = theme.f32(theme.muted);
        self.named_colors.light_red = theme.f32(theme.red);
        self.named_colors.light_green = theme.f32(theme.green);
        self.named_colors.light_yellow = theme.f32(theme.yellow);
        self.named_colors.light_blue = theme.f32(theme.blue);
        self.named_colors.light_magenta = theme.f32(theme.magenta);
        self.named_colors.light_cyan = theme.f32(theme.cyan);
        self.named_colors.light_white = theme.f32(theme.fg);
        self.named_colors.tabs = theme.f32(theme.bg);
        self.named_colors.tabs_active = theme.f32(theme.surface);
        self.named_colors.tab_border = theme.f32(theme.border);
        self.named_colors.bar = theme.f32(theme.bg);
        // User-picked cursor color beats the theme accent and is
        // re-applied here so it survives every theme switch.
        self.named_colors.cursor = self
            .cursor_color_override
            .unwrap_or_else(|| theme.f32(theme.accent));
        self.named_colors.vi_cursor = theme.f32(theme.yellow);
        self.named_colors.selection_background = theme.f32(theme.hover);
        self.named_colors.selection_foreground = theme.f32(theme.fg);
        self.named_colors.split = theme.f32(theme.border);
        self.named_colors.split_active = theme.f32(theme.accent);
        self.named_colors.search_match_background = theme.f32(theme.surface);
        self.named_colors.search_match_foreground = theme.f32(theme.fg);
        self.named_colors.search_focused_match_background = theme.f32(theme.yellow);
        self.named_colors.search_focused_match_foreground = theme.f32(theme.black);
        self.named_colors.hint_background = theme.f32(theme.yellow);
        self.named_colors.hint_foreground = theme.f32(theme.black);
        self.colors.fill_named(&self.named_colors);

        if let Some(island) = &mut self.island {
            island.update_colors(
                theme.f32(theme.muted),
                theme.f32(theme.fg),
                theme.f32(theme.border),
            );
            island.progress_bar_color = theme.f32(theme.blue);
            island.progress_bar_error_color = theme.f32(theme.red);
        }
    }

    /// Push a new scale to every chrome submodule. Clamping happens
    /// inside the submodules; we just clamp here so the cached scalar
    /// matches what the submodules ended up with.
    pub fn set_chrome_scale(&mut self, scale: f32) {
        let clamped = scale.clamp(0.5, 3.0);
        self.chrome_scale = clamped;
        self.file_tree.set_scale(clamped);
        self.notes_sidebar.set_scale(clamped);
        self.buffer_tabs.set_scale(clamped);
        self.top_bar.set_scale(clamped);
        if let Some(island) = self.island.as_mut() {
            island.set_scale(clamped);
        }
        for tabs in self.pane_tabs.values_mut() {
            tabs.set_scale(clamped);
        }
        for crumbs in self.pane_breadcrumbs.values_mut() {
            crumbs.set_scale(clamped);
        }
        self.breadcrumbs.set_scale(clamped);
        self.notifications.set_scale(clamped);
        self.finder.set_scale(clamped);
        self.command_palette.set_scale(clamped);
        self.context_menu.set_scale(clamped);
        self.completion_menu.set_scale(clamped);
        self.status_line.set_scale(clamped);
        self.command_composer.set_scale(clamped);
        self.modal.set_scale(clamped);
        self.git_diff_panel.set_scale(clamped);
        self.diagnostics_popup.set_scale(clamped);
        self.minimap.set_scale(clamped);
    }

    #[inline]
    pub fn set_active_search(&mut self, active_search: Option<String>) {
        self.search.set_active_search(active_search);
    }

    #[inline]
    pub fn set_vi_mode(&mut self, is_vi_mode_enabled: bool) {
        self.is_vi_mode_enabled = is_vi_mode_enabled;
    }

    // Get the RGB value for a color index.
    #[inline]
    pub fn color(&self, color: usize, term_colors: &TermColors) -> ColorArray {
        term_colors[color].unwrap_or(self.colors[color])
    }

    /// The color the LOCAL cursor wears this frame: the rainbow preset
    /// animates on the shared clock; solid reads `named_colors.cursor`
    /// (theme accent or the user's `cursor-color` override, applied in
    /// `set_ide_theme`).
    #[inline]
    pub fn live_cursor_color(&self) -> [f32; 4] {
        if self.cursor_style.is_animated() {
            neoism_ui::cursor_style::rainbow_color_f32(
                neoism_ui::cursor_style::rainbow_now_seconds(),
            )
        } else {
            self.named_colors.cursor
        }
    }

    /// True when the local cursor preset is animated (rainbow).
    #[inline]
    pub fn cursor_is_animated(&self) -> bool {
        self.cursor_style.is_animated()
    }

    /// Returns the first component that needs continuous redraw.
    #[inline]
    pub fn redraw_reason(&mut self) -> Option<&'static str> {
        // Agent pane keeps the loop spinning while the model is streaming
        // or the status row is mid-animation — without this, the event
        // loop sleeps between SSE events and the timer / dots / scramble
        // visibly freeze for hundreds of ms at a time.
        if self.neoism_agent_animating {
            return Some("neoism_agent");
        }
        // Rainbow cursors (local preset or a remote peer's broadcast
        // flag) derive their color from the clock — keep frames coming
        // while one is on screen or the animation freezes between
        // input events.
        if self.cursor_style.is_animated() || self.remote_rainbow_active {
            return Some("rainbow_cursor");
        }
        if self.trail_cursor_enabled && self.trail_cursor.is_animating() {
            return Some("trail_cursor");
        }
        if self.terminal_block_prompt_animating {
            return Some("terminal_block_prompt");
        }
        // Notebook execution redraws are event-driven by output/result
        // messages, with a bounded status tick scheduled by the app.
        // Treating every running cell as a vblank animation makes Run All
        // spin the full UI loop for status text alone.
        // Pixel-scroll spring drives smooth slide for nvim editor panes.
        // Without this, the OS only wakes us on the wheel/key event
        // itself — render() ticks the spring once, then the loop sleeps
        // until the next input. Result: scroll feels snap-to-row even
        // though the spring math is correct. Returning true here while
        // the spring is mid-flight makes the event loop keep calling
        // render() until the spring settles, yielding the actual slide.
        if self.editor_scroll.is_animating() {
            return Some("editor_scroll");
        }
        if self.scrollbar.needs_redraw() {
            return Some("scrollbar");
        }
        if self.buffer_tabs.is_animating() {
            return Some("buffer_tabs");
        }
        if self.pane_tabs.values().any(|tabs| tabs.is_animating()) {
            return Some("pane_tabs");
        }
        if self.file_tree.is_animating() {
            return Some("file_tree");
        }
        if self.notes_sidebar.is_animating() {
            return Some("notes_sidebar");
        }
        if self.command_palette.is_enabled() || self.command_palette.is_animating() {
            return Some("command_palette");
        }
        if self.completion_menu.is_animating() {
            return Some("completion_menu");
        }
        if self.notifications.is_active() {
            return Some("notifications");
        }
        if !self.install_tracker.in_flight.is_empty() {
            return Some("extension_install");
        }
        if self.modal.needs_redraw() {
            return Some("modal");
        }
        if self.git_diff_panel.needs_redraw() {
            return Some("git_diff_panel");
        }
        if self.status_line.is_animating() {
            return Some("status_line");
        }
        if self.diagnostics_popup.is_animating() {
            return Some("diagnostics_popup");
        }
        if self.yank_flash.is_animating() {
            return Some("yank_flash");
        }
        // Finder needs continuous redraw so the caret blinks and the
        // debounced grep refresh fires off the per-frame `tick()`.
        if self.finder.is_enabled() {
            return Some("finder");
        }
        if self
            .island
            .as_ref()
            .is_some_and(|island| island.needs_redraw())
        {
            return Some("island");
        }
        // Animated shader overlays sample the wall clock per drawn
        // frame; without an owner here an idle terminal pane only
        // redraws on PTY damage / cursor blink and the overlay's time
        // uniform jumps forward by the whole gap between frames.
        if self.shader_overlay_active {
            return Some("shader_overlay");
        }
        None
    }
}
