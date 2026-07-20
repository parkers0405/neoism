use super::feed::*;
use super::layout::*;
use super::types::*;
use crate::syntax::{Lang, SynTok};

fn buffer(text: &str) -> CodeBuffer {
    CodeBuffer::from_text(text)
}

#[test]
fn from_text_roundtrip_preserves_line_ending_and_trailing_newline() {
    let crlf = CodeBuffer::from_text("a\r\nb\r\n");
    assert_eq!(crlf.lines, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(crlf.text(), "a\nb");
    assert_eq!(crlf.text_for_disk(), "a\r\nb\r\n");

    let lf = CodeBuffer::from_text("a\nb");
    assert_eq!(lf.text_for_disk(), "a\nb");

    let empty = CodeBuffer::from_text("");
    assert_eq!(empty.lines, vec![String::new()]);
}

#[test]
fn insert_burst_coalesces_into_one_undo_entry() {
    let mut buf = buffer("fn main() {}\n");
    buf.insert_char('a');
    buf.insert_char('b');
    buf.insert_char('c');
    assert_eq!(buf.lines[0], "abcfn main() {}");
    assert!(buf.undo());
    assert_eq!(buf.lines[0], "fn main() {}");
    // One undo reverted the whole burst; a second has nothing to do.
    assert!(!buf.undo());
    assert!(buf.redo());
    assert_eq!(buf.lines[0], "abcfn main() {}");
}

#[test]
fn motion_breaks_insert_burst() {
    let mut buf = buffer("xy\n");
    buf.insert_char('a');
    buf.apply_motion(CodeMotion::Left, false);
    buf.insert_char('b');
    assert_eq!(buf.lines[0], "baxy");
    assert!(buf.undo());
    assert_eq!(buf.lines[0], "axy");
    assert!(buf.undo());
    assert_eq!(buf.lines[0], "xy");
}

#[test]
fn newline_carries_auto_indent() {
    let mut buf = buffer("    let x = 1;\n");
    buf.cursor_line = 0;
    buf.cursor_col = buf.lines[0].len();
    buf.insert_newline();
    assert_eq!(buf.cursor_line, 1);
    assert_eq!(buf.lines[1], "    ");
    assert_eq!(buf.cursor_col, 4);
    // Undo restores the single original line.
    assert!(buf.undo());
    assert_eq!(buf.lines.len(), 1);
    assert_eq!(buf.lines[0], "    let x = 1;");
}

#[test]
fn paste_is_verbatim_no_auto_indent() {
    let mut buf = buffer("    body\n");
    buf.cursor_line = 0;
    buf.cursor_col = buf.lines[0].len();
    buf.insert_text("\nplain");
    assert_eq!(buf.lines[1], "plain");
}

#[test]
fn vertical_goal_column_sticks_through_short_lines() {
    let mut buf = buffer("longer line\nab\nanother long line\n");
    buf.cursor_line = 0;
    buf.cursor_col = 8;
    buf.apply_motion(CodeMotion::Down, false);
    assert_eq!(buf.cursor_line, 1);
    assert_eq!(buf.cursor_col, 2); // clamped to "ab"
    buf.apply_motion(CodeMotion::Down, false);
    assert_eq!(buf.cursor_line, 2);
    assert_eq!(buf.cursor_col, 8); // goal restored
}

#[test]
fn smart_home_toggles_between_indent_and_column_zero() {
    let mut buf = buffer("    code here\n");
    buf.cursor_col = 9;
    buf.apply_motion(CodeMotion::LineStartSmart, false);
    assert_eq!(buf.cursor_col, 4);
    buf.apply_motion(CodeMotion::LineStartSmart, false);
    assert_eq!(buf.cursor_col, 0);
    buf.apply_motion(CodeMotion::LineStartSmart, false);
    assert_eq!(buf.cursor_col, 4);
}

#[test]
fn word_motions_hop_symbols_and_words() {
    let mut buf = buffer("foo_bar(baz, qux)\n");
    buf.apply_motion(CodeMotion::WordRight, false);
    assert_eq!(buf.cursor_col, 7); // at '('
    buf.apply_motion(CodeMotion::WordRight, false);
    assert_eq!(buf.cursor_col, 8); // at "baz"
    buf.apply_motion(CodeMotion::WordLeft, false);
    assert_eq!(buf.cursor_col, 7);
    buf.apply_motion(CodeMotion::WordLeft, false);
    assert_eq!(buf.cursor_col, 0);
}

#[test]
fn shift_selection_and_type_over_replaces() {
    let mut buf = buffer("hello world\n");
    buf.apply_motion(CodeMotion::WordRight, true);
    assert!(buf.has_selection());
    assert_eq!(buf.selected_text().as_deref(), Some("hello "));
    buf.insert_char('X');
    assert_eq!(buf.lines[0], "Xworld");
}

#[test]
fn plain_arrow_collapses_selection_to_edge() {
    let mut buf = buffer("hello\n");
    buf.apply_motion(CodeMotion::Right, true);
    buf.apply_motion(CodeMotion::Right, true);
    assert!(buf.has_selection());
    buf.apply_motion(CodeMotion::Left, false);
    assert!(!buf.has_selection());
    assert_eq!(buf.cursor_col, 0); // collapsed to selection start
}

#[test]
fn multi_line_selection_delete_joins_lines() {
    let mut buf = buffer("alpha\nbeta\ngamma\n");
    buf.cursor_line = 0;
    buf.cursor_col = 2;
    buf.set_cursor_position(2, 3, true);
    assert!(buf.delete_selection());
    assert_eq!(buf.lines, vec!["alma".to_string()]);
    assert_eq!(buf.cursor_line, 0);
    assert_eq!(buf.cursor_col, 2);
    assert!(buf.undo());
    assert_eq!(buf.lines.len(), 3);
    assert_eq!(buf.lines[1], "beta");
}

#[test]
fn indent_and_outdent_selection_lines() {
    let mut buf = buffer("one\ntwo\nthree\n");
    buf.set_cursor_position(0, 0, false);
    buf.set_cursor_position(1, 3, true);
    buf.insert_tab();
    assert_eq!(buf.lines[0], "    one");
    assert_eq!(buf.lines[1], "    two");
    assert_eq!(buf.lines[2], "three");
    buf.outdent();
    assert_eq!(buf.lines[0], "one");
    assert_eq!(buf.lines[1], "two");
}

#[test]
fn tab_pads_to_next_tab_stop() {
    let mut buf = buffer("ab\n");
    buf.cursor_col = 2;
    buf.insert_tab();
    assert_eq!(buf.lines[0], "ab  "); // 2 spaces to reach col 4
    assert_eq!(buf.cursor_col, 4);
}

#[test]
fn backspace_in_leading_spaces_eats_to_tab_stop() {
    let mut buf = buffer("      x\n");
    buf.cursor_col = 6;
    buf.backspace();
    assert_eq!(buf.lines[0], "    x");
    assert_eq!(buf.cursor_col, 4);
}

#[test]
fn delete_current_line_undo_redo() {
    let mut buf = buffer("one\ntwo\nthree\n");
    buf.cursor_line = 1;
    let removed = buf.delete_current_line();
    assert_eq!(removed, "two");
    assert_eq!(buf.lines, vec!["one".to_string(), "three".to_string()]);
    assert!(buf.undo());
    assert_eq!(buf.lines.len(), 3);
    assert_eq!(buf.lines[1], "two");
    assert!(buf.redo());
    assert_eq!(buf.lines.len(), 2);
}

#[test]
fn copy_and_cut_payloads_fall_back_to_line() {
    let mut buf = buffer("alpha\nbeta\n");
    let (text, linewise) = buf.copy_payload();
    assert_eq!(text, "alpha");
    assert!(linewise);

    buf.apply_motion(CodeMotion::Right, true);
    let (text, linewise) = buf.copy_payload();
    assert_eq!(text, "a");
    assert!(!linewise);

    buf.clear_selection();
    let (text, linewise) = buf.cut_payload();
    assert_eq!(text, "alpha");
    assert!(linewise);
    assert_eq!(buf.lines[0], "beta");
}

#[test]
fn dirty_tracks_saved_baseline_not_monotonic_flag() {
    let mut buf = buffer("text\n");
    assert!(!buf.is_dirty());
    buf.insert_char('a');
    assert!(buf.is_dirty());
    assert!(buf.undo());
    assert!(!buf.is_dirty()); // undoing back to baseline clears dirty
    buf.insert_char('b');
    buf.mark_saved();
    assert!(!buf.is_dirty());
}

#[test]
fn doc_bound_history_queues_requests() {
    let mut buf = buffer("text\n");
    buf.set_doc_history_bound(true);
    buf.insert_char('a');
    assert!(buf.undo());
    assert!(buf.redo());
    assert_eq!(
        buf.take_doc_history_requests(),
        vec![CodeDocHistoryRequest::Undo, CodeDocHistoryRequest::Redo]
    );
    assert!(buf.take_doc_history_requests().is_empty());
}

#[test]
fn select_all_and_selection_text() {
    let mut buf = buffer("one\ntwo\n");
    buf.select_all();
    assert_eq!(buf.selected_text().as_deref(), Some("one\ntwo"));
}

#[test]
fn styled_runs_cover_line_and_merge_overlays() {
    // Lang::Other uses the keyword fallback → single Plain span, so the
    // run boundaries below come purely from selection + diagnostics.
    let line = "abcdefgh";
    let runs = styled_runs_for_line(line, Lang::Other, None, &[]);
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].start, runs[0].end), (0, 8));
    assert!(!runs[0].selected);
    assert_eq!(runs[0].severity, None);

    let diag = CodeLineDiagnostic {
        start: 2,
        end: 6,
        message: String::new(),
        severity: CodeDiagnosticSeverity::Error,
    };
    let runs = styled_runs_for_line(line, Lang::Other, Some((4, 8)), &[diag]);
    // Expected cuts: 0..2 plain, 2..4 error, 4..6 error+selected, 6..8 selected.
    assert_eq!(runs.len(), 4);
    assert_eq!(
        runs.iter()
            .map(|run| (run.start, run.end, run.selected, run.severity.is_some()))
            .collect::<Vec<_>>(),
        vec![
            (0, 2, false, false),
            (2, 4, false, true),
            (4, 6, true, true),
            (6, 8, true, false),
        ]
    );
    // Runs tile the whole line with no gaps.
    for pair in runs.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
    }
}

#[test]
fn styled_runs_strongest_severity_wins() {
    let warn = CodeLineDiagnostic {
        start: 0,
        end: 4,
        message: String::new(),
        severity: CodeDiagnosticSeverity::Warn,
    };
    let error = CodeLineDiagnostic {
        start: 2,
        end: 4,
        message: String::new(),
        severity: CodeDiagnosticSeverity::Error,
    };
    let runs = styled_runs_for_line("abcd", Lang::Other, None, &[warn, error]);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].severity, Some(CodeDiagnosticSeverity::Warn));
    assert_eq!(runs[1].severity, Some(CodeDiagnosticSeverity::Error));
}

#[test]
fn selection_on_line_spans_middle_lines_fully() {
    let mut buf = buffer("alpha\nbeta\ngamma\n");
    buf.set_cursor_position(0, 2, false);
    buf.set_cursor_position(2, 3, true);
    assert_eq!(buf.selection_on_line(0), Some((2, 5)));
    assert_eq!(buf.selection_on_line(1), Some((0, 4)));
    assert_eq!(buf.selection_on_line(2), Some((0, 3)));
    assert_eq!(buf.selection_on_line(3), None);
    // Keyword fallback still produces tokens for real languages.
    let runs = styled_runs_for_line("let x = 1;", Lang::Rust, None, &[]);
    assert!(runs.iter().any(|run| run.token == SynTok::Keyword));
}

#[test]
fn tab_display_columns_round_trip() {
    let line = "\tfn x(\t) {";
    // Tab at col 0 expands to 4; 'f' starts at display col 4.
    assert_eq!(display_col_for_byte(line, 0, 4), 0);
    assert_eq!(display_col_for_byte(line, 1, 4), 4);
    // Second tab starts at col 10 ("fn x(" = 5 cells after col 4 → 9,
    // wait: f=4,n=5,space=6,x=7,(=8 → tab at byte 6 starts col 9, pads to 12.
    assert_eq!(display_col_for_byte(line, 6, 4), 9);
    assert_eq!(display_col_for_byte(line, 7, 4), 12);
    // Hit-testing lands inside the tab cell back at the tab's byte.
    assert_eq!(byte_for_display_col(line, 0, 4), 0);
    assert_eq!(byte_for_display_col(line, 3, 4), 0);
    assert_eq!(byte_for_display_col(line, 4, 4), 1);
    assert_eq!(byte_for_display_col(line, 10, 4), 6);
    // Past line end clamps.
    assert_eq!(byte_for_display_col(line, 99, 4), line.len());
}

#[test]
fn expand_tabs_respects_running_column() {
    assert_eq!(expand_tabs_from("\tx", 0, 4), "    x");
    assert_eq!(expand_tabs_from("\tx", 2, 4), "  x");
    assert_eq!(expand_tabs_from("ab", 3, 4), "ab");
    assert_eq!(display_width("\t\t", 4), 8);
}

#[test]
fn gutter_width_floors_at_three_digits() {
    assert_eq!(gutter_digits(1), 3);
    assert_eq!(gutter_digits(999), 3);
    assert_eq!(gutter_digits(1000), 4);
    assert_eq!(gutter_digits(43210), 5);
}

#[test]
fn geometry_hit_position_maps_rows_and_tabs() {
    let lines = vec!["\tindent".to_string(), "plain".to_string()];
    // Default (never-built) wrap index degrades to the identity
    // mapping — the NoWrap behavior.
    let geometry = CodePaneGeometry {
        rect: [0.0, 100.0, 800.0, 400.0],
        text_x: 60.0,
        gutter_w: 52.0,
        cell_w: 10.0,
        row_h: 20.0,
        first_row: 0,
        scroll_y: 0.0,
        scroll_x: 0.0,
        wrap: std::sync::Arc::default(),
    };
    // Click row 0 inside the tab's 4-cell span → byte 0.
    assert_eq!(geometry.hit_position(&lines, 75.0, 105.0), (0, 0));
    // Click row 0 at display col 4 → byte 1 ('i').
    assert_eq!(geometry.hit_position(&lines, 100.0, 110.0), (0, 1));
    // Click second row.
    assert_eq!(geometry.hit_position(&lines, 82.0, 125.0), (1, 2));
    // Click below the last line clamps to it.
    assert_eq!(geometry.hit_position(&lines, 60.0, 390.0), (1, 0));
    assert_eq!(geometry.viewport_rows(), 20);
}

// --- soft wrap ---

fn wrap_lines(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|t| t.to_string()).collect()
}

#[test]
fn wrap_index_prefix_sum_and_totals() {
    // Widths 10, 0, 25 at 10 cols → 1, 1, 3 visual rows.
    let lines = wrap_lines(&["a234567890", "", &"x".repeat(25)]);
    let index = WrapIndex::build(&lines, 10, TAB_DISPLAY_WIDTH);
    assert_eq!(index.cols(), 10);
    assert!(index.is_valid_for(3));
    assert_eq!(index.total_rows(3), 5);
    assert_eq!(index.first_row_of_line(0), 0);
    assert_eq!(index.first_row_of_line(1), 1);
    assert_eq!(index.first_row_of_line(2), 2);
    // Half-open convention: line == count yields the total.
    assert_eq!(index.first_row_of_line(3), 5);
    assert_eq!(index.rows_of_line(0), 1);
    assert_eq!(index.rows_of_line(1), 1);
    assert_eq!(index.rows_of_line(2), 3);

    // A width that's an exact multiple of cols does NOT gain a
    // trailing empty row (20 wide / 10 cols = 2 rows).
    let exact = wrap_lines(&[&"y".repeat(20)]);
    let index = WrapIndex::build(&exact, 10, TAB_DISPLAY_WIDTH);
    assert_eq!(index.total_rows(1), 2);

    // NoWrap build (cols = 0) is the identity.
    let index = WrapIndex::build(&lines, 0, TAB_DISPLAY_WIDTH);
    assert_eq!(index.total_rows(3), 3);
    assert_eq!(index.line_of_row(2, 3), (2, 0));
}

#[test]
fn wrap_visual_buffer_mapping_round_trips() {
    let lines = wrap_lines(&["ab", &"z".repeat(23), "", "tail"]);
    let index = WrapIndex::build(&lines, 8, TAB_DISPLAY_WIDTH);
    for (line, text) in lines.iter().enumerate() {
        let rows = wrap_rows(text, 8, TAB_DISPLAY_WIDTH);
        assert_eq!(index.rows_of_line(line), rows);
        for seg in 0..rows {
            let vrow = index.first_row_of_line(line) + seg;
            assert_eq!(index.line_of_row(vrow, lines.len()), (line, seg));
        }
    }
    // Past-the-end visual rows clamp to the buffer's last row.
    let total = index.total_rows(lines.len());
    assert_eq!(index.line_of_row(total + 5, lines.len()), (3, 0));
    // A stale index (wrong line count) degrades to identity.
    assert_eq!(index.line_of_row(1, 2), (1, 0));
}

#[test]
fn wrap_segment_starts_cut_on_char_boundaries() {
    // 10 chars at 4 cols → segments at bytes 0, 4, 8.
    assert_eq!(
        wrap_segment_starts("abcdefghij", 4, TAB_DISPLAY_WIDTH),
        vec![(0, 0), (4, 4), (8, 8)]
    );
    // Tab straddling the wrap column: "abcde\tx" at 6 cols — the tab
    // starts at col 5, pads to col 8, so it opens segment 1.
    assert_eq!(
        wrap_segment_starts("abcde\tx", 6, TAB_DISPLAY_WIDTH),
        vec![(0, 0), (5, 5)]
    );
    assert_eq!(wrap_rows("abcde\tx", 6, TAB_DISPLAY_WIDTH), 2);
    // Empty line and NoWrap keep a single segment.
    assert_eq!(wrap_segment_starts("", 4, TAB_DISPLAY_WIDTH), vec![(0, 0)]);
    assert_eq!(
        wrap_segment_starts("abcdefghij", 0, TAB_DISPLAY_WIDTH),
        vec![(0, 0)]
    );
}

#[test]
fn wrap_visual_position_places_caret_on_continuation_rows() {
    let line = "abcdefghij"; // 10 wide, 4 cols → rows of width 4/4/2
    assert_eq!(wrap_visual_position(line, 0, 4, TAB_DISPLAY_WIDTH), (0, 0));
    assert_eq!(wrap_visual_position(line, 3, 4, TAB_DISPLAY_WIDTH), (0, 3));
    assert_eq!(wrap_visual_position(line, 4, 4, TAB_DISPLAY_WIDTH), (1, 0));
    assert_eq!(wrap_visual_position(line, 6, 4, TAB_DISPLAY_WIDTH), (1, 2));
    // EOL caret rests one cell past the last char of the last segment.
    assert_eq!(wrap_visual_position(line, 10, 4, TAB_DISPLAY_WIDTH), (2, 2));
    // EOL on an exact-fit line sits at the right edge, not a new row.
    assert_eq!(
        wrap_visual_position("abcdefgh", 8, 4, TAB_DISPLAY_WIDTH),
        (1, 4)
    );
    // NoWrap: full display column on segment 0.
    assert_eq!(wrap_visual_position(line, 6, 0, TAB_DISPLAY_WIDTH), (0, 6));
}

#[test]
fn hit_position_resolves_wrapped_visual_rows() {
    let lines = wrap_lines(&["abcdefghij", "next"]);
    let geometry = CodePaneGeometry {
        rect: [0.0, 100.0, 800.0, 400.0],
        text_x: 60.0,
        gutter_w: 52.0,
        cell_w: 10.0,
        row_h: 20.0,
        first_row: 0,
        scroll_y: 0.0,
        scroll_x: 0.0,
        wrap: std::sync::Arc::new(WrapIndex::build(&lines, 4, TAB_DISPLAY_WIDTH)),
    };
    // Visual row 0, col 2 → byte 2.
    assert_eq!(geometry.hit_position(&lines, 85.0, 105.0), (0, 2));
    // Visual row 1 (continuation), col 2 → byte 6.
    assert_eq!(geometry.hit_position(&lines, 85.0, 125.0), (0, 6));
    // Clicking the slack right of a wrapped row parks on that row's
    // last char (byte 7), not the next visual row.
    assert_eq!(geometry.hit_position(&lines, 500.0, 125.0), (0, 7));
    // Last segment: clicking past EOL clamps to line end.
    assert_eq!(geometry.hit_position(&lines, 500.0, 145.0), (0, 10));
    // Visual row 3 is buffer line 1.
    assert_eq!(geometry.hit_position(&lines, 62.0, 165.0), (1, 0));
    // Fractional glide offset still lands on the padded visual row.
    let scrolled = CodePaneGeometry {
        scroll_y: 30.0,
        ..geometry.clone()
    };
    // my=105 → content y 135 → visual row 1 → (0, byte 4 + cells).
    assert_eq!(scrolled.hit_position(&lines, 60.0, 105.0), (0, 4));
}

#[test]
fn hit_position_nowrap_honors_horizontal_scroll() {
    let lines = wrap_lines(&["0123456789abcdef"]);
    let geometry = CodePaneGeometry {
        rect: [0.0, 0.0, 400.0, 200.0],
        text_x: 60.0,
        gutter_w: 52.0,
        cell_w: 10.0,
        row_h: 20.0,
        first_row: 0,
        scroll_y: 0.0,
        scroll_x: 50.0,
        wrap: std::sync::Arc::new(WrapIndex::build(&lines, 0, TAB_DISPLAY_WIDTH)),
    };
    // Pointer at text_x sits at display col 5 once scroll_x shifts.
    assert_eq!(geometry.hit_position(&lines, 60.0, 5.0), (0, 5));
    assert_eq!(geometry.hit_position(&lines, 90.0, 5.0), (0, 8));
}

#[test]
fn wheel_drag_along_tracks_center_visual_row() {
    use std::path::PathBuf;
    let text = format!("{}\n{}\n{}\n", "w".repeat(30), "short", "tail");
    let mut pane = CodePane::new(PathBuf::from("wrap.rs"), &text);
    // Paint-time state the wheel math reads: 10-col wrap → line 0
    // spans 3 visual rows; 5 total. Viewport shows 2 rows of 20px.
    pane.wrap_index =
        std::sync::Arc::new(WrapIndex::build(&pane.buffer.lines, 10, TAB_DISPLAY_WIDTH));
    pane.geometry.row_h = 20.0;
    pane.content_height = 5.0 * 20.0;
    // Scroll down 3 rows: center visual row = 3 + (2-1)/2 ≈ row 4 →
    // buffer line 2 under the wrap index (rows 0-2 are line 0).
    pane.scroll_pixels(-60.0, 40.0);
    assert_eq!(pane.buffer.cursor_line, 2);
    // Identity index would have parked on a nonexistent "line 4" and
    // clamped; the wrap index maps through segments instead.
}

#[test]
fn indent_detection() {
    let tabs = CodeBuffer::from_text("fn x() {\n\tbody\n}\n");
    assert!(tabs.indent.use_tabs);
    let two = CodeBuffer::from_text("a:\n  b: 1\n");
    assert!(!two.indent.use_tabs);
    assert_eq!(two.indent.width, 2);
    let four = CodeBuffer::from_text("fn x() {\n    body\n}\n");
    assert_eq!(four.indent.width, 4);
}
