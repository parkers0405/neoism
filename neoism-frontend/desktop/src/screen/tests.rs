// Tests moved from screen/mod.rs.

use super::*;
use neoism_terminal_core::crosswords::square::LineLength;
use neoism_ui::editor::scroll_model::raw_scroll_has_room;
use neoism_ui::editor::selection_model::post_process_hyperlink_uri;

#[test]
fn editor_scroll_render_offset_uses_mutated_snapshot_split() {
    // The desktop editor renders daemon-applied grid snapshots (nvim's
    // `grid_scroll` is already baked in), so `editor_scroll_render_offset`
    // forwards to `editor_scroll_render_offset_for_mutated_snapshot`: positive
    // fractional scroll CEILs the source line (keep sampling the previous
    // visible row) and eases the new top row in from above. The plain floor
    // split is covered by `neoism_ui::render_policy`'s own test.
    let cell = 20.0;

    assert_eq!(
        editor_scroll_render_offset(-1.0, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: -1,
            pixel_offset_y: 0.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset(-0.2, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: -1,
            pixel_offset_y: -16.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset(1.0, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: 1,
            pixel_offset_y: 0.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset(0.2, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: 1,
            pixel_offset_y: 16.0,
        }
    );
    // Round to whole pixels so the GPU uniform path stays on cell
    // boundaries — see neoism_ui::render_policy. (1 - 0.125) * 39 = 34.125.
    assert_eq!(
        editor_scroll_render_offset(0.125, 0.0, 39.0, None),
        EditorScrollRenderOffset {
            source_line_offset: 1,
            pixel_offset_y: 34.0,
        }
    );
}

#[test]
fn inline_diagnostic_uses_the_grid_source_inverse_during_fractional_scroll() {
    let cell_h = 20.0;
    let visible_rows = 10;
    // Buffer line 101 is source row zero when nvim's 0-based topline is 100.
    let line = 101;
    let topline = 100;

    // Scrolling down: the already-mutated snapshot starts with the new top
    // line at output row 1, then glides it upward with the pixel residual.
    let placement =
        editor_inline_diagnostic_placement(line, topline, -1, -8.0, cell_h, visible_rows)
            .expect("new top line remains visible during the fractional glide");
    assert_eq!(placement.source_row, 0);
    assert_eq!(placement.output_row, 1);
    assert_eq!(
        placement.output_row - 1,
        placement.source_row,
        "diagnostic placement must invert grid source = output + offset",
    );
    assert_eq!(placement.output_row as f32 * cell_h - 8.0, 12.0);

    // At rest the identical diagnostic lands on output row zero. The
    // transition is continuous because row 1 at -20px and row 0 at 0px
    // describe the same physical position at the integer boundary.
    let settled =
        editor_inline_diagnostic_placement(line, topline, 0, 0.0, cell_h, visible_rows)
            .unwrap();
    assert_eq!(settled.source_row, 0);
    assert_eq!(settled.output_row, 0);
}

#[test]
fn inline_diagnostic_keeps_fractional_top_and_bottom_edge_rows() {
    let cell_h = 20.0;
    let visible_rows = 10;
    let topline = 100;

    // Scrolling up: the current top line is in the retained row above the
    // ordinary output band and is still partially visible after +8px.
    let top_edge =
        editor_inline_diagnostic_placement(101, topline, 1, 8.0, cell_h, visible_rows)
            .unwrap();
    assert_eq!(top_edge.source_row, 0);
    assert_eq!(top_edge.output_row, -1);

    // Scrolling down: the last current viewport line is in the retained
    // row below the ordinary band and is partially visible after -8px.
    let bottom_edge =
        editor_inline_diagnostic_placement(110, topline, -1, -8.0, cell_h, visible_rows)
            .unwrap();
    assert_eq!(bottom_edge.source_row, 9);
    assert_eq!(bottom_edge.output_row, 10);

    // The same edge rows do not exist in the visible clip once the pixel
    // residual reaches zero.
    assert!(editor_inline_diagnostic_placement(
        101,
        topline,
        1,
        0.0,
        cell_h,
        visible_rows,
    )
    .is_none());
    assert!(editor_inline_diagnostic_placement(
        110,
        topline,
        -1,
        0.0,
        cell_h,
        visible_rows,
    )
    .is_none());
}

#[test]
fn inline_diagnostic_reappears_at_the_same_row_after_scrolling_away_and_back() {
    let before = editor_inline_diagnostic_placement(123, 120, 0, 0.0, 20.0, 10)
        .expect("diagnostic starts in the viewport");
    assert_eq!(before.output_row, 2);

    assert!(
        editor_inline_diagnostic_placement(123, 150, 0, 0.0, 20.0, 10).is_none(),
        "offscreen diagnostics are culled, not deleted",
    );

    let after = editor_inline_diagnostic_placement(123, 120, 0, 0.0, 20.0, 10)
        .expect("the unchanged diagnostic must reappear after returning");
    assert_eq!(after, before);
}

#[test]
fn inline_diagnostic_rejects_invalid_or_unrepresentable_lines() {
    assert!(editor_inline_diagnostic_placement(0, 0, 0, 0.0, 20.0, 10).is_none());
    assert!(editor_inline_diagnostic_placement(u64::MAX, 0, 0, 0.0, 20.0, 10,).is_none());
}

#[test]
fn daemon_editor_row_trims_explicit_space_cells_for_inline_lens_width() {
    let mut row = Row::<Square>::new(80);
    let code = "let value = missing_name;";
    for (column, ch) in code.chars().enumerate() {
        row[Column(column)].set_c(ch);
    }
    // Daemon GridUpdate carries the cleared tail as real spaces rather than
    // the terminal grid's default NUL cells. `LineLength` consequently reads
    // this as a full 80-column row, which used to leave no room for a lens.
    for column in code.len()..80 {
        row[Column(column)].set_c(' ');
    }

    assert_eq!(row.line_length().0, 80, "reproduce the daemon-row trap");
    assert_eq!(
        editor_row_text_end_col(&row, 80),
        code.len() as u32,
        "inline placement must use visual code width, not cleared tail width",
    );
}

#[test]
fn editor_row_text_end_keeps_internal_whitespace_and_honors_column_cap() {
    let mut row = Row::<Square>::new(20);
    for (column, ch) in "a  b trailing".chars().enumerate() {
        row[Column(column)].set_c(ch);
    }

    assert_eq!(editor_row_text_end_col(&row, 20), 13);
    assert_eq!(editor_row_text_end_col(&row, 4), 4);
}

#[test]
fn absolute_row_mapping_round_trips_visible_indices() {
    // history_size + line = absolute row. When scrolled back by 4,
    // visible index 2 maps to terminal line -2 and abs 98.
    let history_size = 100;
    let display_offset = 4;
    let abs = 98;

    assert_eq!(line_for_absolute_row(abs, history_size), Line(-2));
    assert_eq!(
        visible_index_for_absolute_row(abs, history_size, display_offset),
        Some(2)
    );
}

#[test]
fn composed_cursor_row_skips_virtual_chrome_rows() {
    let sources = [None, None, Some(42), Some(43), None, Some(44)];

    assert_eq!(composed_display_row_for_abs(&sources, 42), Some(2));
    assert_eq!(composed_display_row_for_abs(&sources, 44), Some(5));
    assert_eq!(composed_display_row_for_abs(&sources, 45), None);
}

#[test]
fn composer_prompt_row_trim_drops_matching_row_even_when_shell_painted_it() {
    fn row_with_text(text: &str) -> Row<Square> {
        let mut row = Row::new(8);
        for (idx, ch) in text.chars().enumerate() {
            row.inner[idx] = Square::from_char(ch);
        }
        row
    }

    let mut rows = vec![row_with_text("out"), Row::new(8), row_with_text("next")];
    let mut sources = vec![10, 11, 12];

    drop_composer_owned_prompt_row(&mut rows, &mut sources, Some(11));
    assert_eq!(sources, vec![10, 12]);

    drop_composer_owned_prompt_row(&mut rows, &mut sources, Some(10));
    assert_eq!(sources, vec![12]);
}

#[test]
fn block_scroll_cursor_survives_raw_viewport_mismatch() {
    let existing = crate::terminal::scroll::BlockScrollCursor {
        raw_top_abs: 91,
        chrome_row: 1,
    };

    assert_eq!(block_scroll_cursor_or_anchor(Some(existing), 89), existing);
    assert_eq!(
        block_scroll_cursor_or_anchor(None, 89),
        crate::terminal::scroll::BlockScrollCursor {
            raw_top_abs: 89,
            chrome_row: 0,
        }
    );
}

#[test]
fn raw_scroll_room_detects_mid_history_recovery() {
    assert!(raw_scroll_has_room(1, 4, 10));
    assert!(raw_scroll_has_room(-1, 4, 10));
    assert!(!raw_scroll_has_room(1, 10, 10));
    assert!(!raw_scroll_has_room(-1, 0, 10));
    assert!(!raw_scroll_has_room(0, 4, 10));
}

#[test]
fn passthrough_shell_detection_handles_raw_remote_sessions() {
    assert!(!starts_passthrough_session("bash"));
    assert!(!starts_passthrough_session("zsh"));
    assert!(!starts_passthrough_session("fish"));
    assert!(!starts_passthrough_session("nix-shell"));
    assert!(starts_passthrough_session("sh"));
    assert!(!starts_passthrough_session("bash script.sh"));
    assert!(starts_passthrough_session("ssh devbox"));
    assert!(starts_passthrough_session("ssh -t devbox"));
    assert!(starts_passthrough_session("/usr/bin/ssh devbox"));
    assert!(starts_passthrough_session("mosh devbox"));
    assert!(!starts_passthrough_session("python"));
    assert!(ends_passthrough_session("exit"));
    assert!(ends_passthrough_session(" logout "));
    assert!(!ends_passthrough_session("exit now"));
}

#[test]
fn git_state_event_filter_ignores_lock_churn() {
    let event =
        notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(PathBuf::from("/repo/.git/index.lock"));

    assert!(!git_state_event_relevant(&event));
}

#[test]
fn git_state_event_filter_ignores_pathless_churn() {
    let event =
        notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any));

    assert!(!git_state_event_relevant(&event));
}

#[test]
fn git_state_event_filter_accepts_stable_git_state() {
    let index = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(PathBuf::from("/repo/.git/index"));
    let head = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(PathBuf::from("/repo/.git/HEAD"));
    let branch_ref = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(PathBuf::from("/repo/.git/refs/heads/main"));

    assert!(git_state_event_relevant(&index));
    assert!(git_state_event_relevant(&head));
    assert!(git_state_event_relevant(&branch_ref));
}

#[test]
fn file_tree_fs_event_filter_ignores_noisy_paths() {
    let root = PathBuf::from("/repo");
    let git = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join(".git/index"));
    let target = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join("target/debug/build/output"));
    let claude = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join(".claude/worktrees/agent-a123/src/main.rs"));
    let swap = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join("src/main.rs.swp"));

    assert!(!file_tree_fs_event_relevant(&root, &git));
    assert!(!file_tree_fs_event_relevant(&root, &target));
    assert!(!file_tree_fs_event_relevant(&root, &claude));
    assert!(!file_tree_fs_event_relevant(&root, &swap));
}

#[test]
fn file_tree_fs_event_filter_accepts_worktree_changes() {
    let root = PathBuf::from("/repo");
    let modified = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join("src/main.rs"));
    let created =
        notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(root.join("src/new_file.rs"));
    let write_closed = notify::Event::new(notify::EventKind::Access(
        notify::event::AccessKind::Close(notify::event::AccessMode::Write),
    ))
    .add_path(root.join("src/saved.rs"));

    assert!(file_tree_fs_event_relevant(&root, &modified));
    assert!(file_tree_fs_event_relevant(&root, &created));
    assert!(file_tree_fs_event_relevant(&root, &write_closed));
}

#[test]
fn editor_scroll_elastic_does_not_change_source_row() {
    let cell = 20.0;

    assert_eq!(
        editor_scroll_render_offset(-1.0, 7.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: -1,
            pixel_offset_y: 7.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset(0.0, 7.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: 0,
            pixel_offset_y: 7.0,
        }
    );
}

#[test]
fn editor_scroll_render_offset_ignores_invalid_cell_height() {
    assert_eq!(
        editor_scroll_render_offset(1.0, 7.0, 0.0, None),
        EditorScrollRenderOffset::default()
    );
}

#[test]
fn editor_scroll_state_changes_only_rebuilds_source_rows_on_line_change() {
    let previous = EditorScrollGridState {
        render: EditorScrollGridRenderState {
            source_line_offset: -1,
            pixel_offset_y: -12.0,
            scrollback_origin: None,
        },
        ..Default::default()
    };

    assert_eq!(
        editor_scroll_state_changes(
            Some(&previous),
            EditorScrollRenderOffset {
                source_line_offset: -1,
                pixel_offset_y: -8.0,
            },
            None,
        ),
        (false, true)
    );
    assert_eq!(
        editor_scroll_state_changes(
            Some(&previous),
            EditorScrollRenderOffset {
                source_line_offset: -2,
                pixel_offset_y: -8.0,
            },
            None,
        ),
        (true, true)
    );
}

#[test]
fn editor_scroll_state_changes_uses_ring_origin_plus_source_offset() {
    let previous = EditorScrollGridState {
        render: EditorScrollGridRenderState {
            source_line_offset: -12,
            scrollback_origin: Some(100),
            pixel_offset_y: -4.0,
        },
        ..Default::default()
    };

    assert_eq!(
        editor_scroll_effective_source_base(Some(101), -13),
        editor_scroll_effective_source_base(Some(100), -12)
    );
    assert_eq!(
        editor_scroll_state_changes(
            Some(&previous),
            EditorScrollRenderOffset {
                source_line_offset: -13,
                pixel_offset_y: -4.0,
            },
            Some(101),
        ),
        (false, false)
    );
    assert_eq!(
        editor_scroll_state_changes(
            Some(&previous),
            EditorScrollRenderOffset {
                source_line_offset: -12,
                pixel_offset_y: -4.0,
            },
            Some(101),
        ),
        (true, false)
    );
}

// `editor_scroll_edge_rows_need_update` is now tested in
// `neoism_ui::render_policy::tests`. The native shim no longer wraps
// it because there's no call site here.

#[test]
fn editor_scroll_source_plan_shifts_small_integer_source_changes() {
    assert_eq!(
        editor_scroll_source_plan(Some(-1), -2, 10),
        EditorScrollSourcePlan::Shift {
            delta: -1,
            exposed: (0, 1),
        }
    );
    assert_eq!(
        editor_scroll_source_plan(Some(-2), -1, 10),
        EditorScrollSourcePlan::Shift {
            delta: 1,
            exposed: (9, 10),
        }
    );
    assert_eq!(
        editor_scroll_shifted_row_count(
            EditorScrollSourcePlan::Shift {
                delta: 1,
                exposed: (9, 10),
            },
            10,
        ),
        9
    );
}

#[test]
fn editor_scroll_source_plan_uses_effective_source_base() {
    // Holding down-arrow changes the nvim scrollback ring origin
    // on every one-line viewport scroll. If the spring's integer
    // source offset changes in the opposite direction, the
    // effective physical row base has not moved and resident rows
    // can stay untouched.
    assert_eq!(
        editor_scroll_source_plan(Some(88), 88, 34),
        EditorScrollSourcePlan::None
    );
    assert_eq!(
        editor_scroll_source_plan(Some(88), 89, 34),
        EditorScrollSourcePlan::Shift {
            delta: 1,
            exposed: (33, 34),
        }
    );
}

#[test]
fn editor_scroll_source_plan_rebuilds_large_integer_changes() {
    assert_eq!(
        editor_scroll_source_plan(Some(0), 10, 10),
        EditorScrollSourcePlan::RebuildAll
    );
    assert_eq!(
        editor_scroll_source_plan(Some(3), 3, 10),
        EditorScrollSourcePlan::None
    );
}

#[test]
fn editor_cursor_row_inverts_scroll_source_lookup() {
    let cursor_row = 10;
    for source_line_offset in [-3, -1, 0, 1, 3] {
        let output_row = editor_cursor_output_row(cursor_row, source_line_offset);

        assert_eq!(output_row + source_line_offset, cursor_row);
    }
}

#[test]
fn test_post_process_hyperlink_uri() {
    // Test removing trailing parenthesis
    assert_eq!(
        post_process_hyperlink_uri("https://example.com)"),
        "https://example.com"
    );

    // Test removing trailing comma
    assert_eq!(
        post_process_hyperlink_uri("https://example.com,"),
        "https://example.com"
    );

    // Test removing trailing period
    assert_eq!(
        post_process_hyperlink_uri("https://example.com."),
        "https://example.com"
    );

    // Test handling balanced parentheses (should keep them)
    assert_eq!(
        post_process_hyperlink_uri("https://example.com/path(with)parens"),
        "https://example.com/path(with)parens"
    );

    // Test handling unbalanced parentheses
    assert_eq!(
        post_process_hyperlink_uri("https://example.com/path)"),
        "https://example.com/path"
    );

    // Test handling multiple trailing delimiters
    assert_eq!(
        post_process_hyperlink_uri("https://example.com.'),"),
        "https://example.com"
    );

    // Test markdown-style URLs
    assert_eq!(
        post_process_hyperlink_uri("https://example.com)"),
        "https://example.com"
    );

    // Test handling unbalanced brackets
    assert_eq!(
        post_process_hyperlink_uri("https://example.com/path]"),
        "https://example.com/path"
    );

    // Test balanced brackets (should keep them)
    assert_eq!(
        post_process_hyperlink_uri("https://example.com/path[with]brackets"),
        "https://example.com/path[with]brackets"
    );
}
