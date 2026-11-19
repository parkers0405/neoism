//! Card chrome is tiled independently of editable source rows. A 100k-line fence
//! still mounts only viewport rows, never a single enormous code text element.
use super::*;
use crate::agent::timeline::{code_header, code_surface};

pub(super) struct Card {
    pub range: Range<usize>,
    pub top: f32,
    pub bottom: f32,
    pub inset: f32,
    language: String,
    end: usize,
    ink: Vec<Vec<Span>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(raw: &str) -> rune::Runtime<App> {
        let mut a = super::super::tests::app(raw);
        a.active_buffer_mut().unwrap().view_rect = None;
        let mut rt = rune::Runtime::new(a, |a: &App, _: &mut rune::Cx<App>| {
            super::super::super::view::view(a)
        }, (620.0, 380.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        rt
    }

    #[test]
    fn native_header_copy_and_unicode_body_click_edit_undo() {
        let raw = "```rust\nlet hé = \"世界\";\n```\n\n| h | v |\n| --- | --- |\n| x | y |\n";
        let mut rt = runtime(raw);
        let clipboard = rune::clipboard::memory();
        rt.use_clipboard(&clipboard);
        let cursor = rt.state.active_buffer().unwrap().doc.cursor();
        let copy = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Copy")).unwrap().rect;
        rt.pointer_down(copy[0] + 3.0, copy[1] + 3.0);
        rt.pointer_up(copy[0] + 3.0, copy[1] + 3.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.copy_feedback.len(), 1, "real shared Copy handler ran");
        assert_eq!(clipboard.text().unwrap().as_str(), "let hé = \"世界\";\n");
        assert_eq!(rt.state.active_buffer().unwrap().doc.cursor(), cursor, "header control must not place the source caret");
        assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), raw);
        let point = {
            let b = rt.state.active_buffer().unwrap();
            let laid = layout(&rt.state, b);
            let e = laid.rows.iter().find(|e| e.line == 1).unwrap();
            let p = &e.row.pieces[0];
            let byte = p.text.find('世').unwrap();
            let rect = b.view_rect.unwrap();
            [rect[0] + laid.left + p.x + p.x_at(byte) + 0.1, rect[1] + e.y + 3.0]
        };
        rt.pointer_down(point[0], point[1]); rt.pointer_up(point[0], point[1]);
        rt.frame(1.0 / 60.0);
        let b = rt.state.active_buffer().unwrap();
        assert_eq!((b.doc.cursor_line, b.doc.cursor_col), (1, b.doc.lines[1].find('世').unwrap()));
        rt.update(|a, _| {
            assert!(before_key(a, Key::Char('i'), false));
            let b = a.active_buffer_mut().unwrap();
            b.doc.type_text("!");
            b.doc.key(Key::Esc, &mut b.prompt, None);
            assert!(b.dirty()); assert!(b.doc.undo());
            assert_eq!(b.doc.text_for_disk(), raw); assert!(!b.dirty());
        });
    }

    #[test]
    fn wrapped_numbers_once_nearby_table_and_raw_fence_reveal() {
        let raw = format!("```rust\n{}\nlet x = 2;\n```\n| h | v |\n| --- | --- |\n| a | b |\n", "hé 世界 ".repeat(35));
        let mut rt = runtime(&raw);
        let b = rt.state.active_buffer().unwrap();
        let laid = layout(&rt.state, b);
        assert!(laid.rows.iter().filter(|e| e.line == 1).count() > 2);
        assert_eq!(laid.rows.iter().filter_map(|e| laid.cards[0].number(e)).collect::<Vec<_>>(), vec![1, 2]);
        assert!(laid.tables[0].top >= laid.cards[0].bottom);
        for e in laid.rows.iter().filter(|e| e.line == 1 || e.line == 4) {
            for p in &e.row.pieces {
                assert_eq!(hit(&rt.state, b, laid.left + p.x + 0.1, e.y), Some((e.line, p.source[0])));
            }
        }
        rt.update(|a, _| { assert!(before_key(a, Key::Char('i'), false)); });
        rt.frame(1.0 / 60.0);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("```rust")));
        assert!(!rt.state.active_buffer().unwrap().dirty());
    }

    #[test]
    fn huge_fence_mounts_viewport_rows_not_whole_block() {
        let raw = format!("```rust\n{}```\n", "let value = 123;\n".repeat(10_000));
        let mut rt = runtime(&raw);
        assert!(rt.elements().iter().filter(|e| e.text.as_deref() == Some("let value = 123;")).count() < 80);
        rt.wheel_event(WheelEvent::new(0.0, -1300.0).precise(true).at(300.0, 180.0));
        for frame in 0..180 {
            rt.update(|a, tx| { a.scrollers.step(frame as f64 / 60.0, tx); });
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.active_buffer().unwrap().scroll.get().0 > 500.0);
        let mounted = rt.elements().iter().filter(|e| e.text.as_deref() == Some("let value = 123;")).count();
        // Rune retains two viewports of overscan on each side; tile rounding can
        // add one tile at each edge. This bound is independent of file length.
        let viewport = rt.state.active_buffer().unwrap().scroll.get().2;
        let tile = tile_height(&rt.state);
        let tiles = (viewport * 5.0 / tile).ceil() as usize + 2;
        // A row straddling a tile boundary is represented in both clipped tiles.
        let bound = tiles * ((tile / rt.state.ty.body().line_px()).ceil() as usize + 1);
        assert!(mounted <= bound, "settled virtual row count: {mounted}, bound: {bound}");
        assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), raw);
    }

    #[test]
    fn multiline_syntax_and_unchanged_rows_refresh_after_delimiter_edit() {
        let mut rt = runtime("```rust\n/* comment\nlet value = 2;\n*/\n```\n");
        let spans = |a: &App| {
            let laid = layout(a, a.active_buffer().unwrap());
            format!("{:?}", laid.rows.iter().find(|e| e.line == 2).unwrap().row.pieces[0].spans)
        };
        let before = spans(&rt.state);
        rt.update(|a, _| {
            let b = a.active_buffer_mut().unwrap();
            b.doc.cursor_line = 1; b.doc.cursor_col = 0;
            b.doc.mode = Mode::Insert; b.doc.type_text("//");
        });
        rt.frame(1.0 / 60.0);
        assert_ne!(spans(&rt.state), before, "cached unchanged body line follows the block's syntax state");
    }

    #[test]
    fn remote_unicode_caret_uses_the_same_card_offsets_after_source_revision() {
        use neoism_protocol::crdt::{CrdtCursorPosition, CrdtPeerPresence, CrdtPresenceColor};
        let mut rt = runtime("```rust\nlet hé = \"😀世界\";\n```\n");
        for edit in [false, true] {
            rt.update(|a, _| {
                let b = a.active_buffer_mut().unwrap();
                b.doc.mode = Mode::Insert; b.doc.cursor_line = 1; b.doc.cursor_col = 0;
                if edit { b.doc.type_text("  "); }
                let byte = b.doc.lines[1].find('世').unwrap();
                b.doc.cursor_col = byte;
                let id = b.shared.as_ref().unwrap().buffer_id.clone();
                let column = b.doc.lines[1][..byte].encode_utf16().count() as u32;
                a.collab.stage(vec![CrdtPeerPresence {
                    buffer_id: id, peer_id: "code-peer".into(), display_name: "Ada".into(), client_os: None,
                    color: CrdtPresenceColor { r: 10, g: 200, b: 100 },
                    cursor: CrdtCursorPosition { line: 1, column, offset: None },
                    selection: None, insert: true, rainbow: false, updated_at_ms: if edit { 2 } else { 1 },
                }]);
            });
            for _ in 0..4 { rt.frame(1.0 / 60.0); }
            let local = cursor_rect(&rt.state, rt.state.active_buffer().unwrap()).unwrap();
            let peers: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("peer-caret:Ada")).collect();
            assert_eq!(peers.len(), 1);
            assert!((peers[0].rect[0] - local[0]).abs() < 0.2);
            // Presence deliberately uses a shorter, vertically centered beam.
            assert!((peers[0].rect[1] + peers[0].rect[3] * 0.5 - local[1] - local[3] * 0.5).abs() < 0.2);
        }
    }
}

pub(super) fn cards(app: &App, buffer: &Buffer, lines: &[Line], diagrams: &[mermaid::Diagram]) -> Vec<Card> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !matches!(lines[i].kind, Kind::Fence) { i += 1; continue; }
        let start = i;
        i += 1;
        while i < lines.len() && matches!(lines[i].kind, Kind::Code { .. }) { i += 1; }
        let end = i;
        if i < lines.len() && matches!(lines[i].kind, Kind::Fence) { i += 1; }
        let diagram = diagrams.get(diagrams.partition_point(|d| d.range.end <= start));
        if diagram.is_some_and(|d| d.range == (start..i) && !matches!(d.entry, crate::mermaid::Entry::Failed(_))) { continue; }
        let raw = buffer.doc.lines[start].trim_start();
        let language = raw.trim_start_matches(['`', '~']).split_whitespace().next().unwrap_or("text").to_owned();
        let digits = end.saturating_sub(start + 1).max(1).to_string();
        let number_width = rune::text::geometry(&digits, &[], app.ty.body(), None).caret(digits.len())[0];
        let body = &buffer.doc.lines[start + 1..end];
        let source = body.join("\n");
        let mut offsets = Vec::with_capacity(body.len());
        let mut offset = 0;
        for line in body { offsets.push(offset); offset += line.len() + 1; }
        let mut ink = vec![Vec::new(); body.len()];
        // Highlight the entire fence, as chat does, so multiline strings/comments
        // keep their state. Distribute tokens, not repeated scans per source line.
        if language != "mermaid" {
        for (range, paint) in rune_kit::syntax::highlight(Some(&language), &source).iter() {
            let color = match paint {
                rune_kit::syntax::Paint::Kind(kind) => super::super::view::syntax_color(app.pal, *kind).map(rgb),
                rune_kit::syntax::Paint::Color(color) => Some(*color),
            };
            let Some(color) = color else { continue };
            let first = offsets.partition_point(|&at| at <= range.start).saturating_sub(1);
            for n in first..body.len() {
                let start = offsets[n];
                if start >= range.end { break; }
                let a = range.start.max(start) - start;
                let b = range.end.min(start + body[n].len()).saturating_sub(start);
                if a < b { ink[n].push(Span::new(a, b).color(color)); }
            }
        }
        }
        result.push(Card { range: start..i, top: 0.0, bottom: 0.0, inset: 22.0 + number_width + 18.0, language, end, ink });
    }
    result
}

impl Card {
    pub fn ink(&self, line: usize) -> Option<&[Span]> {
        line.checked_sub(self.range.start + 1).and_then(|n| self.ink.get(n)).map(Vec::as_slice)
    }
    pub fn number(&self, entry: &Entry) -> Option<usize> {
        (entry.row.first && entry.line > self.range.start && entry.line < self.end).then_some(entry.line - self.range.start)
    }
    pub fn view(&self, app: &App, buffer: &Buffer, width: f32, tile_top: f32) -> El {
        let surface = code_surface(app.pal).w(width).h(self.bottom - self.top);
        if self.top + 38.0 <= tile_top { return surface; }
        // Match the chat Copy payload: body only, with its terminating newline.
        let mut source = buffer.doc.lines[self.range.start + 1..self.end].join("\n");
        if self.end > self.range.start + 1 { source.push('\n'); }
        let header = if mermaid::language(&self.language) {
            let editing = buffer.doc.mode == Mode::Insert && self.range.contains(&buffer.doc.cursor_line);
            crate::agent::timeline::code_header_with_action(app.pal, app.ty, &self.language, source,
                Some(mermaid::edit_button(app, buffer, self.range.start, editing)))
        } else { code_header(app.pal, app.ty, &self.language, source) };
        surface.child(header)
    }
}