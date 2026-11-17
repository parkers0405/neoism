//! Markdown-only edits over the editor's existing document and undo history.

use crate::editor::{Buffer, Document, Mode, input::Key};
use std::ops::Range;

fn escaped(line: &str, at: usize) -> bool {
    line.as_bytes()[..at].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

/// Structural pipes only: escaped pipes and pipes inside matched code spans
/// belong to cell content. An unmatched backtick is ordinary source text.
fn pipes(line: &str) -> Vec<usize> {
    let bytes = line.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' && !escaped(line, i) {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            let mut p = i + run;
            let mut close = None;
            while p < bytes.len() {
                if bytes[p] == b'`' {
                    let n = bytes[p..].iter().take_while(|&&b| b == b'`').count();
                    if n == run { close = Some(p + n); break; }
                    p += n;
                } else { p += 1; }
            }
            if let Some(end) = close { i = end; continue; }
            i += run;
            continue;
        }
        if bytes[i] == b'|' && !escaped(line, i) { result.push(i); }
        i += 1;
    }
    result
}

/// Raw, UTF-8 byte ranges of cell content, without surrounding padding or
/// optional outer pipes. Empty cells have an empty range at an insertion stop.
/// A line without structural pipes is not a table row and returns no cells.
pub(super) fn cells(line: &str) -> Vec<Range<usize>> {
    let pipes = pipes(line);
    if pipes.is_empty() { return Vec::new(); }
    let left = line.len() - line.trim_start().len();
    let right = line.trim_end().len();
    let outer_left = pipes.first() == Some(&left);
    let outer_right = pipes.last().is_some_and(|&p| p + 1 == right);
    let mut from = if outer_left { left + 1 } else { 0 };
    let mut ranges = Vec::new();
    for &pipe in pipes.iter().skip(usize::from(outer_left)) {
        ranges.push(trim_range(line, from..pipe));
        from = pipe + 1;
    }
    if !outer_right { ranges.push(trim_range(line, from..line.len())); }
    ranges
}

fn trim_range(line: &str, range: Range<usize>) -> Range<usize> {
    let text = &line[range.clone()];
    let start = range.start + text.len() - text.trim_start().len();
    let end = range.start + text.trim_end().len();
    start..end.max(start)
}

pub(super) fn separator(line: &str) -> bool {
    let ranges = cells(line);
    !ranges.is_empty() && ranges.iter().all(|r| {
        let text = line[r.clone()].strip_prefix(':').unwrap_or(&line[r.clone()]);
        let text = text.strip_suffix(':').unwrap_or(text);
        text.len() >= 3 && text.bytes().all(|b| b == b'-')
    })
}

fn fence(line: &str) -> Option<(u8, usize, &str)> {
    let text = line.trim_start();
    let marker = *text.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') { return None; }
    let n = text.bytes().take_while(|&b| b == marker).count();
    (n >= 3).then_some((marker, n, text[n..].trim()))
}

/// True for lines where Markdown editing syntax must remain literal.
fn literal_lines(lines: &[String]) -> Vec<bool> {
    let mut result = Vec::with_capacity(lines.len());
    let mut code = None;
    let mut yaml = lines.first().is_some_and(|s| s.trim() == "---");
    for (i, line) in lines.iter().enumerate() {
        if yaml {
            result.push(true);
            if i > 0 && matches!(line.trim(), "---" | "...") { yaml = false; }
            continue;
        }
        if let Some((marker, count)) = code {
            result.push(true);
            if fence(line).is_some_and(|(m, n, rest)| m == marker && n >= count && rest.is_empty()) { code = None; }
        } else if let Some((marker, count, _)) = fence(line) {
            result.push(true);
            code = Some((marker, count));
        } else {
            result.push(line.starts_with("    ") || line.starts_with('\t'));
        }
    }
    result
}

/// All actual header + delimiter tables, in source order and nonoverlapping.
/// Compute literal context once, then scan forward without revisiting tables.
/// Layout can cache these exclusive-end ranges per document revision rather
/// than rediscovering the whole document for each rendered source line.
pub(super) fn tables(lines: &[String]) -> Vec<Range<usize>> {
    let literal = literal_lines(lines);
    let mut result = Vec::new();
    let mut start = 0;
    while start + 1 < lines.len() {
        let header = cells(&lines[start]);
        if literal[start] || literal[start + 1] || header.is_empty()
            || !separator(&lines[start + 1]) || header.len() != cells(&lines[start + 1]).len()
        { start += 1; continue; }
        let mut end = start + 2;
        while end < lines.len() && !literal[end] && !cells(&lines[end]).is_empty()
            && !separator(&lines[end])
        { end += 1; }
        result.push(start..end);
        start = end;
    }
    result
}

/// Recognize an actual header + delimiter table, excluding fenced code and
/// frontmatter. Returns an exclusive-end source-line range including delimiter.
pub(super) fn table_lines(lines: &[String], line: usize) -> Option<Range<usize>> {
    if line >= lines.len() { return None; }
    tables(lines).into_iter().find(|range| range.contains(&line))
}

fn begin(doc: &mut Document) {
    // Flush pending insert typing before taking the structural edit snapshot.
    doc.end_insert_group();
    doc.break_undo_group();
    doc.save_undo();
    doc.clear_vertical_goal();
}

fn finish(doc: &mut Document) {
    doc.clamp_cursor();
    doc.mark_edited();
    doc.commit_undo();
}

struct Prefix {
    indent: usize,
    body: usize,
    next: String,
    task: Option<usize>,
}

fn prefix(line: &str) -> Option<Prefix> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let text = &line[indent..];
    let bytes = text.as_bytes();
    let mut end;
    let mut next;
    if bytes.first() == Some(&b'>') {
        end = 0;
        // Retain nested blockquote prefixes, including their spacing.
        while bytes.get(end) == Some(&b'>') {
            end += 1;
            while bytes.get(end).is_some_and(u8::is_ascii_whitespace) { end += 1; }
        }
        next = text[..end].to_owned();
        if !next.ends_with(' ') { next.push(' '); }
    } else {
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if matches!(bytes.first(), Some(b'-' | b'+' | b'*')) {
            end = 1;
            next = text[..1].to_owned();
        } else if digits > 0 && matches!(bytes.get(digits), Some(b'.' | b')')) {
            end = digits + 1;
            let number = text[..digits].parse::<u64>().ok()?.checked_add(1)?;
            next = format!("{number}{}", bytes[digits] as char);
        } else { return None; }
        if !bytes.get(end).is_some_and(u8::is_ascii_whitespace) { return None; }
        let spacing = end;
        while bytes.get(end).is_some_and(u8::is_ascii_whitespace) { end += 1; }
        next.push_str(&text[spacing..end]);
    }
    let mut task = None;
    let shorthand = bytes.get(end..end + 2) == Some(b"[]")
        && (end + 2 == bytes.len() || bytes[end + 2].is_ascii_whitespace());
    if !text.starts_with('>') && (shorthand || (bytes.get(end) == Some(&b'[')
        && matches!(bytes.get(end + 1), Some(b' ' | b'x' | b'X'))
        && bytes.get(end + 2) == Some(&b']')
        && (end + 3 == bytes.len() || bytes[end + 3].is_ascii_whitespace())))
    {
        task = Some(indent + end + 1);
        end += if shorthand { 2 } else { 3 };
        let spacing = end;
        while bytes.get(end).is_some_and(u8::is_ascii_whitespace) { end += 1; }
        next.push_str("[ ]");
        if spacing == end { next.push(' '); } else { next.push_str(&text[spacing..end]); }
    }
    Some(Prefix { indent, body: indent + end, next: format!("{}{next}", &line[..indent]), task })
}

pub(super) fn toggle_task(buffer: &mut Buffer, line: usize) -> bool {
    let doc = &mut buffer.doc;
    let Some(raw) = doc.lines.get(line) else { return false; };
    let Some(marker) = prefix(raw).and_then(|p| p.task) else { return false; };
    // Indented nested lists are still tasks, unlike fenced code/frontmatter.
    let projected = super::parse::project(&doc.lines, usize::MAX..=usize::MAX);
    if !matches!(projected[line].kind, super::parse::Kind::Task { .. }) { return false; }
    let empty = raw.as_bytes()[marker] == b']';
    let replacement = if empty || raw.as_bytes()[marker] == b' ' { "x" } else { " " };
    begin(doc);
    doc.lines[line].replace_range(marker..marker + usize::from(!empty), replacement);
    finish(doc);
    true
}

fn insert_row(doc: &mut Document, table: Range<usize>) {
    let count = cells(&doc.lines[table.start]).len();
    let indent_len = doc.lines[table.start].len() - doc.lines[table.start].trim_start().len();
    let mut row = doc.lines[table.start][..indent_len].to_owned();
    row.push('|');
    for _ in 0..count { row.push_str("   |"); }
    let at = if doc.cursor_line <= table.start + 1 { table.start + 2 } else { doc.cursor_line + 1 };
    begin(doc);
    doc.lines.insert(at, row);
    doc.cursor_line = at;
    doc.cursor_col = cells(&doc.lines[at])[0].start;
    doc.mode = Mode::Insert;
    doc.visual_anchor = None;
    doc.vim.clear_pending();
    finish(doc);
}

fn tab(doc: &mut Document, table: Range<usize>, backwards: bool) {
    let row = doc.cursor_line;
    let bounds = cells(&doc.lines[row]);
    let cell = bounds.iter().position(|r| doc.cursor_col <= r.end).unwrap_or(bounds.len() - 1);
    let target = if row == table.start + 1 {
        Some((if backwards { table.start } else if row + 1 < table.end { row + 1 } else { table.start }, 0))
    } else if backwards {
        if cell > 0 { Some((row, cell - 1)) }
        else if row > table.start {
            let prev = if row == table.start + 2 { table.start } else { row - 1 };
            Some((prev, cells(&doc.lines[prev]).len() - 1))
        } else { None }
    } else if cell + 1 < bounds.len() { Some((row, cell + 1)) }
    else {
        let next = if row == table.start { row + 2 } else { row + 1 };
        (next < table.end).then_some((next, 0))
    };
    if let Some((line, cell)) = target {
        doc.end_insert_group();
        doc.break_undo_group();
        doc.clear_vertical_goal();
        doc.cursor_line = line;
        doc.cursor_col = cells(&doc.lines[line])[cell].start;
        doc.follow_cursor = true;
        doc.vim.clear_pending();
    } else if !backwards { insert_row(doc, table); }
}

/// Called only for Markdown buffers, before `Document::key`. Prompts and
/// pending Vim operators keep the normal dispatch path. Link opening belongs
/// to the host and takes precedence over the Normal Enter checkbox action.
pub(super) fn key(buffer: &mut Buffer, key: Key, shift: bool) -> bool {
    // Ordinary typing must not scan the document or compute literal context.
    if !matches!(key, Key::Enter | Key::Tab | Key::Char('|')) { return false; }
    if buffer.prompt.is_some() || buffer.doc.mode == Mode::Visual
        || (buffer.doc.mode == Mode::Normal && !buffer.doc.vim.pending.is_empty())
    { return false; }
    let row = buffer.doc.cursor_line;
    if row >= buffer.doc.lines.len() { return false; }
    if let Some(table) = table_lines(&buffer.doc.lines, row) {
        match key {
            Key::Tab => { tab(&mut buffer.doc, table, shift); return true; }
            Key::Enter if buffer.doc.mode == Mode::Insert || shift => {
                insert_row(&mut buffer.doc, table);
                return true;
            }
            Key::Char('|') if buffer.doc.mode == Mode::Insert && row != table.start + 1 => {
                let doc = &mut buffer.doc;
                doc.clamp_cursor();
                let text = if escaped(&doc.lines[row], doc.cursor_col) { "|" } else { "\\|" };
                doc.type_text(text);
                return true;
            }
            _ => {}
        }
    }
    if key != Key::Enter { return false; }
    if buffer.doc.mode == Mode::Normal {
        if shift { return false; }
        let projected = super::parse::project(&buffer.doc.lines, usize::MAX..=usize::MAX);
        let line = &projected[row];
        if line.marks.iter().any(|mark| mark.link
            && buffer.doc.cursor_col >= line.source[mark.start]
            && buffer.doc.cursor_col < line.source[mark.end])
        { return false; }
        return toggle_task(buffer, row);
    }
    let doc = &mut buffer.doc;
    let Some(prefix) = prefix(&doc.lines[row]) else { return false; };
    // The projection knows fenced/frontmatter context without treating nested
    // list indentation as an indented code block.
    let projected = super::parse::project(&doc.lines, usize::MAX..=usize::MAX);
    if !matches!(projected[row].kind, super::parse::Kind::List | super::parse::Kind::Task { .. } | super::parse::Kind::Quote)
        || doc.cursor_col < prefix.body
    { return false; }
    begin(doc);
    doc.clamp_cursor();
    if doc.lines[row][prefix.body..].trim().is_empty() {
        doc.lines[row].truncate(prefix.indent);
        doc.cursor_col = prefix.indent;
    } else {
        doc.split_line_at_cursor(false);
        doc.lines[doc.cursor_line].insert_str(0, &prefix.next);
        doc.cursor_col = prefix.next.len();
    }
    finish(doc);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str, mode: Mode) -> Buffer {
        let mut buffer = Buffer::new("test.md".into(), String::new(), None);
        buffer.doc = Document::from_text(text);
        buffer.doc.mode = mode;
        buffer.doc.cursor_col = buffer.doc.lines[0].len();
        buffer
    }

    #[test]
    fn raw_cell_ranges_preserve_unicode_escapes_and_code() {
        let raw = r" | 世 | a\|b | `x|y` | ``a`|b`` |   | ";
        let ranges = cells(raw);
        assert_eq!(ranges.iter().map(|r| &raw[r.clone()]).collect::<Vec<_>>(), ["世", r"a\|b", "`x|y`", "``a`|b``", ""]);
        assert!(ranges.iter().all(|r| raw.is_char_boundary(r.start) && raw.is_char_boundary(r.end)));
        assert_eq!(cells("a | b").len(), 2);
        assert!(cells("ordinary prose").is_empty());
        assert_eq!(cells("| `unclosed | b |").len(), 2);
    }

    #[test]
    fn valid_tables_only_outside_literal_blocks() {
        assert!(separator("| :--- | ---: | :---: |"));
        for invalid in ["| -- | --- |", "| ---x | --- |", "---", "| : : |"] { assert!(!separator(invalid)); }
        let lines: Vec<String> = ["prose | pipes", "", "a | b", "--- | :---:", "世 | c", "", "```md", "x | y", "--- | ---", "```"].into_iter().map(str::to_owned).collect();
        assert_eq!(table_lines(&lines, 0), None);
        assert_eq!(table_lines(&lines, 2), Some(2..5));
        assert_eq!(table_lines(&lines, 3), Some(2..5));
        assert_eq!(table_lines(&lines, 4), Some(2..5));
        assert_eq!(table_lines(&lines, 7), None);
        assert_eq!(tables(&lines), vec![2..5]);
    }

    #[test]
    fn all_tables_are_returned_once_in_source_order() {
        let lines: Vec<String> = [
            "---", "title: | yaml", "---", "",
            "| a | b |", "| --- | --- |", "| c | d |", "",
            "~~~md", "x | y", "--- | ---", "~~~", "",
            "x | y", ":--- | ---:", "", "prose | only",
        ].into_iter().map(str::to_owned).collect();
        assert_eq!(tables(&lines), vec![4..7, 13..15]);
        assert!(tables(&[]).is_empty());
        for row in 0..lines.len() {
            let expected = tables(&lines).into_iter().find(|r| r.contains(&row));
            assert_eq!(table_lines(&lines, row), expected);
        }
    }

    #[test]
    fn continuation_splits_source_and_is_one_undo_step() {
        for (raw, next) in [("- [X] 世界", "- [ ] "), ("  9) 世界", "  10) "), ("> 世界", "> "), ("* 世界", "* ")] {
            let mut b = buffer(raw, Mode::Insert);
            b.doc.cursor_col = raw.find('界').unwrap();
            assert!(key(&mut b, Key::Enter, false));
            assert_eq!(b.doc.lines[1], format!("{next}界"));
            assert_eq!(b.doc.cursor_col, next.len());
            assert!(b.doc.is_dirty());
            assert!(b.doc.undo());
            assert_eq!(b.doc.lines, [raw]);
            assert!(!b.doc.is_dirty());
            assert!(b.doc.redo());
            assert_eq!(b.doc.lines.len(), 2);
        }
        let mut b = buffer("  - [ ] ", Mode::Insert);
        assert!(key(&mut b, Key::Enter, false));
        assert_eq!(b.doc.lines, ["  "]);
        assert!(b.doc.undo());
        assert_eq!(b.doc.lines, ["  - [ ] "]);
    }

    #[test]
    fn task_toggle_keeps_source_cursor_and_saved_baseline() {
        let raw = "- [ ] **世界**";
        let mut b = buffer(raw, Mode::Normal);
        let col = b.doc.cursor_col;
        assert!(toggle_task(&mut b, 0));
        assert_eq!(b.doc.lines, ["- [x] **世界**"]);
        assert_eq!(b.doc.cursor_col, col);
        assert_eq!(b.doc.saved_text_for_disk(), raw);
        assert!(b.doc.undo());
        assert_eq!(b.doc.text_for_disk(), raw);
        let mut code = buffer("```\n- [ ] literal\n```", Mode::Normal);
        assert!(!toggle_task(&mut code, 1));
        let mut link = buffer("- [ ] [label](target)", Mode::Normal);
        link.doc.cursor_col = 8;
        assert!(!key(&mut link, Key::Enter, false));
        assert!(!link.doc.is_dirty());
    }

    #[test]
    fn table_navigation_skips_delimiter_and_append_is_undoable() {
        let raw = "| 世 | b |\n| --- | --- |\n| c | d |";
        let mut b = buffer(raw, Mode::Normal);
        b.doc.cursor_col = cells(&b.doc.lines[0])[1].start;
        assert!(key(&mut b, Key::Tab, false));
        assert_eq!(b.doc.cursor_line, 2);
        assert!(!b.doc.is_dirty());
        assert!(key(&mut b, Key::Tab, true));
        assert_eq!(b.doc.cursor_line, 0);
        assert_eq!(b.doc.cursor_col, cells(&b.doc.lines[0])[1].start);
        b.doc.cursor_line = 2;
        b.doc.cursor_col = cells(&b.doc.lines[2])[1].start;
        assert!(key(&mut b, Key::Tab, false));
        assert_eq!(b.doc.lines.len(), 4);
        assert_eq!(b.doc.mode, Mode::Insert);
        assert_eq!(b.doc.cursor_line, 3);
        assert!(b.doc.undo());
        assert_eq!(b.doc.text_for_disk(), raw);
        assert!(!b.doc.is_dirty());
        b.doc.cursor_line = 0;
        assert!(key(&mut b, Key::Enter, false));
        assert_eq!(b.doc.cursor_line, 2);
        assert_eq!(&b.doc.lines[..2], &["| 世 | b |", "| --- | --- |"]);
    }

    #[test]
    fn prompts_visual_and_plain_source_fall_through() {
        let mut b = buffer("- item", Mode::Insert);
        b.prompt = Some(crate::editor::input::Prompt { kind: '/', text: String::new() });
        assert!(!key(&mut b, Key::Enter, false));
        b.prompt = None;
        b.doc.mode = Mode::Visual;
        assert!(!key(&mut b, Key::Enter, false));
        let mut plain = buffer("ordinary prose", Mode::Insert);
        assert!(!key(&mut plain, Key::Enter, false));
        assert!(!plain.doc.is_dirty());
    }

    #[test]
    fn literal_pipe_typing_and_structural_undo_do_not_merge() {
        let raw = "| a | b |\n| --- | --- |";
        let mut b = buffer(raw, Mode::Insert);
        b.doc.cursor_col = cells(&b.doc.lines[0])[0].end;
        assert!(key(&mut b, Key::Char('|'), false));
        assert_eq!(b.doc.lines[0], r"| a\| | b |");
        assert_eq!(cells(&b.doc.lines[0]).len(), 2);
        assert!(key(&mut b, Key::Enter, false));
        assert_eq!(b.doc.lines.len(), 3);
        assert!(b.doc.undo());
        assert_eq!(b.doc.lines.len(), 2);
        assert_eq!(b.doc.lines[0], r"| a\| | b |");
        assert!(b.doc.undo());
        assert_eq!(b.doc.text_for_disk(), raw);

        let mut list = buffer("- item", Mode::Insert);
        list.doc.type_text("世");
        assert!(key(&mut list, Key::Enter, false));
        assert!(list.doc.undo());
        assert_eq!(list.doc.lines, ["- item世"]);
        assert!(list.doc.undo());
        assert_eq!(list.doc.lines, ["- item"]);
    }
}