//! Source-safe diagram projection. Discovery is revision-scoped; rendering is asynchronous.
use super::*;

pub(super) struct Fence {
    pub range: Range<usize>,
    pub source: std::sync::Arc<str>,
}

pub(super) struct Diagram {
    pub range: Range<usize>,
    pub entry: crate::mermaid::Entry,
    pub row: usize,
    pub source: std::sync::Arc<str>,
}

fn delimiter(raw: &str) -> Option<(u8, usize, &str)> {
    let indent = raw.bytes().take_while(|&b| b == b' ').count();
    if indent > 3 { return None; }
    let text = &raw[indent..];
    let marker = *text.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') { return None; }
    let count = text.bytes().take_while(|&b| b == marker).count();
    (count >= 3).then(|| (marker, count, text[count..].trim()))
}

pub(super) fn language(info: &str) -> bool {
    info.split(|c: char| c.is_whitespace() || c == ',').next().is_some_and(|s| s.eq_ignore_ascii_case("mermaid"))
}

pub(super) fn fences(raw: &[String], lines: &[Line]) -> Vec<Fence> {
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
    if !raw.iter().zip(lines).any(|(raw, line)| matches!(line.kind, Kind::Fence)
        && delimiter(raw).is_some_and(|(_, _, info)| language(info))) { return Vec::new(); }
    let mut result = Vec::new();
    // The Markdown block parser excludes literal HTML, indented code and nested
    // examples. Run once per revision, never from a mounted row or animation.
    let text = raw.join("\n");
    let mut offsets = Vec::with_capacity(raw.len());
    let mut offset = 0;
    for line in raw { offsets.push(offset); offset += line.len() + 1; }
    for (event, range) in Parser::new(&text).into_offset_iter() {
        let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = event else { continue };
        if !language(info.trim()) { continue; }
        let start = offsets.partition_point(|&at| at <= range.start).saturating_sub(1);
        let end = offsets.partition_point(|&at| at < range.end).saturating_sub(1);
        if start >= end || !matches!(lines[start].kind, Kind::Fence) { continue; }
        let Some((marker, count, _)) = delimiter(&raw[start]) else { continue };
        if !delimiter(&raw[end]).is_some_and(|(m, n, info)| m == marker && n >= count && info.is_empty()) { continue; }
        result.push(Fence { range: start..end + 1, source: raw[start + 1..end].join("\n").into() });
    }
    result
}

pub(super) fn marks(app: &App, buffer: &Buffer, diagram: &Diagram) -> Vec<crate::collab::view::Mark> {
    let mut marks = Vec::new();
    // Hidden logical source positions share a diagram anchor, not fabricated text
    // columns. Iterate peers, not the potentially enormous source range.
    if let Some(shared) = &buffer.shared {
        let peers = app.collab.peers_in(&shared.buffer_id);
        let mut lines = Vec::new();
        for peer in peers {
            let cursor = peer.cursor.line as usize;
            if diagram.range.contains(&cursor) { lines.push(cursor); }
            if let Some(selection) = peer.selection {
                let (a, b) = (selection.anchor.line as usize, selection.head.line as usize);
                let line = a.min(b).max(diagram.range.start);
                if line < diagram.range.end && line <= a.max(b) { lines.push(line); }
            }
        }
        lines.sort_unstable(); lines.dedup();
        for line in lines {
            for mut mark in presence::marks(app, buffer, line) {
                mark.caret = mark.caret.map(|(_, insert)| (0, insert));
                mark.band = mark.band.map(|_| (0, 0, true));
                if !marks.contains(&mark) { marks.push(mark); }
            }
        }
    }
    marks
}

pub(super) fn edit_button(app: &App, buffer: &Buffer, start: usize, preview: bool) -> El {
    let tab = app.active_tab().id;
    let path = buffer.path.clone();
    el().key(if preview { "mermaid-preview" } else { "mermaid-edit" })
        .h(26.0).px(10.0).radius(6.0).center().cursor(rune::Cursor::Pointer)
        .hover(rgb(app.pal.fg).alpha(0.085))
        .child(rune::text(if preview { "Preview" } else { "Edit" }).font(app.ty.small()).color(rgb(app.pal.dim)))
        .on_press_tx(move |a: &mut App, tx| {
            a.select_tab(tab);
            let Some(b) = a.active_buffer().filter(|b| b.path == path) else { return };
            if !b.doc.lines.get(start).and_then(|s| delimiter(s)).is_some_and(|(_, _, info)| language(info.trim())) { return; }
            tx.blur();
            a.focus = crate::app::Focus::Content;
            a.editor_key(&rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() }, tx);
            if !preview {
                let line = (start + 1).min(a.active_buffer().unwrap().doc.lines.len().saturating_sub(1));
                a.editor_click(line, 0, false);
                a.editor_key(&rune::KeyEvent { key: rune::Key::Text("i".into()), mods: Default::default() }, tx);
            }
            a.editor_follow = true;
        })
}

pub(super) fn view(app: &App, buffer: &Buffer, diagram: &Diagram, width: f32, marks: Vec<crate::collab::view::Mark>, selected: bool) -> El {
    let mut source = diagram.source.to_string();
    if diagram.range.len() > 2 { source.push('\n'); }
    let mut visual = col().w(width);
    if !matches!(diagram.entry, crate::mermaid::Entry::Failed(_)) {
        visual = visual.child(crate::agent::timeline::code_header_with_action(app.pal, app.ty, "mermaid", source,
            Some(edit_button(app, buffer, diagram.range.start, false))));
    }
    visual = visual.child(crate::ui::mermaid::body(app, &diagram.entry, width));
    if selected {
        visual = visual.border(1.0, rgb(app.pal.accent).alpha(0.6));
    }
    for mark in marks {
        let x_of = |_: usize| 0.0;
        let geometry = crate::collab::view::RowGeometry { x_of: &x_of, start: 0, end: 0, last: true,
            line_len: 0, line_h: app.ty.body().line_px(), adv: app.ty.body().size * 0.55,
            font: app.ty.body().size, top: false };
        visual = visual.child(crate::collab::view::decorate(app,
            el().absolute(0.0, 38.0).w(width).h(app.ty.body().line_px()), &mark, &geometry));
    }
    visual
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(raw: &str) -> Vec<Fence> {
        let raw: Vec<_> = raw.lines().map(str::to_owned).collect();
        fences(&raw, &parse::project(&raw, usize::MAX..=usize::MAX))
    }

    #[test]
    fn only_closed_real_mermaid_fences_are_discovered() {
        for raw in ["```mermaid\ngraph TD; A-->B\n```", "~~~mermaid\ngraph TD; A-->B\n~~~~", "```MERMAID title=chart\ngraph TD; A-->B\n```"] {
            let found = found(raw);
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].range, 0..3);
            assert_eq!(&*found[0].source, "graph TD; A-->B");
        }
        for raw in [
            "```mermaid\ngraph TD; A-->B",
            "```rust\nlet s = \"```mermaid\";\n```",
            "````text\n```mermaid\ngraph TD; A-->B\n```\n````",
            "~~~text\n```mermaid\ngraph TD; A-->B\n```\n~~~",
            "    ```mermaid\n    graph TD; A-->B\n    ```",
            "` ```mermaid `\ntext",
            "---\n```mermaid\ngraph TD; A-->B\n```\n---",
            "```mermaid\ngraph TD; A-->B\n~~~",
            "<pre>\n```mermaid\ngraph TD; A-->B\n```\n</pre>",
            "<!--\n```mermaid\ngraph TD; A-->B\n```\n-->",
        ] { assert!(found(raw).is_empty(), "not a diagram: {raw}"); }
    }

    #[test]
    fn projection_is_cached_and_every_hidden_source_position_can_reveal_and_undo() {
        let raw = "before\n```mermaid\ngraph TD\n  A[hé😀] --> B[世界]\n```\nafter\n";
        let mut app = super::super::tests::app(raw);
        let b = app.active_buffer().unwrap();
        let laid = layout(&app, b);
        assert_eq!(laid.diagrams.len(), 1);
        assert!(Rc::ptr_eq(&laid, &layout(&app, b)), "unchanged frames reuse layout and requests");
        assert!(laid.rows.iter().all(|e| !(2..4).contains(&e.line)));
        let row = &laid.rows[laid.diagrams[0].row];
        assert_eq!(hit(&app, b, laid.left + 10.0, row.y + 50.0), Some((2, 0)));
        for line in 1..5 {
            let source = app.active_buffer().unwrap().doc.lines[line].clone();
            for col in source.char_indices().map(|(col, _)| col).chain(std::iter::once(source.len())) {
                let b = app.active_buffer_mut().unwrap();
                b.doc.cursor_line = line; b.doc.cursor_col = col; b.doc.mode = Mode::Normal;
                assert!(cursor_entry(&layout(&app, app.active_buffer().unwrap()), app.active_buffer().unwrap()).is_some());
                app.active_buffer_mut().unwrap().doc.mode = Mode::Insert;
                let b = app.active_buffer().unwrap();
                let revealed = layout(&app, b);
                assert!(revealed.diagrams.is_empty(), "active Insert never submits partial diagrams");
                for source_line in 1..5 {
                    let displayed: String = revealed.rows.iter().filter(|e| e.line == source_line)
                        .flat_map(|e| e.row.pieces.iter().map(|p| p.text.as_str())).collect();
                    assert_eq!(displayed, b.doc.lines[source_line]);
                }
            }
        }
        let b = app.active_buffer_mut().unwrap();
        b.doc.cursor_line = 3; b.doc.cursor_col = b.doc.lines[3].find('世').unwrap();
        b.doc.type_text("!");
        b.doc.key(Key::Esc, &mut b.prompt, None);
        assert!(b.dirty()); assert!(b.doc.undo());
        assert_eq!(b.doc.text_for_disk(), raw); assert!(!b.dirty());
        let b = app.active_buffer().unwrap();
        assert_eq!(layout(&app, b).diagrams.len(), 1);
    }

    #[test]
    fn huge_chart_projects_one_virtual_row_and_survives_wrap_and_scroll_changes() {
        let raw = format!("```mermaid\ngraph TD\n{}A --> B\n```\n\n| h | v |\n| --- | --- |\n| a | b |\n", "%% note\n".repeat(3_000));
        let mut app = super::super::tests::app(&raw);
        for width in [620.0, 280.0, 900.0] {
            let b = app.active_buffer_mut().unwrap();
            b.view_rect = Some([0.0, 0.0, width, 400.0]);
            b.scroll.set((100.0, 2000.0, 400.0));
            let b = app.active_buffer().unwrap();
            // Initial native font fallback registration may change font_epoch once.
            let _ = layout(&app, b);
            let laid = layout(&app, b);
            assert_eq!(laid.diagrams.len(), 1);
            assert!(laid.rows.len() < 20, "source lines do not become mounted diagram rows");
            assert_eq!(laid.tables.len(), 1);
            assert!(laid.tables[0].top > laid.rows[0].y + laid.rows[0].row.height);
            assert!(Rc::ptr_eq(&laid, &layout(&app, b)));
            assert_eq!(b.doc.text_for_disk(), raw);
        }
    }

    fn runtime(raw: &str) -> rune::Runtime<App> {
        let mut app = super::super::tests::app(raw);
        app.active_buffer_mut().unwrap().view_rect = None;
        let mut rt = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
            super::super::super::view::view(a)
        }, (620.0, 380.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        rt
    }

    #[test]
    fn opening_many_charts_does_not_schedule_offscreen_diagrams() {
        let source: String = (0..48).map(|i| format!("```mermaid\nflowchart TD\nA{i}-->B{i}\n```\n\n")).collect();
        let rt = runtime(&source);
        let b = rt.state.active_buffer().unwrap();
        assert_eq!(layout(&rt.state, b).diagrams.len(), 48);
        assert!(rt.state.mermaid_cache.cached("flowchart TD\nA47-->B47", rt.state.pal).is_none(), "offscreen source never enters the worker queue");
        assert_eq!(b.doc.text_for_disk(), source);
    }

    fn finish(rt: &mut rune::Runtime<App>) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            rt.update(|a, _| { a.mermaid_cache.poll(); });
            rt.frame(1.0 / 60.0);
            let b = rt.state.active_buffer().unwrap();
            if !matches!(layout(&rt.state, b).diagrams[0].entry, crate::mermaid::Entry::Pending) { break; }
            assert!(std::time::Instant::now() < until, "diagram worker did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn native_edit_and_preview_buttons_preserve_source_changes_and_undo() {
        let raw = "```mermaid\nflowchart TD\nA[Start] --> B[Done]\n```\n";
        let mut rt = runtime(raw);
        finish(&mut rt);
        let click = |rt: &mut rune::Runtime<App>, key: &str| {
            let rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
            rt.pointer_down(rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5);
            rt.pointer_up(rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5);
            for _ in 0..4 { rt.frame(1.0 / 60.0); }
        };
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-edit")));
        click(&mut rt, "mermaid-edit");
        assert_eq!(rt.state.active_buffer().unwrap().doc.mode, Mode::Insert);
        assert!(layout(&rt.state, rt.state.active_buffer().unwrap()).diagrams.is_empty());
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("flowchart TD")));
        assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), raw);
        for ch in "%% note".chars() {
            rt.key(rune::KeyEvent { key: rune::Key::Text(ch.to_string()), mods: Default::default() });
            rt.frame(1.0 / 60.0);
        }
        rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.active_buffer().unwrap().dirty());
        click(&mut rt, "mermaid-preview");
        finish(&mut rt);
        let b = rt.state.active_buffer().unwrap();
        assert_eq!(b.doc.mode, Mode::Normal);
        assert!(b.doc.text_for_disk().contains("%% note\nflowchart TD"));
        assert!(b.dirty(), "Preview never marks unsaved edits clean");
        assert!(matches!(layout(&rt.state, b).diagrams[0].entry, crate::mermaid::Entry::Ready { .. }));
        assert!(!rt.elements().iter().any(|e| e.text.as_deref() == Some("Rendering diagram...")));
        rt.update(|a, _| { assert!(a.active_buffer_mut().unwrap().doc.undo()); });
        rt.frame(1.0 / 60.0);
        finish(&mut rt);
        assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), raw);
        assert!(!rt.state.active_buffer().unwrap().dirty());
        if let Some(path) = std::env::var_os("NEOISM_RUNE_MERMAID_EDIT_PNG") {
            use rune::render::{Gpu, Offscreen, Renderer, OFFSCREEN_FORMAT};
            let gpu = Gpu::headless();
            let target = Offscreen::with_format(&gpu.device, 620, 380, OFFSCREEN_FORMAT);
            let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (620, 380));
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
            target.copy(&mut enc);
            renderer.submit(&gpu.queue, enc);
            rune::render::write_png(std::path::Path::new(&path), 620, 380, &target.read(&gpu.device)).unwrap();
        }
    }

    #[test]
    fn native_ready_chart_click_reveal_edit_undo_and_async_layout_invalidation() {
        let raw = "```mermaid\nflowchart TD\nA[hé] --> B[αβ]\n```\n";
        let mut rt = runtime(raw);
        let pending = layout(&rt.state, rt.state.active_buffer().unwrap());
        finish(&mut rt);
        let clipboard = rune::clipboard::memory();
        rt.use_clipboard(&clipboard);
        let copy = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Copy")).unwrap().rect;
        rt.pointer_down(copy[0] + 3.0, copy[1] + 3.0); rt.pointer_up(copy[0] + 3.0, copy[1] + 3.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(clipboard.text().unwrap().as_str(), "flowchart TD\nA[hé] --> B[αβ]\n");
        assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, 0, "Copy leaves logical caret alone");
        let b = rt.state.active_buffer().unwrap();
        let ready = layout(&rt.state, b);
        assert_ne!(pending.version.7, ready.version.7);
        assert!(matches!(ready.diagrams[0].entry, crate::mermaid::Entry::Ready { .. }));
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-diagram")));
        rt.update(|a, _| {
            let b = a.active_buffer_mut().unwrap();
            b.external_edit = Some(crate::editor::external_edit::Flash::new(2, 2, b.doc.revision));
        });
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("external-edit")), "folded agent edits retain their landing animation");
        rt.update(|a, _| {
            if let Some(f) = a.active_buffer_mut().unwrap().external_edit.take() { f.tick(false, f.revision); }
        });
        rt.frame(1.0 / 60.0);
        let b = rt.state.active_buffer().unwrap();
        let rect = b.view_rect.unwrap();
        let entry = &ready.rows[ready.diagrams[0].row];
        let point = [rect[0] + ready.left + 30.0, rect[1] + entry.y + 45.0];
        rt.pointer_down(point[0], point[1]); rt.pointer_up(point[0], point[1]);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, 1);
        rt.update(|a, _| { assert!(before_key(a, Key::Char('i'), false)); });
        rt.frame(1.0 / 60.0);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("```mermaid")));
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("A[hé] --> B[αβ]")));
        rt.update(|a, _| {
            let b = a.active_buffer_mut().unwrap();
            b.doc.type_text("!"); b.doc.key(Key::Esc, &mut b.prompt, None);
            assert!(b.doc.undo()); assert_eq!(b.doc.text_for_disk(), raw);
        });
        rt.frame(1.0 / 60.0);
        assert!(matches!(layout(&rt.state, rt.state.active_buffer().unwrap()).diagrams[0].entry, crate::mermaid::Entry::Ready { .. }));
    }

    #[test]
    fn native_invalid_chart_retains_visible_editable_source() {
        let raw = "~~~mermaid\nnot a diagram\n~~~\n";
        let mut rt = runtime(raw);
        finish(&mut rt);
        let b = rt.state.active_buffer().unwrap();
        let laid = layout(&rt.state, b);
        assert!(matches!(laid.diagrams[0].entry, crate::mermaid::Entry::Failed(_)));
        assert_eq!(laid.cards.len(), 1);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("not a diagram")));
        assert_eq!(b.doc.text_for_disk(), raw);
        assert!(!b.dirty());
    }
}