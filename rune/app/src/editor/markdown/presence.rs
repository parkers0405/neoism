//! Project the code editor's presence marks through Markdown's measured source map.
//! No second collaboration path: UTF-16 wire positions are decoded by collab::view,
//! then rendered with its same colours, blink, mode shapes and orb/name flags.
use super::*;
use crate::collab::view::{self, Mark, RowGeometry};

pub(super) fn marks(app: &App, buffer: &Buffer, line: usize) -> Vec<Mark> {
    view::line_marks(app, buffer.shared.as_ref().map(|s| s.buffer_id.as_str()), line, &buffer.doc.lines[line])
}

/// Resolve hidden syntax, wrap boundaries and cell padding to exactly one piece. Like
/// the local cursor, prefer a containing piece, then the nearest preceding endpoint.
fn owner(layout: &Layout, line: usize, source: usize) -> Option<(usize, usize)> {
    let mut before = None;
    let mut after = None;
    let first = layout.rows.partition_point(|e| e.line < line);
    for (i, entry) in layout.rows.iter().enumerate().skip(first).take_while(|(_, e)| e.line == line) {
        for (j, piece) in entry.row.pieces.iter().enumerate() {
            let (start, end) = (piece.source[0], *piece.source.last().unwrap());
            if start <= source && source < end { return Some((i, j)); }
            if end <= source && before.is_none_or(|(_, _, best)| end > best) { before = Some((i, j, end)); }
            if start > source && after.is_none_or(|(_, _, best)| start < best) { after = Some((i, j, start)); }
        }
    }
    before.or(after).map(|(i, j, _)| (i, j))
}

fn project(mark: &Mark, piece: &Piece, owns_caret: bool, newline: bool) -> Mark {
    let mut projected = mark.clone();
    projected.caret = mark.caret.filter(|_| owns_caret).map(|(byte, insert)| (piece.visible(byte), insert));
    projected.band = mark.band.and_then(|(a, b, runs_on)| {
        let range = piece.selected(a..b);
        let runs_on = runs_on && newline;
        (!range.is_empty() || runs_on).then_some((if range.is_empty() { piece.text.len() } else { range.start },
            if range.is_empty() { piece.text.len() } else { range.end }, runs_on))
    });
    projected
}

pub(super) fn decorate(app: &App, mut visual: El, layout: &Layout, i: usize, marks: &[Mark]) -> El {
    let entry = &layout.rows[i];
    for (peer, mark) in marks.iter().enumerate() {
        let caret_owner = mark.caret.and_then(|(byte, _)| owner(layout, entry.line, byte));
        // The newline stub belongs only to the final cell/row, never to padded cells.
        let newline_owner = owner(layout, entry.line, usize::MAX);
        for (j, piece) in entry.row.pieces.iter().enumerate() {
            let projected = project(mark, piece, caret_owner == Some((i, j)), newline_owner == Some((i, j)));
            let byte = projected.caret.map_or(piece.text.len(), |(b, _)| b);
            let x = piece.x_at(byte);
            let advance = piece.text[byte..].graphemes(true).next()
                .map_or(piece.font.size * 0.55, |g| (piece.x_at(byte + g.len()) - x).abs()).max(2.0);
            let x_of = |byte| piece.x_at(byte);
            let geometry = RowGeometry { x_of: &x_of, start: 0, end: piece.text.len(), last: true,
                line_len: piece.text.len(), line_h: entry.row.height, adv: advance,
                font: piece.font.size, top: i == 0 };
            // Isolate child keys: one peer can select several cells on the same row.
            let overlay = el().key(format!("md-peer:{peer}:{j}")).w(piece.width).h(entry.row.height).absolute(piece.x, 0.0);
            visual = visual.child(view::decorate(app, overlay, &projected, &geometry));
        }
    }
    visual
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_protocol::crdt::{CrdtCursorPosition as PeerCursor, CrdtPeerPresence as PeerPresence, CrdtPresenceColor as PresenceColor};

    fn mark(source: &str, byte: usize) -> Mark {
        let mut app = App::new();
        app.collab.stage(vec![PeerPresence {
            buffer_id: "file:///note.md".into(), peer_id: "other@host".into(), display_name: "Ada".into(),
            client_os: None, color: PresenceColor { r: 10, g: 20, b: 30 },
            cursor: PeerCursor { line: 0, column: source[..byte].encode_utf16().count() as u32, offset: None },
            selection: None, insert: true, rainbow: false, updated_at_ms: 0,
        }]);
        view::line_marks(&app, Some("file:///note.md"), 0, source).remove(0)
    }

    #[test]
    fn markdown_presence_projects_hidden_markup_and_unicode() {
        let app = App::new();
        let raw = "# **hé😀llo**";
        let line = parse::project(&[raw.into()], usize::MAX..=usize::MAX).remove(0);
        let piece = pieces(&app, &line, 900.0, 0.0).remove(0);
        let byte = raw.find('😀').unwrap();
        let mut peer = mark(raw, byte);
        peer.band = Some((raw.find('h').unwrap(), byte + '😀'.len_utf8(), false));
        let projected = project(&peer, &piece, true, true);
        assert_eq!(projected.caret, Some(("hé".len(), true)));
        assert_eq!(projected.band, Some((0, "hé😀".len(), false)));
        assert_eq!(project(&peer, &piece, false, false).caret, None);
    }

    #[test]
    fn markdown_presence_owns_one_piece_like_local_cursor_in_preview_and_insert() {
        let mut app = App::new();
        let raw = "# **hé😀llo** and more text that wraps onto several lines\n\n| **one** | two and more words |\n| --- | --- |\n| a | b |\n";
        app.open_file("/notes/presence.md", None);
        app.apply_opened_file("/notes/presence.md", raw.as_bytes());
        app.active_buffer_mut().unwrap().view_rect = Some([0.0, 0.0, 230.0, 400.0]);
        for mode in [Mode::Normal, Mode::Insert] {
            for line in [0, 1, 2, 4] {
                let source = app.active_buffer().unwrap().doc.lines[line].clone();
                for byte in source.char_indices().map(|(b, _)| b).chain(std::iter::once(source.len())) {
                    let buffer = app.active_buffer_mut().unwrap();
                    buffer.doc.mode = mode;
                    buffer.doc.cursor_line = line;
                    buffer.doc.cursor_col = byte;
                    let buffer = app.active_buffer().unwrap();
                    let laid = layout(&app, buffer);
                    let (i, j) = owner(&laid, line, byte).unwrap();
                    let (local_entry, local_piece) = cursor_entry(&laid, buffer).unwrap();
                    assert!(std::ptr::eq(local_entry, &laid.rows[i]));
                    assert!(std::ptr::eq(local_piece, &laid.rows[i].row.pieces[j]));
                }
            }
        }
        let laid = layout(&app, app.active_buffer().unwrap());
        assert!(laid.rows.iter().filter(|e| e.line == 0).count() > 1, "exercise soft wrapping");
    }

    #[test]
    fn markdown_presence_empty_newline_and_hidden_only_selection() {
        let app = App::new();
        let line = parse::inline_line("**word**");
        let piece = pieces(&app, &line, 900.0, 0.0).remove(0);
        let mut peer = mark("**word**", 0);
        peer.band = Some((0, 2, false));
        assert_eq!(project(&peer, &piece, true, false).band, None);
        let empty = pieces(&app, &raw_line("", Kind::Prose), 900.0, 0.0).remove(0);
        peer.band = Some((0, 0, true));
        assert_eq!(project(&peer, &empty, true, true).band, Some((0, 0, true)));
        assert_eq!(project(&peer, &empty, false, false).band, None);
    }

    #[test]
    fn markdown_presence_mounts_native_marks_and_invalidates_cached_rows() {
        use neoism_protocol::crdt::CrdtSelectionRange;
        let mut app = App::new();
        app.open_file("/notes/presence.md", None);
        app.apply_opened_file("/notes/presence.md", "# Shared **hé😀llo**\n\nSome prose\n".as_bytes());
        let id = app.active_buffer().unwrap().shared.as_ref().unwrap().buffer_id.clone();
        let peer = |column, insert| PeerPresence {
            buffer_id: id.clone(), peer_id: "other@host".into(), display_name: "Ada".into(), client_os: None,
            color: PresenceColor { r: 10, g: 200, b: 100 },
            cursor: PeerCursor { line: 0, column, offset: None },
            selection: Some(CrdtSelectionRange { anchor: PeerCursor { line: 0, column: 2, offset: None },
                head: PeerCursor { line: 0, column, offset: None } }),
            insert, rainbow: false, updated_at_ms: column as u64,
        };
        app.collab.stage(vec![peer(12, true)]);
        let mut rt = rune::Runtime::new(app, |app, _| {
            el().size(620.0, 380.0).child(super::super::view(app, app.active_buffer().unwrap()))
        }, (620.0, 380.0), rune::Env::new(true, 1.0));
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        let caret = |rt: &rune::Runtime<App>| rt.elements().into_iter()
            .find(|e| e.key.as_deref() == Some("peer-caret:Ada")).unwrap().rect;
        let before = caret(&rt);
        assert_eq!(before[2], 2.0, "insert peer is a beam");
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("peer-selection:Ada")));
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("peer-flag:Ada")));
        rt.update(|app, _| app.collab.stage(vec![peer(15, false)]));
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        let after = caret(&rt);
        assert!(after[0] > before[0], "moving presence invalidates the row memo");
        assert!(after[2] > 2.0, "normal peer is a measured glyph block");
        // Optional synthetic-only headless PNG for the visual parity pass.
        if let Some(path) = std::env::var_os("NEOISM_RUNE_PRESENCE_PNG") {
            use rune::render::{Gpu, Offscreen, Renderer, OFFSCREEN_FORMAT};
            let gpu = Gpu::headless();
            let target = Offscreen::with_format(&gpu.device, 620, 380, OFFSCREEN_FORMAT);
            let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (620, 380));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
            target.copy(&mut enc);
            renderer.submit(&gpu.queue, enc);
            rune::render::write_png(std::path::Path::new(&path), 620, 380, &target.read(&gpu.device)).unwrap();
        }
    }
}