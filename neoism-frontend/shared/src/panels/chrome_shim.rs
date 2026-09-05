//! Adapter shims so the lifted panels conform to the chrome.rs
//! Panel-trait dispatch surface (`is_visible`, `handle_event`, `draw`).
//!
//! The native panels exposed their own wider entry points
//! (`render(&mut self, sugarloaf, ...lots of args)`). These shims keep
//! chrome.rs compiling and the slim contract visible to web bridges.
//! (The composer itself renders through the real
//! `CommandComposer::render` call in `chrome/draw.rs`, fed by the
//! chrome-owned `terminal_input` buffer — it has no draw shim.)
//!
//! Keyboard dispatch for `CommandPalette` and `Finder` mirrors
//! `frontends/neoism/src/router/route.rs` (the native overlay-key
//! gate). Only the panel-local mutations are reproduced — host-side
//! side effects (nvim preview, clipboard, theme/shader pickers,
//! workspace buffer activation, search dispatch) belong to the bridge
//! layer and are not invoked here.

use sugarloaf::Sugarloaf;

use crate::chrome::active_ide_theme;
use crate::event::{KeyState, LogicalKey, NamedKey, UiEvent};
use crate::layout::PanelLayout;
use crate::panels::command_composer::CommandComposer;
use crate::panels::command_palette::CommandPalette;
use crate::panels::finder::Finder;

/// Approximate visible-rows count for finder PageDown / ArrowDown
/// math. Native passes `18` (see `router/route.rs`'s finder block);
/// the real value is recomputed from the overlay height each frame.
const FINDER_APPROX_VISIBLE_ROWS: usize = 18;

impl CommandComposer {
    /// The composer is purely a viewer of an external `InputBuffer`
    /// (terminal grid or editor); native never feeds keystrokes into
    /// the composer itself. Keys flow into the underlying surface and
    /// the composer reflects them via `render(..&dyn InputBuffer..)`.
    /// So this is intentionally a no-op — not a missing dispatch.
    pub fn handle_event(
        &mut self,
        _event: &UiEvent,
        _ctx: &mut crate::panels::PanelContext,
    ) {
        // intentional no-op — composer mirrors InputBuffer, no own state
    }
}

impl CommandPalette {
    pub fn is_visible(&self) -> bool {
        self.is_enabled()
    }

    /// Mirror of the command-palette key block in
    /// `frontends/neoism/src/router/route.rs` (~lines 434–865), minus
    /// host-side side effects (nvim preview, search/ex dispatch,
    /// clipboard, theme/shader pickers, workspace buffers). Those are
    /// the bridge's job; this only updates panel-local state so the
    /// query string and selection track keystrokes.
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        _ctx: &mut crate::panels::PanelContext,
    ) {
        if !self.is_enabled() {
            return;
        }
        match event {
            UiEvent::Key(key) if key.state == KeyState::Pressed => {
                match &key.logical {
                    LogicalKey::Named(NamedKey::Escape) => {
                        self.set_enabled(false);
                        // TODO(wave6-cutover): native also dispatches
                        // `vim_search_clear_command` when bailing out
                        // of search mode — bridge concern.
                    }
                    LogicalKey::Named(NamedKey::ArrowUp) => {
                        self.move_selection_up();
                        // TODO(wave6-cutover): `preview_palette_search_match_if_any` — bridge.
                    }
                    LogicalKey::Named(NamedKey::ArrowDown) => {
                        self.move_selection_down();
                        // TODO(wave6-cutover): `preview_palette_search_match_if_any` — bridge.
                    }
                    LogicalKey::Named(NamedKey::Tab) => {
                        if self.is_cd_query() {
                            if key.modifiers.contains(crate::event::Modifiers::SHIFT) {
                                self.cycle_cd_selection(true);
                            } else if !self.tab_complete() {
                                self.cycle_cd_selection(false);
                            }
                        } else if !self.tab_complete() {
                            self.move_selection_down();
                        }
                        // TODO(wave6-cutover): re-dispatch search query when
                        // tab-completing in search mode — bridge.
                    }
                    LogicalKey::Named(NamedKey::Enter) => {
                        // TODO(wave6-cutover): native commits via
                        // ex/search/font/buffer/action paths. Bridge
                        // resolves the selection; until then, close.
                        if !(self.workspace_directory_target().is_some() && self.is_cd_query()) {
                            self.set_enabled(false);
                        }
                    }
                    LogicalKey::Named(NamedKey::Backspace) => {
                        if self.backspace_query() {
                            // TODO(wave6-cutover): re-dispatch search
                            // preview when in search mode — bridge.
                        }
                    }
                    LogicalKey::Character(text) => {
                        if !text.is_empty()
                            && text.chars().all(|c| !c.is_control())
                            && !key.modifiers.contains(crate::event::Modifiers::CTRL)
                            && !key.modifiers.contains(crate::event::Modifiers::ALT)
                            && !key.modifiers.contains(crate::event::Modifiers::META)
                        {
                            self.insert_query_text(text);
                        }
                    }
                    LogicalKey::Named(NamedKey::ArrowLeft) => self.move_query_cursor_left(),
                    LogicalKey::Named(NamedKey::ArrowRight) => self.move_query_cursor_right(),
                    LogicalKey::Named(NamedKey::Home) => self.set_query_cursor(0),
                    LogicalKey::Named(NamedKey::End) => self.set_query_cursor(self.query.len()),
                    _ => {}
                }
            }
            UiEvent::Text(text) => {
                // Host may emit a separate `Text` event (IME commit
                // path / DOM `beforeinput`). Treat as query append.
                if !text.is_empty() && text.chars().all(|c| !c.is_control()) {
                    self.insert_query_text(text);
                }
            }
            _ => {}
        }
    }

    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        _layout: &PanelLayout,
        _ctx: &crate::panels::PanelContext,
    ) {
        // Defer to the lifted `CommandPalette::render` (see
        // `command_palette/render.rs`). It computes its own centered
        // modal rect from the window dimensions, so `_layout.bounds`
        // (the chrome's slot for the palette) is intentionally
        // unused — the panel paints itself.
        let theme = active_ide_theme();
        let size = sugarloaf.window_size();
        let scale_factor = sugarloaf.scale_factor();
        self.render(sugarloaf, (size.width, size.height, scale_factor), &theme);
    }
}

impl Finder {
    pub fn is_visible(&self) -> bool {
        self.is_enabled()
    }

    /// Mirror of the finder key block in
    /// `frontends/neoism/src/router/route.rs` (~lines 358–431), minus
    /// host-side side effects (`open_finder_selection` to activate a
    /// hit — bridge concern).
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        _ctx: &mut crate::panels::PanelContext,
    ) {
        if !self.is_enabled() {
            return;
        }
        match event {
            UiEvent::ServiceReply {
                request_id,
                payload,
            } => {
                self.handle_service_reply(*request_id, payload, _ctx.services.search);
            }
            UiEvent::Key(key) if key.state == KeyState::Pressed => {
                let ctrl_only = key.modifiers.contains(crate::event::Modifiers::CTRL)
                    && !key.modifiers.contains(crate::event::Modifiers::ALT)
                    && !key.modifiers.contains(crate::event::Modifiers::META);
                match &key.logical {
                    LogicalKey::Named(NamedKey::Tab) if ctrl_only => {
                        self.cycle_search_mode();
                    }
                    LogicalKey::Named(NamedKey::Escape) => {
                        self.close();
                    }
                    LogicalKey::Named(NamedKey::ArrowUp) => {
                        self.move_selection_up();
                    }
                    LogicalKey::Named(NamedKey::ArrowDown) => {
                        self.move_selection_down(FINDER_APPROX_VISIBLE_ROWS);
                    }
                    LogicalKey::Named(NamedKey::Enter) => {
                        // TODO(wave6-cutover): native calls
                        // `open_finder_selection` to load the hit into
                        // an editor tab. The web bridge needs to call a
                        // host-side `pick_finder_selection` (or similar)
                        // method that reads `self.selected_index` +
                        // `self.results` and dispatches an "open buffer
                        // at <path>:<line>" command before invoking
                        // `self.close()`. Until that bridge call is
                        // wired, just close so Enter isn't an inert
                        // keystroke.
                        self.close();
                    }
                    LogicalKey::Named(NamedKey::Backspace) => {
                        let mut q = self.query.clone();
                        if q.pop().is_some() {
                            self.set_query(q);
                        }
                    }
                    LogicalKey::Character(text) => {
                        if !text.is_empty()
                            && text.chars().all(|c| !c.is_control())
                            && !key.modifiers.contains(crate::event::Modifiers::CTRL)
                            && !key.modifiers.contains(crate::event::Modifiers::ALT)
                            && !key.modifiers.contains(crate::event::Modifiers::META)
                        {
                            let new_query = format!("{}{}", self.query, text.as_str());
                            self.set_query(new_query);
                        }
                    }
                    _ => {}
                }
            }
            UiEvent::Text(text) => {
                if !text.is_empty() && text.chars().all(|c| !c.is_control()) {
                    let new_query = format!("{}{}", self.query, text);
                    self.set_query(new_query);
                }
            }
            _ => {}
        }
    }

    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        _layout: &PanelLayout,
        ctx: &crate::panels::PanelContext,
    ) {
        // Defer to the lifted `Finder::render` (see
        // `finder/render.rs`). Like the palette, the finder centers
        // its own overlay from window dimensions, so `_layout.bounds`
        // is informational only. `SearchService` + `FilesService` come
        // off `PanelContext::services`, which the host already wires
        // for every other panel.
        let theme = active_ide_theme();
        let size = sugarloaf.window_size();
        let scale_factor = sugarloaf.scale_factor();
        self.render(
            sugarloaf,
            (size.width, size.height, scale_factor),
            &theme,
            ctx.services.search,
            ctx.services.files,
        );
    }
}
