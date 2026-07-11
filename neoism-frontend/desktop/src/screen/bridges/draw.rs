// Part of the `impl Screen<'_>` block (see sibling mod.rs). Mirrors
// `bridges/markdown.rs` for the `.neodraw` sketch surface: open a file
// into a `DrawPane` context, then render every visible draw pane.

use super::super::*;
use neoism_window::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use neoism_window::keyboard::{Key, ModifiersState, NamedKey};
use std::path::{Path, PathBuf};

impl Screen<'_> {
    /// Run `f` against the active `.neodraw` pane, if any.
    fn with_current_draw<R>(
        &mut self,
        f: impl FnOnce(&mut crate::editor::neodraw::DrawPane) -> R,
    ) -> Option<R> {
        self.context_manager.current_mut().draw.as_mut().map(f)
    }

    /// Window-logical pointer position (matches `markdown_mouse_logical`).
    fn draw_mouse_logical(&self) -> [f32; 2] {
        let scale = self.sugarloaf.scale_factor();
        [self.mouse.x as f32 / scale, self.mouse.y as f32 / scale]
    }

    /// Whether a `.neodraw` pane currently has an active pointer gesture.
    pub fn draw_drag_active(&self) -> bool {
        self.context_manager
            .current()
            .draw
            .as_ref()
            .map(|d| d.pointer_active())
            .unwrap_or(false)
    }

    pub fn handle_draw_mouse_press(&mut self, button: MouseButton) -> bool {
        if self.context_manager.current().draw.is_none() || button != MouseButton::Left {
            return false;
        }
        let [x, y] = self.draw_mouse_logical();
        let additive = self.modifiers.state().shift_key();
        // Double-click edits the text under the cursor (or drops a new
        // text box), regardless of the active tool.
        let double = matches!(
            self.mouse.click_state,
            crate::event::ClickState::DoubleClick
        );
        let consumed = self
            .context_manager
            .current_mut()
            .draw
            .as_mut()
            .map(|d| {
                if double {
                    d.double_click(x, y)
                } else {
                    d.begin_pointer(x, y, additive)
                }
            })
            .unwrap_or(false);
        if consumed {
            self.mark_dirty();
        }
        consumed
    }

    pub fn handle_draw_drag_move(&mut self) -> bool {
        let [x, y] = self.draw_mouse_logical();
        let changed = self
            .context_manager
            .current_mut()
            .draw
            .as_mut()
            .map(|d| d.drag_pointer(x, y))
            .unwrap_or(false);
        if changed {
            self.mark_dirty();
        }
        changed
    }

    /// Wheel over a `.neodraw` pane pans the canvas; Ctrl+wheel zooms
    /// about the cursor. Returns whether the wheel was consumed.
    pub fn handle_draw_wheel(&mut self, delta: &MouseScrollDelta) -> bool {
        if self.context_manager.current().draw.is_none() {
            return false;
        }
        let [x, y] = self.draw_mouse_logical();
        let in_bounds = self
            .context_manager
            .current()
            .draw
            .as_ref()
            .map(|d| d.window_in_bounds(x, y))
            .unwrap_or(false);
        if !in_bounds {
            return false;
        }
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(c, l) => (*c, *l),
            MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 40.0, p.y as f32 / 40.0),
        };
        let ctrl = self.modifiers.state().control_key();
        if let Some(d) = self.context_manager.current_mut().draw.as_mut() {
            if ctrl {
                d.zoom_at(x, y, 1.12_f32.powf(dy));
            } else {
                d.pan_by(dx * 40.0, dy * 40.0);
            }
        }
        self.mark_dirty();
        true
    }

    /// Update the graph's hovered node from the cursor (graph view only).
    pub fn handle_draw_hover(&mut self) -> bool {
        let [x, y] = self.draw_mouse_logical();
        self.context_manager
            .current_mut()
            .draw
            .as_mut()
            .map(|d| d.set_graph_hover(x, y))
            .unwrap_or(false)
    }

    /// Whether a graph node/label is under the cursor (→ pointer cursor).
    pub fn draw_graph_hovering(&self) -> bool {
        self.context_manager
            .current()
            .draw
            .as_ref()
            .is_some_and(|d| d.graph.is_some() && d.graph_hover.is_some())
    }

    /// Whether the active pane is a graph view.
    pub fn draw_is_graph(&self) -> bool {
        self.context_manager
            .current()
            .draw
            .as_ref()
            .is_some_and(|d| d.graph.is_some())
    }

    /// Trackpad pinch zoom about the cursor for the active draw pane.
    pub fn handle_draw_pinch(&mut self, delta: f32) -> bool {
        if self.context_manager.current().draw.is_none() {
            return false;
        }
        let [x, y] = self.draw_mouse_logical();
        let in_bounds = self
            .context_manager
            .current()
            .draw
            .as_ref()
            .map(|d| d.window_in_bounds(x, y))
            .unwrap_or(false);
        if !in_bounds {
            return false;
        }
        if let Some(d) = self.context_manager.current_mut().draw.as_mut() {
            d.zoom_at(x, y, 1.0 + delta);
        }
        self.mark_dirty();
        true
    }

    pub fn handle_draw_mouse_release(&mut self) -> bool {
        let finalized = self
            .context_manager
            .current_mut()
            .draw
            .as_mut()
            .map(|d| d.end_pointer())
            .unwrap_or(false);
        if finalized {
            self.mark_dirty();
        }
        finalized
    }

    /// Tool shortcuts + Escape/Delete for the active `.neodraw` pane.
    /// Returns whether the key was consumed.
    pub(crate) fn dispatch_draw_key(
        &mut self,
        key: &KeyEvent,
        mods: ModifiersState,
        text: &str,
    ) -> bool {
        use crate::editor::neodraw::Tool;
        if key.state == ElementState::Released {
            return false;
        }
        let ctrl = mods.control_key() || mods.super_key();
        let editing = self
            .context_manager
            .current()
            .draw
            .as_ref()
            .map(|d| d.editing())
            .unwrap_or(false);

        // Undo / redo (Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y) take priority.
        if ctrl && !mods.alt_key() {
            if let Key::Character(c) = &key.logical_key {
                let lower = c.to_lowercase();
                let did = match lower.as_str() {
                    "z" if mods.shift_key() => self.with_current_draw(|d| d.redo()),
                    "z" => self.with_current_draw(|d| d.undo()),
                    "y" => self.with_current_draw(|d| d.redo()),
                    "c" => self.with_current_draw(|d| d.copy_selection()),
                    "v" => self.with_current_draw(|d| d.paste()),
                    "d" => self.with_current_draw(|d| d.duplicate_selection()),
                    "=" | "+" => self.with_current_draw(|d| d.change_text_size(1.15)),
                    "-" | "_" => {
                        self.with_current_draw(|d| d.change_text_size(1.0 / 1.15))
                    }
                    "a" => self.with_current_draw(|d| {
                        d.select_all();
                        true
                    }),
                    _ => None,
                };
                if let Some(changed) = did {
                    if changed {
                        self.mark_dirty();
                    }
                    return true;
                }
            }
        }

        // While typing into a text shape, route printable + edit keys to
        // the text buffer instead of tool shortcuts.
        if editing {
            if let Key::Named(named) = &key.logical_key {
                let handled = match named {
                    NamedKey::Backspace => self.with_current_draw(|d| d.text_backspace()),
                    NamedKey::Enter => self.with_current_draw(|d| d.text_newline()),
                    NamedKey::Escape => self.with_current_draw(|d| d.cancel()),
                    NamedKey::Space => self.with_current_draw(|d| d.insert_text(" ")),
                    _ => None,
                };
                if let Some(_) = handled {
                    self.mark_dirty();
                    return true;
                }
            }
            if !ctrl && !mods.alt_key() && !text.is_empty() {
                self.with_current_draw(|d| d.insert_text(text));
                self.mark_dirty();
                return true;
            }
            // Swallow other keys while editing so they don't trigger
            // tool shortcuts mid-word.
            return true;
        }

        // Vim-style undo with a bare `u` (redo stays on Ctrl+R/Ctrl+Y).
        if !ctrl && !mods.alt_key() {
            if let Key::Character(c) = &key.logical_key {
                if c.eq_ignore_ascii_case("u")
                    && self.with_current_draw(|d| d.undo()).is_some()
                {
                    self.mark_dirty();
                    return true;
                }
            }
        }

        // Close-tab keys (not while editing): `q`, or the `Space x`
        // leader chord.
        if !ctrl && !mods.alt_key() {
            let armed = self
                .context_manager
                .current()
                .draw
                .as_ref()
                .map(|d| d.space_armed)
                .unwrap_or(false);
            if armed {
                if let Some(d) = self.context_manager.current_mut().draw.as_mut() {
                    d.space_armed = false;
                }
                if matches!(&key.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("x"))
                {
                    self.close_active_buffer_tab();
                }
                return true; // leader consumes the follow-up key either way
            }
            if matches!(&key.logical_key, Key::Named(NamedKey::Space)) {
                if let Some(d) = self.context_manager.current_mut().draw.as_mut() {
                    d.space_armed = true;
                }
                return true;
            }
            if matches!(&key.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("q"))
            {
                self.close_active_buffer_tab();
                return true;
            }
        }

        if let Key::Named(named) = &key.logical_key {
            let action = match named {
                NamedKey::Escape => self.with_current_draw(|d| d.cancel()),
                NamedKey::Delete | NamedKey::Backspace => {
                    self.with_current_draw(|d| d.delete_selection())
                }
                _ => None,
            };
            if let Some(changed) = action {
                if changed {
                    self.mark_dirty();
                }
                return true;
            }
        }
        // Leave remaining modifier combos to the global binding path.
        if ctrl || mods.alt_key() {
            return false;
        }
        let tool = match text {
            "v" | "V" => Tool::Select,
            "r" | "R" => Tool::Rect,
            "o" | "O" => Tool::Ellipse,
            "a" | "A" => Tool::Arrow,
            "l" | "L" => Tool::Line,
            "p" | "P" => Tool::Pen,
            "t" | "T" => Tool::Text,
            "h" | "H" => Tool::Hand,
            _ => return false,
        };
        if let Some(d) = self.context_manager.current_mut().draw.as_mut() {
            d.set_tool(tool);
            self.mark_dirty();
            return true;
        }
        false
    }

    /// Command-palette "Draw on Note": open (creating if needed) the active
    /// markdown note's handwriting layer (`"<note> (reMarkable).neodraw"`)
    /// in the full neodraw editor — toolbar, tools, scroll, save — so you
    /// can annotate the note in Neoism. The strokes become the note's ink
    /// overlay (the same layer the reMarkable writes to).
    /// Toggle in-place "Draw on Note": draw ink directly over the rendered
    /// markdown (no separate file/pane). Run again to finish (saves).
    pub(crate) fn draw_on_current_note(&mut self) {
        use neoism_ui::panels::notifications::NotificationLevel;
        if self.draw_over_note.is_some() {
            self.exit_draw_over_note();
            self.renderer
                .notifications
                .push("Draw on Note: saved".to_string(), NotificationLevel::Info);
            return;
        }
        let Some(note_path) = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .map(|m| m.path.clone())
        else {
            self.renderer.notifications.push(
                "Draw on Note: open a markdown note first".to_string(),
                NotificationLevel::Warn,
            );
            return;
        };
        let scene = crate::editor::neodraw::load_ink_layer(&note_path);
        let sidecar = crate::editor::neodraw::ink_sidecar_path(&note_path);
        let mut pane =
            crate::editor::neodraw::DrawPane::from_source(sidecar, &scene.to_json());
        pane.set_tool(crate::editor::neodraw::Tool::Pen);
        pane.fit_pending = false; // camera is locked to the note's scroll
        self.draw_over_note = Some(crate::screen::DrawOverNote {
            note: note_path,
            pane,
        });
        self.renderer.notifications.push(
            "Draw mode on — pen draws over the note; run \"Draw on Note\" again to finish".to_string(),
            NotificationLevel::Info,
        );
        self.mark_dirty();
    }

    /// Finish drawing: persist the ink sidecar and refresh the overlay.
    pub(crate) fn exit_draw_over_note(&mut self) {
        if let Some(d) = self.draw_over_note.take() {
            let note = d.note.clone();
            let _ = std::fs::write(
                crate::editor::neodraw::ink_sidecar_path(&note),
                d.pane.to_source(),
            );
            self.ink_overlay_cache.remove(&note);
            self.sync_markdown_tab_modified(&note, false); // saved → dot off
            self.mark_dirty();
        }
    }

    /// Feed a pointer event to the draw-over-note `DrawPane`. `phase`:
    /// 0=press, 1=drag, 2=release. `(x,y)` are logical screen pixels — the
    /// pane maps them via its locked camera + `last_rect`. Persists on
    /// release. Returns true if drawing consumed the event.
    pub(crate) fn draw_over_note_pointer(&mut self, phase: u8, x: f32, y: f32) -> bool {
        let (consumed, save) = {
            let Some(d) = self.draw_over_note.as_mut() else {
                return false;
            };
            let consumed = match phase {
                0 => d.pane.begin_pointer(x, y, false),
                1 => d.pane.drag_pointer(x, y),
                _ => d.pane.end_pointer(),
            };
            let save = if consumed && phase == 2 {
                Some((
                    crate::editor::neodraw::ink_sidecar_path(&d.note),
                    d.pane.to_source(),
                    d.note.clone(),
                ))
            } else {
                None
            };
            (consumed, save)
        };
        if let Some((path, src, note)) = save {
            let _ = std::fs::write(path, src);
            self.ink_overlay_cache.remove(&note);
            self.sync_markdown_tab_modified(&note, true); // the yellow edited dot
        }
        if consumed {
            self.mark_dirty();
        }
        consumed
    }

    /// Keys for in-place draw mode: undo/redo, Esc (finish), Ctrl+S (save).
    pub(crate) fn handle_draw_over_note_key(
        &mut self,
        key: &KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        if self.draw_over_note.is_none() || key.state != ElementState::Pressed {
            return false;
        }
        if let Key::Named(NamedKey::Escape) = &key.logical_key {
            self.exit_draw_over_note();
            return true;
        }
        let ctrl = mods.control_key() || mods.super_key();
        let did = if let Key::Character(c) = &key.logical_key {
            match c.to_lowercase().as_str() {
                "z" if ctrl && mods.shift_key() => self.draw_over_apply(|p| p.redo()),
                "z" if ctrl => self.draw_over_apply(|p| p.undo()),
                "y" if ctrl => self.draw_over_apply(|p| p.redo()),
                "u" if !ctrl && !mods.alt_key() => self.draw_over_apply(|p| p.undo()),
                "s" if ctrl => {
                    self.draw_over_save();
                    true
                }
                _ => false,
            }
        } else {
            false
        };
        if did {
            self.mark_dirty();
        }
        did
    }

    fn draw_over_apply(
        &mut self,
        f: impl FnOnce(&mut crate::editor::neodraw::DrawPane) -> bool,
    ) -> bool {
        let result = {
            let Some(d) = self.draw_over_note.as_mut() else {
                return false;
            };
            f(&mut d.pane).then(|| (d.note.clone(), d.pane.to_source()))
        };
        if let Some((note, src)) = result {
            let _ = std::fs::write(crate::editor::neodraw::ink_sidecar_path(&note), src);
            self.ink_overlay_cache.remove(&note);
            self.sync_markdown_tab_modified(&note, true);
            true
        } else {
            false
        }
    }

    pub(crate) fn draw_over_save(&mut self) {
        let saved = self
            .draw_over_note
            .as_ref()
            .map(|d| (d.note.clone(), d.pane.to_source()));
        if let Some((note, src)) = saved {
            let _ = std::fs::write(crate::editor::neodraw::ink_sidecar_path(&note), src);
            self.ink_overlay_cache.remove(&note);
            self.sync_markdown_tab_modified(&note, false);
        }
    }

    /// Open `path` as a `.neodraw` document in the active pane.
    pub fn open_path_in_draw(&mut self, path: PathBuf) {
        let workspace_root = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| path.parent().map(Path::to_path_buf));
        if let Some(root) = workspace_root.clone() {
            self.set_active_workspace_root(root, false);
        }
        self.clear_current_workspace_buf_enter_guard();
        self.renderer.buffer_tabs.ensure_terminal_tab();
        // Reuse the markdown tab registration — the buffer tab is just
        // bar metadata; content type is driven by the Context fields.
        self.renderer.buffer_tabs.open_markdown(path.clone());
        self.renderer.file_tree.set_active_path(Some(path.clone()));
        if let Some(id) = self.current_workspace_id() {
            self.workspace_editor_active_paths.insert(id, path.clone());
        }

        self.activate_draw_path(path);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    pub(crate) fn activate_draw_path(&mut self, path: PathBuf) {
        if let Some((_route_id, node)) = self.context_manager.draw_node_by_path(&path) {
            let _ = self
                .context_manager
                .current_grid_mut()
                .set_current_node(node, &mut self.sugarloaf);
            self.context_manager.select_route_from_current_grid();
            return;
        }

        let rich_text_id = crate::context::factories::next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        if !self
            .context_manager
            .add_stacked_draw(path, rich_text_id, &mut self.sugarloaf)
        {
            self.file_tree_notify(
                "Could not open drawing pane",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
    }

    /// Mirror the active drawing's dirty flag onto its buffer tab so the
    /// unsaved-changes dot shows (like markdown / nvim tabs).
    pub(crate) fn sync_active_draw_modified(&mut self) {
        let Some((path, dirty)) = self
            .context_manager
            .current()
            .draw
            .as_ref()
            // The graph view is a transient, file-less tab — never show
            // the unsaved-changes dot for it.
            .map(|d| (d.path.clone(), d.is_dirty() && d.graph.is_none()))
        else {
            return;
        };
        self.sync_markdown_tab_modified(&path, dirty);
    }

    /// Paint every visible `.neodraw` pane. Returns whether a redraw is
    /// still needed (currently always false — no canvas animation yet).
    pub(crate) fn render_draw_panels(&mut self) -> bool {
        self.sync_active_draw_modified();
        let scale = self.sugarloaf.scale_factor();
        let theme = self.renderer.theme;
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
        let mut animating = false;
        let mut open_requests: Vec<String> = Vec::new();
        for (key, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if !visible_nodes.contains(key) {
                continue;
            }
            let Some(draw) = item.val.draw.as_mut() else {
                continue;
            };
            let rect = [
                (scaled_margin.left + item.layout_rect[0]) / scale,
                (scaled_margin.top + item.layout_rect[1]) / scale,
                item.layout_rect[2] / scale,
                item.layout_rect[3] / scale,
            ];
            neoism_ui::editor::neodraw::render_pane(
                &mut self.sugarloaf,
                draw,
                rect,
                &theme,
            );
            // Keep redrawing while a graph simulation is still settling.
            if draw.graph.as_ref().is_some_and(|g| g.is_animating()) {
                animating = true;
            }
            if let Some(path) = draw.graph_open_request.take() {
                open_requests.push(path);
            }
        }
        // A clicked graph node opens its note (after the render borrow ends).
        for path in open_requests {
            self.open_path_in_editor(std::path::PathBuf::from(path));
        }
        animating
    }

    pub(crate) fn save_current_draw(&mut self) -> bool {
        // The graph view is a transient, file-less tab — don't let a
        // stray Ctrl+S write an (empty) `Note Graph.neodraw` to disk.
        if self
            .context_manager
            .current()
            .draw
            .as_ref()
            .is_some_and(|d| d.graph.is_some())
        {
            return true;
        }
        let Some(result) = self
            .context_manager
            .current_mut()
            .draw
            .as_mut()
            .map(|draw| draw.save().map_err(|err| err.to_string()))
        else {
            return false;
        };
        match result {
            Ok(()) => {
                self.mark_dirty();
                true
            }
            Err(err) => {
                self.file_tree_notify(
                    &format!("Could not save drawing: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                true
            }
        }
    }
}
