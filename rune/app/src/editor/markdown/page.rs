//! Optional page decorations and source-mapped YAML properties. The document
//! remains the only editable state; this is a presentation of its frontmatter.
use super::*;
use std::path::Path;

pub(super) struct Property {
    line: usize,
    key: String,
    value: Range<usize>,
    text: String,
    chips: bool,
    tags: bool,
}

pub(super) struct Page {
    pub range: Range<usize>,
    properties: Vec<Property>,
}

fn value_range(raw: &str, from: usize) -> Range<usize> {
    let start = from + raw[from..].len() - raw[from..].trim_start().len();
    let mut end = raw.trim_end().len().max(start);
    let quote = raw.as_bytes().get(start).copied().filter(|c| matches!(c, b'\'' | b'"'));
    if let Some(quote) = quote {
        if let Some(close) = raw[start + 1..end].rfind(quote as char) {
            return start + 1..start + 1 + close;
        }
    } else if let Some(comment) = raw[start..end].find(" #") {
        end = start + raw[start..start + comment].trim_end().len();
    }
    start..end
}

impl Page {
    pub fn new(lines: &[String]) -> Option<Self> {
        if lines.first()?.trim() != "---" { return None; }
        let end = lines.iter().enumerate().skip(1).find(|(_, s)| matches!(s.trim(), "---" | "..."))?.0;
        let mut properties = Vec::new();
        let mut list = false;
        let mut tag_list = false;
        for (line, raw) in lines.iter().enumerate().take(end).skip(1) {
            if list && raw.trim_start().starts_with("- ") {
                let from = raw.find('-').unwrap() + 2;
                let value = value_range(raw, from);
                properties.push(Property { line, key: String::new(), text: raw[value.clone()].to_owned(), value, chips: true, tags: tag_list });
                continue;
            }
            if raw.trim().is_empty() || raw.trim_start().starts_with('#') { continue; }
            list = false;
            // Leave nested mappings, block scalars and unusual YAML syntax raw.
            if raw.starts_with(char::is_whitespace) { continue; }
            let Some((key, _)) = raw.split_once(':') else { continue; };
            let key = key.trim();
            if key.is_empty() || key.contains(['\'', '"', '[', ']', '{', '}']) { continue; }
            let value = value_range(raw, raw.find(':').unwrap() + 1);
            let text = raw[value.clone()].to_owned();
            if matches!(text.as_str(), "|" | ">" | "|-" | ">-" | "|+" | ">+") { continue; }
            let chips = ["tags", "aliases", "cssclasses"].iter().any(|k| key.eq_ignore_ascii_case(k));
            list = chips && text.is_empty();
            tag_list = key.eq_ignore_ascii_case("tags");
            properties.push(Property { line, key: key.to_owned(), value, text, chips, tags: tag_list });
        }
        Some(Self { range: 0..end + 1, properties })
    }

    fn property(&self, key: &str) -> Option<&Property> {
        self.properties.iter().find(|p| p.key.eq_ignore_ascii_case(key) && !p.text.is_empty())
    }

    pub fn project(&self, raw: &str, line: usize) -> Line {
        let mut out = raw_line("", Kind::Frontmatter);
        if line == self.range.start || line + 1 == self.range.end {
            out.source[0] = raw.len();
            return out;
        }
        let Some(property) = self.properties.get(self.properties.partition_point(|p| p.line < line)).filter(|p| p.line == line) else {
            return raw_line(raw, Kind::Frontmatter);
        };
        if !property.key.is_empty() {
            let start = raw.find(&property.key).unwrap_or(0);
            append(&mut out, raw, start..start + property.key.len());
        }
        let gap = 18usize.saturating_sub(property.key.chars().count()).max(2);
        for n in 0..gap {
            out.text.push(' ');
            out.source.push(if n + 1 == gap { property.value.start } else { *out.source.last().unwrap() });
        }
        if property.chips {
            let mut range = property.value.clone();
            let array = raw[range.clone()].starts_with('[') && raw[range.clone()].ends_with(']');
            if array { range = range.start + 1..range.end - 1; }
            let mut from = range.start;
            let mut quote = None;
            let mut escaped = false;
            let mut tokens = Vec::new();
            for (at, ch) in raw[range.clone()].char_indices() {
                if escaped { escaped = false; continue; }
                if ch == '\\' && quote == Some('"') { escaped = true; continue; }
                if quote == Some(ch) { quote = None; }
                else if quote.is_none() && matches!(ch, '\'' | '"') { quote = Some(ch); }
                else if ch == ',' && quote.is_none() && array { tokens.push(from..range.start + at); from = range.start + at + 1; }
            }
            tokens.push(from..range.end);
            for token in tokens {
                let text = &raw[token.clone()];
                let start = token.start + text.len() - text.trim_start().len();
                let mut token = start..token.start + text.trim_end().len();
                if token.is_empty() { continue; }
                if matches!(raw.as_bytes()[token.start], b'\'' | b'"')
                    && raw.as_bytes()[token.end - 1] == raw.as_bytes()[token.start] && token.len() >= 2 {
                    token = token.start + 1..token.end - 1;
                }
                let start = out.text.len();
                if property.tags && !raw[token.clone()].starts_with('#') {
                    out.text.push('#'); out.source.push(token.start);
                }
                append(&mut out, raw, token.clone());
                out.marks.push(parse::Mark { start, end: out.text.len(), bold: false, italic: false,
                    strike: false, code: true, link: false });
                out.text.push(' '); out.source.push(token.end);
            }
        } else {
            append(&mut out, raw, property.value.clone());
        }
        *out.source.last_mut().unwrap() = raw.len();
        out
    }

    pub fn rows(&self, app: &App, line: &Line, index: usize, width: f32, hash: u64) -> Option<Vec<Rc<VisualRow>>> {
        self.properties.get(self.properties.partition_point(|p| p.line < index)).filter(|p| p.line == index)?;
        let key_end = line.text.find("  ")?;
        let value = key_end + line.text[key_end..].len() - line.text[key_end..].trim_start().len();
        let font = super::font(app, &Kind::Frontmatter);
        let key_width = rune::text::geometry(&line.text[..key_end], &[], font, None).caret(key_end)[0];
        let x = (key_width + 18.0 * app.ty.scale()).max(96.0 * app.ty.scale()).min(width * 0.45);
        let slice = |range: Range<usize>| Line {
            text: line.text[range.clone()].to_owned(), source: line.source[range.start..=range.end].to_vec(),
            marks: Vec::new(), kind: Kind::Frontmatter, indent: 0,
        };
        let spans = ink(app, line);
        let key_spans = slice_spans(&spans, 0..key_end);
        let value_spans = slice_spans(&spans, value..line.text.len());
        let keys = pieces_with_ink(app, &slice(0..key_end), (x - 12.0 * app.ty.scale()).max(16.0), 0.0, Some(&key_spans));
        let values = pieces_with_ink(app, &slice(value..line.text.len()), (width - x).max(16.0), x, Some(&value_spans));
        let mut rows = Vec::new();
        for n in 0..keys.len().max(values.len()) {
            let mut pieces = Vec::new();
            if let Some(key) = keys.get(n).filter(|p| !p.text.is_empty()) { pieces.push(key.clone()); }
            if let Some(value) = values.get(n) { pieces.push(value.clone()); }
            rows.push(Rc::new(VisualRow { pieces, height: font.line_px(), first: n == 0, hash, image: None }));
        }
        Some(rows)
    }

    pub fn height(&self, app: &App, width: f32) -> f32 {
        let s = app.ty.scale();
        let cover = if self.property("cover").is_some() { 170.0 * s } else { 0.0 };
        let Some(title) = self.property("title").or_else(|| self.property("icon")) else { return cover; };
        let icon = self.property("icon").filter(|p| p.line != title.line).map_or(0.0, |_| app.ty.chat_heading(1).size * 1.6);
        let geometry = rune::text::geometry_with(&title.text, &[], app.ty.chat_heading(1), Some((width - icon).max(20.0)),
            rune::text::RichLayoutOptions::new().wrap_anywhere());
        cover + 20.0 * s + geometry.line_ranges().len().max(1) as f32 * app.ty.chat_heading(1).line_px() + 12.0 * s
    }

    pub fn view(&self, app: &App, buffer: &Buffer, width: f32, left: f32, viewport: f32) -> El {
        let s = app.ty.scale();
        let height = self.height(app, width);
        let mut header = el().key("md-page-header").w(viewport).h(height);
        let cover_h = if let Some(cover) = self.property("cover") {
            let h = 170.0 * s;
            let note = Path::new(&buffer.path);
            let note = if note.is_absolute() { note.to_path_buf() } else { Path::new(&app.cwd).join(note) };
            let base = note.parent().unwrap_or_else(|| Path::new(&app.cwd));
            let mut pending = None;
            let mut ready = None;
            for path in cover_sources(&cover.text) {
                if let Some(image) = app.markdown_image_at(&path, &base.to_string_lossy()) {
                    if image.size().is_some() { ready = Some(image); break; }
                    if !image.is_ready() && pending.is_none() { pending = Some(image); }
                }
            }
            let mut banner = el().key("md-page-cover").w(viewport).h(h).clip_rounded(0.0);
            if let Some(image) = ready.or(pending) {
                banner = banner.pixels_fit(image.pixels(), rune::SurfaceFit::Fill);
            } else {
                banner = banner.child(rune::text("Cover unavailable").font(app.ty.small()).color(rgb(app.pal.muted)).absolute(left, h * 0.5));
            }
            header = header.child(property_click(app, cover, banner));
            h
        } else { 0.0 };
        let title_font = app.ty.chat_heading(1);
        let mut x = left;
        if let Some(icon) = self.property("icon") {
            let icon_w = title_font.size * 1.6;
            header = header.child(property_click(app, icon, rune::text(icon.text.clone()).font(title_font)
                .w(icon_w).h(title_font.line_px()).absolute(x, cover_h + 20.0 * s)));
            x += icon_w;
        }
        if let Some(title) = self.property("title") {
            header = header.child(property_click(app, title, rune::text(title.text.clone()).font(title_font).color(rgb(app.pal.fg)).wrap_anywhere()
                .w((width - (x - left)).max(20.0)).h((height - cover_h - 20.0 * s).max(title_font.line_px())).absolute(x, cover_h + 20.0 * s)));
        }
        header
    }
}

fn append(out: &mut Line, raw: &str, range: Range<usize>) {
    *out.source.last_mut().unwrap() = range.start;
    out.text.push_str(&raw[range.clone()]);
    out.source.extend(range.start + 1..=range.end);
}

pub(super) fn ink(app: &App, line: &Line) -> Vec<Span> {
    let mut spans = Vec::new();
    if let Some(end) = line.text.find("  ").filter(|&n| n > 0) {
        spans.push(Span::new(0, end).color(rgb(app.pal.muted)));
    }
    for mark in &line.marks {
        if mark.code { spans.push(Span::new(mark.start, mark.end).color(rgb(app.pal.green)).bg(rgb(app.pal.surface), 4.0 * app.ty.scale())); }
    }
    spans
}

fn property_click(app: &App, property: &Property, element: El) -> El {
    let (tab, line, col) = (app.active_tab().id, property.line, property.value.start);
    element.cursor(rune::Cursor::Pointer).on_press_tx(move |a: &mut App, tx: &mut Tx| {
        a.select_tab(tab); a.focus = crate::app::Focus::Content;
        a.editor_click(line, col, false); tx.blur();
    })
}

fn cover_sources(value: &str) -> Vec<String> {
    let value = value.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")).unwrap_or(value);
    if value.contains('/') || value.starts_with("data:") || Path::new(value).is_absolute() { return vec![value.to_owned()]; }
    let mut paths = Vec::new();
    if let Some(config) = dirs::config_dir() {
        let path = config.join("neoism/covers").join(value);
        paths.push(path.to_string_lossy().into_owned());
        if path.extension().is_none() {
            for ext in ["png", "jpg", "jpeg", "webp", "gif", "svg"] { paths.push(path.with_extension(ext).to_string_lossy().into_owned()); }
        }
    }
    paths.push(value.to_owned());
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_optional_header_and_properties_edit_source_and_do_not_leak_across_tabs() {
        let cover = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/splash/neoism-wordmark.png");
        let raw = format!("---\ntitle: 'Field Note'\nicon: 🦊\ncover: {cover}\ntags: [work, 世界]\ndescription: {}\n---\n# Body\n{}", "A long property value. ".repeat(30), "A paragraph.\n".repeat(60));
        let mut app = super::super::tests::app(&raw);
        app.ty.size = 14.0;
        app.images = Some(rune::ImageCache::new(std::sync::Arc::new(|| {})));
        app.markdown_image_at(cover, "/notes").unwrap().wait().unwrap();
        app.inline_images_changed();
        let mut rt = rune::Runtime::new(app, crate::ui::chrome::root,
            (900.0, 800.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("md-page-cover")));
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("tags")));
        assert!(rt.elements().iter().any(|e| e.text.as_deref().is_some_and(|s| s.contains("#世界"))));
        assert!(!rt.elements().iter().any(|e| e.text.as_deref() == Some("---")));
        let b = rt.state.active_buffer().unwrap();
        let laid = layout(&rt.state, b);
        assert!(laid.rows.iter().find(|e| e.line == 7).unwrap().y >= 170.0 + rt.state.ty.chat_heading(1).line_px());
        let value_rows: Vec<_> = laid.rows.iter().filter(|e| e.line == 5).collect();
        assert!(value_rows.len() > 1, "exercise a long property value");
        let value_x = value_rows[0].row.pieces.last().unwrap().x;
        assert!(value_x > 0.0);
        for row in &value_rows[1..] { assert_eq!(row.row.pieces[0].x, value_x, "wrapped values stay in their column"); }
        let title = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Field Note")).unwrap().rect;
        rt.pointer_down(title[0] + 4.0, title[1] + 4.0);
        rt.pointer_up(title[0] + 4.0, title[1] + 4.0);
        rt.frame(1.0 / 60.0);
        let b = rt.state.active_buffer().unwrap();
        assert_eq!((b.doc.cursor_line, b.doc.cursor_col), (1, b.doc.lines[1].find("Field").unwrap()));
        assert_eq!(b.doc.mode, Mode::Normal);
        rt.key(rune::KeyEvent { key: rune::Key::Text("i".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("title: 'Field Note'")));
        rt.key(rune::KeyEvent { key: rune::Key::Text("!".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("!Field Note")));
        rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() });
        rt.key(rune::KeyEvent { key: rune::Key::Text("u".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        let b = rt.state.active_buffer().unwrap();
        assert_eq!(b.doc.text_for_disk(), raw);
        assert!(!b.dirty());
        let page = Page::new(&b.doc.lines).unwrap();
        // Real source hit geometry and peer decorations agree through hidden YAML.
        let value = page.property("title").unwrap().value.start;
        let laid = layout(&rt.state, b);
        let (entry, piece) = cursor_entry(&laid, b).unwrap();
        assert_eq!(entry.line, 1);
        assert_eq!(hit(&rt.state, b, laid.left + piece.x + piece.x_at(piece.visible(value)) + 0.1, entry.y), Some((1, value)));
        let expected_x = b.view_rect.unwrap()[0] + laid.left + piece.x + piece.x_at(piece.visible(value));
        let id = b.shared.as_ref().unwrap().buffer_id.clone();
        let column = b.doc.lines[1][..value].encode_utf16().count() as u32;
        rt.update(|a, _| a.collab.stage(vec![neoism_protocol::crdt::CrdtPeerPresence {
            buffer_id: id, peer_id: "page-peer".into(), display_name: "Reader".into(), client_os: None,
            color: neoism_protocol::crdt::CrdtPresenceColor { r: 50, g: 180, b: 100 },
            cursor: neoism_protocol::crdt::CrdtCursorPosition { line: 1, column, offset: None },
            selection: None, insert: true, rainbow: false, updated_at_ms: 1,
        }]));
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        let peer = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("peer-caret:Reader")).unwrap().rect;
        assert!((peer[0] - expected_x).abs() < 0.2);
        rt.wheel_event(WheelEvent::new(0.0, -1200.0).precise(true).at(500.0, 500.0));
        for frame in 0..180 {
            rt.update(|a, tx| { a.scrollers.step(frame as f64 / 60.0, tx); });
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.active_buffer().unwrap().scroll.get().0 > 250.0);
        rt.update(|a, _| {
            a.open_file("/notes/other.rs", None);
            a.apply_opened_file("/notes/other.rs", b"let code = 1;\n");
        });
        for _ in 0..120 { rt.frame(1.0 / 60.0); }
        assert_eq!(rt.state.active_buffer().unwrap().path, "/notes/other.rs");
        assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("md-page-cover")), "cover belongs to its Markdown document, not a global overlay");
    }

    #[test]
    fn optional_frontmatter_projects_keys_tags_and_unicode_without_touching_source() {
        let raw = "---\ntitle: 'hé 世界' # comment\nicon: 🦊\ncover: creation\ntags: [work, 'New York, USA', 世界]\naliases:\n  - 'other name'\nmetadata:\n  nested: keep raw\nnotes: |\n  multiline text\n---\n# Body";
        let lines: Vec<String> = raw.lines().map(str::to_owned).collect();
        let page = Page::new(&lines).unwrap();
        assert_eq!(page.property("title").unwrap().text, "hé 世界");
        assert_eq!(page.property("icon").unwrap().text, "🦊");
        assert_eq!(page.property("cover").unwrap().text, "creation");
        assert!(page.project(&lines[0], 0).text.is_empty());
        let tags = page.project(&lines[4], 4);
        assert_eq!(tags.marks.len(), 3);
        assert!(tags.text.contains("New York, USA"));
        assert!(page.project(&lines[6], 6).text.contains("other name"));
        for line in 0..page.range.end {
            let projection = page.project(&lines[line], line);
            assert_eq!(projection.source.len(), projection.text.len() + 1);
            assert!(projection.source.windows(2).all(|w| w[0] <= w[1]), "line={line}");
            for (byte, _) in projection.text.char_indices().chain(std::iter::once((projection.text.len(), '\0'))) {
                assert!(lines[line].is_char_boundary(projection.source[byte]));
            }
        }
        assert_eq!(page.project(&lines[8], 8).text, lines[8]);
        assert_eq!(page.project(&lines[9], 9).text, lines[9]);
        assert!(Page::new(&["# No metadata".into()]).is_none());
        assert!(Page::new(&["---".into(), "title: unfinished".into()]).is_none());
        let minimal = Page::new(&["---".into(), "tags: [work]".into(), "---".into()]).unwrap();
        assert_eq!(minimal.height(&App::new(), 600.0), 0.0, "no icon, title or cover means no invented header");
        assert_eq!(lines.join("\n"), raw);
    }
}