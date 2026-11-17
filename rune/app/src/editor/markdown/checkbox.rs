//! Rendered task controls edit source bytes without borrowing the editor's caret.
use super::{Kind, Mode, layout, parsed};
use crate::app::App;

/// Consume only the measured checkbox glyph, never its trailing space or label.
/// `Kind::Task::marker` is the interior byte (the closing `]` for shorthand `[]`).
pub(super) fn press(app: &mut App, point: [f32; 2]) -> bool {
    let Some(buffer) = app.active_buffer() else { return false; };
    if !super::enabled(buffer) || !buffer.loaded || buffer.prompt.is_some() { return false; }
    let Some(rect) = buffer.view_rect else { return false; };
    // Do not shape an unrelated document for clicks in chrome or another pane.
    if !(point[0] >= rect[0] && point[0] < rect[0] + rect[2]
        && point[1] >= rect[1] && point[1] < rect[1] + rect[3]) { return false; }
    let laid = layout(app, buffer);
    let y = point[1] - rect[1] + buffer.scroll.get().0;
    let i = laid.rows.partition_point(|e| e.y <= y).saturating_sub(1);
    let Some(entry) = laid.rows.get(i) else { return false; };
    if y < entry.y || y >= entry.y + entry.row.height
        || (buffer.doc.mode == Mode::Insert && buffer.doc.cursor_line == entry.line)
    { return false; }
    let projected = parsed(buffer);
    let Kind::Task { checked, marker } = projected[entry.line].kind else { return false; };
    let Some(open) = marker.checked_sub(1) else { return false; };
    let x = point[0] - rect[0] - laid.left;
    let hit = entry.row.pieces.iter().any(|piece| {
        // The source map distinguishes the task prefix from checkbox characters
        // written in its label, including on wrapped continuation rows.
        let Some((byte, ch)) = piece.text.char_indices().find(|&(byte, ch)| {
            matches!(ch, '☐' | '☑') && piece.source[byte] == open
        }) else { return false; };
        piece.geometry.range_rects(byte..byte + ch.len_utf8()).into_iter().any(|r| {
            let local = [x - piece.x, y - entry.y];
            local[0] >= r[0] && local[0] < r[0] + r[2]
                && local[1] >= r[1] && local[1] < r[1] + r[3]
        })
    });
    if !hit { return false; }
    let line = entry.line;
    let doc = &mut app.active_buffer_mut().unwrap().doc;
    // Validate the parser contract before starting a transaction. For [] only
    // insert the interior byte; all other toggles are a one-byte replacement.
    let raw = doc.lines[line].as_bytes();
    if raw.get(open) != Some(&b'[') { return false; }
    let (end, replacement) = match raw.get(marker) {
        Some(b']') if !checked => (marker, "x"),
        Some(b' ') if raw.get(marker + 1) == Some(&b']') => (marker + 1, "x"),
        Some(b'x' | b'X') if raw.get(marker + 1) == Some(&b']') => (marker + 1, " "),
        _ => return false,
    };
    let follow = doc.follow_cursor;
    doc.end_insert_group();
    doc.break_undo_group();
    doc.save_undo();
    doc.lines[line].replace_range(marker..end, replacement);
    if end == marker {
        // [] grows by one byte: keep positions attached to the same source text.
        // Snapshot the original caret above so undo still returns to its old byte.
        if doc.cursor_line == line && doc.cursor_col >= marker { doc.cursor_col += 1; }
        if let Some(anchor) = &mut doc.visual_anchor {
            if anchor.line == line && anchor.col >= marker { anchor.col += 1; }
        }
    }
    doc.mark_edited();
    doc.follow_cursor = follow;
    doc.commit_undo();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::Focus, editor::vim::Pos};

    fn app(raw: &str, mode: Mode) -> App {
        let mut app = App::new();
        app.open_file("/notes/checkbox.md", None);
        app.apply_opened_file("/notes/checkbox.md", raw.as_bytes());
        let b = app.active_buffer_mut().unwrap();
        b.view_rect = Some([20.0, 30.0, 360.0, 240.0]);
        b.doc.mode = mode;
        b.doc.cursor_line = 0;
        b.doc.cursor_col = 2;
        b.doc.visual_anchor = Some(Pos { line: 0, col: 1 });
        b.doc.goal_visual_col = Some(19);
        b.doc.follow_cursor = false;
        app.focus = Focus::Sidebar;
        app.editor_follow = false;
        app
    }

    fn glyph_rect(app: &App, line: usize) -> [f32; 4] {
        let b = app.active_buffer().unwrap();
        let rect = b.view_rect.unwrap();
        let laid = layout(app, b);
        for entry in laid.rows.iter().filter(|e| e.line == line) {
            for piece in &entry.row.pieces {
                if let Some((byte, ch)) = piece.text.char_indices().find(|&(_, c)| c == '☐' || c == '☑') {
                    let r = piece.geometry.range_rects(byte..byte + ch.len_utf8())[0];
                    return [rect[0] + laid.left + piece.x + r[0],
                        rect[1] + entry.y - b.scroll.get().0 + r[1], r[2], r[3]];
                }
            }
        }
        panic!("missing rendered checkbox");
    }

    fn center(r: [f32; 4]) -> [f32; 2] { [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5] }

    fn assert_stationary(app: &App, mode: Mode, scroll: (f32, f32, f32)) {
        let b = app.active_buffer().unwrap();
        assert_eq!(b.doc.cursor(), Pos { line: 0, col: 2 });
        assert_eq!(b.doc.visual_anchor, Some(Pos { line: 0, col: 1 }));
        assert_eq!(b.doc.goal_visual_col, Some(19));
        assert_eq!(b.doc.mode, mode);
        assert!(!b.doc.follow_cursor);
        assert!(!app.editor_follow);
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(b.scroll.get(), scroll);
    }

    #[test]
    fn source_only_click_in_all_modes_and_undo_keeps_original_caret() {
        for mode in [Mode::Normal, Mode::Visual, Mode::Insert] {
            for (before, after) in [("- [ ] **世界**", "- [x] **世界**"),
                ("  * [x] hé", "  * [ ] hé"), ("1. [X] yes", "1. [ ] yes"),
                ("- [] shorthand", "- [x] shorthand")]
            {
                let raw = format!("caret elsewhere\n{before}\n");
                let mut app = app(&raw, mode);
                app.active_buffer().unwrap().scroll.set((8.0, 100.0, 240.0));
                let scroll = app.active_buffer().unwrap().scroll.get();
                let point = center(glyph_rect(&app, 1));
                assert!(press(&mut app, point), "{mode:?}: {before}");
                assert_stationary(&app, mode, scroll);
                let b = app.active_buffer_mut().unwrap();
                assert_eq!(b.doc.text_for_disk(), format!("caret elsewhere\n{after}\n"));
                assert!(b.dirty());
                assert!(b.doc.undo());
                assert_eq!(b.doc.text_for_disk(), raw);
                assert_eq!(b.doc.cursor(), Pos { line: 0, col: 2 });
                assert!(!b.dirty());
            }
        }
    }

    #[test]
    fn shorthand_same_line_rebases_text_positions_but_keeps_rendered_caret_and_undo_cursor() {
        let raw = "- [] hé 世界 label\n";
        let caret = raw.find('世').unwrap();
        for mode in [Mode::Normal, Mode::Visual] {
            for anchor in [1, raw.find('é').unwrap()] {
                let mut app = app(raw, mode);
                let b = app.active_buffer_mut().unwrap();
                b.doc.cursor_col = caret;
                b.doc.visual_anchor = Some(Pos { line: 0, col: anchor });
                let before = super::super::cursor_rect(&app, app.active_buffer().unwrap()).unwrap();
                let point = center(glyph_rect(&app, 0));
                assert!(press(&mut app, point));
                let b = app.active_buffer().unwrap();
                assert_eq!(b.doc.cursor_col, caret + 1);
                assert_eq!(b.doc.visual_anchor, Some(Pos { line: 0, col: anchor + usize::from(anchor >= 3) }));
                assert!(b.doc.lines[0][b.doc.cursor_col..].starts_with('世'));
                assert_eq!(b.doc.mode, mode);
                let after = super::super::cursor_rect(&app, b).unwrap();
                for i in 0..4 { assert!((before[i] - after[i]).abs() < 0.1); }
                let b = app.active_buffer_mut().unwrap();
                assert!(b.doc.undo());
                assert_eq!(b.doc.text_for_disk(), raw);
                assert_eq!(b.doc.cursor(), Pos { line: 0, col: caret });
                // Document history stores the caret, not the visual anchor. An
                // anchor before the insertion is unaffected through undo too.
                if anchor < 3 { assert_eq!(b.doc.visual_anchor, Some(Pos { line: 0, col: anchor })); }
                assert!(!b.dirty());
            }
        }
    }

    #[test]
    fn only_prefix_glyph_not_indent_space_label_or_wrapped_literal_is_a_control() {
        let raw = format!("caret elsewhere\n  - [ ] {} ☐ label\n", "long label ".repeat(15));
        let mut app = app(&raw, Mode::Normal);
        let r = glyph_rect(&app, 1);
        for point in [[r[0] - 0.1, r[1] + r[3] * 0.5],
            [r[0] + r[2] + 0.1, r[1] + r[3] * 0.5],
            [r[0] + r[2] + 35.0, r[1] + r[3] * 0.5], [0.0, 0.0]]
        { assert!(!press(&mut app, point)); }
        assert_eq!(app.active_buffer().unwrap().doc.text_for_disk(), raw);
        assert!(press(&mut app, center(r)));
    }

    #[test]
    fn active_insert_line_is_raw_and_outside_view_does_not_build_layout() {
        let mut app = app("caret elsewhere\n- [ ] task\n", Mode::Insert);
        assert!(!press(&mut app, [0.0, 0.0]));
        assert!(app.active_buffer().unwrap().markdown_cache.layout.borrow().is_none());
        let point = center(glyph_rect(&app, 1));
        app.active_buffer_mut().unwrap().doc.cursor_line = 1;
        assert!(!press(&mut app, point));
        assert_eq!(app.active_buffer().unwrap().doc.lines[1], "- [ ] task");
    }

    #[test]
    fn checkbox_undo_is_separate_from_pending_insert_typing() {
        let raw = "caret elsewhere\n- [ ] task\n";
        let mut app = app(raw, Mode::Insert);
        app.active_buffer_mut().unwrap().doc.type_text("é");
        let caret = app.active_buffer().unwrap().doc.cursor();
        let point = center(glyph_rect(&app, 1));
        assert!(press(&mut app, point));
        let b = app.active_buffer_mut().unwrap();
        assert!(b.doc.follow_cursor, "preserve true follow as well as false");
        assert_eq!(b.doc.cursor(), caret);
        assert!(b.doc.undo());
        assert_eq!(b.doc.lines[0], "caéret elsewhere");
        assert_eq!(b.doc.lines[1], "- [ ] task");
        assert_eq!(b.doc.cursor(), caret);
        assert!(b.doc.undo());
        assert_eq!(b.doc.text_for_disk(), raw);
    }

    #[test]
    fn shared_document_records_checkbox_transaction_boundary() {
        let mut app = app("caret elsewhere\n- [ ] task\n", Mode::Insert);
        app.active_buffer_mut().unwrap().doc.set_shared_history(true);
        let point = center(glyph_rect(&app, 1));
        assert!(press(&mut app, point));
        let b = app.active_buffer_mut().unwrap();
        let boundaries = b.doc.take_history_breaks();
        assert_eq!(boundaries.last().unwrap().0, b.doc.revision);
        assert_eq!(boundaries.last().unwrap().1.as_deref(), Some("caret elsewhere\n- [x] task\n"));
        assert!(b.doc.undo());
        assert_eq!(b.doc.take_history_requests(), vec![false]);
    }

    #[test]
    fn native_pointer_down_up_toggles_without_editor_selection_or_focus() {
        for mode in [Mode::Normal, Mode::Visual, Mode::Insert] {
            let raw = "caret elsewhere\n- [ ] task **世界**\n";
            let mut app = app(raw, mode);
            app.active_buffer_mut().unwrap().view_rect = None;
            let mut rt = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
                rune::col().w_full().h_full().child(super::super::view(a, a.active_buffer().unwrap()))
            }, (360.0, 240.0), rune::Env::new(true, 1.0));
            for _ in 0..3 { rt.frame(1.0 / 60.0); }
            let scroll = rt.state.active_buffer().unwrap().scroll.get();
            let [x, y] = center(glyph_rect(&rt.state, 1));
            rt.pointer_down(x, y);
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.state.active_buffer().unwrap().doc.lines[1], "- [x] task **世界**");
            assert_stationary(&rt.state, mode, scroll);
            rt.pointer_move(x + 50.0, y + 35.0);
            rt.frame(1.0 / 60.0);
            assert_stationary(&rt.state, mode, scroll);
            rt.pointer_up(x + 50.0, y + 35.0);
            rt.frame(1.0 / 60.0);
            assert_stationary(&rt.state, mode, scroll);
            rt.update(|a, _| {
                let b = a.active_buffer_mut().unwrap();
                assert!(b.doc.undo());
                assert_eq!(b.doc.text_for_disk(), raw);
                assert_eq!(b.doc.cursor(), Pos { line: 0, col: 2 });
            });
        }
    }
}