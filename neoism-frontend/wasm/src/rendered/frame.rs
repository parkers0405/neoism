use super::*;
use neoism_ui::layout::Rect as ChromeRect;
use neoism_ui::services::Services;
use neoism_ui::PanelKey;
use web_time::Duration;

#[wasm_bindgen]
impl ChromeBridge {
    // -------- event ingestion ------------------------------------

    /// Feed a JSON-encoded `UiEvent` from the TS event translator.
    /// Errors are returned as a `JsValue` carrying the parse error
    /// message so the host can log and recover.
    pub fn handle_event(&mut self, event_json: &str) -> Result<(), JsValue> {
        let event: neoism_ui::event::UiEvent = serde_json::from_str(event_json)
            .map_err(|e| JsValue::from_str(&format!("UiEvent parse: {e}")))?;

        if is_neoism_agent_shortcut(&event) {
            self.queue_agent_tab_open();
            return Ok(());
        }

        let palette_enter_action = palette_enter_action(&self.chrome, &event);

        // Capture finder + palette picks BEFORE dispatch — the
        // chrome_shim handlers close the panel on Enter and clear
        // the selection, so reading state after dispatch would lose
        // the pick. Mirrors how `palette_enter_action` is captured
        // above for the OpenNeoismAgent post-dispatch hook.
        if is_enter_press(&event) {
            if self.chrome.finder.is_enabled() {
                self.pick_finder_selection();
            } else if self.chrome.command_palette.is_enabled() {
                self.pick_palette_action();
            }
        }

        // Resize events flow through us into a re-layout: chrome
        // panels read their bounds from `chrome.set_layout`, so
        // we have to push the new viewport before dispatching the
        // event further (panels themselves still see `Resize`).
        if let neoism_ui::event::UiEvent::Resize { w, h, .. } = &event {
            self.viewport = ChromeRect::new(0.0, 0.0, *w as f32, *h as f32);
            self.relayout_chrome();
        }

        let time = Duration::from_micros(
            (self.services_state.0.borrow().now_ms * 1000.0).max(0.0) as u64,
        );
        let services = Services {
            files: &*self.files,
            clipboard: &*self.clipboard,
            commands: &*self.commands,
            git: &*self.git,
            clock: &*self.clock,
            search: &*self.search,
            notifications: &*self.notifications,
        };
        self.chrome.handle_event(&event, services, time);
        let _ = self.drain_agent_outbound();
        if matches!(
            palette_enter_action,
            Some(neoism_ui::panels::command_palette::PaletteAction::OpenNeoismAgent)
        ) {
            self.queue_agent_tab_open();
        }
        // Modal visibility may have flipped (e.g. Cmd+P opened
        // the palette); re-layout so the next render and the
        // next event see the new rect set.
        self.relayout_chrome();
        Ok(())
    }

    /// Re-dispatch a previously-Pending service request as a
    /// `UiEvent::ServiceReply`. `payload_json` is parsed as a
    /// `serde_json::Value` and handed verbatim to the panels.
    pub fn service_reply(
        &mut self,
        request_id: u64,
        payload_json: &str,
    ) -> Result<(), JsValue> {
        let payload: serde_json::Value = serde_json::from_str(payload_json)
            .map_err(|e| JsValue::from_str(&format!("payload parse: {e}")))?;
        let event = neoism_ui::event::UiEvent::ServiceReply {
            request_id,
            payload,
        };
        let time = Duration::from_micros(
            (self.services_state.0.borrow().now_ms * 1000.0).max(0.0) as u64,
        );
        let services = Services {
            files: &*self.files,
            clipboard: &*self.clipboard,
            commands: &*self.commands,
            git: &*self.git,
            clock: &*self.clock,
            search: &*self.search,
            notifications: &*self.notifications,
        };
        self.chrome.handle_event(&event, services, time);
        let _ = self.drain_agent_outbound();
        self.relayout_chrome();
        Ok(())
    }

    // -------- rendering ------------------------------------------

    /// Paint one frame: terminal cells, then chrome panels, then
    /// a single `present()` to flip the swapchain. `time_ms` is
    /// `performance.now()` from JS — used by panels for
    /// animations and stored for `ClockService::now_monotonic`.
    pub fn render(&mut self, time_ms: f64) -> Result<(), JsValue> {
        self.services_state.0.borrow_mut().now_ms = time_ms;
        self.chrome
            .set_animation_phase(((time_ms / 1000.0) % 10_000.0) as f32);
        self.sync_terminal_command_composer_visibility();
        // Flip the status-line mode pill to match the focused
        // surface (terminal / file / markdown / agent). On a change
        // `set_info` kicks off the scramble/rainbow transition; the
        // status line's own wasm-safe clock (js_sys::Date) drives it.
        self.chrome.sync_status_mode();

        // Sync the terminal grid size to the current chrome layout.
        // The composer show/hide (triggered above) changes the
        // terminal rect height but does NOT call `resize()` — so
        // when a TUI starts (composer hides) the grid is still sized
        // to the old smaller rect. Re-derive cols/rows here so the
        // grid matches the actual paint area.
        //
        // We use `resize_grid` (grid only, no surface) rather than
        // `resize_grid_and_surface` because the sugarloaf surface
        // size tracks the viewport (window), which hasn't changed.
        // Calling `s.resize()` with unchanged physical pixel dimensions
        // still calls `surface.configure()` unconditionally, which
        // clears the WebGL swap chain mid-frame and makes full-screen
        // TUIs show a black screen on their first rendered frame.
        // `syncTerminalRectDependents` in JS picks up the terminal
        // rect change next frame and calls `resizePty(cols, rows)` to
        // send SIGWINCH to the foreground process.
        {
            let cell_w = 8.0 * self.active_font_scale;
            let cell_h = 16.0 * self.active_font_scale;
            let term_rect = self.chrome.layout().terminal;
            let want_cols = ((term_rect.w / cell_w).floor() as u32).max(1);
            let want_rows = ((term_rect.h / cell_h).floor() as u32).max(1);
            let have_cols = self.rendered.terminal_ref().inner.columns() as u32;
            let have_rows = self.rendered.terminal_ref().inner.screen_lines() as u32;
            if want_cols != have_cols || want_rows != have_rows {
                self.rendered.set_cell_metrics(
                    term_rect.w / want_cols as f32,
                    term_rect.h / want_rows as f32,
                );
                self.rendered.resize_grid(want_cols, want_rows);
            }
        }

        // 1. Terminal cells into sugarloaf (no present).
        let terminal_rect = self.chrome.layout().terminal;
        let chrome_owns_prompt = self.chrome.command_composer.is_visible()
            || self.chrome.command_palette.is_visible()
            || self.chrome.finder.is_visible()
            || self.chrome.git_diff.is_visible();
        if self.chrome.is_terminal_tab_active()
            && !self.chrome.is_neoism_agent_tab_active()
        {
            self.draw_terminal_blocks_or_cells(terminal_rect, chrome_owns_prompt);
        } else if let Some(s) = self.rendered.sugarloaf_mut() {
            let theme = *self.chrome.ide_theme();
            s.set_background_color(Some(theme.sugar(theme.bg)));
        }

        // 2. Shared workspace Island + chrome panels into the *same* sugarloaf.
        let time = Duration::from_micros((time_ms * 1000.0).max(0.0) as u64);
        let services = Services {
            files: &*self.files,
            clipboard: &*self.clipboard,
            commands: &*self.commands,
            git: &*self.git,
            clock: &*self.clock,
            search: &*self.search,
            notifications: &*self.notifications,
        };
        let theme = *self.chrome.ide_theme();
        let island_tabs = self.workspace_island_tabs.clone();
        let contexts = WorkspaceIslandContexts {
            tabs: &island_tabs,
            active_index: self.active_workspace_island_index(),
        };
        let strip_h = self.workspace_island_height();
        if let Some(s) = self.rendered.sugarloaf_mut() {
            if self.chrome.is_terminal_tab_active() && strip_h > 0.0 {
                // Full-width workspace strip background: sits between
                // the top bar and the buffer tabs, spanning the whole
                // viewport width (the side panels live in the band
                // below this top chrome).
                let top = self
                    .chrome
                    .layout()
                    .top_bar
                    .map(|r| r.y + r.h)
                    .unwrap_or(self.viewport.y);
                s.rect(
                    None,
                    self.viewport.x,
                    top,
                    self.viewport.w,
                    strip_h,
                    theme.f32(theme.surface),
                    0.0,
                    2,
                );
            }
            self.workspace_island.render(
                s,
                (self.viewport.w, self.viewport.h, 1.0),
                &contexts,
                &theme,
            );
            self.chrome.draw(s, services, time);
        }

        // 3. Single present.
        self.rendered.present();
        Ok(())
    }

    /// Resize the surface and reflow the chrome layout. `cols` /
    /// `rows` size the terminal grid; `width_px` / `height_px`
    /// size the chrome viewport (these can disagree when the
    /// terminal sits inside a chrome sidebar / strips).
    pub fn resize(
        &mut self,
        _cols: u32,
        _rows: u32,
        scale: f32,
        width_px: u32,
        height_px: u32,
    ) {
        self.viewport = ChromeRect::new(0.0, 0.0, width_px as f32, height_px as f32);
        self.last_dpr_scale = scale;

        // Resolve cell metrics from the user font scale, not from
        // the temporary viewport/grid ratio. The JS side can call
        // `resize` before and after it computes the chrome content
        // rect; deriving metrics from that transient ratio made
        // Ctrl+/- affect mostly nvim cols/rows while chrome panels
        // snapped back to the default scale.
        let cell_w = 8.0 * self.active_font_scale;
        let cell_h = 16.0 * self.active_font_scale;
        self.chrome.set_chrome_scale(self.active_font_scale);
        self.chrome.set_cell_metrics(cell_w, cell_h);
        self.relayout_chrome();

        // Shrink the terminal grid so its cells stay inside the
        // chrome-reserved terminal rect. Tabs eat rows off the top,
        // status line + composer eat rows off the bottom. Without
        // this the grid keeps painting all `rows` worth of cells and
        // the prompt/shell content bleeds into the composer band.
        let term_rect = self.chrome.layout().terminal;
        let term_cols = ((term_rect.w / cell_w).floor() as u32).max(1);
        let term_rows = ((term_rect.h / cell_h).floor() as u32).max(1);
        self.rendered.set_cell_metrics(
            term_rect.w / term_cols.max(1) as f32,
            term_rect.h / term_rows.max(1) as f32,
        );
        self.rendered
            .resize_grid_and_surface(term_cols, term_rows, scale, width_px, height_px);
    }

    /// PTY responses (DSR / OSC) the terminal wants written back.
    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        self.rendered.take_pty_writes()
    }

    /// Install the JS callback that forwards PTY response bytes
    /// (DSR / OSC / cursor pos / OSC-52 clipboard write) to the
    /// daemon. The host passes a function of shape
    /// `(bytesBase64: string) => void`; once installed,
    /// `feed_pty_output` auto-flushes pending PTY writes through
    /// this callback so JS hosts don't have to poll
    /// `take_pty_writes`.
    pub fn set_pty_outbox(&mut self, cb: js_sys::Function) {
        self.pty_outbox = Some(cb);
    }

    /// Drain `take_pty_writes` once and push the bytes through the
    /// installed `pty_outbox` callback. No-op when no callback is
    /// installed or nothing is queued. Called automatically by
    /// `feed_pty_output`; exposed so JS hosts that need to flush
    /// after their own out-of-band feed paths (e.g. clipboard
    /// paste injected via wasm) can opt in.
    pub fn flush_pty_outbox(&mut self) {
        let bytes = self.rendered.take_pty_writes();
        if bytes.is_empty() {
            return;
        }
        let Some(cb) = self.pty_outbox.as_ref() else {
            return;
        };
        let b64 = base64_encode(&bytes);
        // Callback receives a single JsString argument so the bytes
        // survive structured clone / postMessage paths without
        // UTF-8 sanitisation.
        let _ = cb.call1(&JsValue::NULL, &JsValue::from_str(&b64));
    }

    /// Feed PTY output from the daemon into the terminal grid.
    /// After the feed lands, any DSR / OSC responses the terminal
    /// emitted are auto-flushed through the installed `pty_outbox`
    /// callback (if any). Hosts that prefer to poll can leave
    /// `pty_outbox` unset and keep using `take_pty_writes`.
    pub fn feed_pty_output(&mut self, bytes: &[u8]) {
        self.rendered.terminal_mut().feed(bytes);
        self.sync_terminal_block_prompt_state();
        self.flush_pty_outbox();
    }

    /// Mirrors `RenderedTerminal::drain_effects_json` — host pulls
    /// non-PTY effects (title, bell, …) as serde-encoded JSON.
    pub fn drain_effects_json(&mut self) -> JsValue {
        self.rendered.drain_effects_json()
    }

    /// Read-only snapshot of the cell grid. Cost is proportional
    /// to grid size; most hosts don't need this because the
    /// `render()` pump already paints.
    pub fn snapshot(&self) -> JsValue {
        self.rendered.snapshot()
    }

    /// Snapshot of the per-panel layout rects as JSON.
    pub fn layout_json(&self) -> JsValue {
        serde_wasm_bindgen::to_value(self.chrome.layout()).unwrap_or(JsValue::NULL)
    }

    pub fn drain_top_bar_action(&mut self) -> Option<String> {
        self.chrome
            .drain_top_bar_action()
            .map(|action| match action {
                neoism_ui::panels::TopBarAction::OpenSettings => {
                    "open_settings".to_string()
                }
                neoism_ui::panels::TopBarAction::OpenServers => {
                    "open_servers".to_string()
                }
                neoism_ui::panels::TopBarAction::OpenWorkspaces => {
                    "open_workspaces".to_string()
                }
                neoism_ui::panels::TopBarAction::StartWebServer => {
                    "start_web_server".to_string()
                }
                neoism_ui::panels::TopBarAction::OpenThemes => "open_themes".to_string(),
                neoism_ui::panels::TopBarAction::OpenExtensions => {
                    "open_extensions".to_string()
                }
                neoism_ui::panels::TopBarAction::TogglePanel => {
                    "toggle_panel".to_string()
                }
                neoism_ui::panels::TopBarAction::ToggleRightPanel => {
                    "toggle_right_panel".to_string()
                }
            })
    }

    /// True when visible chrome owns keyboard input and the host
    /// must not translate key presses into PTY bytes. The command
    /// composer is sticky-visible but only OWNS the keyboard when
    /// it has explicit focus — otherwise typing belongs to the
    /// terminal underneath. Same for the file tree.
    pub fn keyboard_capture_active(&self) -> bool {
        self.chrome.command_palette.is_visible()
            || self.chrome.finder.is_visible()
            || self.chrome.git_diff.is_visible()
            || self.chrome.git_diff_panel.is_focused()
            || (self.chrome.notes_sidebar.is_visible()
                && self.chrome.notes_sidebar.is_focused())
            || self.chrome.focused() == Some(PanelKey::FileTree)
            || self.chrome.focused() == Some(PanelKey::BufferTabs)
            || self.chrome.focused() == Some(PanelKey::CommandComposer)
    }

    pub fn editor_input_modal_active(&self) -> bool {
        self.chrome.command_palette.is_visible()
            || self.chrome.finder.is_visible()
            || self.chrome.git_diff.is_visible()
    }

    pub fn focus_editor_input(&mut self) {
        self.chrome.focus_content_surface();
    }

    /// Replace the active IdeTheme by name (e.g. `"pastel_dark"`,
    /// `"nvchad_one"`, `"tokyo_night"`, `"catppuccin_mocha"`).
    /// Unknown names fall back to `pastel_dark`.
    ///
    /// Flows the new theme to:
    ///   1. `Chrome::set_ide_theme` — derives `ChromeTheme` and
    ///      publishes the active theme so shim panels read the
    ///      same palette.
    ///   2. `RenderedTerminal::apply_ide_theme` — reseeds the
    ///      terminal's named-color palette and pushes the resolved
    ///      bg into sugarloaf's swapchain clear color.
    pub fn set_ide_theme(&mut self, name: &str) {
        self.chrome.set_ide_theme(name);
        self.rendered.apply_ide_theme(name);
        self.relayout_chrome();
    }

    /// Configure the user cursor style: an optional `#RRGGBB`
    /// override (beats the theme cursor color, survives theme
    /// switches) and a preset name (`"rainbow"` animates through
    /// hues and ignores the color; anything else is solid).
    pub fn set_cursor_style(&mut self, color_hex: Option<String>, style: String) {
        self.chrome
            .set_cursor_style_config(color_hex.as_deref(), &style);
    }

    pub fn animations_active(&self) -> bool {
        self.chrome.animations_active()
    }
}
