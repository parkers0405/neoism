//! Inline media shares the asynchronous agent image cache. Each image is a virtual
//! row belonging to its source line; surrounding prose retains its source stops.
use super::*;

pub(super) struct Row {
    pub source: usize,
    pub width: f32,
    pub height: f32,
    image: Option<rune::Image>,
    label: String,
}

#[derive(Debug)]
struct Source { offset: usize, end: usize, src: String, width: Option<f32> }

fn sources(raw: &str) -> Vec<Source> {
    if !raw.contains("![") { return Vec::new(); }
    use pulldown_cmark::{Event, Parser, Tag};
    let mut out = Vec::new();
    let mut excluded = Vec::new();
    for (event, range) in Parser::new(raw).into_offset_iter() {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                excluded.push(range.clone());
                out.push(Source { offset: range.start, end: range.end, src: dest_url.into_string(), width: None });
            }
            Event::Code(_) | Event::InlineHtml(_) | Event::Html(_) | Event::Start(Tag::CodeBlock(_)) => excluded.push(range),
            _ => {}
        }
    }
    // Obsidian embeds are line-local. A numeric suffix is a requested width, not
    // part of the filename. Code/standard-image ranges are never interpreted twice.
    for (offset, _) in raw.match_indices("![[") {
        if excluded.iter().any(|r| r.contains(&offset)) || raw.as_bytes()[..offset].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1 { continue; }
        let Some(end) = raw[offset + 3..].find("]]") else { continue };
        let inner = &raw[offset + 3..offset + 3 + end];
        let (src, width) = inner.rsplit_once('|').map_or((inner, None), |(src, size)| (src, size.parse::<f32>().ok().filter(|n| n.is_finite() && *n > 0.0)));
        out.push(Source { offset, end: offset + 3 + end + 2, src: src.to_owned(), width });
    }
    out.sort_by_key(|s| s.offset);
    out
}

/// Projection and rendering agree on exactly which source spans own image rows.
/// In particular, code, escapes and malformed Markdown never disappear merely
/// because they resemble an embed. No image loading is needed for projection.
pub(super) fn spans(raw: &str) -> Vec<Range<usize>> {
    sources(raw).into_iter().map(|source| source.offset..source.end).collect()
}

pub(super) fn rows(app: &App, buffer: &Buffer, raw: &str, width: f32) -> Vec<Row> {
    let note = std::path::Path::new(&buffer.path);
    let note = if note.is_absolute() { note.to_path_buf() } else { std::path::Path::new(&app.cwd).join(note) };
    let base = note.parent().and_then(|p| p.to_str()).unwrap_or("");
    sources(raw).into_iter().map(|source| {
        let image = app.markdown_image_at(&source.src, base);
        let size = image.as_ref().and_then(|i| i.size());
        let max = source.width.unwrap_or(width).min(width).max(1.0);
        let (w, h) = size.map_or((max, 48.0 * app.ty.scale()), |[w, h]| {
            let fit = max.min(w as f32);
            (fit, fit * h as f32 / (w as f32).max(1.0))
        });
        let label = if source.src.starts_with("data:") { "Embedded image".to_owned() } else { source.src.chars().take(160).collect() };
        Row { source: source.offset, width: w, height: h, image, label }
    }).collect()
}

pub(super) fn view(app: &App, row: &Row) -> El {
    let frame = el().w(row.width).h(row.height);
    if let Some(image) = &row.image && image.size().is_some() {
        frame.pixels_fit(image.pixels(), rune::SurfaceFit::Fit)
    } else {
        let loading = row.image.as_ref().is_some_and(|i| !i.is_ready());
        frame.fill(rgb(app.pal.surface)).padding(8.0 * app.ty.scale())
            .child(rune::text(format!("{}: {}", if loading { "Loading image" } else { "Image unavailable" }, row.label))
                .font(app.ty.body()).color(rgb(app.pal.muted)).wrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn syntax_ignores_code_and_preserves_multiple_embeds_in_prose() {
        let found = sources("before ![a](a.png) middle ![[b.png|123]] after `![[ignored.png]]` `![no](no.png)`");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].src, "a.png");
        assert_eq!(found[1].src, "b.png");
        assert_eq!(found[1].width, Some(123.0));
    }

    #[test]
    fn preview_hides_only_rendered_embed_spans_and_keeps_unicode_source_stops() {
        let raw = "hé ![**redundant label**](a(b).png) 世界 ![[b.png|120]] after";
        let lines = parse::project(&[raw.into()], usize::MAX..=usize::MAX);
        let line = &lines[0];
        assert_eq!(line.text, "hé  世界  after");
        assert_eq!(line.source.len(), line.text.len() + 1);
        for (byte, ch) in line.text.char_indices() {
            assert!(raw.is_char_boundary(line.source[byte]));
            assert_eq!(raw[line.source[byte]..].chars().next(), Some(ch));
        }
        assert!(line.source.windows(2).all(|w| w[0] <= w[1]));
        assert!(line.marks.is_empty(), "hidden alt labels must not leave styling spans");
        assert_eq!(parse::project(&[raw.into()], 0..=0)[0].text, raw);

        for raw in ["![label](image.png)", "![[image.png|120]]"] {
            let line = parse::project(&[raw.into()], usize::MAX..=usize::MAX).remove(0);
            assert!(line.text.is_empty());
            assert_eq!(line.source, vec![raw.len()], "image-only preview retains its empty source anchor");
        }
        let raw = "`![code](image.png)` and \\![[literal.png|120]]";
        let line = parse::project(&[raw.into()], usize::MAX..=usize::MAX).remove(0);
        assert!(line.text.contains("![code](image.png)"));
        assert!(line.text.contains("!"));
        assert!(spans(raw).is_empty());
        let literal = vec!["---".into(), "cover: ![[header.png|120]]".into(), "---".into(), "```md".into(), "![code](image.png)".into(), "```".into()];
        let projected = parse::project(&literal, usize::MAX..=usize::MAX);
        assert_eq!(projected[1].text, literal[1]);
        assert_eq!(projected[4].text, literal[4]);
    }

    #[test]
    fn remote_presence_survives_hidden_embeds_and_empty_image_line_anchor() {
        use neoism_protocol::crdt::{CrdtCursorPosition, CrdtPeerPresence, CrdtPresenceColor};
        let raw = "hé ![label](image.png) 世界\n![[image.png|120]]\n";
        let mut app = super::super::tests::app(raw);
        let id = app.active_buffer().unwrap().shared.as_ref().unwrap().buffer_id.clone();
        app.collab.stage(raw.lines().enumerate().map(|(line, text)| CrdtPeerPresence {
            buffer_id: id.clone(), peer_id: format!("image-peer-{line}"), display_name: format!("ImagePeer{line}"), client_os: None,
            color: CrdtPresenceColor { r: 10, g: 200, b: 100 },
            cursor: CrdtCursorPosition { line: line as u32, column: text.encode_utf16().count() as u32, offset: None },
            selection: None, insert: true, rainbow: false, updated_at_ms: 1,
        }).collect());
        let mut rt = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
            col().w_full().h_full().child(super::super::view(a, a.active_buffer().unwrap()))
        }, (620.0, 380.0), rune::Env::new(true, 1.0));
        for mode in [Mode::Normal, Mode::Insert] {
            rt.state.active_buffer_mut().unwrap().doc.mode = mode;
            for _ in 0..4 { rt.frame(1.0 / 60.0); }
            for line in 0..2 {
                let key = format!("peer-caret:ImagePeer{line}");
                let carets: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some(key.as_str())).collect();
                assert_eq!(carets.len(), 1, "exactly one peer caret per source line in {mode:?}");
                assert!(carets[0].rect.iter().all(|n| n.is_finite()));
            }
            let b = rt.state.active_buffer().unwrap();
            assert_eq!(b.doc.text_for_disk(), raw);
            assert!(!b.dirty());
        }
    }

    #[test]
    fn runtime_decode_resize_hit_reveal_and_remote_source_revision() {
        let raw = "before ![wordmark](neoism-wordmark.png) after\n![[neoism-wordmark.png|120]]\n```md\n![not](neoism-wordmark.png)\n```\n";
        let mut app = super::super::tests::app(raw);
        app.images = Some(rune::ImageCache::new(Arc::new(|| {})));
        let base = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/splash");
        app.active_buffer_mut().unwrap().path = format!("{base}/note.md");
        // Layout is nonblocking; deterministic tests alone wait for the worker.
        let initial = layout(&app, app.active_buffer().unwrap());
        assert_eq!(initial.rows.iter().filter(|r| r.row.image.is_some()).count(), 2);
        let decoded = app.markdown_image_at("neoism-wordmark.png", base).unwrap();
        decoded.wait().unwrap();
        assert!(app.inline_images_changed());
        let mut rt = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
            col().w_full().h_full().child(super::super::view(a, a.active_buffer().unwrap()))
        }, (620.0, 380.0), rune::Env::new(true, 1.0));
        rt.frame(1.0 / 60.0);
        let b = rt.state.active_buffer().unwrap();
        let laid = layout(&rt.state, b);
        assert!(!Rc::ptr_eq(&initial, &laid));
        let images: Vec<_> = laid.rows.iter().filter(|e| e.row.image.is_some()).collect();
        let first = images[0];
        let row = first.row.image.as_ref().unwrap();
        let [w, h] = decoded.size().unwrap();
        assert!(row.width <= laid.width);
        assert!((row.height / row.width - h as f32 / w as f32).abs() < 0.001);
        assert_eq!(images[1].row.image.as_ref().unwrap().width, 120.0);
        assert_eq!(hit(&rt.state, b, laid.left + 10.0, first.y + 2.0), Some((0, 7)));
        assert!(laid.rows.iter().any(|e| e.line == 0 && e.row.pieces.iter().any(|p| p.text.contains("after"))));
        assert_eq!(laid.rows.iter().filter(|e| e.line == 0).flat_map(|e| e.row.pieces.iter()).map(|p| p.text.as_str()).collect::<String>(), "before  after");
        let blank = laid.rows.iter().find(|e| e.line == 1 && e.row.image.is_none()).unwrap();
        assert_eq!(blank.row.pieces[0].text, "");
        assert_eq!(blank.row.pieces[0].source, vec![raw.lines().nth(1).unwrap().len()]);
        rt.state.active_buffer_mut().unwrap().view_rect.as_mut().unwrap()[2] = 300.0;
        let resized = layout(&rt.state, rt.state.active_buffer().unwrap());
        let narrow = resized.rows.iter().find_map(|e| e.row.image.as_ref()).unwrap();
        assert!(narrow.width < row.width);
        assert!((narrow.height / narrow.width - row.height / row.width).abs() < 0.001);
        let b = rt.state.active_buffer_mut().unwrap();
        b.doc.mode = Mode::Insert;
        b.doc.cursor_line = 0;
        rt.frame(1.0 / 60.0);
        let b = rt.state.active_buffer().unwrap();
        let revealed = layout(&rt.state, b);
        assert!(!revealed.rows.iter().any(|e| e.line == 0 && e.row.image.is_some()));
        assert_eq!(revealed.rows.iter().filter(|e| e.line == 0).flat_map(|e| e.row.pieces.iter()).map(|p| p.text.as_str()).collect::<String>(), raw.lines().next().unwrap());
        assert_eq!(b.doc.text_for_disk(), raw);
        assert!(!b.dirty());
        // This is the same revisioned remote snapshot path used by the daemon.
        rt.state.apply_opened_file("/notes/test.md", b"![changed](missing.png)\n");
        rt.state.active_buffer_mut().unwrap().doc.mode = Mode::Normal;
        rt.frame(1.0 / 60.0);
        let changed = layout(&rt.state, rt.state.active_buffer().unwrap());
        assert!(!Rc::ptr_eq(&revealed, &changed));
        assert!(changed.rows.iter().any(|e| e.row.image.as_ref().is_some_and(|r| r.label == "missing.png")));
    }

    #[test]
    fn shared_loader_respects_base_file_data_missing_and_no_network() {
        use base64::Engine as _;
        let mut app = App::new();
        app.images = Some(rune::ImageCache::new(Arc::new(|| {})));
        let base = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/splash");
        let good = app.markdown_image_at("neoism-wordmark.png", base).unwrap();
        good.wait().unwrap();
        let url = url::Url::from_file_path(format!("{base}/neoism-wordmark.png")).unwrap();
        assert_eq!(app.markdown_image_at(url.as_str(), "/elsewhere").unwrap().id(), good.id());
        let missing = app.markdown_image_at("neoism-wordmark.png", "/not-the-note").unwrap();
        assert!(missing.wait().is_err());
        assert!(app.markdown_image_at("https://example.com/image.png", base).is_none());
        let data = app.markdown_image_at("data:image/png;base64,AQID", base).unwrap();
        assert!(data.wait().is_err());
        let data = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(include_bytes!("../../../assets/splash/neoism-wordmark.png")));
        let embedded = app.markdown_image_at(&data, base).unwrap();
        embedded.wait().unwrap();
        assert_eq!(embedded.size(), good.size());
        assert!(app.inline_images_changed());
        assert!(!app.inline_images_changed());
    }
}