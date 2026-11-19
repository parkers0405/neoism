//! Source-coordinate Markdown links. Following happens on input, never during layout.

use crate::{app::App, editor::Mode};
use pulldown_cmark::{Event, Parser, Tag};
use std::{collections::HashMap, ops::Range, path::{Component, Path, PathBuf}};
use unicode_segmentation::UnicodeSegmentation;

/// Only painted link glyphs are clickable: hit-testing the nearest caret alone
/// would also activate links from the empty margin after a line or table cell.
pub(in crate::editor) fn pointer_target(app: &App, point: [f32; 2]) -> Option<crate::editor::vim::Pos> {
    let buffer = app.active_buffer()?;
    if !super::enabled(buffer) || !buffer.loaded || buffer.prompt.is_some() || buffer.doc.mode != Mode::Normal { return None; }
    let rect = buffer.view_rect?;
    if point[0] < rect[0] || point[0] >= rect[0] + rect[2] || point[1] < rect[1] || point[1] >= rect[1] + rect[3] { return None; }
    let layout = super::layout(app, buffer);
    let y = point[1] - rect[1] + buffer.scroll.get().0;
    let entry = layout.rows.iter().find(|r| y >= r.y && y < r.y + r.row.height)?;
    if matches!(super::parsed(buffer)[entry.line].kind,
        super::parse::Kind::Code { .. } | super::parse::Kind::Fence | super::parse::Kind::Frontmatter) { return None; }
    let x = point[0] - rect[0] - layout.left + layout.table(entry.line).map_or(0.0, super::table::Table::offset);
    let raw = &buffer.doc.lines[entry.line];
    for piece in &entry.row.pieces {
        for (byte, glyph) in piece.text.grapheme_indices(true) {
            let col = piece.source[byte];
            if piece.geometry.range_rects(byte..byte + glyph.len()).iter().any(|r|
                x >= piece.x + r[0] && x < piece.x + r[0] + r[2])
                && link_at(raw, col).is_some() {
                return Some(crate::editor::vim::Pos { line: entry.line, col });
            }
        }
    }
    None
}

pub(in crate::editor) fn click(app: &mut App, pos: crate::editor::vim::Pos) -> bool {
    follow_at(app, pos.line, pos.col)
}

struct Link {
    target: String,
    wiki: bool,
}

fn escaped(raw: &str, at: usize) -> bool {
    raw.as_bytes()[..at].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

fn unescape(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
            out.push(chars.next().unwrap());
        } else { out.push(c); }
    }
    out
}

fn link_at(raw: &str, col: usize) -> Option<Link> {
    if col >= raw.len() || !raw.is_char_boundary(col) { return None; }
    let mut code: Vec<Range<usize>> = Vec::new();
    let mut standard = Vec::new();
    for (event, range) in Parser::new(raw).into_offset_iter() {
        match event {
            Event::Code(_) | Event::Start(Tag::CodeBlock(_)) => code.push(range),
            Event::Start(Tag::Link { dest_url, .. }) => {
                standard.push((range, dest_url.into_string()));
            }
            _ => {}
        }
    }
    if code.iter().any(|range| range.contains(&col)) { return None; }
    // Wiki syntax is not CommonMark: scan it separately, using the parser's
    // code-span ranges so backticks (including multi-backticks) remain literal.
    let mut from = 0;
    while let Some(relative) = raw[from..].find("[[") {
        let start = from + relative;
        from = start + 2;
        if escaped(raw, start) || code.iter().any(|range| range.contains(&start)) { continue; }
        let Some(close) = raw[from..].match_indices("]]").find_map(|(p, _)| {
            (!escaped(raw, from + p)).then_some(from + p)
        }) else { continue; };
        let body = &raw[from..close];
        if body.contains("[[") { continue; }
        if (start..close + 2).contains(&col) {
            let end = body.match_indices('|').find_map(|(p, _)| {
                (!escaped(raw, from + p)).then_some(p)
            }).unwrap_or(body.len());
            let target = unescape(body[..end].trim());
            return (!target.is_empty()).then_some(Link { target, wiki: true });
        }
        from = close + 2;
    }
    standard.into_iter().find_map(|(range, target)| {
        (range.contains(&col) && !target.is_empty()).then_some(Link { target, wiki: false })
    })
}

/// `col` is a raw UTF-8 byte offset, not a rendered character index. Includes
/// delimiters and destinations; excludes the byte boundary after a link.
pub(super) fn at(raw: &str, col: usize) -> Option<String> {
    link_at(raw, col).map(|link| link.target)
}

pub(crate) fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(a), Some(b)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_owned())
}

/// Lexical normalization only: no filesystem reads or canonicalization.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if out.file_name().is_some_and(|name| name != "..") { out.pop(); }
                else if !out.has_root() { out.push(".."); }
            }
            _ => out.push(part.as_os_str()),
        }
    }
    out
}

fn destination(source: &str, scope: &str, target: &str, wiki: bool) -> (String, Option<String>) {
    let (file, anchor) = target.split_once('#').map_or((target, None), |(f, a)| (f, Some(decode(a))));
    let source = if Path::new(source).is_absolute() { PathBuf::from(source) } else { Path::new(scope).join(source) };
    let file = decode(file.strip_prefix("file://").unwrap_or(file));
    if file.is_empty() { return (normalize(&source).to_string_lossy().into_owned(), anchor); }
    let mut file = PathBuf::from(file);
    if wiki && file.extension().is_none() { file.set_extension("md"); }
    let path = if file.is_absolute() { file } else { source.parent().unwrap_or(Path::new(scope)).join(file) };
    (normalize(&path).to_string_lossy().into_owned(), anchor)
}

fn slug(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).filter_map(|c| {
        if c.is_alphanumeric() || matches!(c, '-' | '_') { Some(c) }
        else if c.is_whitespace() { Some('-') } else { None }
    }).collect()
}

fn heading(lines: &[String], anchor: &str) -> Option<usize> {
    let wanted = slug(anchor.trim_start_matches('#'));
    let projected = super::parse::project(lines, usize::MAX..=usize::MAX);
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (row, line) in projected.iter().enumerate() {
        if !matches!(line.kind, super::parse::Kind::Heading(_)) { continue; }
        // Optional closing ATX hashes are not part of the heading identifier.
        let text = line.text.trim();
        let text = if let Some(without) = text.trim_end_matches('#').strip_suffix(char::is_whitespace) { without.trim_end() } else { text };
        let base = slug(text);
        let count = seen.entry(base.clone()).or_default();
        let id = if *count == 0 { base.clone() } else { format!("{base}-{count}") };
        *count += 1;
        if id == wanted { return Some(row); }
    }
    None
}

pub(in crate::editor) fn jump(buffer: &mut crate::editor::Buffer, anchor: &str) -> bool {
    if !super::enabled(buffer) { return false; }
    let Some(row) = heading(&buffer.doc.lines, anchor) else { return false; };
    buffer.doc.end_insert_group();
    buffer.doc.break_undo_group();
    buffer.doc.clear_vertical_goal();
    buffer.doc.cursor_line = row;
    buffer.doc.cursor_col = 0;
    buffer.doc.follow_cursor = true;
    buffer.doc.vim.clear_pending();
    buffer.markdown_cache.clear_vertical_goal();
    // A deliberate section jump takes priority over restoring this tab's old scroll.
    buffer.kept_scroll = 0.0;
    buffer.restoring = 0;
    buffer.wheel_target = None;
    buffer.jump_spring = None;
    true
}

/// Normal Enter: consume a link before the host attempts a task toggle. Returns
/// false for nonlinks, prompts, code/frontmatter, or a pending Vim operator.
pub(super) fn follow(app: &mut App) -> bool {
    let Some(buffer) = app.active_buffer() else { return false; };
    follow_at(app, buffer.doc.cursor_line, buffer.doc.cursor_col)
}

fn follow_at(app: &mut App, line: usize, col: usize) -> bool {
    let Some(buffer) = app.active_buffer() else { return false; };
    if !super::enabled(buffer) || !buffer.loaded || buffer.prompt.is_some()
        || buffer.doc.mode != Mode::Normal || !buffer.doc.vim.pending.is_empty()
    { return false; }
    let Some(raw) = buffer.doc.lines.get(line) else { return false; };
    let Some(link) = link_at(raw, col) else { return false; };
    let projected = super::parse::project(&buffer.doc.lines, usize::MAX..=usize::MAX);
    if matches!(projected[line].kind,
        super::parse::Kind::Code { .. } | super::parse::Kind::Fence | super::parse::Kind::Frontmatter)
    { return false; }
    if link.target.starts_with("http://") || link.target.starts_with("https://") || link.target.starts_with("mailto:") {
        let target = link.target;
        app.open_link(&target);
        return true;
    }
    let (path, anchor) = destination(&buffer.path, &buffer.scope, &link.target, link.wiki);
    app.open_file_anchor(&path, anchor.as_deref());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(text: &str) -> rune::Runtime<App> {
        let mut app = App::new();
        app.open_file("/notes/click-links.md", None);
        app.apply_opened_file("/notes/click-links.md", text.as_bytes());
        let mut rt = rune::Runtime::new(app, crate::ui::chrome::root, (900.0, 600.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        rt
    }

    fn glyph_point(app: &App, line: usize, needle: &str) -> [f32; 2] {
        let buffer = app.active_buffer().unwrap();
        let col = buffer.doc.lines[line].find(needle).unwrap();
        let layout = super::super::layout(app, buffer);
        let rect = buffer.view_rect.unwrap();
        for entry in layout.rows.iter().filter(|e| e.line == line) {
            for piece in &entry.row.pieces {
                if let Some((byte, glyph)) = piece.text.grapheme_indices(true).find(|(b, _)| piece.source[*b] == col) {
                    let r = piece.geometry.range_rects(byte..byte + glyph.len())[0];
                    return [rect[0] + layout.left + piece.x + r[0] + r[2] * 0.5
                        - layout.table(line).map_or(0.0, super::super::table::Table::offset),
                        rect[1] + entry.y + entry.row.height * 0.5 - buffer.scroll.get().0];
                }
            }
        }
        panic!("missing glyph {needle}");
    }

    #[test]
    fn native_link_click_follows_on_release_in_prose_and_table_cells() {
        for (text, line) in [
            ("A [世 **界**](#destination) link\n\n# Destination\n", 0),
            ("| Label | Other |\n| --- | --- |\n| [世 **界**](#destination) | text |\n\n# Destination\n", 2),
        ] {
            let mut rt = runtime(text);
            let point = glyph_point(&rt.state, line, "界");
            assert!(pointer_target(&rt.state, point).is_some());
            rt.pointer_down(point[0], point[1]);
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, line);
            rt.pointer_up(point[0], point[1]);
            rt.frame(1.0 / 60.0);
            let buffer = rt.state.active_buffer().unwrap();
            assert_eq!(buffer.doc.cursor_line, buffer.doc.lines.iter().position(|l| l == "# Destination").unwrap());
            assert_eq!(buffer.doc.mode, Mode::Normal);
            assert_eq!(buffer.doc.text_for_disk(), text);
            assert!(!buffer.dirty());
        }
    }

    #[test]
    fn native_click_opens_relative_files_and_wiki_links_without_changing_source() {
        for text in ["[label](sub/target.md)\n", "[[sub/target|label]]\n"] {
            let mut rt = runtime(text);
            rt.update(|app, _| {
                app.open_file("/notes/sub/target.md", None);
                app.apply_opened_file("/notes/sub/target.md", b"# Target\n");
                app.open_file("/notes/click-links.md", None);
            });
            for _ in 0..4 { rt.frame(1.0 / 60.0); }
            let point = glyph_point(&rt.state, 0, "label");
            rt.pointer_down(point[0], point[1]);
            rt.frame(1.0 / 60.0);
            rt.pointer_up(point[0], point[1]);
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.state.active_buffer().unwrap().path, "/notes/sub/target.md");
            let source = &rt.state.buffers["/notes/click-links.md"];
            assert_eq!(source.doc.text_for_disk(), text);
            assert!(!source.dirty());
        }
    }

    #[test]
    fn selecting_cancelling_and_editing_links_do_not_follow_them() {
        let text = "A [label](#destination) other text\n\n# Destination\n";
        for action in ["select", "cancel", "insert"] {
            let mut rt = runtime(text);
            if action == "insert" {
                rt.key(rune::KeyEvent { key: rune::Key::Text("i".into()), mods: Default::default() });
                rt.frame(1.0 / 60.0);
            }
            let point = glyph_point(&rt.state, 0, "label");
            rt.pointer_down(point[0], point[1]);
            rt.frame(1.0 / 60.0);
            let end = if action == "select" { glyph_point(&rt.state, 0, "other") } else { point };
            if action == "select" { rt.pointer_move(end[0], end[1]); }
            if action == "cancel" { rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() }); }
            rt.frame(1.0 / 60.0);
            rt.pointer_up(end[0], end[1]);
            rt.frame(1.0 / 60.0);
            let buffer = rt.state.active_buffer().unwrap();
            assert_eq!(buffer.doc.cursor_line, 0, "{action}");
            assert_eq!(buffer.doc.text_for_disk(), text);
            assert!(!buffer.dirty());
            if action == "select" { assert_eq!(buffer.doc.mode, Mode::Visual); }
            if action == "insert" { assert_eq!(buffer.doc.mode, Mode::Insert); }
        }
        for text in ["`[label](#destination)`\n", "```md\n[label](#destination)\n```\n"] {
            let rt = runtime(text);
            let line = usize::from(text.starts_with("```"));
            assert_eq!(pointer_target(&rt.state, glyph_point(&rt.state, line, "label")), None);
        }
        let rt = runtime("[label](#destination)\n");
        let point = glyph_point(&rt.state, 0, "label");
        assert_eq!(pointer_target(&rt.state, [point[0] + 300.0, point[1]]), None);
    }

    #[test]
    fn links_use_raw_unicode_byte_positions() {
        let raw = "世 [é **界**](../目标.md#hé) [[页面|别名]]";
        for needle in ["é", "界", "../", "目标"] {
            assert_eq!(at(raw, raw.find(needle).unwrap()).as_deref(), Some("../目标.md#hé"));
        }
        assert_eq!(at(raw, raw.find("别名").unwrap()).as_deref(), Some("页面"));
        assert_eq!(at(raw, 1), None);
        assert_eq!(at(raw, raw.len()), None);
        let projected = super::super::parse::inline_line(raw);
        let display = projected.text.find('界').unwrap();
        assert_eq!(at(raw, projected.source[display]).as_deref(), Some("../目标.md#hé"));
    }

    #[test]
    fn escaped_brackets_and_code_do_not_create_links() {
        for raw in [r"\[no](x)", r"\[[page]]", "`[no](x)`", "``[[no|alias]]``", "```\n[x](y)\n```", "![image](x)"] {
            for (col, _) in raw.char_indices() { assert_eq!(at(raw, col), None, "{raw} at {col}"); }
        }
        let raw = r"[label \] 世](a\(b\).md)";
        assert_eq!(at(raw, raw.find('世').unwrap()).as_deref(), Some("a(b).md"));
        let raw = r"[[a\|b|alias]]";
        assert_eq!(at(raw, raw.find("alias").unwrap()).as_deref(), Some("a|b"));
    }

    #[test]
    fn destinations_are_relative_to_source_parent() {
        assert_eq!(destination("notes/sub/source.md", "/workspace", "../other%20note.md#hé", false), ("/workspace/notes/other note.md".into(), Some("hé".into())));
        assert_eq!(destination("/vault/sub/source.md", "/workspace", "Page#Heading", true), ("/vault/sub/Page.md".into(), Some("Heading".into())));
        assert_eq!(destination("/vault/source.md", "/workspace", "#local", true), ("/vault/source.md".into(), Some("local".into())));
        assert_eq!(destination("/vault/source.md", "/workspace", "/other/file.rs", true).0, "/other/file.rs");
    }

    #[test]
    fn local_headings_use_rendered_text_unicode_and_duplicate_slugs() {
        let lines: Vec<String> = ["# **Hé 世界**", "```", "# ignored", "```", "## Repeat", "## Repeat", "## Closed ##"].into_iter().map(str::to_owned).collect();
        assert_eq!(heading(&lines, "hé-世界"), Some(0));
        assert_eq!(heading(&lines, "Repeat"), Some(4));
        assert_eq!(heading(&lines, "repeat-1"), Some(5));
        assert_eq!(heading(&lines, "closed"), Some(6));
        assert_eq!(heading(&lines, "ignored"), None);
    }

    #[test]
    fn section_survives_async_file_load_and_newer_navigation_wins() {
        let mut rt = runtime("[other](other.md#repeat-1)\n");
        let at = glyph_point(&rt.state, 0, "other");
        rt.pointer_down(at[0], at[1]); rt.pointer_up(at[0], at[1]);
        for _ in 0..4 { rt.frame(1.0 / 60.0); }
        let buffer = rt.state.active_buffer().unwrap();
        assert_eq!(buffer.path, "/notes/other.md");
        assert!(!buffer.loaded);
        let raw = "# Repeat\n\nfirst\n\n# Repeat\n\nsecond\n";
        rt.state.apply_opened_file("/notes/other.md", raw.as_bytes());
        assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, 4);
        assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), raw);
        assert!(!rt.state.active_buffer().unwrap().dirty());

        rt.state.open_file_anchor("/notes/pending.md", Some("repeat-1"));
        rt.state.open_file("/notes/pending.md", Some(3));
        rt.state.apply_opened_file("/notes/pending.md", raw.as_bytes());
        assert_eq!(rt.state.active_buffer().unwrap().doc.cursor_line, 2, "later line navigation cancels the pending section");

        rt.state.open_file_anchor("/notes/background.md", Some("repeat-1"));
        rt.state.open_file("/notes/other.md", None);
        rt.state.apply_opened_file("/notes/background.md", raw.as_bytes());
        assert_eq!(rt.state.active_buffer().unwrap().path, "/notes/other.md");
        assert_eq!(rt.state.buffers["/notes/background.md"].doc.cursor_line, 4);
    }
}