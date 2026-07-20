use super::*;

impl Screen<'_> {
    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }

    pub(crate) fn selection_click_kind(click_state: &ClickState) -> SelectionClickKind {
        match click_state {
            ClickState::Click => SelectionClickKind::Single,
            ClickState::DoubleClick => SelectionClickKind::Double,
            ClickState::TripleClick => SelectionClickKind::Triple,
            ClickState::None => SelectionClickKind::None,
        }
    }

    pub(crate) fn selection_modifiers(&self) -> SelectionModifiers {
        let mods = self.modifiers.state();
        SelectionModifiers::new(mods.shift_key(), mods.control_key(), mods.alt_key())
    }

    pub fn reset_mouse(&mut self) {
        self.mouse.accumulated_scroll = crate::input::mouse::AccumulatedScroll::default();
    }

    pub fn select_current_based_on_mouse(&mut self) -> bool {
        if self
            .context_manager
            .current_grid_mut()
            .select_current_based_on_mouse(&self.mouse)
        {
            self.context_manager.select_route_from_current_grid();
            self.renderer.file_tree.set_focused(false);
            self.renderer.trail_cursor.reset();
            self.reapply_chrome_layout();
            self.mark_dirty();
            return true;
        }
        false
    }

    pub(crate) fn current_visual_mouse_position(&self) -> (ContextDimension, Pos) {
        let (context_dimension, margin) = {
            let current_grid = self.context_manager.current_grid();
            let (context, margin) =
                current_grid.current_context_with_computed_dimension();
            (context.dimension, margin)
        };
        let visual_pos = calculate_mouse_position(
            &self.mouse,
            0,
            (context_dimension.columns, context_dimension.lines),
            margin.left,
            margin.top,
            (
                context_dimension.dimension.width,
                context_dimension.dimension.height,
            ),
        );

        (context_dimension, visual_pos)
    }

    pub fn terminal_body_mouse_position(&self, display_offset: usize) -> Option<Pos> {
        let (context_dimension, mut visual_pos) = self.current_visual_mouse_position();
        if let Some(item) = self.context_manager.current_grid().current_item() {
            let ctx = &item.val;
            if ctx.code.is_none()
                && ctx.markdown.is_none()
                && ctx.neoism_agent.is_none()
                && ctx.neoism_tags.is_none()
            {
                let scale = self.sugarloaf.scale_factor();
                let cell_h_logical =
                    (ctx.dimension.dimension.height.round().max(1.0) / scale).max(1.0);
                let panel_top_logical = (item.layout_rect[1]
                    + self.context_manager.current_grid().get_scaled_margin().top)
                    / scale;
                let terminal_scroll_offset_logical = self
                    .renderer
                    .terminal_scroll
                    .current_offset(ctx.rich_text_id)
                    / scale;
                let mouse_y_logical = self.mouse.y as f32 / scale;
                let row = terminal_body_visual_row(TerminalBodyMouseRowInput {
                    panel_top_logical,
                    terminal_scroll_offset_logical,
                    cell_height_logical: cell_h_logical,
                    mouse_y_logical,
                });
                visual_pos.row = Line(row);
            }
        }

        match self.terminal_block_source_row_at_visual_row(
            visual_pos.row.0.max(0) as usize,
            context_dimension.columns,
            context_dimension.lines,
        ) {
            Some(Some(source_row)) => Some(Pos::new(source_row, visual_pos.col)),
            Some(None) => None,
            None => Some(Pos::new(visual_pos.row - display_offset, visual_pos.col)),
        }
    }

    pub fn mouse_position(&self, display_offset: usize) -> Pos {
        self.terminal_body_mouse_position(display_offset)
            .unwrap_or_else(|| {
                let visual_pos = self.current_visual_mouse_position().1;
                Pos::new(visual_pos.row - display_offset, visual_pos.col)
            })
    }

    pub fn mouse_mode(&self) -> bool {
        let mode = self.get_mode();
        mode.intersects(Mode::MOUSE_MODE) && !mode.contains(Mode::VI)
    }
}
