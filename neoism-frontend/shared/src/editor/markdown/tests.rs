#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        path::PathBuf,
    };

    use super::super::helpers::*;
    use super::super::links::markdown_link_open_action;
    use super::super::source_map::InlineSourceMap;
    use super::super::types::*;
    use super::super::vim::VimState;

    fn pane_for_test() -> MarkdownPane {
        MarkdownPane {
            path: PathBuf::from("test.md"),
            remote_loading_started: None,
            value_picker_suppressed: None,
            remote_content_pending: false,
            cover_overlay_rect: None,
            value_picker: None,
            available_covers: Vec::new(),
            title_edit: None,
            pending_title_rename: None,
            title: "test".to_string(),
            lines: vec![String::new()],
            blocks: Vec::new(),
            source_len_bytes: 0,
            source_revision: 1,
            pending_line_edit: None,
            mode: MarkdownMode::Normal,
            cursor_line: 0,
            cursor_col: 0,
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
            notebook_image_preview_dimensions: HashMap::new(),
            block_wrap_rows: HashMap::new(),
            block_wrap_hit_stops: HashMap::new(),
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
        }
    }

    fn hit_row(start: usize, stops: &[f32]) -> MarkdownWrapHitRow {
        MarkdownWrapHitRow {
            start,
            stops: stops.to_vec(),
        }
    }

    fn hit_rows(rows: &[(usize, &[f32])]) -> Vec<MarkdownWrapHitRow> {
        rows.iter()
            .map(|(start, stops)| hit_row(*start, stops))
            .collect()
    }

    fn measured_text_line(
        pane: &mut MarkdownPane,
        line: usize,
        visible_chars: usize,
        cell_w: f32,
    ) {
        let stops = (0..=visible_chars)
            .map(|ix| ix as f32 * cell_w)
            .collect::<Vec<_>>();
        pane.register_block_wrap_row_spans(
            line,
            vec![MarkdownWrapRow {
                start: 0,
                len: visible_chars,
            }],
        );
        pane.register_block_wrap_hit_stops(line, vec![hit_row(0, &stops)]);
    }

    #[test]
    fn parses_frontmatter_headings_tasks_code_and_dividers() {
        let blocks = parse_blocks(
            "---\ntitle: Test\n---\n# Project\n\nIntro **bold** text.\n\n- [ ] Ship viewer\n- [x] Wire scroll\n\n---\n\n```rust\nfn main() {}\n```\n",
        );

        assert_eq!(
            blocks,
            vec![
                MarkdownBlock::Heading {
                    level: 1,
                    text: "Project".to_string(),
                },
                MarkdownBlock::Paragraph("Intro **bold** text.".to_string()),
                MarkdownBlock::Task {
                    checked: false,
                    text: "Ship viewer".to_string(),
                },
                MarkdownBlock::Task {
                    checked: true,
                    text: "Wire scroll".to_string(),
                },
                MarkdownBlock::Divider,
                MarkdownBlock::Code {
                    lang: Some("rust".to_string()),
                    code: "fn main() {}".to_string(),
                },
            ]
        );
    }

    #[test]
    fn keeps_unclosed_code_block() {
        let blocks = parse_blocks("```ts\nconst ready = true;\n");

        assert_eq!(
            blocks,
            vec![MarkdownBlock::Code {
                lang: Some("ts".to_string()),
                code: "const ready = true;".to_string(),
            }]
        );
    }

    #[test]
    fn editing_rebuilds_rendered_markdown_live() {
        let mut pane = pane_for_test();

        pane.enter_insert();
        pane.insert_text("### that is header");

        assert_eq!(
            pane.blocks,
            vec![MarkdownBlock::Heading {
                level: 3,
                text: "that is header".to_string(),
            }]
        );
    }

    #[test]
    fn heading_marker_renders_raw_until_space() {
        assert_eq!(
            parse_blocks("###that is header"),
            vec![MarkdownBlock::Paragraph("###that is header".to_string())]
        );
        assert_eq!(
            parse_blocks("### that is header"),
            vec![MarkdownBlock::Heading {
                level: 3,
                text: "that is header".to_string(),
            }]
        );
    }

    #[test]
    fn normal_mode_moves_and_insert_mode_edits() {
        let mut pane = pane_for_test();

        pane.enter_insert();
        pane.insert_text("one\ntwo");
        pane.enter_normal();
        pane.move_up();
        pane.move_line_end();
        pane.enter_insert();
        pane.insert_text("!");

        assert_eq!(pane.lines, vec!["one!".to_string(), "two".to_string()]);
        assert_eq!(pane.mode, MarkdownMode::Insert);
    }

    #[test]
    fn normal_paste_after_inserts_text_after_cursor() {
        let mut pane = pane_for_test();
        pane.lines = vec!["abc".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = 1;
        pane.enter_normal();

        assert!(pane.paste_after("X"));

        assert_eq!(pane.lines, vec!["abXc".to_string()]);
        assert_eq!(pane.cursor_col, 3);
        assert_eq!(pane.mode, MarkdownMode::Normal);
    }

    #[test]
    fn normal_paste_after_inserts_linewise_clipboard_below_cursor_line() {
        let mut pane = pane_for_test();
        pane.lines = vec!["top".to_string(), "bottom".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = 0;
        pane.enter_normal();

        assert!(pane.paste_after("one\ntwo\n"));

        assert_eq!(
            pane.lines,
            vec![
                "top".to_string(),
                "one".to_string(),
                "two".to_string(),
                "bottom".to_string(),
            ]
        );
        assert_eq!((pane.cursor_line, pane.cursor_col), (1, 0));
        assert_eq!(pane.mode, MarkdownMode::Normal);
    }

    #[test]
    fn append_enters_insert_without_crossing_line_at_end() {
        let mut pane = pane_for_test();
        pane.lines = vec!["one".to_string(), "two".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = pane.lines[0].len();
        pane.enter_normal();

        pane.enter_append();

        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, "one".len());
        assert_eq!(pane.mode, MarkdownMode::Insert);
    }

    #[test]
    fn normal_navigation_reaches_revealed_heading_marker() {
        // Live Preview: the cursor's line shows raw `### `, so normal-mode
        // navigation can reach the marker columns.
        let mut pane = pane_for_test();
        pane.lines = vec!["### Heading".to_string()];
        pane.cursor_col = 0;

        pane.enter_normal();
        assert_eq!(pane.cursor_col, 0);

        pane.move_left();
        assert_eq!(pane.cursor_col, 0);

        pane.move_right();
        assert_eq!(pane.cursor_col, 1);
        pane.move_line_start();
        assert_eq!(pane.cursor_col, 0);
    }

    #[test]
    fn normal_navigation_reaches_revealed_markdown_line_suffixes() {
        let mut pane = pane_for_test();
        pane.lines = vec!["### Heading ###".to_string(), "- item   ".to_string()];
        pane.cursor_col = pane.lines[0].len();

        // The revealed cursor line keeps its trailing `###` reachable
        // (trailing whitespace is still trimmed).
        pane.enter_normal();
        assert_eq!(pane.cursor_col, "### Heading ###".len());

        // Stepping onto the next line lands at its raw start (revealed).
        pane.move_right();
        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, 0);

        pane.move_line_end();
        assert_eq!(pane.cursor_col, "- item".len());

        pane.move_right();
        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, "- item".len());
    }

    #[test]
    fn wiki_link_completion_replaces_query_and_keeps_cursor_before_close() {
        let mut pane = pane_for_test();
        pane.lines = vec!["See [[@not]]".to_string()];
        pane.cursor_col = "See [[@not".len();

        let query = pane.wiki_link_query_before_cursor().unwrap();
        assert_eq!(query.query, "not");
        assert_eq!(query.kind, MarkdownWikiLinkKind::CodeRef);
        assert!(pane.apply_wiki_link_completion("notes/page.md"));

        assert_eq!(pane.lines[0], "See [[@notes/page.md]]");
        assert_eq!(pane.cursor_col, "See [[@notes/page.md".len());
    }

    #[test]
    fn markdown_link_open_policy_keeps_side_effects_out_of_shared_state() {
        let note_target = MarkdownLinkTarget {
            path: PathBuf::from("notes/missing.md"),
            line: None,
            code_ref: false,
        };
        assert_eq!(
            markdown_link_open_action(&note_target, false, true, false),
            MarkdownLinkOpenAction::OpenMarkdown {
                create_missing_note: true,
            }
        );

        let code_ref_target = MarkdownLinkTarget {
            path: PathBuf::from("src/lib.md"),
            line: Some(42),
            code_ref: true,
        };
        assert_eq!(
            markdown_link_open_action(&code_ref_target, false, true, false),
            MarkdownLinkOpenAction::OpenMarkdown {
                create_missing_note: false,
            }
        );

        assert_eq!(
            markdown_link_open_action(&note_target, true, true, false),
            MarkdownLinkOpenAction::OpenDirectory
        );
        assert_eq!(
            markdown_link_open_action(&note_target, false, false, false),
            MarkdownLinkOpenAction::OpenEditor
        );
    }

    #[test]
    fn bare_wiki_link_completion_inserts_note_without_code_prefix() {
        let mut pane = pane_for_test();
        pane.lines = vec!["See [[road]]".to_string()];
        pane.cursor_col = "See [[road".len();

        let query = pane.wiki_link_query_before_cursor().unwrap();
        assert_eq!(query.query, "road");
        assert_eq!(query.kind, MarkdownWikiLinkKind::Note);
        assert!(pane.apply_wiki_link_completion("Roadmap.md"));

        assert_eq!(pane.lines[0], "See [[Roadmap.md]]");
        assert_eq!(pane.cursor_col, "See [[Roadmap.md".len());
    }

    #[test]
    fn heading_wiki_link_query_tracks_target_and_heading_text() {
        let mut pane = pane_for_test();
        pane.lines = vec!["See [[Roadmap#No]]".to_string()];
        pane.cursor_col = "See [[Roadmap#No".len();

        let query = pane.wiki_link_query_before_cursor().unwrap();
        assert_eq!(query.kind, MarkdownWikiLinkKind::Heading);
        assert_eq!(query.target.as_deref(), Some("Roadmap"));
        assert_eq!(query.query, "No");
    }

    #[test]
    fn cursor_moves_across_code_fence_edges() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "before".to_string(),
            "```rust".to_string(),
            "code".to_string(),
            "```".to_string(),
            "after".to_string(),
        ];
        pane.cursor_line = 4;
        pane.cursor_col = 0;

        // Code-fence lines are now navigable (they render hidden behind the
        // block header but the caret can still land on them), so vertical
        // motion steps onto each line including the fences.
        pane.move_up();
        assert_eq!(pane.cursor_line, 3);
        pane.move_up();
        assert_eq!(pane.cursor_line, 2);
        pane.move_up();
        assert_eq!(pane.cursor_line, 1);
        pane.move_up();
        assert_eq!(pane.cursor_line, 0);
        pane.move_down();
        assert_eq!(pane.cursor_line, 1);
        pane.move_down();
        assert_eq!(pane.cursor_line, 2);
    }

    #[test]
    fn vertical_motion_preserves_literal_spaces_in_markdown_text() {
        let mut pane = pane_for_test();
        pane.lines = vec!["alpha  beta".to_string(), "gamma  delta".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = "alpha  ".len();

        pane.move_down();

        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, "gamma  ".len());
    }

    #[test]
    fn vertical_motion_from_revealed_cursor_line_uses_raw_columns() {
        // Obsidian Live Preview: the cursor's own line renders RAW, so its
        // visual column includes the markup (`**`). Moving down preserves that
        // raw column — col 4 (`**bo`) lands at col 4 (`plai`) on the next line.
        let mut pane = pane_for_test();
        pane.lines = vec!["**bold** tail".to_string(), "plain tail".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = "**bo".len();

        pane.move_down();

        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, "plai".len());
    }

    #[test]
    fn vertical_motion_from_revealed_cursor_line_clamps_long_raw_column() {
        // The revealed cursor line shows the full wiki-link source, so its raw
        // column can exceed a shorter target line and clamps to that line's end.
        let mut pane = pane_for_test();
        pane.lines = vec![
            "[[docs/Guide.md|Label]] tail".to_string(),
            "plain text".to_string(),
        ];
        pane.cursor_line = 0;
        pane.cursor_col = "[[docs/Guide.md|La".len();

        pane.move_down();

        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, "plain text".len());
    }

    #[test]
    fn wrapped_down_arrow_steps_into_wrapped_visual_row() {
        // A single SOURCE line that the renderer wrapped into THREE visual
        // rows. Down-arrow from row 0 must land on row 1 of the SAME source
        // line (not skip to the next source line).
        let mut pane = pane_for_test();
        pane.lines = vec![
            "alpha bravo charlie delta echo foxtrot golf hotel".to_string(),
            "next paragraph".to_string(),
        ];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 200.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            60.0,
            None,
        );
        // 3 visual rows: "alpha bravo " | "charlie delta " | "echo foxtrot golf hotel"
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 12 },
                MarkdownWrapRow { start: 12, len: 14 },
                MarkdownWrapRow { start: 26, len: 23 },
            ],
        );
        pane.cursor_line = 0;
        pane.cursor_col = 2; // on visual row 0

        pane.move_down();

        assert_eq!(
            pane.cursor_line, 0,
            "down-arrow on a wrapped line must stay on the SAME source line, not skip to the next paragraph"
        );
        assert!(
            pane.cursor_col >= 12,
            "caret should have moved into visual row 1 (col>=12), got {}",
            pane.cursor_col
        );
    }

    #[test]
    fn wrapped_vertical_motion_uses_rendered_inline_columns() {
        let mut pane = pane_for_test();
        pane.lines = vec!["**bold** tail".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 200.0, 40.0],
            [-30.0, 0.0, 20.0, 40.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            40.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 4 },
                MarkdownWrapRow { start: 4, len: 5 },
            ],
        );
        pane.cursor_line = 0;
        pane.cursor_col = "**bold** t".len();

        pane.move_up();

        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, "**bo".len());
    }

    #[test]
    fn mouse_scroll_moves_view_without_cursor_follow_snap() {
        let mut pane = pane_for_test();
        pane.lines = (0..8).map(|line| format!("line {line}")).collect();
        pane.set_content_height(1000.0, 200.0);

        pane.scroll_pixels(-60.0, 200.0);

        assert_eq!(pane.cursor_line, 0);
        assert!(pane.target_scroll_y > 0.0);
        assert!(!pane.follow_cursor);
    }

    #[test]
    fn mouse_scroll_injects_inertia_without_moving_cursor() {
        let mut pane = pane_for_test();
        pane.lines = (0..80).map(|line| format!("line {line}")).collect();
        pane.set_content_height(3200.0, 400.0);

        pane.scroll_pixels(-80.0, 400.0);
        let first_target = pane.target_scroll_y;

        assert!(pane.tick_scroll());
        assert!(pane.target_scroll_y > first_target);
        assert_eq!(pane.cursor_line, 0);
    }

    #[test]
    fn ctrl_scroll_moves_cursor_without_follow_snap() {
        let mut pane = pane_for_test();
        pane.lines = (0..20).map(|line| format!("line {line}")).collect();
        pane.set_content_height(1200.0, 400.0);

        pane.scroll_cursor_by_content_pixels(210.0, 400.0);

        assert!(pane.cursor_line > 0);
        assert!(!pane.follow_cursor);
    }

    #[test]
    fn cursor_scroll_injects_inertial_target_motion() {
        let mut pane = pane_for_test();
        pane.lines = (0..80).map(|line| format!("line {line}")).collect();
        pane.set_content_height(3200.0, 400.0);

        pane.scroll_cursor_by_content_pixels(120.0, 400.0);
        let first_target = pane.target_scroll_y;

        assert!(pane.tick_scroll());
        assert!(pane.target_scroll_y > first_target);
    }

    #[test]
    fn cursor_scroll_velocity_compounds_across_small_wheel_deltas() {
        let mut pane = pane_for_test();
        pane.lines = (0..80).map(|line| format!("line {line}")).collect();
        pane.set_content_height(3200.0, 400.0);

        pane.scroll_cursor_by_content_pixels(4.0, 400.0);
        let first_velocity = pane.scroll_velocity_px_s;
        pane.scroll_cursor_by_content_pixels(4.0, 400.0);

        assert!(pane.scroll_velocity_px_s > first_velocity);
    }

    #[test]
    fn only_enter_created_blank_lines_continue_paragraph_background() {
        let mut pane = pane_for_test();
        pane.enter_insert();
        pane.insert_text("paragraph");
        pane.insert_newline();

        assert_eq!(pane.cursor_line, 1);
        assert!(pane.is_enter_continuation_line(1));

        let mut loaded = pane_for_test();
        loaded.lines = vec!["paragraph".to_string(), String::new()];

        assert!(!loaded.is_enter_continuation_line(1));
    }

    #[test]
    fn enter_continues_task_bullet_number_and_letter_lists() {
        let cases = [
            ("- [ ] Task", "- [ ] "),
            ("- [x] Done", "- [ ] "),
            ("- Bullet", "- "),
            ("1) One", "2) "),
            ("a) Alpha", "b) "),
            ("Z) Last", "AA) "),
        ];

        for (source, expected_next) in cases {
            let mut pane = pane_for_test();
            pane.lines = vec![source.to_string()];
            pane.cursor_col = source.len();

            pane.insert_newline();

            assert_eq!(
                pane.lines,
                vec![source.to_string(), expected_next.to_string()]
            );
            assert_eq!(pane.cursor_line, 1);
            assert_eq!(pane.cursor_col, expected_next.len());
        }
    }

    #[test]
    fn enter_before_list_marker_does_not_duplicate_marker() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- item".to_string()];
        pane.cursor_col = 0;

        pane.insert_newline();

        assert_eq!(pane.lines, vec![String::new(), "- item".to_string()]);
        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, 0);
    }

    #[test]
    fn enter_on_empty_list_marker_exits_list() {
        let mut pane = pane_for_test();
        pane.lines = vec!["  - [ ] ".to_string()];
        pane.cursor_col = pane.lines[0].len();

        pane.insert_newline();

        assert_eq!(pane.lines, vec!["  ".to_string()]);
        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, 2);
    }

    #[test]
    fn tab_indents_and_shift_tab_outdents_list_items() {
        let mut pane = pane_for_test();
        pane.lines = vec!["1) item".to_string()];
        pane.cursor_col = pane.lines[0].len();

        assert!(pane.indent_list_item(false));
        assert_eq!(pane.lines, vec!["  1) item".to_string()]);
        assert_eq!(pane.cursor_col, "  1) item".len());

        assert!(pane.indent_list_item(true));
        assert_eq!(pane.lines, vec!["1) item".to_string()]);
        assert_eq!(pane.cursor_col, "1) item".len());
    }

    #[test]
    fn editing_nested_task_end_reseeds_vertical_goal() {
        let mut pane = pane_for_test();
        pane.lines = vec!["  - [ ] osdm".to_string(), String::new()];
        pane.cursor_line = 0;
        pane.cursor_col = pane.lines[0].len();

        pane.move_left();
        pane.move_down();
        assert_eq!(pane.cursor_line, 1);

        pane.cursor_line = 0;
        pane.cursor_col = pane.lines[0].len();
        pane.insert_text("  dsds");
        assert_eq!(pane.cursor_col, pane.lines[0].len());

        pane.move_down();
        assert_eq!(pane.cursor_line, 1);
        pane.move_up();

        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, pane.lines[0].len());
    }

    #[test]
    fn backspace_inside_list_marker_deletes_single_chars() {
        // Backspace is a plain character delete even inside the marker — the
        // old outdent/strip-whole-marker shortcut ate `- [x]` checkboxes when
        // the user only wanted to delete the `x`.
        let mut pane = pane_for_test();
        pane.lines = vec!["- [x] item".to_string()];
        pane.cursor_col = "- [x".len();

        pane.backspace();
        assert_eq!(pane.lines, vec!["- [] item".to_string()]);
        assert_eq!(pane.cursor_col, "- [".len());

        let mut pane = pane_for_test();
        pane.lines = vec!["  - item".to_string()];
        pane.cursor_col = "  - ".len();

        pane.backspace();
        assert_eq!(pane.lines, vec!["  -item".to_string()]);
        assert_eq!(pane.cursor_col, "  -".len());
    }

    #[test]
    fn vertical_movement_steps_through_wrapped_visual_lines() {
        let mut pane = pane_for_test();
        pane.lines = vec!["abcdef".to_string(), "next".to_string()];
        pane.cursor_col = 0;
        pane.register_block_rect(
            0,
            [0.0, 0.0, 200.0, 48.0],
            [-30.0, 0.0, 20.0, 48.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            30.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 3 },
                MarkdownWrapRow { start: 3, len: 3 },
            ],
        );
        pane.register_block_wrap_hit_stops(
            0,
            hit_rows(&[(0, &[0.0, 10.0, 20.0, 30.0]), (3, &[0.0, 10.0, 20.0, 30.0])]),
        );
        pane.register_block_rect(
            1,
            [0.0, 50.0, 200.0, 24.0],
            [-30.0, 50.0, 20.0, 24.0],
            0.0,
            50.0,
            0,
            10.0,
            20.0,
            200.0,
            None,
        );
        let line_1_chars = pane.lines[1].chars().count();
        measured_text_line(&mut pane, 1, line_1_chars, 10.0);

        pane.move_down();
        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, 3);

        pane.move_down();
        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, 0);
    }

    #[test]
    fn enter_on_notebook_output_inserts_after_generated_output_run() {
        let mut pane = pane_for_test();
        pane.mode = MarkdownMode::Normal;
        pane.lines = vec![
            "```python neoism_notebook_cell=0 neoism_state=idle neoism_count=1"
                .to_string(),
            "print('hi')".to_string(),
            "```".to_string(),
            "%%neoism_notebook_output _ _ hi".to_string(),
            "%%neoism_notebook_output _ _ there".to_string(),
            "next".to_string(),
        ];
        pane.cursor_line = 3;
        pane.cursor_col = pane.lines[3].len();

        pane.insert_newline();

        assert_eq!(pane.cursor_line, 5);
        assert_eq!(pane.lines[5], "");
        assert_eq!(pane.lines[6], "next");
        assert_eq!(pane.lines[3], "%%neoism_notebook_output _ _ hi");
        assert_eq!(pane.lines[4], "%%neoism_notebook_output _ _ there");
    }

    #[test]
    fn notebook_code_block_drag_range_includes_outputs() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "intro".to_string(),
            "```python neoism_notebook_cell=0 neoism_state=idle neoism_count=1"
                .to_string(),
            "print('hi')".to_string(),
            "```".to_string(),
            "%%neoism_notebook_output _ _ hi".to_string(),
            "after".to_string(),
        ];

        assert_eq!(pane.drag_block_range(1), 1..5);
        assert_eq!(pane.drag_block_range(4), 1..5);
    }

    #[test]
    fn markdown_link_targets_resolve_page_and_line() {
        let mut pane = pane_for_test();
        let dir =
            std::env::temp_dir().join(format!("neoism-md-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("page.md");
        std::fs::write(&target, "hello").unwrap();
        pane.path = dir.join("index.md");

        let link = pane.resolve_markdown_link("@page-12").unwrap();
        assert_eq!(link.path, target);
        assert_eq!(link.line, Some(12));

        let bare = pane.resolve_markdown_link("page").unwrap();
        assert_eq!(bare.path, target);
        assert_eq!(bare.line, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_link_targets_resolve_heading_text() {
        let mut pane = pane_for_test();
        let dir = std::env::temp_dir()
            .join(format!("neoism-md-link-heading-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("page.md");
        std::fs::write(&target, "# Page\n\n## Now\n").unwrap();
        pane.path = dir.join("index.md");

        let link = pane.resolve_markdown_link("page#Now").unwrap();
        assert_eq!(link.path, target);
        assert_eq!(link.line, Some(3));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wiki_link_template_consumes_slash_trigger() {
        let mut pane = pane_for_test();
        pane.lines = vec!["before / after".to_string()];
        pane.cursor_col = "before /".len();

        pane.apply_block_template(MarkdownBlockTemplate::WikiLink);

        assert_eq!(pane.lines, vec!["before [[]] after".to_string()]);
        assert_eq!(pane.cursor_col, "before [[".len());
    }

    #[test]
    fn code_link_template_consumes_slash_trigger() {
        let mut pane = pane_for_test();
        pane.lines = vec!["before / after".to_string()];
        pane.cursor_col = "before /".len();

        pane.apply_block_template(MarkdownBlockTemplate::CodeLink);

        assert_eq!(pane.lines, vec!["before [[@]] after".to_string()]);
        assert_eq!(pane.cursor_col, "before [[@".len());
    }

    #[test]
    fn slash_block_query_tracks_text_after_trigger() {
        let mut pane = pane_for_test();
        pane.lines = vec!["/task".to_string()];
        pane.cursor_col = "/task".len();

        assert_eq!(
            pane.slash_block_query_before_cursor(),
            Some("task".to_string())
        );
    }

    #[test]
    fn block_template_consumes_slash_query() {
        let mut pane = pane_for_test();
        pane.lines = vec!["/task".to_string()];
        pane.cursor_col = "/task".len();

        pane.apply_block_template(MarkdownBlockTemplate::TaskList);

        assert_eq!(pane.lines, vec!["- [ ] ".to_string()]);
        assert_eq!(pane.cursor_col, "- [ ] ".len());
    }

    #[test]
    fn task_checkbox_hitbox_toggles_source() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- [ ] todo".to_string()];
        pane.register_task_rect(0, [0.0, 0.0, 20.0, 20.0]);

        assert!(pane.toggle_task_at(10.0, 10.0));
        assert_eq!(pane.lines, vec!["- [x] todo".to_string()]);
        assert!(pane.task_toggle_progress(0).is_some());

        pane.register_task_rect(0, [0.0, 0.0, 20.0, 20.0]);
        assert!(pane.toggle_task_at(10.0, 10.0));
        assert_eq!(pane.lines, vec!["- [ ] todo".to_string()]);
    }

    #[test]
    fn table_separator_row_is_skipped_by_navigation() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| Recipient | Job |".to_string(),
            "| --- | --- |".to_string(),
            "| Parker | CNA |".to_string(),
        ];
        pane.cursor_line = 2;

        pane.move_up();
        assert_eq!(pane.cursor_line, 0);

        pane.move_down();
        assert_eq!(pane.cursor_line, 2);
    }

    #[test]
    fn table_horizontal_scroll_moves_cursor_column() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| Recipient | Job |".to_string(),
            "| --- | --- |".to_string(),
            "| Parker Settle | Per Diem CNA |".to_string(),
        ];
        pane.cursor_line = 2;
        pane.cursor_col = 0;
        pane.register_table_rect(0, [0.0, 0.0, 100.0, 100.0], 100.0, 500.0);

        assert!(pane.scroll_table_at(10.0, 10.0, 260.0));
        let right_col = pane.cursor_col;
        assert!(right_col > 0);

        assert!(pane.scroll_table_at(10.0, 10.0, -260.0));
        assert!(pane.cursor_col < right_col);
    }

    #[test]
    fn table_tab_navigation_moves_cell_to_cell() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| C | D |".to_string(),
        ];
        pane.cursor_line = 0;
        pane.cursor_col =
            parse_table_cell_bounds(&pane.lines[0]).unwrap()[0].content_start;

        assert!(pane.move_table_cell(false));
        assert_eq!(pane.cursor_line, 0);
        assert_eq!(
            pane.cursor_col,
            parse_table_cell_bounds(&pane.lines[0]).unwrap()[1].content_start
        );

        assert!(pane.move_table_cell(false));
        assert_eq!(pane.cursor_line, 2);
        assert_eq!(
            pane.cursor_col,
            parse_table_cell_bounds(&pane.lines[2]).unwrap()[0].content_start
        );

        assert!(pane.move_table_cell(true));
        assert_eq!(pane.cursor_line, 0);
    }

    #[test]
    fn table_arrow_horizontal_does_not_wrap_rows() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| C | D |".to_string(),
        ];
        pane.cursor_line = 2;
        let cells = parse_table_cell_bounds(&pane.lines[2]).unwrap();
        pane.cursor_col = cells[0].content_start;

        pane.move_left();
        assert_eq!(pane.cursor_line, 2);
        assert_eq!(
            pane.cursor_col,
            parse_table_cell_bounds(&pane.lines[2]).unwrap()[0].content_start
        );

        let cells = parse_table_cell_bounds(&pane.lines[2]).unwrap();
        pane.cursor_col = cells[1].content_end;
        pane.move_right();
        assert_eq!(pane.cursor_line, 2);
        assert_eq!(
            pane.cursor_col,
            parse_table_cell_bounds(&pane.lines[2]).unwrap()[1].content_end
        );
    }

    #[test]
    fn empty_table_row_backspace_and_delete_remove_row() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| C | D |".to_string(),
            "|   |   |".to_string(),
            "| E | F |".to_string(),
        ];
        pane.cursor_line = 3;
        pane.cursor_col =
            parse_table_cell_bounds(&pane.lines[3]).unwrap()[0].content_start;

        pane.backspace();
        assert_eq!(
            pane.lines,
            vec![
                "| A | B |".to_string(),
                "| --- | --- |".to_string(),
                "| C | D |".to_string(),
                "| E | F |".to_string(),
            ]
        );
        assert_eq!(pane.cursor_line, 3);

        pane.lines.insert(3, "|   |   |".to_string());
        pane.cursor_line = 3;
        pane.cursor_col =
            parse_table_cell_bounds(&pane.lines[3]).unwrap()[0].content_start;

        pane.delete_forward();
        assert_eq!(
            pane.lines,
            vec![
                "| A | B |".to_string(),
                "| --- | --- |".to_string(),
                "| C | D |".to_string(),
                "| E | F |".to_string(),
            ]
        );
        assert_eq!(pane.cursor_line, 3);

        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| C | D |".to_string(),
            "|   |   |".to_string(),
        ];
        pane.cursor_line = 3;
        pane.cursor_col = pane.lines[3].len();

        pane.delete_forward();
        assert_eq!(
            pane.lines,
            vec![
                "| A | B |".to_string(),
                "| --- | --- |".to_string(),
                "| C | D |".to_string(),
            ]
        );
        assert_eq!(pane.cursor_line, 2);
    }

    #[test]
    fn table_vertical_movement_steps_through_wrapped_cell_lines() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| abcdefghi | z |".to_string(),
        ];
        pane.cursor_line = 2;
        let first_cell = parse_table_cell_bounds(&pane.lines[2]).unwrap()[0];
        pane.cursor_col = first_cell.content_start;
        pane.register_table_cell_rect(
            2,
            0,
            [0.0, 0.0, 40.0, 70.0],
            0.0,
            0.0,
            30.0,
            10.0,
            20.0,
            vec![
                hit_row(0, &[0.0, 10.0, 20.0, 30.0]),
                hit_row(3, &[0.0, 10.0, 20.0, 30.0]),
                hit_row(6, &[0.0, 10.0, 20.0, 30.0]),
            ],
        );

        pane.move_down();
        assert_eq!(pane.cursor_line, 2);
        assert_eq!(pane.cursor_col, first_cell.content_start + "abc".len());

        pane.move_down();
        assert_eq!(pane.cursor_line, 2);
        assert_eq!(pane.cursor_col, first_cell.content_start + "abcdef".len());

        pane.move_up();
        assert_eq!(pane.cursor_col, first_cell.content_start + "abc".len());
    }

    #[test]
    fn table_vertical_movement_uses_rendered_markdown_cell_columns() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| **bold** tail | z |".to_string(),
        ];
        pane.cursor_line = 2;
        let first_cell = parse_table_cell_bounds(&pane.lines[2]).unwrap()[0];
        pane.cursor_col = "| **bold** t".len();
        pane.register_table_cell_rect(
            2,
            0,
            [0.0, 0.0, 40.0, 70.0],
            0.0,
            0.0,
            30.0,
            10.0,
            20.0,
            vec![
                hit_row(0, &[0.0, 10.0, 20.0, 30.0]),
                hit_row(3, &[0.0, 10.0, 20.0, 30.0]),
                hit_row(6, &[0.0, 10.0, 20.0, 30.0]),
            ],
        );

        pane.move_up();

        assert_eq!(pane.cursor_line, 2);
        assert_eq!(pane.cursor_col, first_cell.content_start + "**bol".len());
    }

    #[test]
    fn table_cell_click_uses_rendered_markdown_columns() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| **bold** tail | z |".to_string(),
        ];
        let first_cell = parse_table_cell_bounds(&pane.lines[2]).unwrap()[0];
        let cell = MarkdownTableCellRect {
            line: 2,
            cell_ix: 0,
            rect: [0.0, 0.0, 200.0, 40.0],
            text_x: 0.0,
            text_y: 0.0,
            text_width: 120.0,
            cell_width: 10.0,
            line_height: 20.0,
            hit_rows: vec![hit_row(0, &[0.0, 10.0, 20.0, 30.0, 40.0])],
        };

        let col = pane.cursor_col_from_table_cell_point(cell, 30.0, 0.0);

        assert_eq!(col, first_cell.content_start + "**bol".len());
    }

    #[test]
    fn table_cell_bounds_preserve_extra_trailing_space() {
        let padded = "| foo | bar |";
        let edited = "| foo  | bar |";

        let padded_first = parse_table_cell_bounds(padded).unwrap()[0];
        let edited_first = parse_table_cell_bounds(edited).unwrap()[0];

        assert_eq!(
            &padded[padded_first.content_start..padded_first.content_end],
            "foo"
        );
        assert_eq!(
            &edited[edited_first.content_start..edited_first.content_end],
            "foo "
        );
    }

    #[test]
    fn table_add_row_action_inserts_below_hovered_row() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| C | D |".to_string(),
        ];
        pane.register_table_add_row_rect(2, [0.0, 0.0, 20.0, 20.0], None);

        assert!(pane.activate_table_action_at(10.0, 10.0));
        assert_eq!(
            pane.lines,
            vec![
                "| A | B |".to_string(),
                "| --- | --- |".to_string(),
                "| C | D |".to_string(),
                "|   |   |".to_string(),
            ]
        );
        assert_eq!(pane.cursor_line, 3);
        assert_eq!(
            pane.cursor_col,
            parse_table_cell_bounds(&pane.lines[3]).unwrap()[0].content_start
        );
    }

    #[test]
    fn table_add_column_action_inserts_left_and_right() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| C | D |".to_string(),
        ];
        pane.register_table_add_column_rect(0, 0, [0.0, 0.0, 20.0, 20.0], None);

        assert!(pane.activate_table_action_at(10.0, 10.0));
        assert_eq!(
            pane.lines,
            vec![
                "| Column 1 | A | B |".to_string(),
                "| --- | --- | --- |".to_string(),
                "|   | C | D |".to_string(),
            ]
        );
        assert_eq!(pane.cursor_line, 0);
        assert_eq!(
            pane.cursor_col,
            parse_table_cell_bounds(&pane.lines[0]).unwrap()[0].content_start
        );

        pane.register_table_add_column_rect(0, 3, [0.0, 0.0, 20.0, 20.0], None);

        assert!(pane.activate_table_action_at(10.0, 10.0));
        assert_eq!(
            pane.lines,
            vec![
                "| Column 1 | A | B | Column 4 |".to_string(),
                "| --- | --- | --- | --- |".to_string(),
                "|   | C | D |   |".to_string(),
            ]
        );
    }

    #[test]
    fn visual_mode_yanks_selected_text() {
        let mut pane = pane_for_test();
        pane.lines = vec!["alpha beta".to_string()];
        pane.cursor_col = 0;

        pane.enter_visual();
        pane.move_right();
        pane.move_right();

        assert_eq!(pane.selection_for_line(0), Some((0, 3)));
        assert_eq!(pane.yank_selection().as_deref(), Some("alp"));
        assert_eq!(pane.mode, MarkdownMode::Normal);
        assert!(pane.yank_flash_for_line(0).is_some());
    }

    #[test]
    fn line_yank_seeds_yank_flash() {
        let mut pane = pane_for_test();
        pane.lines = vec!["alpha".to_string()];

        pane.flash_current_line_yank();

        assert_eq!(pane.yank_current_line(), "alpha".to_string());
        assert!(pane.yank_flash_for_line(0).is_some());
    }

    #[test]
    fn mouse_drag_enters_visual_mode_and_updates_cursor() {
        let mut pane = pane_for_test();
        pane.lines = vec!["hello".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 200.0, 24.0],
            [-30.0, 0.0, 20.0, 24.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            200.0,
            None,
        );
        let line_0_chars = pane.lines[0].chars().count();
        measured_text_line(&mut pane, 0, line_0_chars, 10.0);

        assert!(pane.click_at(0.0, 4.0));
        assert!(pane.update_drag(30.0, 4.0));
        assert!(pane.end_drag());

        assert_eq!(pane.mode, MarkdownMode::Visual);
        assert_eq!(pane.selection_for_line(0), Some((0, 4)));
        assert_eq!(pane.yank_selection().as_deref(), Some("hell"));
    }

    #[test]
    fn click_prefers_most_recent_specific_block_rect() {
        let mut pane = pane_for_test();
        pane.lines = vec!["parent".to_string(), "child".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 300.0, 80.0],
            [-30.0, 0.0, 20.0, 80.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            300.0,
            None,
        );
        let parent_chars = pane.lines[0].chars().count();
        measured_text_line(&mut pane, 0, parent_chars, 10.0);
        pane.register_block_rect(
            1,
            [0.0, 24.0, 300.0, 24.0],
            [-30.0, 24.0, 20.0, 24.0],
            0.0,
            24.0,
            0,
            10.0,
            20.0,
            300.0,
            None,
        );
        let child_chars = pane.lines[1].chars().count();
        measured_text_line(&mut pane, 1, child_chars, 10.0);

        assert!(pane.click_at(14.0, 28.0));
        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, 1);
    }

    #[test]
    fn click_end_uses_rendered_markdown_text_not_raw_markers() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- item with **bold** and [[Roadmap#Now|roadmap]]".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 300.0, 24.0],
            [-30.0, 0.0, 20.0, 24.0],
            20.0,
            0.0,
            2,
            10.0,
            20.0,
            280.0,
            None,
        );
        let visible_chars = InlineSourceMap::new(&pane.lines[0][2..]).visible_len();
        measured_text_line(&mut pane, 0, visible_chars, 10.0);

        assert!(pane.click_at(295.0, 4.0));
        assert_eq!(pane.cursor_col, pane.lines[0].len());
    }

    #[test]
    fn click_near_end_of_single_line_task_moves_to_source_line_end() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- [ ] finish the markdown hit test".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 320.0, 24.0],
            [-30.0, 0.0, 20.0, 24.0],
            30.0,
            0.0,
            "- [ ] ".len(),
            10.0,
            20.0,
            290.0,
            None,
        );
        let visible_chars =
            InlineSourceMap::new(&pane.lines[0]["- [ ] ".len()..]).visible_len();
        measured_text_line(&mut pane, 0, visible_chars, 10.0);

        assert!(pane.click_at(306.0, 4.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, pane.lines[0].len());
    }

    #[test]
    fn click_near_end_of_single_line_list_item_moves_to_source_line_end() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- finish the markdown hit test".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 320.0, 24.0],
            [-30.0, 0.0, 20.0, 24.0],
            20.0,
            0.0,
            "- ".len(),
            10.0,
            20.0,
            300.0,
            None,
        );
        let visible_chars =
            InlineSourceMap::new(&pane.lines[0]["- ".len()..]).visible_len();
        measured_text_line(&mut pane, 0, visible_chars, 10.0);

        assert!(pane.click_at(296.0, 4.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_col, pane.lines[0].len());
    }

    #[test]
    fn click_near_end_of_wrapped_markdown_line_uses_wrapped_visual_end() {
        let mut pane = pane_for_test();
        pane.lines = vec!["### very long heading text that wraps here".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 120.0, 44.0],
            [-30.0, 0.0, 20.0, 44.0],
            0.0,
            0.0,
            "### ".len(),
            10.0,
            20.0,
            80.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 10 },
                MarkdownWrapRow { start: 10, len: 28 },
            ],
        );
        pane.register_block_wrap_hit_stops(
            0,
            hit_rows(&[
                (
                    0,
                    &[
                        0.0, 7.0, 15.0, 23.0, 31.0, 39.0, 47.0, 55.0, 63.0, 71.0, 79.0,
                    ],
                ),
                (
                    10,
                    &[
                        0.0, 3.0, 6.0, 9.0, 12.0, 15.0, 18.0, 21.0, 24.0, 27.0, 30.0,
                        33.0, 36.0, 39.0, 42.0, 45.0, 48.0, 51.0, 54.0, 57.0, 60.0, 63.0,
                        66.0, 69.0, 72.0, 75.0, 78.0, 80.0, 82.0,
                    ],
                ),
            ]),
        );

        assert!(pane.click_at(78.0, 24.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_col, pane.lines[0].len());
    }

    #[test]
    fn click_heading_uses_measured_hit_stops_even_when_unwrapped() {
        let mut pane = pane_for_test();
        pane.lines = vec!["### Wide heading".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 320.0, 34.0],
            [-30.0, 0.0, 20.0, 34.0],
            0.0,
            0.0,
            "### ".len(),
            10.0,
            24.0,
            280.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![MarkdownWrapRow { start: 0, len: 12 }],
        );
        pane.register_block_wrap_hit_stops(
            0,
            hit_rows(&[(
                0,
                &[
                    0.0, 5.0, 18.0, 24.0, 60.0, 70.0, 82.0, 95.0, 111.0, 122.0, 140.0,
                    154.0, 170.0,
                ],
            )]),
        );

        assert!(pane.click_at(58.0, 4.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_col, "### Wide".len());
    }

    #[test]
    fn click_wrapped_body_uses_measured_hit_stops_for_visual_row() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- [ ] alpha beta gamma delta epsilon".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 300.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            60.0,
            0.0,
            "- [ ] ".len(),
            10.0,
            20.0,
            200.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 10 },
                MarkdownWrapRow { start: 11, len: 10 },
                MarkdownWrapRow { start: 22, len: 7 },
            ],
        );
        pane.register_block_wrap_hit_stops(
            0,
            hit_rows(&[
                (
                    0,
                    &[
                        0.0, 7.0, 15.0, 23.0, 31.0, 39.0, 47.0, 55.0, 63.0, 71.0, 79.0,
                    ],
                ),
                (
                    11,
                    &[
                        0.0, 8.0, 16.0, 24.0, 40.0, 50.0, 61.0, 73.0, 86.0, 100.0, 116.0,
                    ],
                ),
                (22, &[0.0, 9.0, 17.0, 26.0, 36.0, 47.0, 59.0, 72.0]),
            ]),
        );

        assert!(pane.click_at(115.0, 24.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_col, "- [ ] alpha beta gamma".len());
    }

    #[test]
    fn click_code_fence_line_enters_insert_at_source_column() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "```rust".to_string(),
            "let value = 1;".to_string(),
            "```".to_string(),
        ];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 320.0, 24.0],
            [-30.0, 0.0, 20.0, 24.0],
            10.0,
            0.0,
            0,
            10.0,
            20.0,
            290.0,
            None,
        );

        assert!(pane.click_at(55.0, 4.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_line, 0);
        // Hidden fence rows (no measured stops registered) snap the caret to
        // the END of the fence so backspace edits the ```lang immediately.
        assert_eq!(pane.cursor_col, pane.lines[0].len());
    }

    #[test]
    fn click_horizontal_rule_enters_insert_at_rule_end() {
        let mut pane = pane_for_test();
        pane.lines = vec!["---".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 320.0, 24.0],
            [-30.0, 0.0, 20.0, 24.0],
            10.0,
            0.0,
            0,
            10.0,
            20.0,
            290.0,
            None,
        );
        assert!(pane.click_at(306.0, 4.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_col, pane.lines[0].len());
    }

    #[test]
    fn click_divider_variants_enter_insert_at_source_end() {
        for divider in ["***", "___", "  ***  "] {
            let mut pane = pane_for_test();
            pane.lines = vec![divider.to_string()];
            pane.register_block_rect(
                0,
                [0.0, 0.0, 320.0, 24.0],
                [-30.0, 0.0, 20.0, 24.0],
                10.0,
                0.0,
                0,
                10.0,
                20.0,
                290.0,
                None,
            );

            assert!(pane.click_at(306.0, 4.0), "divider {divider}");

            assert_eq!(pane.mode, MarkdownMode::Insert, "divider {divider}");
            assert_eq!(pane.cursor_line, 0, "divider {divider}");
            assert_eq!(pane.cursor_col, pane.lines[0].len(), "divider {divider}");
        }
    }

    #[test]
    fn click_quote_line_maps_after_quote_marker() {
        let mut pane = pane_for_test();
        pane.lines = vec!["> quoted words".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 320.0, 24.0],
            [-30.0, 0.0, 20.0, 24.0],
            20.0,
            0.0,
            "> ".len(),
            10.0,
            20.0,
            290.0,
            None,
        );
        let visible_chars =
            InlineSourceMap::new(&pane.lines[0]["> ".len()..]).visible_len();
        measured_text_line(&mut pane, 0, visible_chars, 10.0);

        assert!(pane.click_at(34.0, 4.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_col, "> q".len());
    }

    #[test]
    fn delete_visual_selection_removes_text_range() {
        let mut pane = pane_for_test();
        pane.lines = vec!["alpha beta".to_string(), "gamma delta".to_string()];
        pane.mode = MarkdownMode::Visual;
        pane.visual_anchor = Some(MarkdownPosition { line: 0, col: 6 });
        pane.cursor_line = 1;
        pane.cursor_col = 5;

        let removed = pane.delete_selection().unwrap();

        assert_eq!(removed, "beta\ngamma ");
        assert_eq!(pane.lines, vec!["alpha delta".to_string()]);
        assert_eq!(pane.mode, MarkdownMode::Normal);
        assert!(pane.is_dirty());
    }

    #[test]
    fn click_wrapped_task_line_maps_to_visual_row() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- [ ] alpha beta gamma delta epsilon zeta".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 120.0, 60.0],
            [-30.0, 0.0, 20.0, 24.0],
            30.0,
            0.0,
            "- [ ] ".len(),
            10.0,
            20.0,
            80.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 10 },
                MarkdownWrapRow { start: 11, len: 10 },
                MarkdownWrapRow { start: 22, len: 16 },
            ],
        );
        pane.register_block_wrap_hit_stops(
            0,
            hit_rows(&[
                (
                    0,
                    &[
                        0.0, 7.0, 15.0, 23.0, 31.0, 39.0, 47.0, 55.0, 63.0, 71.0, 79.0,
                    ],
                ),
                (
                    11,
                    &[
                        0.0, 8.0, 16.0, 24.0, 40.0, 50.0, 61.0, 73.0, 86.0, 100.0, 116.0,
                    ],
                ),
                (
                    22,
                    &[
                        0.0, 8.0, 16.0, 24.0, 40.0, 50.0, 61.0, 73.0, 86.0, 100.0, 116.0,
                        126.0, 136.0, 146.0, 156.0, 166.0, 176.0,
                    ],
                ),
            ]),
        );

        assert!(pane.click_at(35.0, 24.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert!(pane.cursor_col > "- [ ] alpha".len());
        assert!(pane.cursor_col < pane.lines[0].len());
    }

    #[test]
    fn click_end_of_wrapped_row_uses_real_wrap_layout() {
        // Body "alpha beta gamma delta epsilon" wraps into three visual rows
        // whose lengths are *not* uniform (10 / 11 / 7 chars). A click at the
        // visual end of the middle row must land at the end of that row's
        // content ("delta"), not many words away — the old uniform estimate
        // mis-mapped this.
        let mut pane = pane_for_test();
        pane.lines = vec!["- [ ] alpha beta gamma delta epsilon".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 300.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            60.0,
            0.0,
            "- [ ] ".len(),
            10.0,
            20.0,
            200.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 10 },
                MarkdownWrapRow { start: 11, len: 11 },
                MarkdownWrapRow { start: 23, len: 7 },
            ],
        );
        pane.register_block_wrap_hit_stops(
            0,
            hit_rows(&[
                (
                    0,
                    &[
                        0.0, 7.0, 15.0, 23.0, 31.0, 39.0, 47.0, 55.0, 63.0, 71.0, 79.0,
                    ],
                ),
                (
                    11,
                    &[
                        0.0, 8.0, 16.0, 24.0, 40.0, 50.0, 61.0, 73.0, 86.0, 100.0, 116.0,
                        132.0,
                    ],
                ),
                (23, &[0.0, 9.0, 17.0, 26.0, 36.0, 47.0, 59.0, 72.0]),
            ]),
        );

        // Middle row (y in [20,40)), click past the rendered end of the row.
        assert!(pane.click_at(200.0, 24.0));
        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, "- [ ] alpha beta gamma delta".len());

        // Last row end snaps to the true source line end.
        assert!(pane.click_at(200.0, 44.0));
        assert_eq!(pane.cursor_col, pane.lines[0].len());

        // Start of the middle row maps just past the first row's content.
        assert!(pane.click_at(60.0, 24.0));
        assert_eq!(pane.cursor_col, "- [ ] alpha beta ".len());
    }

    #[test]
    fn vertical_motion_uses_real_wrap_rows_for_nested_tasks() {
        let mut pane = pane_for_test();
        pane.lines = vec!["  - [ ] alpha beta gamma delta epsilon".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 300.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            60.0,
            0.0,
            "  - [ ] ".len(),
            10.0,
            20.0,
            200.0,
            None,
        );
        pane.register_block_wrap_rows(0, vec![10, 11, 7]);
        pane.cursor_line = 0;
        pane.cursor_col = "  - [ ] alpha beta gamma delta".len();

        pane.move_up();

        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, "  - [ ] alpha beta".len());

        pane.move_down();

        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, "  - [ ] alpha beta gamma delta".len());
    }

    #[test]
    fn vertical_motion_on_revealed_raw_rows_aligns_marker_hang() {
        // The cursor's line wraps RAW (rows span the whole line, marker_len
        // 0; continuation rows hang at the body column). Down from the first
        // body letter must land on the first letter of the wrapped row, and
        // Up must come back — the goal column is visual, not a flat index.
        let mut pane = pane_for_test();
        pane.lines = vec!["- [ ] alpha beta gamma delta".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 260.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            10.0,
            0.0,
            0,
            10.0,
            20.0,
            180.0,
            None,
        );
        // Raw rows: "- [ ] alpha beta " (separator space ends row 0), "gamma delta".
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 17 },
                MarkdownWrapRow { start: 17, len: 11 },
            ],
        );
        pane.cursor_line = 0;
        pane.cursor_col = "- [ ] ".len();

        pane.move_down();
        assert_eq!(pane.cursor_col, "- [ ] alpha beta ".len());

        pane.move_up();
        assert_eq!(pane.cursor_col, "- [ ] ".len());
    }

    #[test]
    fn revealed_task_wraps_using_full_line_width() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- [ ] aaaaaaaaaa bbbbbbbbbb cccccccccc".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 260.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            10.0,
            0.0,
            0,
            10.0,
            20.0,
            180.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 18 },
                MarkdownWrapRow { start: 19, len: 10 },
                MarkdownWrapRow { start: 30, len: 10 },
            ],
        );
        pane.cursor_line = 0;
        pane.cursor_col = "- [ ] aaaaaaaaaa".len();

        pane.move_down();

        assert_eq!(pane.cursor_line, 0);
        assert_eq!(pane.cursor_col, "- [ ] aaaaaaaaaa bbbbbbbbbb c".len());
    }

    #[test]
    fn vertical_motion_consumes_wrap_spaces_for_tabbed_list_rows() {
        let mut pane = pane_for_test();
        pane.lines = vec!["\t- alpha beta gamma delta".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 260.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            50.0,
            0.0,
            "\t- ".len(),
            10.0,
            20.0,
            180.0,
            None,
        );
        pane.register_block_wrap_rows(0, vec![10, 10]);
        pane.cursor_line = 0;
        pane.cursor_col = "\t- alpha beta gamma".len();
        let original_col = pane.cursor_col;

        pane.move_up();

        assert_eq!(pane.cursor_col, "\t- alpha".len());

        pane.move_down();

        assert_eq!(pane.cursor_col, original_col);
    }

    #[test]
    fn wrapped_row_end_uses_after_last_glyph_slot() {
        let mut pane = pane_for_test();
        pane.lines = vec!["dddddddddd dddddddddd".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 260.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            180.0,
            None,
        );
        pane.register_block_wrap_rows(0, vec![10, 10]);
        pane.cursor_line = 0;
        pane.cursor_col = "dddddddddd ddddddddd".len();

        pane.move_up();

        assert_eq!(pane.cursor_col, "dddddddddd".len());

        pane.move_down();

        assert_eq!(pane.cursor_col, "dddddddddd dddddddddd".len());
    }

    #[test]
    fn wrapped_rows_use_exact_starts_for_multiple_spaces() {
        let mut pane = pane_for_test();
        pane.lines = vec!["aaaaaaaaaa   bbbbbbbbbb".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 260.0, 60.0],
            [-30.0, 0.0, 20.0, 60.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            180.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 10 },
                MarkdownWrapRow { start: 13, len: 10 },
            ],
        );
        pane.cursor_line = 0;
        pane.cursor_col = "aaaaaaaaaa   bbbbbbbbb".len();

        pane.move_up();

        assert_eq!(pane.cursor_col, "aaaaaaaaaa".len());

        pane.move_down();

        assert_eq!(pane.cursor_col, "aaaaaaaaaa   bbbbbbbbbb".len());
    }

    #[test]
    fn leading_spaces_before_sentence_are_rendered_and_navigable() {
        let mut pane = pane_for_test();
        pane.lines = vec!["   hello".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = 0;

        pane.insert_text(" ");

        assert_eq!(pane.lines[0], "    hello");
        assert_eq!(pane.cursor_col, 1);
    }

    #[test]
    fn extra_spaces_after_heading_marker_are_visible_and_editable() {
        let mut pane = pane_for_test();
        pane.lines = vec!["#   hello".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = "# ".len();

        pane.insert_text(" ");

        assert_eq!(pane.lines[0], "#    hello");
        assert_eq!(pane.cursor_col, "#  ".len());
        // Cursor's own line is revealed raw — the marker is reachable.
        assert_eq!(pane.visible_start_col(0), 0);
        pane.cursor_line = 1;
        pane.lines.push(String::new());
        assert_eq!(pane.visible_start_col(0), "# ".len());
    }

    #[test]
    fn extra_spaces_after_quote_marker_are_visible_and_editable() {
        let mut pane = pane_for_test();
        pane.lines = vec![">   hello".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = "> ".len();

        pane.insert_text(" ");

        assert_eq!(pane.lines[0], ">    hello");
        assert_eq!(pane.cursor_col, ">  ".len());
        // Cursor's own line is revealed raw — the marker is reachable.
        assert_eq!(pane.visible_start_col(0), 0);
        pane.cursor_line = 1;
        pane.lines.push(String::new());
        assert_eq!(pane.visible_start_col(0), "> ".len());
    }

    #[test]
    fn arrow_keys_walk_into_revealed_markers() {
        let mut pane = pane_for_test();
        pane.lines = vec!["### head".to_string(), "- item".to_string()];
        pane.cursor_line = 0;
        pane.cursor_col = "### ".len();

        // Left steps through the revealed `### ` all the way to col 0.
        for expected in (0.."### ".len()).rev() {
            pane.move_left();
            assert_eq!((pane.cursor_line, pane.cursor_col), (0, expected));
        }
        // And back right through it.
        pane.move_right();
        assert_eq!((pane.cursor_line, pane.cursor_col), (0, 1));

        // Same for a list marker.
        pane.cursor_line = 1;
        pane.cursor_col = "- ".len();
        pane.move_left();
        assert_eq!((pane.cursor_line, pane.cursor_col), (1, 1));
        pane.move_left();
        assert_eq!((pane.cursor_line, pane.cursor_col), (1, 0));
    }

    #[test]
    fn list_task_and_inline_symbols_preserve_space_cursor_source() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- [ ] a.b".to_string(), "- item a.b".to_string()];

        pane.cursor_line = 0;
        pane.cursor_col = "- [ ] a".len();
        pane.insert_text("   ");
        assert_eq!(pane.lines[0], "- [ ] a   .b");
        assert_eq!(pane.cursor_col, "- [ ] a   ".len());

        pane.cursor_line = 1;
        pane.cursor_col = "- item a".len();
        pane.insert_text("   ");
        assert_eq!(pane.lines[1], "- item a   .b");
        assert_eq!(pane.cursor_col, "- item a   ".len());
    }

    #[test]
    fn table_cell_spaces_before_symbols_preserve_cursor_source() {
        let mut pane = pane_for_test();
        pane.lines = vec!["| a.b | c |".to_string()];
        let cell = parse_table_cell_bounds(&pane.lines[0]).unwrap()[0];
        pane.cursor_line = 0;
        pane.cursor_col = cell.content_start + "a".len();

        pane.insert_text("   ");

        assert_eq!(pane.lines[0], "| a   .b | c |");
        assert_eq!(pane.cursor_col, cell.content_start + "a   ".len());
    }

    #[test]
    fn down_arrow_steps_through_wrapped_plain_paragraph_rows() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "aaaaaaaaaa bbbbbbbbbb cccccccccc".to_string(),
            "next line".to_string(),
        ];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 260.0, 80.0],
            [-30.0, 0.0, 20.0, 80.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            180.0,
            None,
        );
        pane.register_block_wrap_row_spans(
            0,
            vec![
                MarkdownWrapRow { start: 0, len: 10 },
                MarkdownWrapRow { start: 11, len: 10 },
                MarkdownWrapRow { start: 22, len: 10 },
            ],
        );
        pane.cursor_line = 0;
        pane.cursor_col = "aaa".len();

        pane.move_down();

        assert_eq!(pane.cursor_col, "aaaaaaaaaa bbb".len());

        pane.move_down();

        assert_eq!(pane.cursor_col, "aaaaaaaaaa bbbbbbbbbb ccc".len());

        pane.move_down();

        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, "nex".len());
    }

    #[test]
    fn click_end_of_padded_single_line_heading_still_moves_to_source_end() {
        let mut pane = pane_for_test();
        pane.lines = vec!["## short heading".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 260.0, 44.0],
            [-30.0, 0.0, 20.0, 44.0],
            0.0,
            18.0,
            "## ".len(),
            10.0,
            20.0,
            220.0,
            None,
        );

        assert!(pane.click_at(128.0, 22.0));

        assert_eq!(pane.mode, MarkdownMode::Insert);
        assert_eq!(pane.cursor_col, pane.lines[0].len());
    }

    #[test]
    fn drag_reorders_source_lines() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- A".to_string(), "- B".to_string(), "- C".to_string()];
        for (line, y) in [0.0, 30.0, 60.0].into_iter().enumerate() {
            pane.register_block_rect(
                line,
                [0.0, y, 200.0, 24.0],
                [-30.0, y, 20.0, 24.0],
                0.0,
                y,
                0,
                10.0,
                20.0,
                200.0,
                None,
            );
        }

        assert!(pane.begin_drag_at(-20.0, 34.0));
        assert!(pane.update_drag(-20.0, 100.0));
        assert!(pane.end_drag());

        assert_eq!(
            pane.lines,
            vec!["- A".to_string(), "- C".to_string(), "- B".to_string()]
        );
        assert!(pane.is_dirty());
    }

    #[test]
    fn handle_click_queues_block_menu_without_reorder() {
        let mut pane = pane_for_test();
        pane.lines = vec!["- A".to_string(), "- B".to_string()];
        pane.register_block_rect(
            0,
            [0.0, 0.0, 200.0, 24.0],
            [-30.0, 0.0, 20.0, 24.0],
            0.0,
            0.0,
            0,
            10.0,
            20.0,
            200.0,
            None,
        );

        assert!(pane.begin_drag_at(-20.0, 12.0));
        assert!(pane.end_drag());
        assert_eq!(
            pane.take_pending_block_menu_rect(),
            Some([-30.0, 0.0, 20.0, 24.0])
        );
        assert_eq!(pane.lines, vec!["- A".to_string(), "- B".to_string()]);
    }

    #[test]
    fn drag_reorders_contiguous_paragraph_lines_as_one_block() {
        let mut pane = pane_for_test();
        pane.lines = vec![
            "first line".to_string(),
            "same paragraph".to_string(),
            String::new(),
            "after".to_string(),
        ];
        for (line, y) in [(0usize, 0.0), (1, 30.0), (3, 80.0)] {
            pane.register_block_rect(
                line,
                [0.0, y, 200.0, 24.0],
                [-30.0, y, 20.0, 24.0],
                0.0,
                y,
                0,
                10.0,
                20.0,
                200.0,
                None,
            );
        }

        assert!(pane.begin_drag_at(-20.0, 34.0));
        assert!(pane.update_drag(-20.0, 120.0));
        assert!(pane.end_drag());
        assert_eq!(
            pane.lines,
            vec![
                String::new(),
                "after".to_string(),
                "first line".to_string(),
                "same paragraph".to_string(),
            ]
        );
    }

    #[test]
    fn drag_reorders_tables_as_single_block_and_save_writes_result() {
        let mut pane = pane_for_test();
        let unique = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        pane.path = std::env::temp_dir().join(format!("neoism-markdown-{unique}.md"));
        pane.lines = vec![
            "before".to_string(),
            "| H | J |".to_string(),
            "| --- | --- |".to_string(),
            "| A | B |".to_string(),
            "after".to_string(),
        ];
        for (line, y) in [0.0, 30.0, 60.0].into_iter().enumerate() {
            let source_line = match line {
                0 => 0,
                1 => 1,
                _ => 4,
            };
            pane.register_block_rect(
                source_line,
                [0.0, y, 200.0, 24.0],
                [-30.0, y, 20.0, 24.0],
                0.0,
                y,
                0,
                10.0,
                20.0,
                200.0,
                None,
            );
        }

        assert!(pane.begin_drag_at(-20.0, 34.0));
        assert!(pane.update_drag(-20.0, 100.0));
        assert!(pane.end_drag());
        assert!(pane.save().is_ok());

        let saved = std::fs::read_to_string(&pane.path).unwrap();
        assert_eq!(saved, "before\nafter\n| H | J |\n| --- | --- |\n| A | B |");
        let _ = std::fs::remove_file(&pane.path);
    }

    #[test]
    fn undo_and_redo_restore_cursor_position() {
        let mut pane = pane_for_test();

        pane.enter_insert();
        pane.insert_text("alpha");
        pane.cursor_col = 2;
        pane.insert_text("X");
        assert_eq!(pane.lines, vec!["alXpha".to_string()]);
        assert_eq!(pane.cursor_col, 3);

        pane.move_line_end();
        assert!(pane.undo());
        assert_eq!(pane.lines, vec!["alpha".to_string()]);
        assert_eq!(pane.cursor_col, 2);

        assert!(pane.redo());
        assert_eq!(pane.lines, vec!["alXpha".to_string()]);
        assert_eq!(pane.cursor_col, 3);
    }

    #[test]
    fn undo_back_to_saved_state_clears_dirty_then_redo_re_sets() {
        // A freshly loaded pane matches its saved baseline -> clean.
        let mut pane = pane_for_test();
        assert!(!pane.is_dirty(), "fresh pane should match its baseline");

        // Edit diverges the buffer from disk -> dirty (tab dot shows).
        pane.enter_insert();
        pane.insert_text("hello");
        assert_eq!(pane.lines, vec!["hello".to_string()]);
        assert!(pane.is_dirty(), "an edit must mark the buffer dirty");

        // Undo back to the saved (empty) text -> clean again. This is
        // the regression: a monotonic flag left the dot stuck here.
        assert!(pane.undo());
        assert_eq!(pane.lines, vec![String::new()]);
        assert!(
            !pane.is_dirty(),
            "undo back to the saved baseline must clear the dirty dot"
        );

        // Redo back into the divergent text -> dirty re-sets.
        assert!(pane.redo());
        assert_eq!(pane.lines, vec!["hello".to_string()]);
        assert!(
            pane.is_dirty(),
            "redo into divergent text must re-set the dirty dot"
        );

        // A save re-anchors the baseline so the edited text reads clean.
        pane.mark_saved();
        assert!(!pane.is_dirty(), "mark_saved should clear dirty");
    }

    #[test]
    fn roster_dot_click_queues_reveal_without_moving_cursor() {
        let mut pane = pane_for_test();
        pane.lines = (0..40).map(|ix| format!("line {ix}")).collect();
        pane.cursor_line = 1;
        pane.cursor_col = 3;
        pane.register_roster_rect([100.0, 10.0, 18.0, 18.0], 30);

        // Miss: outside the dot.
        assert!(!pane.roster_jump_at(50.0, 50.0));
        assert_eq!(pane.pending_reveal_line, None);

        // Hit: queues the peer's line for the next render frame and
        // leaves the LOCAL caret alone.
        assert!(pane.roster_jump_at(109.0, 19.0));
        assert_eq!(pane.pending_reveal_line, Some(30));
        assert_eq!(pane.cursor_line, 1);
        assert_eq!(pane.cursor_col, 3);
    }

    #[test]
    fn roster_jump_clamps_stale_peer_line_to_document() {
        let mut pane = pane_for_test();
        pane.lines = vec!["only".to_string(), "two".to_string()];
        pane.register_roster_rect([0.0, 0.0, 18.0, 18.0], 99);

        assert!(pane.roster_jump_at(9.0, 9.0));
        assert_eq!(pane.pending_reveal_line, Some(1));
    }
}
