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
