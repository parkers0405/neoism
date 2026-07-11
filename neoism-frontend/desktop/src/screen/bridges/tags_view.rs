// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use crate::workspace::tags_view::TagsViewAction;
use neoism_window::event::MouseButton;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn open_neoism_tags_view(
        &mut self,
        workspace_root: PathBuf,
        path: PathBuf,
    ) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let write_marker = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !path.exists() {
                std::fs::write(&path, "# Tags\n")?;
            }
            Ok(())
        })();
        if let Err(err) = write_marker {
            self.renderer.notifications.push(
                format!("Could not open Tags view {}: {err}", path.display()),
                NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        }

        self.set_active_workspace_root(workspace_root.clone(), false);
        self.clear_current_workspace_buf_enter_guard();
        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.renderer.buffer_tabs.open_markdown(path.clone());
        self.renderer.file_tree.set_active_path(None);
        self.activate_neoism_tags_path(path, workspace_root);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    pub(crate) fn activate_neoism_tags_path(
        &mut self,
        path: PathBuf,
        workspace_root: PathBuf,
    ) {
        if let Some((_route_id, node)) =
            self.context_manager.neoism_tags_node_by_path(&path)
        {
            let _ = self
                .context_manager
                .current_grid_mut()
                .set_current_node(node, &mut self.sugarloaf);
            self.context_manager.select_route_from_current_grid();
            return;
        }

        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        if !self.context_manager.add_stacked_neoism_tags(
            path,
            workspace_root,
            rich_text_id,
            &mut self.sugarloaf,
        ) {
            self.file_tree_notify(
                "Could not open Tags pane",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
    }

    pub(crate) fn render_neoism_tags_panels(&mut self) -> bool {
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
            .then_some([self.mouse.x as f32 / scale, self.mouse.y as f32 / scale]);
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

        let mut needs_redraw = false;
        for (key, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if !visible_nodes.contains(key) {
                continue;
            }
            let Some(tags) = item.val.neoism_tags.as_mut() else {
                continue;
            };
            needs_redraw |= tags.refresh_if_needed();
            let rect = [
                (scaled_margin.left + item.layout_rect[0]) / scale,
                (scaled_margin.top + item.layout_rect[1]) / scale,
                item.layout_rect[2] / scale,
                item.layout_rect[3] / scale,
            ];
            tags.render(
                &mut self.sugarloaf,
                rect,
                &theme,
                mouse,
                chrome_scale,
                &text_occlusions,
            );
        }
        needs_redraw
    }

    pub(crate) fn handle_neoism_tags_mouse_press(&mut self, button: MouseButton) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        if self.context_manager.current().neoism_tags.is_none() {
            return false;
        }
        let [x, y] = self.markdown_mouse_logical();
        let Some(action) = self
            .context_manager
            .current_mut()
            .neoism_tags
            .as_mut()
            .and_then(|tags| tags.click_at(x, y))
        else {
            return false;
        };

        match action {
            TagsViewAction::ToggleTag(_) => {
                self.mark_dirty();
                true
            }
            TagsViewAction::OpenFile { path, line } => {
                self.open_path_in_markdown(path);
                if let Some(markdown) =
                    self.context_manager.current_mut().markdown.as_mut()
                {
                    markdown.jump_to_line(line.max(1));
                    markdown.flash_line(line.max(1));
                }
                self.renderer.trail_cursor.reset();
                self.mark_dirty();
                true
            }
        }
    }

    pub(crate) fn mark_neoism_tags_views_stale(&mut self, root: &Path) {
        let root = Self::normalize_workspace_root(root.to_path_buf());
        for grid in self.context_manager.all_grids_mut() {
            for item in grid.contexts_mut().values_mut() {
                let Some(tags) = item.val.neoism_tags.as_mut() else {
                    continue;
                };
                let pane_root =
                    Self::normalize_workspace_root(tags.workspace_root().to_path_buf());
                if pane_root == root {
                    tags.mark_stale();
                }
            }
        }
    }
}
