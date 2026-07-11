use super::*;
use neoism_window::keyboard::{Key, ModifiersState};

impl Screen<'_> {
    pub(crate) fn is_file_tree_command_palette_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        if Self::is_command_colon_key(key, mods) {
            return true;
        }

        let no_global_modifier =
            !mods.control_key() && !mods.alt_key() && !mods.super_key();
        no_global_modifier
            && (key.text_with_all_modifiers() == Some(":")
                || key.text.as_deref() == Some(":")
                || matches!(key.logical_key.as_ref(), Key::Character(ch) if ch == ":"))
    }

    pub fn handle_file_tree_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        if !self.renderer.file_tree.is_visible() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let mouse_x = self.mouse.x as f32 / scale_factor;
        if mouse_x < 0.0 || mouse_x > self.renderer.file_tree.width() {
            return false;
        }
        let row_h = self.renderer.file_tree.row_height().max(1.0);
        let mouse_y = self.mouse.y as f32 / scale_factor;
        let (tree_top, tree_bottom) = self.side_panel_band();
        let tree_height = (tree_bottom - tree_top).max(0.0);
        if mouse_y < tree_top || mouse_y > tree_top + tree_height {
            return false;
        }
        let rows_visible = self
            .renderer
            .file_tree
            .visible_rows_for_panel_height(tree_height);
        let pixels = match delta {
            // 3 rows per "click" of the wheel mirrors most native UIs.
            neoism_window::event::MouseScrollDelta::LineDelta(_, y) => *y * row_h * 3.0,
            neoism_window::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
        };
        if pixels == 0.0 {
            return true;
        }
        self.renderer.file_tree.scroll_pixels(pixels, rows_visible);
        self.mark_dirty();
        true
    }

    pub(crate) fn file_tree_row_under_mouse(&self) -> (Option<usize>, bool) {
        if !self.renderer.file_tree.is_visible() {
            return (None, false);
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        // Tree occupies the middle band (below the full-width top
        // chrome, above the full-width status bar). MUST match the
        // `tree_top` used in `host/run.rs` render — both read
        // `side_panel_band()` so they can't drift.
        let (tree_top, tree_bottom) = self.side_panel_band();
        let tree_height = (tree_bottom - tree_top).max(0.0);

        let row =
            self.renderer
                .file_tree
                .hit_test(mouse_x, mouse_y, tree_top, tree_height);
        let in_tree_bounds = mouse_x >= 0.0
            && mouse_x <= self.renderer.file_tree.width()
            && mouse_y >= tree_top
            && mouse_y <= tree_top + tree_height;
        (row, in_tree_bounds)
    }

    pub fn handle_file_tree_click(&mut self) -> bool {
        if !self.renderer.file_tree.is_visible() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let _mouse_x = self.mouse.x as f32 / scale_factor;
        let _mouse_y = self.mouse.y as f32 / scale_factor;
        let (row, in_tree_bounds) = self.file_tree_row_under_mouse();
        let Some(row) = row else {
            if in_tree_bounds {
                // Blank space inside the tree still focuses the tree;
                // clicks outside it should fall through to the pane
                // below and clear tree focus first.
                self.renderer.file_tree.set_focused(true);
                self.mark_dirty();
                return true;
            }
            if self.renderer.file_tree.is_focused() {
                self.renderer.file_tree.set_focused(false);
                self.mark_dirty();
            }
            return false;
        };

        // Click always promotes to focused — the user wants their next
        // j/k/Enter to go to the tree, even if the previous click had
        // landed in the editor pane.
        self.renderer.file_tree.set_focused(true);
        self.renderer.file_tree.set_selected(row);
        self.activate_file_tree_selection();
        self.mark_dirty();
        true
    }

    pub fn handle_file_tree_context_click(&mut self) -> bool {
        if !self.renderer.file_tree.is_visible() {
            return false;
        }
        let (row, in_tree_bounds) = self.file_tree_row_under_mouse();
        let Some(row) = row else {
            if in_tree_bounds {
                self.renderer.file_tree.set_focused(true);
                self.mark_dirty();
                return true;
            }
            return false;
        };
        self.renderer.file_tree.set_focused(true);
        self.renderer.file_tree.set_selected(row);
        self.open_file_tree_context_menu();
        self.mark_dirty();
        true
    }
}
