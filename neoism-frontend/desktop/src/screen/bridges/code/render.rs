use super::*;

impl Screen<'_> {
    pub(crate) fn render_code_panels(&mut self) -> bool {
        self.pump_code_lsp();
        let scale = self.sugarloaf.scale_factor();
        let theme = self.renderer.theme;
        let font_scale = self.renderer.chrome_scale();
        let window_size = self.sugarloaf.window_size();
        let text_occlusions = self.renderer.active_text_occlusion_rects(
            window_size.width,
            window_size.height,
            scale,
        );
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
        let mut any_animating = false;
        let mouse = Some(self.markdown_mouse_logical());
        // Snapshot remote collaborator carets per visible code pane
        // BEFORE the mutable grid borrow (presence store and grid are
        // both fields of `self`) — same dance as the markdown bridge.
        let mut remote_by_path: std::collections::HashMap<
            std::path::PathBuf,
            Vec<neoism_ui::editor::markdown::MarkdownRemoteCursor>,
        > = std::collections::HashMap::new();
        {
            let grid = self.context_manager.current_grid();
            let pane_buffers: Vec<(std::path::PathBuf, String)> = grid
                .contexts()
                .iter()
                .filter(|(key, _)| visible_nodes.contains(key))
                .filter_map(|(_, item)| {
                    item.val.code.as_ref().map(|pane| {
                        (
                            pane.path.clone(),
                            crate::screen::markdown_crdt::buffer_id_for_markdown_path(
                                &pane.path,
                            ),
                        )
                    })
                })
                .collect();
            for (path, buffer_id) in pane_buffers {
                let cursors = self
                    .remote_presence
                    .cursors_for(&buffer_id)
                    .map(
                        |presence| neoism_ui::editor::markdown::MarkdownRemoteCursor {
                            name: presence.display_name.clone(),
                            color: [
                                presence.color.r,
                                presence.color.g,
                                presence.color.b,
                            ],
                            rainbow: presence.rainbow,
                            line: presence.cursor.line as usize,
                            col_utf16: presence.cursor.column as usize,
                        },
                    )
                    .collect::<Vec<_>>();
                remote_by_path.insert(path, cursors);
            }
        }
        let grid = self.context_manager.current_grid_mut();
        for (node, item) in grid.contexts_mut().iter_mut() {
            if !visible_nodes.contains(node) {
                continue;
            }
            let rect = [
                (scaled_margin.left + item.layout_rect[0]) / scale,
                (scaled_margin.top + item.layout_rect[1]) / scale,
                item.layout_rect[2] / scale,
                item.layout_rect[3] / scale,
            ];
            let Some(code) = item.val.code.as_mut() else {
                continue;
            };
            // Desktop's chrome trail cursor draws the caret (it glides
            // between panels and into the buffer); the pane only
            // publishes `cursor_rect`.
            code.caret_drawn_by_host = true;
            code.remote_cursors =
                remote_by_path.remove(&code.path).unwrap_or_default();
            let animating = neoism_ui::editor::code::render::render(
                &mut self.sugarloaf,
                code,
                rect,
                &theme,
                &text_occlusions,
                font_scale,
                mouse,
            );
            any_animating |= animating;
        }
        // Keep frames coming while a mouse-hover candidate matures
        // toward its request delay.
        any_animating |= self
            .renderer
            .code_lsp
            .mouse_hover
            .as_ref()
            .is_some_and(|cand| !cand.requested);
        any_animating
    }
}
