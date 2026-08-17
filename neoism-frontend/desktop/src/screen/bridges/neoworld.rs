use super::super::*;
use neoism_ui::panels::buffer_tabs::ChromePageKind;
use neoism_window::event::MouseButton;

impl Screen<'_> {
    pub(crate) fn open_neoworld_page(&mut self) {
        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.renderer.file_tree.set_active_path(None);
        self.activate_neoworld_page();
        let route_id = self
            .context_manager
            .neoworld_node()
            .map(|(route_id, _)| route_id)
            .unwrap_or(0);
        self.renderer
            .buffer_tabs
            .open_chrome_page(ChromePageKind::NeoWorld, route_id);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    pub(crate) fn activate_neoworld_page(&mut self) {
        if let Some((_route_id, node)) = self.context_manager.neoworld_node() {
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
            .add_stacked_neoworld(rich_text_id, &mut self.sugarloaf)
        {
            self.file_tree_notify(
                "Could not open NeoWorld",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
    }

    pub(crate) fn render_neoworld_panels(&mut self) -> bool {
        let scale = self.sugarloaf.scale_factor();
        let theme = self.renderer.theme;
        let chrome_scale = self.renderer.chrome_scale();
        let status_h = self.renderer.status_line_height();
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
            let Some(pane) = item.val.neoworld.as_mut() else {
                continue;
            };
            let rect = [
                (scaled_margin.left + item.layout_rect[0]) / scale,
                (scaled_margin.top + item.layout_rect[1]) / scale,
                item.layout_rect[2] / scale,
                (item.layout_rect[3] / scale - status_h).max(0.0),
            ];
            pane.render(&mut self.sugarloaf, rect, &theme, chrome_scale);
            if let Some(snapshot) = pane.take_periodic_snapshot() {
                crate::neoworld_runtime::persist(snapshot);
            }
            painted = true;
        }
        painted
    }

    pub(crate) fn handle_neoworld_pointer_down(&mut self, button: MouseButton) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let [x, y] = self.markdown_mouse_logical();
        let Some(pane) = self.context_manager.current_mut().neoworld.as_mut() else {
            return false;
        };
        let consumed = pane.pointer_down(x, y);
        if consumed {
            self.mark_dirty();
        }
        consumed
    }

    pub(crate) fn handle_neoworld_pointer_move(&mut self) -> bool {
        let [x, y] = self.markdown_mouse_logical();
        let Some(pane) = self.context_manager.current_mut().neoworld.as_mut() else {
            return false;
        };
        let consumed = pane.pointer_move(x, y);
        if consumed {
            self.mark_dirty();
        }
        consumed
    }

    pub(crate) fn handle_neoworld_pointer_up(&mut self, button: MouseButton) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let [x, y] = self.markdown_mouse_logical();
        let Some(pane) = self.context_manager.current_mut().neoworld.as_mut() else {
            return false;
        };
        let consumed = pane.pointer_up(x, y);
        if consumed {
            crate::neoworld_runtime::persist(*pane.pet());
            self.mark_dirty();
        }
        consumed
    }
}
