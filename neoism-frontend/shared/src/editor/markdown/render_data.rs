use super::helpers::*;
use super::types::*;

fn notebook_image_preview_display_size(
    width: f32,
    height: f32,
    available_width: f32,
    font_scale: f32,
) -> (f32, f32) {
    if width <= 0.0 || height <= 0.0 {
        return (0.0, 0.0);
    }
    let max_w = available_width.min(640.0 * font_scale).max(1.0);
    let max_h = 360.0 * font_scale;
    let fit = (max_w / width).min(max_h / height).min(1.0);
    ((width * fit).max(1.0), (height * fit).max(1.0))
}

impl MarkdownPane {
    pub fn begin_block_layout(&mut self) {
        self.block_rects.clear();
        self.block_wrap_rows.clear();
        self.block_wrap_hit_stops.clear();
        self.table_rects.clear();
        self.table_cell_rects.clear();
        self.table_action_rects.clear();
        self.task_rects.clear();
        self.roster_rects.clear();
        self.outline_rects.clear();
        self.table_scrollbar_rects.clear();
        self.link_rects.clear();
        self.copy_rects.clear();
        self.notebook_run_rects.clear();
        self.hovered_line = None;
        self.scrollbar_rect = None;
        self.scrollbar_hovered = false;
        self.table_action_hovered = false;
    }

    pub fn set_notebook_image_preview_dimensions<I>(&mut self, dimensions: I)
    where
        I: IntoIterator<Item = (usize, u32, u32)>,
    {
        let mut next = std::collections::HashMap::new();
        for (line, width, height) in dimensions {
            if width == 0 || height == 0 {
                continue;
            }
            next.entry(line)
                .and_modify(|current: &mut (u32, u32)| {
                    let current_area =
                        u64::from(current.0).saturating_mul(u64::from(current.1));
                    let next_area = u64::from(width).saturating_mul(u64::from(height));
                    if next_area > current_area {
                        *current = (width, height);
                    }
                })
                .or_insert((width, height));
        }
        if self.notebook_image_preview_dimensions != next {
            self.notebook_image_preview_dimensions = next;
            self.virtual_render.measurement_cache.clear();
        }
    }

    pub fn clear_notebook_image_preview_dimensions(&mut self) {
        if !self.notebook_image_preview_dimensions.is_empty() {
            self.notebook_image_preview_dimensions.clear();
            self.virtual_render.measurement_cache.clear();
        }
    }

    pub fn notebook_image_preview_dimensions_for_line(
        &self,
        line: usize,
    ) -> Option<(u32, u32)> {
        self.notebook_image_preview_dimensions.get(&line).copied()
    }

    pub(super) fn notebook_image_preview_extra_h(
        &self,
        line: usize,
        available_width: f32,
        font_scale: f32,
    ) -> f32 {
        let Some((width, height)) = self.notebook_image_preview_dimensions_for_line(line)
        else {
            return 0.0;
        };
        let (_, preview_h) = notebook_image_preview_display_size(
            width as f32,
            height as f32,
            available_width,
            font_scale,
        );
        8.0 * font_scale + preview_h
    }

    pub fn cached_wrap_lines(&self, key: &MarkdownWrapKey) -> Option<Vec<String>> {
        self.wrap_cache.borrow().get(key).cloned()
    }

    pub fn store_wrap_lines(&self, key: MarkdownWrapKey, lines: Vec<String>) {
        const MAX_MARKDOWN_WRAP_CACHE: usize = 4096;
        let mut cache = self.wrap_cache.borrow_mut();
        if cache.len() >= MAX_MARKDOWN_WRAP_CACHE {
            cache.clear();
        }
        cache.insert(key, lines);
    }

    pub fn register_table_rect(
        &mut self,
        start_line: usize,
        rect: [f32; 4],
        viewport_width: f32,
        content_width: f32,
    ) {
        let max_scroll = (content_width - viewport_width).max(0.0);
        self.table_scroll_x
            .entry(start_line)
            .and_modify(|x| *x = x.clamp(0.0, max_scroll));
        self.table_rects.push(MarkdownTableRect {
            start_line,
            rect,
            viewport_width,
            content_width,
        });
    }

    pub(super) fn register_table_cell_rect(
        &mut self,
        line: usize,
        cell_ix: usize,
        rect: [f32; 4],
        text_x: f32,
        text_y: f32,
        text_width: f32,
        cell_width: f32,
        line_height: f32,
        hit_rows: Vec<MarkdownWrapHitRow>,
    ) {
        self.table_cell_rects.push(MarkdownTableCellRect {
            line,
            cell_ix,
            rect,
            text_x,
            text_y,
            text_width,
            cell_width,
            line_height,
            hit_rows,
        });
    }

    pub fn register_table_add_row_rect(
        &mut self,
        after_line: usize,
        rect: [f32; 4],
        mouse: Option<[f32; 2]>,
    ) -> bool {
        self.register_table_action_rect(
            MarkdownTableAction::AddRowBelow { after_line },
            rect,
            mouse,
        )
    }

    pub fn register_table_add_column_rect(
        &mut self,
        start_line: usize,
        col_ix: usize,
        rect: [f32; 4],
        mouse: Option<[f32; 2]>,
    ) -> bool {
        self.register_table_action_rect(
            MarkdownTableAction::AddColumn { start_line, col_ix },
            rect,
            mouse,
        )
    }

    /// Record the per-visual-row visible char counts for a wrapped source
    /// line. This compatibility path assumes one consumed source-space between
    /// rendered rows; renderers that know exact starts should use
    /// `register_block_wrap_row_spans`.
    pub fn register_block_wrap_rows(&mut self, line: usize, rows: Vec<usize>) {
        let mut start = 0usize;
        let rows = rows
            .into_iter()
            .map(|len| {
                let row = MarkdownWrapRow { start, len };
                start = start.saturating_add(len).saturating_add(1);
                row
            })
            .collect::<Vec<_>>();
        self.register_block_wrap_row_spans(line, rows);
    }

    pub(super) fn register_block_wrap_row_spans(
        &mut self,
        line: usize,
        rows: Vec<MarkdownWrapRow>,
    ) {
        if !rows.is_empty() {
            self.block_wrap_rows.insert(line, rows);
        } else {
            self.block_wrap_rows.remove(&line);
        }
        self.block_wrap_hit_stops.remove(&line);
    }

    pub(super) fn register_block_wrap_hit_stops(
        &mut self,
        line: usize,
        rows: Vec<MarkdownWrapHitRow>,
    ) {
        if !rows.is_empty() {
            self.block_wrap_hit_stops.insert(line, rows);
        } else {
            self.block_wrap_hit_stops.remove(&line);
        }
    }

    pub fn register_task_rect(&mut self, line: usize, rect: [f32; 4]) {
        self.task_rects.push(MarkdownTaskRect { line, rect });
    }

    /// Wave 7G: record a "who's here" roster dot's hit rect for this
    /// frame. `line` is the peer's 0-based cursor line (jump target).
    pub fn register_roster_rect(&mut self, rect: [f32; 4], line: usize) {
        self.roster_rects.push(MarkdownRosterRect { rect, line });
    }

    pub fn register_table_scrollbar_rect(
        &mut self,
        start_line: usize,
        track_rect: [f32; 4],
        thumb_rect: [f32; 4],
        viewport_width: f32,
        content_width: f32,
    ) {
        self.table_scrollbar_rects.push(MarkdownTableScrollbarRect {
            start_line,
            track_rect,
            thumb_rect,
            viewport_width,
            content_width,
        });
    }

    pub fn register_scrollbar_rect(
        &mut self,
        track_rect: [f32; 4],
        thumb_rect: [f32; 4],
        viewport_height: f32,
        mouse: Option<[f32; 2]>,
    ) {
        self.scrollbar_rect = Some(MarkdownScrollbarRect {
            track_rect,
            thumb_rect,
            viewport_height,
        });
        if mouse.is_some_and(|[x, y]| markdown_scrollbar_hit(x, y, track_rect)) {
            self.scrollbar_hovered = true;
        }
    }

    /// Progress (0..1) of the post-drop flash on the block that just landed
    /// from a handle drag, clearing it once elapsed — `task_toggle_progress`'s
    /// pattern, keyed by the moved source-line range.
    pub fn drag_drop_flash_progress(&mut self) -> Option<(std::ops::Range<usize>, f32)> {
        const DRAG_DROP_FLASH: web_time::Duration = web_time::Duration::from_millis(550);
        let (range, started) = self.drag_drop_flash.clone()?;
        let elapsed = web_time::Instant::now().saturating_duration_since(started);
        if elapsed >= DRAG_DROP_FLASH {
            self.drag_drop_flash = None;
            return None;
        }
        Some((range, elapsed.as_secs_f32() / DRAG_DROP_FLASH.as_secs_f32()))
    }

    pub fn task_toggle_progress(&mut self, line: usize) -> Option<f32> {
        let started = *self.task_toggle_animations.get(&line)?;
        let elapsed = web_time::Instant::now().saturating_duration_since(started);
        if elapsed >= TASK_TOGGLE_ANIMATION {
            self.task_toggle_animations.remove(&line);
            return None;
        }
        Some(elapsed.as_secs_f32() / TASK_TOGGLE_ANIMATION.as_secs_f32())
    }

    pub fn is_enter_continuation_line(&self, line: usize) -> bool {
        self.enter_continuation_lines.contains(&line)
    }

    /// The `title:` property from the frontmatter block, when present —
    /// shown as the note's big inline title (and editable right there in
    /// the metadata section like any other line).
    pub(super) fn frontmatter_title(&self) -> Option<String> {
        self.frontmatter_property("title")
    }

    /// A frontmatter property's trimmed value (quotes stripped), when the
    /// note has a properties block and the key is present and non-empty.
    pub fn frontmatter_property(&self, key: &str) -> Option<String> {
        let fm = self.frontmatter_range()?;
        self.lines[fm].iter().find_map(|line| {
            let (k, value) = line.split_once(':')?;
            if !k.trim().eq_ignore_ascii_case(key) {
                return None;
            }
            let value = value.trim().trim_matches('"').trim_matches('\'');
            (!value.is_empty()).then(|| value.to_string())
        })
    }

    /// Notion-style page icon: the `icon:` property (an emoji), rendered
    /// large above the title.
    pub fn frontmatter_icon(&self) -> Option<String> {
        self.frontmatter_property("icon")
    }

    /// Notion-style cover banner: the `cover:` property — a bundled cover
    /// name (resolved against the covers directory by the host) or a
    /// path. Rendered edge-to-edge above the icon/title.
    pub fn frontmatter_cover(&self) -> Option<String> {
        self.frontmatter_property("cover")
    }

    /// YAML-frontmatter line range: a `---` fence pair starting at line 0,
    /// rendered as Obsidian-style properties (keys, values, tag chips)
    /// instead of raw dashes + text. The cursor's line still reveals raw.
    pub(super) fn frontmatter_range(&self) -> Option<std::ops::Range<usize>> {
        if self.lines.first().map(|line| line.trim()) != Some("---") {
            return None;
        }
        let end = self
            .lines
            .iter()
            .enumerate()
            .skip(1)
            .take(64)
            .find(|(_, line)| line.trim() == "---")?
            .0;
        Some(0..end + 1)
    }

    pub fn register_copy_lines_rect(&mut self, rect: [f32; 4], start: usize, end: usize) {
        self.copy_rects.push(MarkdownCopyRect {
            rect,
            kind: MarkdownCopyKind::Lines { start, end },
        });
    }

    pub fn register_copy_code_rect(&mut self, rect: [f32; 4], start: usize, end: usize) {
        self.copy_rects.push(MarkdownCopyRect {
            rect,
            kind: MarkdownCopyKind::Code { start, end },
        });
    }

    pub fn register_notebook_run_rect(&mut self, rect: [f32; 4], cell_index: usize) {
        self.register_notebook_action_rect(
            rect,
            cell_index,
            crate::editor::notebook::NotebookCellAction::Run,
        );
    }

    pub fn register_notebook_action_rect(
        &mut self,
        rect: [f32; 4],
        cell_index: usize,
        action: crate::editor::notebook::NotebookCellAction,
    ) {
        self.notebook_run_rects.push(MarkdownNotebookActionRect {
            rect,
            cell_index,
            action,
        });
    }

    pub fn register_link_rect(&mut self, rect: [f32; 4], target: MarkdownLinkTarget) {
        self.link_rects.push(MarkdownLinkRect { rect, target });
    }

    pub fn register_block_rect(
        &mut self,
        line: usize,
        rect: [f32; 4],
        handle_rect: [f32; 4],
        text_x: f32,
        text_y: f32,
        marker_len: usize,
        cell_width: f32,
        line_height: f32,
        wrap_width: f32,
        mouse: Option<[f32; 2]>,
    ) -> bool {
        let convert_rect = block_convert_rect(rect);
        self.block_rects.push(MarkdownBlockRect {
            line,
            rect,
            handle_rect,
            convert_rect,
            text_x,
            text_y,
            marker_len,
            cell_width,
            line_height,
            wrap_width,
        });
        let hovered = mouse.is_some_and(|[x, y]| {
            point_in_rect(x, y, rect)
                || point_in_rect(x, y, handle_rect)
                || point_in_rect(x, y, convert_rect)
        });
        if hovered && self.hovered_line.is_none() {
            self.hovered_line = Some(line);
        }
        hovered || self.dragging_line == Some(line)
    }

    pub fn block_rect_for_source_line(&self, line: usize) -> Option<MarkdownBlockRect> {
        self.block_rects
            .iter()
            .rev()
            .find(|block| block.line == line)
            .copied()
    }

    pub(super) fn register_table_action_rect(
        &mut self,
        action: MarkdownTableAction,
        rect: [f32; 4],
        mouse: Option<[f32; 2]>,
    ) -> bool {
        let hovered = mouse.is_some_and(|[x, y]| point_in_rect(x, y, rect));
        if hovered {
            self.table_action_hovered = true;
        }
        self.table_action_rects
            .push(MarkdownTableActionRect { rect, action });
        hovered
    }
}
