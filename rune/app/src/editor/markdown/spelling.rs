//! Spelling is a prose decoration, independent of language servers and source projection.
use super::*;
use crate::ui::context::Command;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WordTarget {
    pub path: String,
    pub tab: u64,
    pub revision: u64,
    pub line: usize,
    pub range: Range<usize>,
    pub word: String,
    pub suggestions: Vec<String>,
}

#[derive(Clone)]
pub(super) struct Word {
    pub(super) range: Range<usize>,
    text: String,
    suggestions: Vec<String>,
}

fn prose(line: &Line) -> String {
    if matches!(line.kind, Kind::Code { .. } | Kind::Fence | Kind::Frontmatter | Kind::Rule) { return String::new(); }
    let mut bytes = line.text.as_bytes().to_vec();
    for mark in line.marks.iter().filter(|m| m.code || m.link) {
        bytes[mark.start..mark.end].fill(b' ');
    }
    String::from_utf8(bytes).expect("whole marked spans replaced")
}

pub(super) fn words(app: &App, buffer: &Buffer, line: usize) -> Vec<Word> {
    let projected = parsed(buffer);
    let Some(projected) = projected.get(line) else { return Vec::new() };
    let text = prose(projected);
    if text.is_empty() { return Vec::new(); }
    let Some(found) = buffer.markdown_cache.spelling.check(&text, app.waker.clone()) else { return Vec::new() };
    found.iter().filter_map(|word| {
        let start = *projected.source.get(word.range.start)?;
        // A display boundary after **bold** can skip closing syntax. End at the last
        // actual source byte instead, and never replace words spanning hidden markup.
        let end = projected.source.get(word.range.end.checked_sub(1)?)? + 1;
        (buffer.doc.lines[line].get(start..end) == Some(word.text.as_str())).then(|| Word {
            range: start..end, text: word.text.clone(), suggestions: word.suggestions.clone(),
        })
    }).collect()
}

pub(super) fn decorate(app: &App, mut visual: El, piece: &Piece, height: f32, words: &[Word]) -> El {
    let scale = app.ty.scale();
    for word in words {
        if word.range.end <= piece.source[0] || word.range.start >= *piece.source.last().unwrap() { continue; }
        for [x, _, w, _] in piece.geometry.range_rects(piece.selected(word.range.clone())) {
            if w <= 0.0 { continue; }
            let color = rgb(app.pal.red);
            let steps = (w / (2.0 * scale)).ceil() as usize;
            let points: Vec<_> = (0..=steps).map(|i| [(i as f32 * 2.0 * scale).min(w),
                if i % 2 == 0 { scale } else { 2.0 * scale }]).collect();
            visual = visual.child(rune::canvas(rune::Shape::new().view_box(w, 3.0 * scale)
                .stroke(points, scale.max(1.0), color)).key("spelling-underline")
                .absolute(piece.x + x, height - 3.0 * scale).w(w).h(3.0 * scale));
        }
    }
    visual
}

pub(crate) fn open(app: &mut App, at: [f32; 2]) {
    let Some(buffer) = app.active_buffer().filter(|b| enabled(b)) else { return };
    let Some(rect) = buffer.view_rect else { return };
    let Some((line, col)) = super::super::view::hit_window(app, at[0], at[1], false) else { return };
    let laid = layout(app, buffer);
    let y = at[1] - rect[1] + buffer.scroll.get().0;
    let x = at[0] - rect[0] - laid.left + laid.table(line).map_or(0.0, table::Table::offset);
    let Some(word) = words(app, buffer, line).into_iter().find(|w| w.range.contains(&col) || col == w.range.end) else { return };
    // Nearest-caret hit testing also accepts blank row space. A spelling menu does not.
    if !laid.rows.iter().filter(|e| e.line == line && e.y <= y && y < e.y + e.row.height).any(|e|
        e.row.pieces.iter().any(|p| p.geometry.range_rects(p.selected(word.range.clone())).iter()
            .any(|r| x >= p.x + r[0] && x <= p.x + r[0] + r[2]))) { return; }
    let target = WordTarget { path: buffer.path.clone(), tab: app.active_tab().id, revision: buffer.doc.revision,
        line, range: word.range, word: word.text, suggestions: word.suggestions };
    app.focus = crate::app::Focus::Content;
    app.open_spelling_context(target, at);
}

pub(crate) fn apply(app: &mut App, target: &WordTarget, command: Command) {
    if app.active_tab().id != target.tab { return; }
    let Some(buffer) = app.active_buffer_mut().filter(|b| b.path == target.path
        && b.doc.revision == target.revision && enabled(b)) else { return };
    if buffer.doc.lines.get(target.line).and_then(|s| s.get(target.range.clone())) != Some(target.word.as_str()) { return; }
    match command {
        Command::SpellingReplace(index) => {
            let Some(replacement) = target.suggestions.get(index) else { return };
            let doc = &mut buffer.doc;
            doc.end_insert_group();
            doc.save_undo();
            doc.lines[target.line].replace_range(target.range.clone(), replacement);
            doc.visual_anchor = None;
            if doc.mode == Mode::Visual { doc.mode = Mode::Normal; }
            doc.set_cursor(Pos { line: target.line, col: target.range.start });
            doc.mark_edited();
            doc.commit_undo();
            buffer.markdown_cache.clear_vertical_goal();
            app.editor_follow = true;
        }
        Command::SpellingIgnore => buffer.markdown_cache.spelling.ignore(&target.word),
        Command::SpellingLearn => {
            if let Err(error) = buffer.markdown_cache.spelling.learn(&target.word) {
                buffer.message = Some(format!("Dictionary: {error}"));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn finish(app: &App, line: usize) -> Vec<Word> {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let buffer = app.active_buffer().unwrap();
            let _ = words(app, buffer, line);
            let projected = parsed(buffer);
            if buffer.markdown_cache.spelling.check(&prose(&projected[line]), None).is_some() { return words(app, buffer, line); }
            assert!(Instant::now() < until, "spelling worker did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn spelling_projects_prose_and_table_words_without_touching_markup_or_code() {
        let raw = "**mispelled** `mispelled` [mispelled](https://mispelled.example)\n\n| mispelled | `mispelled` |\n| --- | --- |\n| text | text |\n\n```mermaid\nmispelled\n```\n";
        let mut app = super::super::tests::app(raw);
        let found = finish(&app, 0);
        assert_eq!(found.len(), 1);
        assert_eq!(&raw[found[0].range.clone()], "mispelled");
        assert_eq!(finish(&app, 2).len(), 1);
        assert!(words(&app, app.active_buffer().unwrap(), 7).is_empty());
        let target = WordTarget { path: app.active_buffer().unwrap().path.clone(), tab: app.active_tab().id,
            revision: app.active_buffer().unwrap().doc.revision, line: 0, range: found[0].range.clone(), word: found[0].text.clone(), suggestions: found[0].suggestions.clone() };
        apply(&mut app, &target, Command::SpellingReplace(0));
        let buffer = app.active_buffer_mut().unwrap();
        assert!(buffer.doc.lines[0].starts_with("**misspelled** `mispelled`"));
        assert!(buffer.dirty());
        assert!(buffer.doc.undo());
        assert_eq!(buffer.doc.text_for_disk(), raw);
        assert!(!buffer.dirty());
        // Even if the word is unchanged, an intervening edit invalidates a captured action.
        apply(&mut app, &target, Command::SpellingReplace(0));
        assert_eq!(app.active_buffer().unwrap().doc.text_for_disk(), raw);
    }

    #[test]
    fn native_spelling_right_click_correction_and_undo() {
        let raw = "# Spelling\n\nA **mispelled** word.\n\n| Heading | Notes |\n| --- | --- |\n| mispelled | `mispelled` |\n";
        let mut app = super::super::tests::app(raw);
        app.window = [900.0, 600.0];
        let mut rt = rune::Runtime::with_text(app, crate::ui::chrome::root, (900.0, 600.0), rune::Env::new(true, 1.0),
            rune::text::TextSystem::new(crate::FONTS, ("Geist", "Geist Mono")));
        rt.on_key = Some(crate::on_key);
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        let found = finish(&rt.state, 2);
        assert!(super::super::spelling_changed(&rt.state), "worker completion requests a production view commit");
        rt.update(|_, _| {});
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("spelling-underline")));
        let b = rt.state.active_buffer().unwrap();
        let laid = layout(&rt.state, b);
        let entry = laid.rows.iter().find(|e| e.line == 2).unwrap();
        let piece = &entry.row.pieces[0];
        let r = piece.geometry.range_rects(piece.selected(found[0].range.clone()))[0];
        let rect = b.view_rect.unwrap();
        let point = [rect[0] + laid.left + piece.x + r[0] + r[2] * 0.5,
            rect[1] + entry.y - b.scroll.get().0 + entry.row.height * 0.5];
        rt.secondary_down(point[0], point[1]); rt.secondary_up(point[0], point[1]);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Add to Dictionary")));
        if let Some(path) = std::env::var_os("NEOISM_RUNE_SPELLING_PNG") {
            use rune::render::{Gpu, Offscreen, Renderer, OFFSCREEN_FORMAT};
            let gpu = Gpu::headless();
            let target = Offscreen::with_format(&gpu.device, 900, 600, OFFSCREEN_FORMAT);
            let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (900, 600));
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
            target.copy(&mut enc); renderer.submit(&gpu.queue, enc);
            rune::render::write_png(std::path::Path::new(&path), 900, 600, &target.read(&gpu.device)).unwrap();
        }
        let suggestion = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("misspelled")).unwrap().rect;
        rt.pointer_down(suggestion[0] + 3.0, suggestion[1] + 3.0);
        rt.pointer_up(suggestion[0] + 3.0, suggestion[1] + 3.0);
        rt.frame(1.0 / 60.0);
        let buffer = rt.state.active_buffer_mut().unwrap();
        assert!(buffer.doc.lines[2].contains("**misspelled**"));
        assert!(buffer.doc.undo());
        assert_eq!(buffer.doc.text_for_disk(), raw);
    }
}