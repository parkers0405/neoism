//! Block reordering over Document: no source copy is edited during a gesture.
use super::{App, Buffer, Kind, Layout, Line, Mode, layout, parsed, rgb};
use crate::editor::Document;
use rune::{Drag, El, Phase, Tx, el, text};
use std::{cell::{Cell, RefCell}, ops::Range, rc::Rc};

#[derive(Default)]
pub(super) struct State {
    hover: Cell<Option<usize>>,
    active: RefCell<Option<Gesture>>,
}
struct Gesture {
    revision: u64,
    source: Range<usize>,
    start: [f32; 2],
    point: [f32; 2],
    moved: bool,
    target: Option<(usize, f32)>,
}

/// Partition source, not visual rows. A fence's closing delimiter belongs to the
/// same block; two adjacent fences must never be combined. Unclosed fences own EOF.
pub(super) fn blocks(raw: &[String], lines: &[Line], tables: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut tables = tables.iter().peekable();
    let mut i = 0;
    while i < raw.len() {
        let start = i;
        if tables.peek().is_some_and(|t| t.start == i) { i = tables.next().unwrap().end; }
        else if matches!(lines[i].kind, Kind::Fence) {
            i += 1;
            while i < raw.len() && matches!(lines[i].kind, Kind::Code { .. }) { i += 1; }
            if i < raw.len() && matches!(lines[i].kind, Kind::Fence) { i += 1; }
        } else if matches!(lines[i].kind, Kind::Frontmatter) {
            i += 1;
            while i < raw.len() && matches!(lines[i].kind, Kind::Frontmatter) { i += 1; }
        } else if matches!(lines[i].kind, Kind::Prose) && !raw[i].trim().is_empty() {
            i += 1;
            while i < raw.len() && matches!(lines[i].kind, Kind::Prose) && !raw[i].trim().is_empty() { i += 1; }
        } else { i += 1; }
        out.push(start..i);
    }
    out
}

fn ranges(buffer: &Buffer) -> Rc<Vec<Range<usize>>> {
    parsed(buffer);
    buffer.markdown_cache.parsed.borrow().as_ref().unwrap().blocks.clone()
}

fn boundary(laid: &Layout, line: usize) -> f32 {
    let i = laid.rows.partition_point(|r| r.line < line);
    laid.rows.get(i).map_or_else(|| laid.rows.last().map_or(24.0, |r| r.y + r.row.height), |r| r.y)
}

fn target(laid: &Layout, blocks: &[Range<usize>], y: f32) -> Option<(usize, f32)> {
    let i = blocks.partition_point(|block| y >= (boundary(laid, block.start) + boundary(laid, block.end)) * 0.5);
    if let Some(block) = blocks.get(i) { return Some((block.start, boundary(laid, block.start))); }
    blocks.last().map(|b| (b.end, boundary(laid, b.end)))
}

fn inside(p: [f32; 2], r: [f32; 4]) -> bool {
    p[0].is_finite() && p[1].is_finite() && p[0] >= r[0] && p[0] < r[0] + r[2]
        && p[1] >= r[1] && p[1] < r[1] + r[3]
}

/// Called ahead of text selection by the Markdown surface only. Consume all
/// subsequent events even after a stale revision, so they cannot become selection.
pub(super) fn event(app: &mut App, d: &Drag, tx: &mut Tx) -> bool {
    let Some(b) = app.active_buffer() else { return false; };
    let state = &b.markdown_cache.drag;
    let point = d.window_point();
    if d.phase == Phase::Down {
        if !b.loaded || b.prompt.is_some() { return false; }
        let Some(rect) = b.view_rect.filter(|r| inside(point, *r)) else { return false; };
        let laid = layout(app, b);
        let x = point[0] - rect[0];
        let s = app.ty.scale();
        if x < laid.left - 28.0 * s || x >= laid.left - 4.0 * s { return false; }
        let y = point[1] - rect[1] + b.scroll.get().0;
        let row = laid.rows.partition_point(|r| r.y <= y).saturating_sub(1);
        let Some(entry) = laid.rows.get(row).filter(|r| y >= r.y && y < r.y + r.row.height) else { return false; };
        let blocks = ranges(b);
        let i = blocks.partition_point(|r| r.end <= entry.line);
        let Some(source) = blocks.get(i).filter(|r| r.contains(&entry.line)).cloned() else { return false; };
        if !entry.row.first || entry.line != source.start { return false; }
        *state.active.borrow_mut() = Some(Gesture { revision: b.doc.revision, source, start: point, point,
            moved: false, target: None });
        app.focus = crate::app::Focus::Content;
        tx.blur();
        return true;
    }
    let Some(mut gesture) = state.active.borrow_mut().take() else { return false; };
    gesture.point = point;
    gesture.moved |= (point[0] - gesture.start[0]).hypot(point[1] - gesture.start[1]) >= 4.0 * app.ty.scale();
    gesture.target = if gesture.revision == b.doc.revision && b.view_rect.is_some_and(|r| inside(point, r)) {
        let rect = b.view_rect.unwrap();
        target(&layout(app, b), &ranges(b), point[1] - rect[1] + b.scroll.get().0)
            .filter(|(at, _)| !(gesture.source.start..=gesture.source.end).contains(at))
    } else { None };
    if d.phase == Phase::Up {
        if d.cancelled() {
            // Rune consumes Escape while the pointer is held. Match the ordinary
            // editor selection path's Vim mode transition on cancelled release.
            app.editor_key(&rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() }, tx);
            return true;
        }
        if gesture.moved {
            if let Some((at, _)) = gesture.target {
                let b = app.active_buffer_mut().unwrap();
                reorder(&mut b.doc, gesture.revision, gesture.source, at);
            }
        }
    } else { *state.active.borrow_mut() = Some(gesture); }
    true
}

fn reorder(doc: &mut Document, revision: u64, source: Range<usize>, at: usize) -> bool {
    if doc.revision != revision || source.is_empty() || source.end > doc.lines.len()
        || at > doc.lines.len() || (source.start..=source.end).contains(&at) { return false; }
    let insert = if at > source.end { at - source.len() } else { at };
    doc.end_insert_group();
    doc.break_undo_group();
    doc.save_undo();
    let moved: Vec<_> = doc.lines.drain(source).collect();
    doc.lines.splice(insert..insert, moved);
    doc.cursor_line = insert;
    doc.visual_anchor = None;
    if doc.mode == Mode::Visual { doc.mode = Mode::Normal; }
    doc.clear_vertical_goal();
    doc.clamp_cursor();
    doc.mark_edited();
    doc.commit_undo();
    true
}

/// Full-width hover area, including the gutter, but no new hit surface over prose.
pub(super) fn row(app: &App, b: &Buffer, laid: &Layout, i: usize, content: El) -> El {
    let entry = &laid.rows[i];
    let path = b.path.clone();
    let s = app.ty.scale();
    let mut row = el().w_full().h(entry.row.height)
        .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
            if let Some(b) = a.buffers.get(&path) {
                let hover = &b.markdown_cache.drag.hover;
                if inside { hover.set(Some(i)); } else if hover.get() == Some(i) { hover.set(None); }
            }
        }).child(content.absolute(laid.left, 0.0));
    // Hovering any continuation reveals the block's one handle, anchored to its
    // first visual row. Soft wraps and subsequent source lines get no handles.
    let hovered_start = b.markdown_cache.drag.hover.get().and_then(|hover| laid.rows.get(hover)).and_then(|hover| {
        let blocks = ranges(b);
        let block = blocks.get(blocks.partition_point(|r| r.end <= hover.line))?;
        block.contains(&hover.line).then_some(block.start)
    });
    if entry.row.first && hovered_start == Some(entry.line) && b.markdown_cache.drag.active.borrow().is_none() {
        row = row.child(el().key("md-grab").absolute(laid.left - 28.0 * s, 0.0).w(24.0 * s).h(entry.row.height)
            .center().radius(4.0 * s).hover(rgb(app.pal.hover)).cursor(rune::Cursor::Grab)
            .child(text("⠿").font(app.ty.small()).color(rgb(app.pal.muted))));
    }
    row
}

/// Native keyed elements retain Rune's spring position/opacity animation; they
/// live outside the instant-change virtual tiles and never perturb source layout.
pub(super) fn overlay(app: &App, b: &Buffer, pane: El) -> El {
    let active = b.markdown_cache.drag.active.borrow();
    let Some(g) = active.as_ref().filter(|g| g.moved && g.revision == b.doc.revision) else { return pane; };
    let Some(rect) = b.view_rect else { return pane; };
    let laid = layout(app, b);
    let s = app.ty.scale();
    let mut pane = pane;
    if let Some((_, y)) = g.target {
        pane = pane.child(el().key("md-drop-indicator").absolute(laid.left, y - b.scroll.get().0)
            .w(laid.width).h(3.0 * s).radius(1.5 * s).fill(rgb(app.pal.blue))
            .transition(rune::Transition::opacity()));
    }
    // A bounded first-line preview, never a second editable document.
    let projected = parsed(b);
    let first = &projected[g.source.start].text;
    let first = if first.is_empty() { &b.doc.lines[g.source.start] } else { first };
    let preview: String = first.chars().take(72).collect();
    pane.child(el().key("md-drag-ghost").absolute(
        (g.point[0] - rect[0] + 14.0 * s).clamp(0.0, (rect[2] - 220.0 * s).max(0.0)),
        (g.point[1] - rect[1] + 12.0 * s).clamp(0.0, (rect[3] - 46.0 * s).max(0.0)))
        .w(220.0 * s).h(46.0 * s).px(10.0 * s).center_y().clip_rounded(6.0 * s)
        .fill(rgb(app.pal.surface).alpha(0.94)).border(s, rgb(app.pal.blue).alpha(0.5))
        .transition(rune::Transition::opacity())
        .child(text(format!("⠿ {preview}")).font(app.ty.small()).color(rgb(app.pal.fg))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(raw: &str) -> rune::Runtime<App> {
        let mut app = App::new();
        app.open_file("/notes/drag.md", None);
        app.apply_opened_file("/notes/drag.md", raw.as_bytes());
        let mut rt = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
            rune::col().w_full().h_full().child(super::super::view(a, a.active_buffer().unwrap()))
        }, (620.0, 500.0), rune::Env::new(true, 1.0));
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        rt
    }
    fn point(rt: &rune::Runtime<App>, line: usize) -> [f32; 2] {
        let b = rt.state.active_buffer().unwrap();
        let laid = layout(&rt.state, b);
        let r = b.view_rect.unwrap();
        [r[0] + laid.left - 16.0 * rt.state.ty.scale(),
            r[1] + boundary(&laid, line) - b.scroll.get().0 + 3.0]
    }
    fn step(rt: &mut rune::Runtime<App>) { rt.frame(1.0 / 60.0); }

    #[test]
    fn partition_keeps_multiline_source_units_and_adjacent_fences_separate() {
        let rt = runtime("---\ntitle: Hi\n---\nfirst\nsecond\n\n```rs\nx\n```\n```\ny\n```\n| h |\n| --- |\n| b |\n\n~~~\nunclosed");
        assert_eq!(*ranges(rt.state.active_buffer().unwrap()),
            vec![0..3, 3..5, 5..6, 6..9, 9..12, 12..15, 15..16, 16..18]);
    }

    #[test]
    fn move_is_one_undo_preserves_unicode_crlf_and_noop_has_no_history() {
        let mut rt = runtime("# Top\r\n\r\nhé **世界**\r\nnext\r\n");
        let doc = &mut rt.state.active_buffer_mut().unwrap().doc;
        let original = doc.text_for_disk();
        let revision = doc.revision;
        for at in [2, 3, 4, 99] { assert!(!reorder(doc, revision, 2..4, at)); }
        assert_eq!(doc.revision, revision);
        assert!(!reorder(doc, revision.wrapping_add(1), 2..4, 0));
        assert!(reorder(doc, revision, 2..4, 0));
        assert_eq!(doc.text_for_disk(), "hé **世界**\r\nnext\r\n# Top\r\n\r\n");
        assert!(doc.undo());
        assert_eq!(doc.text_for_disk(), original);
        assert!(!doc.is_dirty());
        assert!(doc.redo());
        assert!(doc.text_for_disk().starts_with("hé **世界**"));
    }

    #[test]
    fn native_hover_drag_drop_and_escape_stale_outside_release() {
        for cancel in ["none", "escape", "stale", "outside", "click"] {
            let raw = "# Top\n\nparagraph hé\nsecond source line\n\n# End\n";
            let mut rt = runtime(raw);
            let from = point(&rt, 2);
            let to = point(&rt, 0);
            assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("md-grab")));
            rt.pointer_move(from[0] + 70.0, from[1]); step(&mut rt);
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("md-grab")));
            rt.pointer_move(from[0], from[1]); step(&mut rt);
            rt.pointer_down(from[0], from[1]); step(&mut rt);
            if cancel != "click" {
                rt.pointer_move(to[0], to[1]); step(&mut rt);
                assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("md-drop-indicator")));
                assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("md-drag-ghost")));
            }
            if cancel == "stale" { rt.update(|a, _| { a.active_buffer_mut().unwrap().doc.mark_edited(); }); }
            if cancel == "escape" { rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() }); }
            let release = if cancel == "outside" { [-30.0, -30.0] } else if cancel == "click" { from } else { to };
            rt.pointer_up(release[0], release[1]); step(&mut rt);
            let b = rt.state.active_buffer_mut().unwrap();
            if cancel == "none" {
                assert_eq!(b.doc.text_for_disk(), "paragraph hé\nsecond source line\n# Top\n\n\n# End\n");
                assert!(b.doc.undo());
            }
            assert_eq!(b.doc.text_for_disk(), raw, "{cancel}");
            assert!(b.markdown_cache.drag.active.borrow().is_none());
        }
    }

    #[test]
    fn wrapped_and_multiline_blocks_have_one_handle_at_the_first_visual_row() {
        let raw = format!("# Top\n\n{}\nsecond source line\n\n# End", "wrapped words ".repeat(14));
        let mut rt = runtime(&raw);
        let b = rt.state.active_buffer().unwrap();
        let laid = layout(&rt.state, b);
        let rect = b.view_rect.unwrap();
        let first_y = rect[1] + boundary(&laid, 2) - b.scroll.get().0;
        let mut probes: Vec<_> = laid.rows.iter().filter(|e| e.line == 2 || e.line == 3)
            .map(|e| [rect[0] + laid.left + 80.0, rect[1] + e.y + e.row.height * 0.5 - b.scroll.get().0]).collect();
        assert!(probes.len() > 3, "exercise soft wraps and a second source line");
        let gutter = rect[0] + laid.left - 16.0 * rt.state.ty.scale();
        let continuation = probes[1];
        // Hovering each part of the paragraph keeps a single, stationary handle.
        for p in probes.drain(..) {
            rt.pointer_move(p[0], p[1]); step(&mut rt);
            let handles: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("md-grab")).collect();
            assert_eq!(handles.len(), 1);
            assert!((handles[0].rect[1] - first_y).abs() < 1.0);
        }
        // An empty continuation gutter must not secretly start a block drag.
        rt.pointer_down(gutter, continuation[1]); step(&mut rt);
        assert!(rt.state.active_buffer().unwrap().markdown_cache.drag.active.borrow().is_none());
        rt.pointer_up(gutter, continuation[1]); step(&mut rt);
        assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), raw);
        assert!(!rt.state.active_buffer().unwrap().dirty());
    }

    #[test]
    fn wrapping_and_multiline_target_midpoints_use_whole_block() {
        let rt = runtime(&format!("# Top\n\n{}\nsecond\n\n# End", "wrapped 世界 ".repeat(60)));
        let b = rt.state.active_buffer().unwrap();
        let laid = layout(&rt.state, b);
        assert!(laid.rows.iter().filter(|e| e.line == 2).count() > 3);
        let blocks = ranges(b);
        let top = boundary(&laid, 2);
        let bottom = boundary(&laid, 4);
        assert_eq!(target(&laid, &blocks, top + (bottom - top) * 0.25).unwrap().0, 2);
        assert_eq!(target(&laid, &blocks, top + (bottom - top) * 0.75).unwrap().0, 4);
    }

    #[test]
    fn escape_from_handle_focuses_content_and_exits_insert_and_visual_without_edit() {
        for mode in [Mode::Insert, Mode::Visual] {
            let raw = "# Top\n\nparagraph\nsecond source line\n";
            let mut rt = runtime(raw);
            rt.on_key = Some(crate::on_key);
            rt.update(|a, _| {
                a.focus = crate::app::Focus::Sidebar;
                let doc = &mut a.active_buffer_mut().unwrap().doc;
                doc.mode = mode;
                if mode == Mode::Visual { doc.visual_anchor = Some(doc.cursor()); }
            });
            step(&mut rt);
            let from = point(&rt, 2);
            let to = point(&rt, 0);
            rt.pointer_down(from[0], from[1]); step(&mut rt);
            assert!(rt.state.focus == crate::app::Focus::Content);
            rt.pointer_move(to[0], to[1]); step(&mut rt);
            rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() });
            step(&mut rt);
            rt.pointer_up(to[0], to[1]); step(&mut rt);
            let b = rt.state.active_buffer().unwrap();
            assert_eq!(b.doc.mode, Mode::Normal, "{mode:?}");
            assert!(b.doc.visual_anchor.is_none());
            assert_eq!(b.doc.text_for_disk(), raw);
            assert!(!b.dirty());
            assert!(b.markdown_cache.drag.active.borrow().is_none());
        }
    }

    #[test]
    fn reorder_flushes_through_shared_replica_and_replica_undo_redo() {
        use crate::{collab::doc::Binding, crdt::CrdtTextBuffer};
        use neoism_protocol::crdt::{CrdtClientMessage as Out, CrdtServerMessage as In};
        const ID: &str = "file:///notes/drag.md";
        let raw = "# Heading\r\n\r\nhé **text**\r\nsecond line\r\n";
        let hub = CrdtTextBuffer::with_text(90_000, raw);
        let snapshot = In::Snapshot { buffer_id: ID.into(), update_v1: hub.encode_full_update_v1(), state_vector_v1: hub.state_vector_v1() };
        let mut doc = Document::from_text(raw);
        let mut peer = Document::from_text(raw);
        let mut a = Binding::new(101, ID.into(), &doc);
        let mut b = Binding::new(102, ID.into(), &peer);
        assert!(a.event(&snapshot, &mut doc).error.is_none());
        assert!(b.event(&snapshot, &mut peer).error.is_none());
        let revision = doc.revision;
        assert!(reorder(&mut doc, revision, 2..4, 0));
        let moved = "hé **text**\r\nsecond line\r\n# Heading\r\n\r\n";
        for action in ["move", "undo", "redo"] {
            if action == "undo" { assert!(doc.undo()); }
            if action == "redo" { assert!(doc.redo()); }
            let sent = a.flush(&mut doc);
            assert!(!sent.is_empty(), "{action} must publish an update");
            for message in sent {
                if let Out::Owned { message, .. } = message {
                    if let Out::ApplySync { envelope } = *message {
                        hub.apply_update_v1(&envelope.update_v1).unwrap();
                        let echo = In::Sync { envelope };
                        assert!(a.event(&echo, &mut doc).error.is_none());
                        assert!(b.event(&echo, &mut peer).error.is_none());
                    }
                }
            }
            let expected = if action == "undo" { raw } else { moved };
            assert_eq!(doc.text_for_disk(), expected, "{action}");
            assert_eq!(peer.text_for_disk(), expected, "peer {action}");
            assert_eq!(hub.text(), expected, "daemon {action}");
        }
    }
}