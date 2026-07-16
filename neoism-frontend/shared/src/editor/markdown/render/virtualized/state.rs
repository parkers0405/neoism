struct VirtualMarkdownDrawItem {
    node: NodeId,
    revision: NodeRevision,
    kind: VirtualNodeKind,
    text_hash: u64,
    bounds: VirtualBounds,
    screen_y: f32,
    first_line: usize,
    line_count: usize,
    text: String,
    measured_layout: bool,
}

#[derive(Clone, Copy, Debug)]
enum LargeLineStartsEdit {
    Insert { line: usize, byte_delta: i64 },
    Delete { line: usize, byte_delta: i64 },
}

fn virtual_markdown_kind_tag(kind: &VirtualNodeKind) -> u8 {
    match kind {
        VirtualNodeKind::Heading => 1,
        VirtualNodeKind::CodeBlock => 2,
        VirtualNodeKind::Table => 3,
        VirtualNodeKind::MarkdownBlock => 4,
        _ => 255,
    }
}

impl MarkdownVirtualRenderState {
    fn line_for_byte(&self, byte: usize) -> usize {
        match self.line_starts.binary_search(&byte) {
            Ok(line) => line,
            Err(next) => next.saturating_sub(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    use super::{
        appended_line_segment, prepare_large_line_surface, tail_inline_append_suffix,
    };
    use crate::editor::markdown::vim::VimState;
    use crate::editor::markdown::{MarkdownCodeFenceCache, MarkdownVirtualRenderState};
    use crate::editor::markdown::{MarkdownMode, MarkdownPane};

    #[test]
    fn inline_word_tokenizers_keep_trailing_whitespace() {
        // A just-typed trailing space must reach the wrap rows or the
        // caret can't advance past it until the next char lands.
        for words in [
            super::plain_inline_words_for_text("- hello "),
            super::inline_words_for_text("hello  "),
        ] {
            let last = words.last().expect("trailing-space word");
            assert!(last.text.is_empty());
            assert!(last.lead_ws > 0);
        }
        // No trailing whitespace → no synthetic word.
        let words = super::plain_inline_words_for_text("- hello");
        assert_eq!(words.last().unwrap().text, "hello");
    }

    #[test]
    fn wrapped_continuation_rows_reserve_hanging_indent_width() {
        let first = super::inline_line_for_row(0, 100.0, 18.0, 0, 0.0, 0);
        let continued = super::inline_line_for_row(1, 100.0, 18.0, 0, 0.0, 0);
        let dropped = super::inline_line_for_row(1, 100.0, 18.0, 3, 12.0, 0);

        assert_eq!(first.row_width, 100.0);
        assert_eq!(continued.row_width, 82.0);
        assert_eq!(dropped.row_width, 70.0);
    }

    #[test]
    fn virtual_item_lines_preserve_empty_source_line() {
        assert_eq!(super::virtual_item_lines(""), vec![String::new()]);
        assert_eq!(
            super::virtual_item_lines("alpha\nbeta"),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[test]
    fn heading_marker_len_includes_marker_space() {
        assert_eq!(super::heading_marker_len("# Title", 1), 2);
        assert_eq!(super::heading_marker_len("###    Deep", 3), 4);
        assert_eq!(super::heading_marker_len("#", 1), 1);
    }

    #[test]
    fn indented_heading_keeps_level_and_marker() {
        // The surface adapter trims before classifying, so "  ### x" IS a
        // heading node — the untrimmed take_while('#') read 0 hashes and
        // clamp() called it h1 with the hashes drawn as text.
        assert_eq!(super::heading_node_level_and_marker("### Title"), (3, 4));
        assert_eq!(super::heading_node_level_and_marker("  ### Title"), (3, 6));
        assert_eq!(super::heading_node_level_and_marker("\t## Title"), (2, 4));
    }

    #[test]
    fn appended_line_segment_accepts_whole_new_lines_only() {
        assert_eq!(
            appended_line_segment("alpha", "alpha\nbeta\ngamma", 1),
            Some(("beta\ngamma", 6, 1))
        );
        assert_eq!(appended_line_segment("alpha", "alphabet", 1), None);
        assert_eq!(
            appended_line_segment("", "first\nsecond", 1),
            Some(("first\nsecond", 0, 0))
        );
    }

    #[test]
    fn tail_inline_append_suffix_detects_bottom_typing_without_join_rebuild() {
        let mut pane = MarkdownPane {
            path: PathBuf::from("tail.md"),
            remote_loading_started: None,
            value_picker_suppressed: None,
            remote_content_pending: false,
            cover_overlay_rect: None,
            value_picker: None,
            available_covers: Vec::new(),
            title_edit: None,
            pending_title_rename: None,
            title: "tail".to_string(),
            lines: vec!["alpha".to_string(), "betaz".to_string()],
            blocks: Vec::new(),
            source_len_bytes: "alpha\nbetaz".len(),
            source_revision: 2,
            block_wrap_rows: std::collections::HashMap::new(),
            block_wrap_hit_stops: std::collections::HashMap::new(),
            pending_line_edit: None,
            mode: MarkdownMode::Normal,
            cursor_line: 1,
            cursor_col: 5,
            visual_anchor: None,
            mouse_select_anchor: None,
            cursor_rect: None,
            follow_cursor: false,
            goal_visual_col: None,
            scroll_y: 0.0,
            target_scroll_y: 0.0,
            cursor_scroll_remainder: 0.0,
            scroll_viewport_height: 0.0,
            scroll_velocity_px_s: 0.0,
            scroll_velocity_moves_cursor: false,
            remote_cursors: Vec::new(),
            scroll_last_tick_at: None,
            content_height: 0.0,
            block_rects: Vec::new(),
            notebook_image_preview_dimensions: std::collections::HashMap::new(),
            table_rects: Vec::new(),
            table_cell_rects: Vec::new(),
            table_action_rects: Vec::new(),
            task_rects: Vec::new(),
            roster_rects: Vec::new(),
            pending_reveal_line: None,
            outline_rects: Vec::new(),
            table_scrollbar_rects: Vec::new(),
            link_rects: Vec::new(),
            copy_rects: Vec::new(),
            notebook_run_rects: Vec::new(),
            notebook_action_hovered: None,
            table_scroll_x: HashMap::new(),
            task_toggle_animations: HashMap::new(),
            yank_flashes: Vec::new(),
            enter_continuation_lines: HashSet::new(),
            hovered_line: None,
            dragging_line: None,
            dragging_table_scroll: None,
            scrollbar_rect: None,
            dragging_scrollbar: None,
            scrollbar_hovered: false,
            table_action_hovered: false,
            drag_mouse_y: 0.0,
            drag_start_y: 0.0,
            drag_moved: false,
            drag_drop_flash: None,
            pending_block_menu_rect: None,
            vim: VimState::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            doc_history_bound: false,
            pending_doc_history: Vec::new(),
            wrap_cache: std::cell::RefCell::new(HashMap::new()),
            code_fence_cache: std::cell::RefCell::new(MarkdownCodeFenceCache::default()),
            link_target_cache: std::cell::RefCell::new(HashMap::new()),
            virtual_render: MarkdownVirtualRenderState::default(),
            saved_baseline: vec![String::new()],
            error: None,
        };
        pane.virtual_render.source_id = "tail.md".to_string();
        pane.virtual_render.source = "alpha\nbeta".to_string();
        pane.virtual_render.line_starts = vec![0, 6];

        assert_eq!(
            tail_inline_append_suffix(&pane, "tail.md"),
            Some("z".to_string())
        );
    }

    #[test]
    fn single_line_count_edits_rebase_large_surface_tail_metadata() {
        let source = (0..96)
            .map(|line| format!("plain line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut pane = MarkdownPane::from_source(PathBuf::from("line-edit.md"), &source);
        assert!(prepare_large_line_surface(&mut pane, "line-edit.md", 400.0));

        let before = pane
            .virtual_render
            .surface
            .nodes()
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        assert!(before.len() >= 3);

        let inserted = "inserted line".to_string();
        let insert_delta = inserted.len() as i64 + 1;
        pane.lines.insert(40, inserted);
        pane.adjust_source_len(insert_delta as isize);
        pane.record_line_insert(40, insert_delta);
        pane.source_revision = pane.source_revision.saturating_add(1);
        assert!(prepare_large_line_surface(&mut pane, "line-edit.md", 400.0));

        let nodes = pane.virtual_render.surface.nodes();
        assert_eq!(nodes.len(), before.len());
        assert_eq!(nodes[2].id, before[2]);
        assert_eq!(nodes[1].content.as_ref().unwrap().line_count, 33);
        assert_eq!(nodes[2].content.as_ref().unwrap().line_start, 65);

        let removed = pane.lines.remove(40);
        let delete_delta = -((removed.len() + 1) as i64);
        pane.adjust_source_len(delete_delta as isize);
        pane.record_line_delete(40, delete_delta);
        pane.source_revision = pane.source_revision.saturating_add(1);
        assert!(prepare_large_line_surface(&mut pane, "line-edit.md", 400.0));

        let nodes = pane.virtual_render.surface.nodes();
        assert_eq!(nodes.len(), before.len());
        assert_eq!(nodes[2].id, before[2]);
        assert_eq!(nodes[1].content.as_ref().unwrap().line_count, 32);
        assert_eq!(nodes[2].content.as_ref().unwrap().line_start, 64);
    }
}
