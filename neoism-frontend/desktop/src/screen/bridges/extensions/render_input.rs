use super::*;
use neoism_ui::panels::extensions_page::{NeoismExtensionsPane, PaneAction};
use neoism_window::event::MouseButton;

impl Screen<'_> {
    /// Per-frame renderer. Mirrors `render_neoism_tags_panels`.
    /// Returns true if any pane painted so the caller can mark dirty.
    pub(crate) fn render_neoism_extensions_panels(&mut self) -> bool {
        let scale = self.sugarloaf.scale_factor();
        let theme = self.renderer.theme;
        let chrome_scale = self.renderer.chrome_scale();
        let window_size = self.sugarloaf.window_size();
        let text_occlusions = self.renderer.active_text_occlusion_rects(
            window_size.width,
            window_size.height,
            scale,
        );
        let mouse = (!self.mouse_hidden_by_typing)
            .then_some((self.mouse.x as f32 / scale, self.mouse.y as f32 / scale));
        let (visible_nodes, scaled_margin) = {
            let grid = self.context_manager.current_grid();
            (
                grid.contexts()
                    .keys()
                    .copied()
                    .filter(|node| grid.is_context_visible(*node))
                    .collect::<Vec<_>>(),
                grid.scaled_margin,
            )
        };

        // If the background catalog seed has finished since the last
        // frame, re-seed visible Extensions panes so the language-server
        // rows' install plans resolve in place (no need for the user to
        // reopen the tab).
        self.drain_catalog_cache_refresh();

        // Pump install progress into the active extensions pane(s).
        // We do this OUTSIDE the contexts_mut borrow loop so the pump
        // can call back into Screen (notifications, lookup_bundled_manifest,
        // modal updates). A second sweep below handles rendering with a
        // fresh borrow.
        if !self.renderer.install_tracker.in_flight.is_empty() {
            // Lift the pane out via swap, pump, then put it back. If no
            // Extensions pane is currently visible, still pump with
            // `None` so modal-sourced installs (the missing-LSP modal)
            // can drive their busy modal to completion even when the
            // user is on a buffer tab.
            let mut taken: Option<(taffy::NodeId, NeoismExtensionsPane)> = None;
            for (key, item) in self
                .context_manager
                .current_grid_mut()
                .contexts_mut()
                .iter_mut()
            {
                if !visible_nodes.contains(key) {
                    continue;
                }
                if let Some(pane) = item.val.neoism_extensions.take() {
                    taken = Some((*key, pane));
                    break;
                }
            }
            if let Some((node, mut pane)) = taken {
                self.pump_install_progress(Some(&mut pane));
                self.mark_dirty();
                // Put it back; the loop below renders.
                if let Some(item) = self
                    .context_manager
                    .current_grid_mut()
                    .contexts_mut()
                    .get_mut(&node)
                {
                    item.val.neoism_extensions = Some(pane);
                }
            } else {
                self.pump_install_progress(None);
                self.mark_dirty();
            }
        }

        // Reserve space at the bottom for the status line — the panel
        // text uses DrawOpts.clip_rect = panel rect, so any rows that
        // would have painted into the status_line strip get cut off
        // before the chrome's status_line paints its black background
        // on top. Geometry (cards / chips / buttons) is below
        // `panels::status_line::ORDER_BG=16` so it's covered
        // automatically; this reservation is what protects the TEXT
        // pipeline (which doesn't honour order).
        let status_h = self.renderer.status_line_height();
        let mut painted = false;
        for (key, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if !visible_nodes.contains(key) {
                continue;
            }
            let Some(pane) = item.val.neoism_extensions.as_mut() else {
                continue;
            };
            let pane_h = (item.layout_rect[3] / scale - status_h).max(0.0);
            let rect = [
                (scaled_margin.left + item.layout_rect[0]) / scale,
                (scaled_margin.top + item.layout_rect[1]) / scale,
                item.layout_rect[2] / scale,
                pane_h,
            ];
            pane.render(
                &mut self.sugarloaf,
                rect,
                &theme,
                chrome_scale,
                mouse,
                &text_occlusions,
            );
            painted = true;
        }
        painted
    }

    /// Mouse click router for the Extensions panel. Returns `true` when
    /// the click was consumed so the outer click pipeline stops here.
    /// Mirrors `handle_neoism_tags_mouse_press`'s shape.
    pub(crate) fn handle_extensions_click(&mut self, button: MouseButton) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        if self.context_manager.current().neoism_extensions.is_none() {
            return false;
        }
        let [x, y] = self.markdown_mouse_logical();
        let action = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
            .and_then(|pane| {
                pane.on_pointer_down(x, y, neoism_ui::event::PointerButton::Left)
            });
        let Some(action) = action else {
            // Still consume the click so the editor pipeline doesn't
            // chew on it (matches the tags-view bridge convention).
            self.mark_dirty();
            return true;
        };
        self.handle_pane_action(action);
        true
    }

    /// Dispatch a `PaneAction` returned from the Extensions panel
    /// (pointer click OR keyboard Enter). Shared so click + Enter
    /// reach the same install / uninstall / open-repo paths.
    fn handle_pane_action(&mut self, action: PaneAction) {
        match action {
            PaneAction::InstallToggleRequested {
                id,
                currently_installed,
            } => {
                if currently_installed {
                    self.dispatch_uninstall(&id);
                } else {
                    self.dispatch_install(&id);
                }
            }
            PaneAction::OpenRepository(url) => {
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open").arg(&url).spawn();
                }
                #[cfg(not(any(target_os = "macos", windows)))]
                {
                    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                }
                #[cfg(windows)]
                {
                    let _ = std::process::Command::new("cmd")
                        .args(["/c", "start", "", &url])
                        .spawn();
                }
            }
        }
        self.mark_dirty();
    }

    /// Forward a mouse-wheel event to the active Extensions pane.
    /// `delta_pixels` is positive when the user is scrolling DOWN
    /// (content moves up). Matches the markdown wheel contract.
    /// Returns true when the pane consumed the wheel.
    pub(crate) fn handle_extensions_wheel(&mut self, delta_pixels: f32) -> bool {
        let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        else {
            return false;
        };
        pane.scroll_pixels(delta_pixels);
        self.mark_dirty();
        true
    }

    /// Keyboard input router for the Extensions pane. Returns `true`
    /// when the panel consumed the key (arrows, Enter, Esc, search
    /// editing) so the outer dispatcher stops here. We CAN'T just let
    /// every press into the pane unconditionally — global shortcuts
    /// (Ctrl+T new tab, Cmd+Q, font zoom, etc.) still need to win.
    /// The convention used elsewhere is to return `false` for anything
    /// involving Ctrl / Alt / Super so the outer chain handles them.
    pub(crate) fn handle_extensions_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
    ) -> bool {
        if self.context_manager.current().neoism_extensions.is_none() {
            return false;
        }
        if key.state != neoism_window::event::ElementState::Pressed {
            return false;
        }
        let mods = self.modifiers.state();
        let alt = mods.alt_key();
        let ctrl = mods.control_key();
        let sup = mods.super_key();
        // Most modified keys still belong to the global chain (Ctrl+T
        // new tab, Cmd+Q, font zoom, …) so we return false and let them
        // through. The two exceptions the Extensions page owns: Alt+
        // Left/Right (cycle category tabs — mirrors the chrome tab strip)
        // and Cmd/Ctrl+F (jump the cursor to the search box). Detect
        // them off the *unmodified* logical key so a platform that
        // mangles Ctrl+F into a control char still matches.
        let alt_arrow_tab = {
            use neoism_window::keyboard::{Key, NamedKey as WinitNamed};
            alt && !ctrl
                && !sup
                && matches!(
                    &key.logical_key,
                    Key::Named(WinitNamed::ArrowLeft | WinitNamed::ArrowRight)
                )
        };
        let focus_search_combo = {
            use neoism_window::keyboard::Key;
            (ctrl || sup)
                && !alt
                && matches!(
                    key.key_without_modifiers().as_ref(),
                    Key::Character(ch) if ch.eq_ignore_ascii_case("f")
                )
        };
        if (ctrl || alt || sup) && !alt_arrow_tab && !focus_search_combo {
            return false;
        }

        let descriptor = winit_key_to_descriptor(key, mods);
        let pane = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
            .expect("checked Some above");

        // Cmd/Ctrl+F handled here (not via the descriptor) so a mangled
        // control-char logical key can't miss the focus jump.
        if focus_search_combo {
            pane.focus_search();
            self.mark_dirty();
            return true;
        }

        // Feed text BEFORE the descriptor: either the dropdown's
        // language-search input (precedence — it sits on top) or the
        // page's main extension search box can swallow the character.
        // `on_text` decides which based on focus state.
        let mut consumed = false;
        if let Some(text) = key.text.as_ref() {
            let typed = text.as_str();
            if !typed.is_empty()
                && (pane.search_focused() || pane.language_search_focused())
                && pane.on_text(typed)
            {
                consumed = true;
            }
        }

        let response = pane.on_key(&descriptor);
        consumed |= response.consumed;
        if let Some(action) = response.action {
            self.handle_pane_action(action);
            consumed = true;
        }
        if consumed {
            self.mark_dirty();
        }
        consumed
    }
}
