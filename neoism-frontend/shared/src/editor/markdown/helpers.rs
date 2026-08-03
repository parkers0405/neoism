use std::path::Path;

use super::types::*;

pub(super) fn source_len_from_lines(lines: &[String]) -> usize {
    if lines.is_empty() {
        return 0;
    }
    lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1)
}

pub(super) fn source_from_lines(lines: &[String]) -> String {
    let mut source = String::with_capacity(source_len_from_lines(lines));
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            source.push('\n');
        }
        source.push_str(line);
    }
    source
}

pub(super) fn point_in_rect(x: f32, y: f32, rect: [f32; 4]) -> bool {
    x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
}

pub(crate) fn is_notebook_output_marker_line(line: &str) -> bool {
    line.starts_with("%%neoism_notebook_output ")
}

pub(crate) const NOTEBOOK_MARKDOWN_CELL_MARKER: &str = "%%neoism_notebook_cell ";

pub(crate) fn notebook_markdown_cell_index(line: &str) -> Option<usize> {
    line.strip_prefix(NOTEBOOK_MARKDOWN_CELL_MARKER)?
        .split_whitespace()
        .next()?
        .parse::<usize>()
        .ok()
}

pub(crate) fn is_notebook_markdown_cell_marker_line(line: &str) -> bool {
    notebook_markdown_cell_index(line).is_some()
}

pub(crate) fn notebook_fenced_cell_index(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("```") {
        return None;
    }
    trimmed
        .split_whitespace()
        .find_map(|part| part.strip_prefix("neoism_notebook_cell="))
        .and_then(|value| value.parse::<usize>().ok())
}

pub(crate) fn is_notebook_cell_anchor_line(line: &str) -> bool {
    is_notebook_markdown_cell_marker_line(line)
        || notebook_fenced_cell_index(line).is_some()
}

pub(super) fn word_bounds_at(line: &str, col: usize) -> Option<(usize, usize)> {
    let col = floor_char_boundary(line, col.min(line.len()));
    let mut start = None;
    let mut last_end = 0usize;
    for (ix, ch) in line.char_indices() {
        let char_end = ix + ch.len_utf8();
        if ch.is_alphabetic() || ch == '\'' {
            start.get_or_insert(ix);
        } else if let Some(word_start) = start.take() {
            if col >= word_start && col <= ix {
                return Some((word_start, ix));
            }
        }
        last_end = char_end;
    }
    start.and_then(|word_start| {
        (col >= word_start && col <= last_end).then_some((word_start, last_end))
    })
}

pub(super) fn block_handle_rect(block_rect: [f32; 4]) -> [f32; 4] {
    [block_rect[0] - 33.0, block_rect[1] + 7.0, 24.0, 24.0]
}

pub(super) fn block_convert_rect(block_rect: [f32; 4]) -> [f32; 4] {
    let _ = block_rect;
    [-1_000_000.0, -1_000_000.0, 0.0, 0.0]
}

pub(super) fn markdown_scrollbar_hit(x: f32, y: f32, track_rect: [f32; 4]) -> bool {
    point_in_rect(
        x,
        y,
        [
            track_rect[0] - MARKDOWN_SCROLLBAR_HIT_PAD_X,
            track_rect[1],
            track_rect[2] + MARKDOWN_SCROLLBAR_HIT_PAD_X * 2.0,
            track_rect[3],
        ],
    )
}

pub fn parse_markdown_link_parts(inner: &str) -> Option<MarkdownParsedLink> {
    let rest = inner.trim();
    let (target_part, alias) = rest
        .split_once('|')
        .map(|(target, alias)| (target.trim(), Some(alias.trim().to_string())))
        .unwrap_or((rest, None));
    let (code_ref, target_part) = target_part
        .trim_start()
        .strip_prefix('@')
        .map(|target| (true, target.trim()))
        .unwrap_or((false, target_part.trim()));
    if target_part.is_empty() {
        return None;
    }
    let host_owned_scheme = target_part.contains("://");
    let (target_part, heading) = if host_owned_scheme {
        (target_part, None)
    } else {
        target_part
            .split_once('#')
            .map(|(target, heading)| {
                (
                    target.trim(),
                    (!heading.trim().is_empty()).then(|| heading.trim().to_string()),
                )
            })
            .unwrap_or((target_part, None))
    };
    if target_part.is_empty() && heading.is_none() {
        return None;
    }
    let (target, line) = if heading.is_none() && !host_owned_scheme {
        if let Some((target, line)) = target_part.rsplit_once('-') {
            if !target.trim().is_empty()
                && !line.is_empty()
                && line.chars().all(|ch| ch.is_ascii_digit())
            {
                (target.trim().to_string(), line.parse().ok())
            } else {
                (target_part.to_string(), None)
            }
        } else {
            (target_part.to_string(), None)
        }
    } else {
        (target_part.to_string(), None)
    };
    Some(MarkdownParsedLink {
        target,
        heading,
        line,
        alias,
        code_ref,
    })
}

pub fn parse_markdown_link_inner(inner: &str) -> Option<(String, Option<usize>)> {
    if !inner.trim_start().starts_with('@') {
        return None;
    }
    let parsed = parse_markdown_link_parts(inner)?;
    Some((parsed.target, parsed.line))
}

pub(super) fn heading_slug(text: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

pub(super) fn markdown_heading_line(source: &str, heading: &str) -> Option<usize> {
    let wanted_slug = heading_slug(heading);
    source.lines().enumerate().find_map(|(line_ix, line)| {
        let trimmed = line.trim_start();
        let level = trimmed.chars().take_while(|ch| *ch == '#').count();
        if !(1..=6).contains(&level)
            || !trimmed.chars().nth(level).is_some_and(|ch| ch == ' ')
        {
            return None;
        }
        let text = trimmed[level..].trim();
        (text == heading || heading_slug(text) == wanted_slug).then_some(line_ix + 1)
    })
}

pub fn parse_table_cell_bounds(line: &str) -> Option<Vec<MarkdownTableCellBounds>> {
    if !line.contains('|') {
        return None;
    }
    let pipe_indices = line
        .char_indices()
        .filter_map(|(ix, ch)| (ch == '|').then_some(ix))
        .collect::<Vec<_>>();
    if pipe_indices.is_empty() {
        return None;
    }

    let leading_pipe = line.trim_start().starts_with('|');
    let trailing_pipe = line.trim_end().ends_with('|');
    let mut raw_start = if leading_pipe { pipe_indices[0] + 1 } else { 0 };
    let mut bounds = Vec::new();
    let skip = usize::from(leading_pipe);
    for pipe in pipe_indices.into_iter().skip(skip) {
        if raw_start <= pipe {
            bounds.push(table_cell_bounds_from_raw(line, raw_start, pipe));
        }
        raw_start = pipe + 1;
    }
    if !trailing_pipe && raw_start <= line.len() {
        bounds.push(table_cell_bounds_from_raw(line, raw_start, line.len()));
    }
    (bounds.len() >= 2).then_some(bounds)
}

pub(super) fn table_cell_bounds_from_raw(
    line: &str,
    raw_start: usize,
    raw_end: usize,
) -> MarkdownTableCellBounds {
    let raw = &line[raw_start..raw_end];
    if raw.trim().is_empty() {
        let entry = (raw_start + raw.len().min(1)).min(raw_end);
        return MarkdownTableCellBounds {
            raw_start,
            raw_end,
            content_start: entry,
            content_end: entry,
        };
    }
    let content_start = raw_start + raw.len().saturating_sub(raw.trim_start().len());
    let content_end = if raw_end > raw_start + raw.trim_end().len() {
        prev_char_boundary(line, raw_end)
    } else {
        raw_end
    }
    .max(content_start)
    .min(raw_end);
    MarkdownTableCellBounds {
        raw_start,
        raw_end,
        content_start,
        content_end,
    }
}

pub(super) fn table_cell_visible_len(
    line: &str,
    bounds: MarkdownTableCellBounds,
) -> usize {
    line[bounds.content_start..bounds.content_end]
        .chars()
        .count()
}

pub(super) fn table_cell_entry_col(bounds: MarkdownTableCellBounds) -> usize {
    bounds.content_start
}

pub(super) fn nearest_table_cell_ix(
    bounds: &[MarkdownTableCellBounds],
    col: usize,
) -> usize {
    bounds
        .iter()
        .enumerate()
        .min_by_key(|(_, cell)| {
            if col < cell.raw_start {
                cell.raw_start - col
            } else {
                col.saturating_sub(cell.raw_end)
            }
        })
        .map(|(ix, _)| ix)
        .unwrap_or(0)
}

pub(super) fn empty_table_row(col_count: usize) -> String {
    let cells = std::iter::repeat(" ")
        .take(col_count.max(1))
        .map(|cell| format!(" {cell} "))
        .collect::<Vec<_>>()
        .join("|");
    format!("|{cells}|")
}

pub(super) fn table_line_with_inserted_cell(
    line: &str,
    col_ix: usize,
    value: &str,
) -> Option<String> {
    let bounds = parse_table_cell_bounds(line)?;
    let mut cells = bounds
        .iter()
        .map(|cell| {
            let text = &line[cell.content_start..cell.content_end];
            if text.is_empty() {
                " ".to_string()
            } else {
                text.to_string()
            }
        })
        .collect::<Vec<_>>();
    cells.insert(col_ix.min(cells.len()), value.to_string());
    Some(format_table_row(&cells))
}

pub(super) fn format_table_row(cells: &[String]) -> String {
    let inner = cells
        .iter()
        .map(|cell| format!(" {cell} "))
        .collect::<Vec<_>>()
        .join("|");
    format!("|{inner}|")
}

pub(super) fn table_row_cells_empty(line: &str) -> bool {
    parse_table_cell_bounds(line).is_some_and(|cells| {
        cells
            .iter()
            .all(|cell| line[cell.content_start..cell.content_end].trim().is_empty())
    })
}

pub(super) fn parse_table_cells(line: &str) -> Option<Vec<&str>> {
    if !line.contains('|') {
        return None;
    }
    let trimmed = line.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    let cells = trimmed.split('|').map(str::trim).collect::<Vec<_>>();
    (cells.len() >= 2).then_some(cells)
}

pub(super) fn is_table_separator_line(line: &str) -> bool {
    parse_table_cells(line).is_some_and(|cells| is_table_separator_cells(&cells))
}

pub(super) fn is_table_separator_cells(cells: &[&str]) -> bool {
    cells.iter().all(|cell| {
        cell.contains('-') && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' ' | '\t'))
    })
}

pub(super) fn parse_markdown_list_marker(line: &str) -> Option<MarkdownListMarker> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    parse_task_marker(trimmed, indent)
        .or_else(|| parse_bullet_marker(trimmed, indent))
        .or_else(|| parse_ordered_marker(trimmed, indent))
}

pub(super) fn parse_task_marker(
    trimmed: &str,
    indent: usize,
) -> Option<MarkdownListMarker> {
    let mut chars = trimmed.chars();
    let bullet = chars.next()?;
    if !is_bullet_marker(bullet) {
        return None;
    }
    let after_bullet = chars.as_str();
    let task = after_bullet.strip_prefix(" [")?;
    let marker = task.chars().next()?;
    if marker == ']' {
        let after_close = &task[marker.len_utf8()..];
        if !after_close.is_empty()
            && !after_close.chars().next().is_some_and(char::is_whitespace)
        {
            return None;
        }
        let spaces = after_close.len() - after_close.trim_start().len();
        return Some(MarkdownListMarker {
            indent,
            marker_len: indent + bullet.len_utf8() + 2 + marker.len_utf8() + spaces,
            kind: MarkdownListMarkerKind::Task { bullet },
        });
    }
    if !matches!(marker, ' ' | 'x' | 'X') {
        return None;
    }
    let after_marker = &task[marker.len_utf8()..];
    let after_close = after_marker.strip_prefix(']')?;
    if !after_close.is_empty()
        && !after_close.chars().next().is_some_and(char::is_whitespace)
    {
        return None;
    }
    let spaces = after_close.len() - after_close.trim_start().len();
    Some(MarkdownListMarker {
        indent,
        marker_len: indent + bullet.len_utf8() + 2 + marker.len_utf8() + 1 + spaces,
        kind: MarkdownListMarkerKind::Task { bullet },
    })
}

pub(super) fn parse_bullet_marker(
    trimmed: &str,
    indent: usize,
) -> Option<MarkdownListMarker> {
    let mut chars = trimmed.chars();
    let bullet = chars.next()?;
    if !is_bullet_marker(bullet) {
        return None;
    }
    let after = chars.as_str();
    if after.is_empty() || !after.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let spaces = after.len() - after.trim_start().len();
    Some(MarkdownListMarker {
        indent,
        marker_len: indent + bullet.len_utf8() + spaces,
        kind: MarkdownListMarkerKind::Bullet(bullet),
    })
}

pub(super) fn parse_ordered_marker(
    trimmed: &str,
    indent: usize,
) -> Option<MarkdownListMarker> {
    let delimiter_ix = trimmed.find(|ch| matches!(ch, ')' | '.'))?;
    let token = &trimmed[..delimiter_ix];
    if token.is_empty() {
        return None;
    }
    let delimiter = trimmed[delimiter_ix..].chars().next()?;
    let after = &trimmed[delimiter_ix + delimiter.len_utf8()..];
    if !after.is_empty() && !after.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let spaces = after.len() - after.trim_start().len();
    let marker_len = indent + delimiter_ix + delimiter.len_utf8() + spaces;
    if token.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(MarkdownListMarker {
            indent,
            marker_len,
            kind: MarkdownListMarkerKind::Number {
                value: token.parse().ok()?,
                width: token.len(),
                delimiter,
            },
        });
    }
    if token.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return Some(MarkdownListMarker {
            indent,
            marker_len,
            kind: MarkdownListMarkerKind::Letter {
                label: token.to_string(),
                delimiter,
            },
        });
    }
    None
}

pub(super) fn is_bullet_marker(ch: char) -> bool {
    matches!(ch, '-' | '*' | '+')
}

pub(super) fn next_alpha_label(label: &str) -> String {
    let uppercase = label
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase());
    let mut value = 0usize;
    for ch in label.chars() {
        let lower = ch.to_ascii_lowercase();
        if !lower.is_ascii_lowercase() {
            return label.to_string();
        }
        value = value * 26 + (lower as usize - 'a' as usize + 1);
    }
    value += 1;

    let mut chars = Vec::new();
    while value > 0 {
        value -= 1;
        chars.push((b'a' + (value % 26) as u8) as char);
        value /= 26;
    }
    chars.reverse();
    let next = chars.into_iter().collect::<String>();
    if uppercase {
        next.to_ascii_uppercase()
    } else {
        next
    }
}

pub(super) fn is_code_fence_line(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

pub(super) fn is_plain_paragraph_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty()
        || is_code_fence_line(line)
        || is_divider(trimmed)
        || parse_markdown_list_marker(line).is_some()
        || trimmed.starts_with('>')
        || trimmed.contains('|')
    {
        return false;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes)
        && trimmed
            .get(hashes..)
            .is_some_and(|rest| rest.chars().next().is_some_and(char::is_whitespace))
    {
        return false;
    }
    true
}

/// Byte length of a blockquote's `> ` prefix (indent + `>` + one space),
/// `None` when the line isn't a quote.
pub(super) fn quote_marker_len(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let rest = trimmed.strip_prefix('>')?;
    let space = rest
        .chars()
        .next()
        .filter(|ch| ch.is_whitespace())
        .map(|ch| ch.len_utf8())
        .unwrap_or(0);
    Some(indent + 1 + space)
}

pub(super) fn visible_marker_len(line: &str) -> usize {
    if is_code_fence_line(line) || is_divider(line.trim()) {
        return 0;
    }
    let trimmed_start = line.trim_start();
    let indent = line.len() - trimmed_start.len();
    let hashes = trimmed_start.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        if let Some(rest) = trimmed_start.get(hashes..) {
            if let Some(space) = rest.chars().next().filter(|ch| ch.is_whitespace()) {
                return indent + hashes + space.len_utf8();
            }
        }
    }
    if let Some(marker) = parse_markdown_list_marker(line) {
        return marker.marker_len;
    }
    if let Some(rest) = trimmed_start.strip_prefix('>') {
        if let Some(space) = rest.chars().next().filter(|ch| ch.is_whitespace()) {
            return indent + 1 + space.len_utf8();
        }
        return indent + 1;
    }
    0
}

pub(super) fn heading_visible_end_col(line: &str) -> Option<usize> {
    let trimmed_start = line.trim_start();
    let indent = line.len() - trimmed_start.len();
    let hashes = trimmed_start.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = trimmed_start.get(hashes..)?;
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let spaces = rest
        .chars()
        .next()
        .filter(|ch| ch.is_whitespace())
        .map(char::len_utf8)
        .unwrap_or(0);
    let start = indent + hashes + spaces;
    let end = line.trim_end().len();
    let visible = line.get(start..end)?.trim_end_matches('#').trim_end();
    Some(start + visible.len())
}

pub(super) fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(super) fn prev_char_boundary(text: &str, index: usize) -> usize {
    let mut prev = 0;
    for (i, _) in text.char_indices() {
        if i >= index {
            break;
        }
        prev = i;
    }
    prev
}

pub(super) fn next_char_boundary(text: &str, index: usize) -> usize {
    text[index..]
        .char_indices()
        .nth(1)
        .map(|(i, _)| index + i)
        .unwrap_or(text.len())
}

pub fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdx"))
        .unwrap_or(false)
}

pub(crate) fn parse_blocks(source: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut in_code = false;
    let mut code_lang = None;
    let mut code = String::new();
    let mut skipping_frontmatter = source.lines().next() == Some("---");

    for line in source
        .lines()
        .skip(if skipping_frontmatter { 1 } else { 0 })
    {
        if skipping_frontmatter {
            if line.trim() == "---" {
                skipping_frontmatter = false;
            }
            continue;
        }

        if let Some(rest) = line.trim_start().strip_prefix("```") {
            flush_paragraph(&mut paragraph, &mut blocks);
            if in_code {
                blocks.push(MarkdownBlock::Code {
                    lang: code_lang.take(),
                    code: code.trim_end_matches('\n').to_string(),
                });
                code.clear();
                in_code = false;
            } else {
                let lang = rest.trim();
                code_lang = (!lang.is_empty()).then(|| lang.to_string());
                in_code = true;
            }
            continue;
        }

        if in_code {
            code.push_str(line);
            code.push('\n');
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            continue;
        }

        if let Some((level, text)) = parse_heading(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(MarkdownBlock::Heading { level, text });
            continue;
        }

        if is_divider(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(MarkdownBlock::Divider);
            continue;
        }

        if let Some((checked, text)) = parse_task(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(MarkdownBlock::Task { checked, text });
            continue;
        }

        if let Some(text) = trimmed.strip_prefix('>') {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(MarkdownBlock::Quote(text.trim().to_string()));
            continue;
        }

        paragraph.push(trimmed.to_string());
    }

    if in_code {
        blocks.push(MarkdownBlock::Code {
            lang: code_lang,
            code: code.trim_end_matches('\n').to_string(),
        });
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    blocks
}

fn flush_paragraph(paragraph: &mut Vec<String>, blocks: &mut Vec<MarkdownBlock>) {
    if !paragraph.is_empty() {
        blocks.push(MarkdownBlock::Paragraph(paragraph.join(" ")));
        paragraph.clear();
    }
}

fn parse_heading(line: &str) -> Option<(u8, String)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = line.get(hashes..)?;
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    Some((hashes as u8, rest.trim_end_matches('#').trim().to_string()))
}

fn parse_task(line: &str) -> Option<(bool, String)> {
    let mut chars = line.chars();
    let bullet = chars.next()?;
    if !is_bullet_marker(bullet) {
        return None;
    }
    let rest = chars.as_str().strip_prefix(" [")?;
    let mut chars = rest.chars();
    let marker = chars.next()?;
    if !matches!(marker, ' ' | 'x' | 'X') {
        return None;
    }
    if chars.next()? != ']' {
        return None;
    }
    let text = chars.as_str().trim_start().to_string();
    Some((matches!(marker, 'x' | 'X'), text))
}

pub(super) fn is_divider(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(marker) = chars.next() else {
        return false;
    };
    matches!(marker, '-' | '*' | '_') && line.len() >= 3 && chars.all(|c| c == marker)
}
