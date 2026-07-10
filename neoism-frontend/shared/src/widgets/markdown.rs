//! Shared markdown rendering primitives.
//!
//! Two callers consume this widget:
//!
//! 1. **`editor/markdown/render/*`** — the markdown editor pane. Owns cursor,
//!    selection, yank-flash, drag, and per-block chrome on top of the parser
//!    and inline link helpers here.
//! 2. **`neoism/view/markdown.rs`** — agent chat. Owns selectable-line +
//!    link-hit-rect registration on top of the wrap/draw helpers here.
//!
//! What lives here:
//! - Block/inline parsing that both renderers agree on.
//! - Word-wrap (measured + char-estimated).
//! - File-ref/link heuristics that previously diverged between callers.
//! - Stateless drawing primitives (rect clipping, text-with-occlusion).
//!
//! What stays in callers: cursor/selection rendering, block chrome with
//! drag handles, scrollbars, mermaid, syntax-highlighted code bodies,
//! anything that touches caller-specific pane state.
//!
//! Visual differences intentionally normalised — none: each caller still
//! drives all of its sizing/color tokens through its own config, so this
//! widget can be adopted incrementally without pixel drift.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

// ---------------------------------------------------------------------------
// Block model
// ---------------------------------------------------------------------------

/// Coarse markdown block kind. Both renderers share this enum but each one
/// derives per-block sizing/chrome on its own — the widget only commits to
/// the *parse*, not to any particular pixel layout.
#[derive(Clone, Copy, Debug)]
pub enum MarkdownBlockKind {
    Empty,
    Heading(u8),
    Paragraph,
    Task { checked: bool, depth: usize },
    Bullet { depth: usize },
    Ordered { depth: usize },
    CodeFence,
    Code,
    Quote,
    Divider,
}

/// One parsed line of source markdown. `text` is a borrow of the input; the
/// `marker_len` field is how many bytes the source line spends on the marker
/// itself (`# `, `- `, `> `, etc.) — callers that need byte-accurate cursor
/// positions key off it.
#[derive(Clone, Copy, Debug)]
pub struct ParsedLine<'a> {
    pub kind: MarkdownBlockKind,
    pub text: &'a str,
    pub marker_len: usize,
    pub list_marker: Option<&'a str>,
}

const LIST_INDENT_SPACES: usize = 2;

/// Parse one source line. `in_code` lets callers track whether the line sits
/// inside a fenced code block — fences toggle this externally.
pub fn parse_line(line: &str, in_code: bool) -> ParsedLine<'_> {
    if line.trim().is_empty() {
        return ParsedLine {
            kind: MarkdownBlockKind::Empty,
            text: "",
            marker_len: 0,
            list_marker: None,
        };
    }

    let trimmed_start = line.trim_start();
    let indent = line.len() - trimmed_start.len();

    if let Some(rest) = trimmed_start.strip_prefix("```") {
        return ParsedLine {
            kind: MarkdownBlockKind::CodeFence,
            text: rest.trim(),
            marker_len: indent + 3,
            list_marker: None,
        };
    }

    if in_code {
        return ParsedLine {
            kind: MarkdownBlockKind::Code,
            text: line,
            marker_len: 0,
            list_marker: None,
        };
    }

    if let Some((level, marker_len, text)) = parse_heading_line(line) {
        return ParsedLine {
            kind: MarkdownBlockKind::Heading(level),
            text,
            marker_len,
            list_marker: None,
        };
    }

    if is_divider_line(trimmed_start) {
        return ParsedLine {
            kind: MarkdownBlockKind::Divider,
            text: "",
            marker_len: indent,
            list_marker: None,
        };
    }

    if let Some((checked, depth, marker_len, text)) = parse_task_line(line) {
        return ParsedLine {
            kind: MarkdownBlockKind::Task { checked, depth },
            text,
            marker_len,
            list_marker: None,
        };
    }

    if let Some((depth, marker_len, text)) = parse_bullet_line(line) {
        return ParsedLine {
            kind: MarkdownBlockKind::Bullet { depth },
            text,
            marker_len,
            list_marker: None,
        };
    }

    if let Some((depth, marker, marker_len, text)) = parse_ordered_line(line) {
        return ParsedLine {
            kind: MarkdownBlockKind::Ordered { depth },
            text,
            marker_len,
            list_marker: Some(marker),
        };
    }

    if let Some(rest) = trimmed_start.strip_prefix('>') {
        let spaces = rest.len() - rest.trim_start().len();
        return ParsedLine {
            kind: MarkdownBlockKind::Quote,
            text: rest.trim_start(),
            marker_len: indent + 1 + spaces,
            list_marker: None,
        };
    }

    ParsedLine {
        kind: MarkdownBlockKind::Paragraph,
        text: line.trim(),
        marker_len: line.len() - line.trim_start().len(),
        list_marker: None,
    }
}

pub fn parse_heading_line(line: &str) -> Option<(u8, usize, &str)> {
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
    let marker_space = rest.chars().next()?.len_utf8();
    let body = rest.get(marker_space..)?;
    let text = body.trim_end_matches('#').trim_end();
    Some((hashes as u8, indent + hashes + marker_space, text))
}

pub fn parse_task_line(line: &str) -> Option<(bool, usize, usize, &str)> {
    let trimmed_start = line.trim_start();
    let indent = line.len() - trimmed_start.len();
    let mut chars = trimmed_start.chars();
    let bullet = chars.next()?;
    if !is_bullet_marker(bullet) {
        return None;
    }
    let rest = chars.as_str().strip_prefix(" [")?;
    let marker = rest.chars().next()?;
    if marker == ']' {
        let after = rest.get(marker.len_utf8()..)?;
        if !after.is_empty() && !after.chars().next().is_some_and(char::is_whitespace) {
            return None;
        }
        let spaces = after.len() - after.trim_start().len();
        return Some((
            false,
            list_depth_from_indent(indent),
            indent + bullet.len_utf8() + 2 + marker.len_utf8() + spaces,
            after.trim_start(),
        ));
    }
    if !matches!(marker, ' ' | 'x' | 'X') || rest.chars().nth(1)? != ']' {
        return None;
    }
    let after = rest.get(2..)?;
    if !after.is_empty() && !after.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let spaces = after.len() - after.trim_start().len();
    Some((
        matches!(marker, 'x' | 'X'),
        list_depth_from_indent(indent),
        indent + bullet.len_utf8() + 2 + marker.len_utf8() + 1 + spaces,
        after.trim_start(),
    ))
}

pub fn parse_bullet_line(line: &str) -> Option<(usize, usize, &str)> {
    let trimmed_start = line.trim_start();
    let indent = line.len() - trimmed_start.len();
    let mut chars = trimmed_start.chars();
    let bullet = chars.next()?;
    if !is_bullet_marker(bullet) {
        return None;
    }
    let after = chars.as_str();
    if after.is_empty() || !after.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let spaces = after.len() - after.trim_start().len();
    Some((
        list_depth_from_indent(indent),
        indent + bullet.len_utf8() + spaces,
        after.trim_start(),
    ))
}

pub fn parse_ordered_line(line: &str) -> Option<(usize, &str, usize, &str)> {
    let trimmed_start = line.trim_start();
    let indent = line.len() - trimmed_start.len();
    let delimiter_ix = trimmed_start.find(|ch| matches!(ch, ')' | '.'))?;
    let token = &trimmed_start[..delimiter_ix];
    if token.is_empty()
        || !token
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch.is_ascii_alphabetic())
    {
        return None;
    }
    let all_digits = token.chars().all(|ch| ch.is_ascii_digit());
    let all_letters = token.chars().all(|ch| ch.is_ascii_alphabetic());
    if !all_digits && !all_letters {
        return None;
    }
    let delimiter = trimmed_start[delimiter_ix..].chars().next()?;
    let marker_end = delimiter_ix + delimiter.len_utf8();
    let after = &trimmed_start[marker_end..];
    if !after.is_empty() && !after.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let spaces = after.len() - after.trim_start().len();
    Some((
        list_depth_from_indent(indent),
        &trimmed_start[..marker_end],
        indent + marker_end + spaces,
        after.trim_start(),
    ))
}

pub fn is_bullet_marker(ch: char) -> bool {
    matches!(ch, '-' | '*' | '+')
}

pub fn list_depth_from_indent(indent: usize) -> usize {
    (indent + LIST_INDENT_SPACES - 1) / LIST_INDENT_SPACES
}

pub fn is_divider_line(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(marker) = chars.next() else {
        return false;
    };
    matches!(marker, '-' | '*' | '_') && line.len() >= 3 && chars.all(|c| c == marker)
}

/// Walk forward from `start` (assumed to be a code fence) and return the
/// index of the closing fence (or `lines.len()` if the block is unclosed).
pub fn code_block_end(lines: &[String], start: usize) -> usize {
    for ix in start + 1..lines.len() {
        if lines[ix].trim_start().starts_with("```") {
            return ix;
        }
    }
    lines.len()
}

/// Extract the language hint from a fenced code line. Supports both
/// ` ``` ` and `~~~` fences; returns the trimmed info-string.
pub fn fence_info(line: &str) -> Option<&str> {
    line.strip_prefix("```")
        .or_else(|| line.strip_prefix("~~~"))
        .map(str::trim)
}

// ---------------------------------------------------------------------------
// Table parsing
// ---------------------------------------------------------------------------

/// Parse a single table row. Returns `None` if the line isn't pipe-delimited
/// or has fewer than two cells.
///
/// Note: this is the agent-view flavor that trims cell content. The editor
/// caller uses a stricter form (`parse_table_cell_bounds`) that preserves
/// trailing space for editable cells — it lives next to the editor state.
pub fn parse_table_row_trimmed(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }

    // A pipe is a cell delimiter only outside code spans and when it is not
    // backslash-escaped. This matches GFM's table tokenizer closely enough for
    // the retained renderer while avoiding the classic false-table case:
    // ordinary prose containing `` `left | right` ``.
    let chars: Vec<char> = trimmed.chars().collect();
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut delimiter_count = 0usize;
    let mut code_ticks = 0usize;
    let mut explicit_leading_pipe = false;
    let mut explicit_trailing_pipe = false;
    let mut index = 0usize;
    while index < chars.len() {
        match chars[index] {
            '\\' if index + 1 < chars.len() => {
                cell.push(chars[index]);
                cell.push(chars[index + 1]);
                index += 2;
            }
            '`' => {
                let start = index;
                while index < chars.len() && chars[index] == '`' {
                    cell.push('`');
                    index += 1;
                }
                let run = index - start;
                if code_ticks == 0 {
                    if has_closing_backtick_run(&chars, index, run) {
                        code_ticks = run;
                    }
                } else if code_ticks == run {
                    code_ticks = 0;
                }
            }
            '|' if code_ticks == 0 => {
                explicit_leading_pipe |= index == 0;
                explicit_trailing_pipe = index + 1 == chars.len();
                cells.push(clean_table_cell(&cell));
                cell.clear();
                delimiter_count += 1;
                index += 1;
            }
            ch => {
                cell.push(ch);
                index += 1;
            }
        }
    }
    cells.push(clean_table_cell(&cell));

    if delimiter_count == 0 {
        return None;
    }
    if explicit_leading_pipe && cells.first().is_some_and(String::is_empty) {
        cells.remove(0);
    }
    if explicit_trailing_pipe && cells.last().is_some_and(String::is_empty) {
        cells.pop();
    }
    (cells.len() >= 2
        || (cells.len() == 1 && (explicit_leading_pipe || explicit_trailing_pipe)))
        .then_some(cells)
}

fn clean_table_cell(cell: &str) -> String {
    cell.trim().replace("\\|", "|")
}

fn has_closing_backtick_run(chars: &[char], mut index: usize, expected: usize) -> bool {
    while index < chars.len() {
        if chars[index] == '\\' {
            index = (index + 2).min(chars.len());
            continue;
        }
        if chars[index] != '`' {
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len() && chars[index] == '`' {
            index += 1;
        }
        if index - start == expected {
            return true;
        }
    }
    false
}

/// `true` if `cells` is the `---|---|---` separator row of a GFM table.
pub fn is_table_separator_trimmed(cells: &[String]) -> bool {
    cells.iter().all(|cell| {
        let cell = cell.trim();
        let core = cell.strip_prefix(':').unwrap_or(cell);
        let core = core.strip_suffix(':').unwrap_or(core);
        !core.is_empty() && core.chars().all(|ch| ch == '-')
    })
}

/// A GFM table starts only when a pipe-delimited header is immediately
/// followed by a delimiter row with the same number of columns.
pub fn is_table_delimiter_for_header(header: &[String], delimiter: &[String]) -> bool {
    header.len() == delimiter.len() && is_table_separator_trimmed(delimiter)
}

// ---------------------------------------------------------------------------
// Inline parsing
// ---------------------------------------------------------------------------

/// Inline link in `[label](target)` form.
pub struct MarkdownLink<'a> {
    pub label: &'a str,
    pub target: &'a str,
    pub consumed: usize,
}

/// Parse a leading `[label](target)` link from `value`. Returns the literal
/// label and target slices plus how many bytes the whole link consumed.
pub fn parse_markdown_link(value: &str) -> Option<MarkdownLink<'_>> {
    let rest = value.strip_prefix('[')?;
    let label_end = rest.find(']')?;
    let label = &rest[..label_end];
    let rest = &rest[label_end + 1..];
    let rest = rest.strip_prefix('(')?;
    let target_end = rest.find(')')?;
    let target = &rest[..target_end];
    Some(MarkdownLink {
        label,
        target,
        consumed: label_end + target_end + 4,
    })
}

/// Find the next inline marker (`**`, `~~`, `` ` ``, `[`) in `value`.
pub fn next_inline_marker(value: &str) -> Option<usize> {
    ["**", "~~", "`", "["]
        .into_iter()
        .filter_map(|needle| value.find(needle))
        .min()
}

/// Trim ambient punctuation that often surrounds a path-like token in prose
/// (`see foo.rs.` should still recognize `foo.rs`).
pub fn clean_link_target(value: &str) -> &str {
    value.trim_matches(|ch: char| {
        matches!(
            ch,
            ',' | '.'
                | ':'
                | ';'
                | ')'
                | ']'
                | '}'
                | '('
                | '['
                | '{'
                | '<'
                | '>'
                | '`'
                | '\''
                | '"'
        )
    })
}

/// `true` if `value` is a recognised source-file extension (one of the
/// languages or formats the renderer knows how to colourise / link).
pub fn has_known_file_extension(value: &str) -> bool {
    let Some(dot) = value.rfind('.') else {
        return false;
    };
    let ext = value[dot + 1..].to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "md"
            | "mdx"
            | "json"
            | "jsonc"
            | "toml"
            | "yaml"
            | "yml"
            | "lua"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cxx"
            | "java"
            | "kt"
            | "kts"
            | "swift"
            | "rb"
            | "php"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "sql"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "vue"
            | "svelte"
            | "txt"
            | "log"
            | "csv"
            | "tsv"
            | "ini"
            | "conf"
            | "lock"
            | "nix"
            | "dockerfile"
    )
}

/// Conservative "prose token" path detection — only rooted paths (`/`, `./`,
/// `../`, `~/`) or tokens with a known file extension qualify. Used to avoid
/// turning `and/or`, `Yes/No`, etc. into bogus clickable links.
pub fn looks_like_file_ref(value: &str) -> bool {
    let value = clean_link_target(value);
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return false;
    }
    let base = value.split(':').next().unwrap_or(value);
    if base.is_empty() {
        return false;
    }
    let starts_with_anchor = base.starts_with('/')
        || base.starts_with("./")
        || base.starts_with("../")
        || base.starts_with("~/");
    if starts_with_anchor && base.contains('/') {
        return true;
    }
    has_known_file_extension(base)
}

/// Looser detection for tokens already wrapped in backticks — the user
/// signaled "this is a path / identifier", so a bare directory path counts.
pub fn looks_like_inline_code_ref(value: &str) -> bool {
    let value = clean_link_target(value);
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return false;
    }
    if looks_like_file_ref(value) {
        return true;
    }
    let base = value.split(':').next().unwrap_or(value);
    if base.contains('/') && !base.starts_with("//") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Wrap helpers
// ---------------------------------------------------------------------------

/// Measure-driven greedy word wrap. Splits oversize words across character
/// boundaries so a single un-spaced URL doesn't blow past `max_w`.
pub fn wrap_words_measured(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    let mut pending_ws = 0usize;
    for token in text.split_inclusive(char::is_whitespace) {
        // Split the TOKEN into word + its trailing whitespace. Splitting the
        // already-trimmed string (the old bug) made `ws` always empty, so
        // every whitespace run collapsed to the `.max(1)` floor — typed
        // spaces/tabs after a word never widened the drawn row while the
        // caret (mapping real source chars) slid onto the next word:
        // phantom "virtual spacing" on headings.
        let (word, ws) =
            token.split_at(token.trim_end_matches(char::is_whitespace).len());
        if word.is_empty() {
            pending_ws = pending_ws.saturating_add(token.chars().count());
            continue;
        }
        let lead = pending_ws.max((!line.is_empty()) as usize);
        pending_ws = ws.chars().count();
        if line.is_empty() && sugarloaf.text_mut().measure(word, opts) > max_w {
            let mut chunks = split_word_to_fit(sugarloaf, word, max_w, opts);
            if let Some(last) = chunks.pop() {
                out.extend(chunks);
                line = last;
            }
            continue;
        }
        let candidate = if line.is_empty() {
            format!("{}{}", " ".repeat(lead), word)
        } else {
            format!("{}{}{}", line, " ".repeat(lead), word)
        };
        if sugarloaf.text_mut().measure(&candidate, opts) <= max_w || line.is_empty() {
            line = candidate;
        } else {
            out.push(std::mem::take(&mut line));
            if sugarloaf.text_mut().measure(word, opts) > max_w {
                let mut chunks = split_word_to_fit(sugarloaf, word, max_w, opts);
                if let Some(last) = chunks.pop() {
                    out.extend(chunks);
                    line = last;
                } else {
                    line.clear();
                }
            } else {
                line = format!("{}{}", " ".repeat(pending_ws), word);
            }
        }
    }
    if pending_ws > 0 && !line.is_empty() {
        line.push_str(&" ".repeat(pending_ws));
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn split_word_to_fit(
    sugarloaf: &mut Sugarloaf,
    word: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut chunk = String::new();
    for ch in word.chars() {
        let mut candidate = chunk.clone();
        candidate.push(ch);
        if !chunk.is_empty() && sugarloaf.text_mut().measure(&candidate, opts) > max_w {
            out.push(chunk);
            chunk = ch.to_string();
        } else {
            chunk = candidate;
        }
    }
    if !chunk.is_empty() {
        out.push(chunk);
    }
    out
}

/// Cheap, measure-free wrap that estimates a max character count from
/// `cursor_cell_width(opts)`. Used inside code blocks where the column grid
/// is monospaced and measuring every line would be wasteful.
#[allow(dead_code)]
pub fn wrap_lines_estimated(text: &str, max_w: f32, opts: &DrawOpts) -> Vec<String> {
    let max_chars = (max_w / cursor_cell_width(opts)).floor().max(1.0) as usize;
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if word.chars().count() > max_chars {
            if !line.is_empty() {
                out.push(line);
                line = String::new();
            }
            let mut chunk = String::new();
            for ch in word.chars() {
                if chunk.chars().count() >= max_chars {
                    out.push(chunk);
                    chunk = String::new();
                }
                chunk.push(ch);
            }
            if !chunk.is_empty() {
                line = chunk;
            }
            continue;
        }
        let candidate_len = if line.is_empty() {
            word.chars().count()
        } else {
            line.chars().count() + 1 + word.chars().count()
        };
        if candidate_len <= max_chars || line.is_empty() {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        } else {
            out.push(line);
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

// ---------------------------------------------------------------------------
// Layout metrics
// ---------------------------------------------------------------------------

pub fn line_height(opts: &DrawOpts) -> f32 {
    (opts.font_size * 1.48).max(opts.font_size + 6.0)
}

pub fn caret_height(opts: &DrawOpts) -> f32 {
    (opts.font_size * 1.18).max(opts.font_size + 2.0)
}

pub fn cursor_cell_width(opts: &DrawOpts) -> f32 {
    (opts.font_size * 0.58).max(7.0)
}

pub fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

pub fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    let [ax, ay, aw, ah] = a;
    let [bx, by, bw, bh] = b;
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}

pub fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

pub fn point_in_rect(x: f32, y: f32, rect: [f32; 4]) -> bool {
    x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_parse_returns_level_and_text() {
        let parsed = parse_line("### hello world", false);
        assert!(matches!(parsed.kind, MarkdownBlockKind::Heading(3)));
        assert_eq!(parsed.text, "hello world");
        assert_eq!(parsed.marker_len, 4);

        let parsed = parse_line("###    Deep", false);
        assert!(matches!(parsed.kind, MarkdownBlockKind::Heading(3)));
        assert_eq!(parsed.text, "   Deep");
        assert_eq!(parsed.marker_len, 4);
    }

    #[test]
    fn bullet_parse_strips_marker() {
        let parsed = parse_line("- item", false);
        assert!(matches!(
            parsed.kind,
            MarkdownBlockKind::Bullet { depth: 0 }
        ));
        assert_eq!(parsed.text, "item");
    }

    #[test]
    fn task_parse_detects_check_state() {
        let parsed = parse_line("- [x] done", false);
        match parsed.kind {
            MarkdownBlockKind::Task { checked, depth } => {
                assert!(checked);
                assert_eq!(depth, 0);
            }
            _ => panic!("expected task"),
        }
        assert_eq!(parsed.text, "done");
    }

    #[test]
    fn fence_info_recognises_both_styles() {
        assert_eq!(fence_info("```rust"), Some("rust"));
        assert_eq!(fence_info("~~~ ts"), Some("ts"));
        assert_eq!(fence_info("hello"), None);
    }

    #[test]
    fn looks_like_file_ref_accepts_extensions_and_anchored_paths() {
        assert!(looks_like_file_ref("src/foo.rs"));
        assert!(looks_like_file_ref("./pkg/main"));
        assert!(!looks_like_file_ref("Yes/No"));
        assert!(!looks_like_file_ref("and/or"));
        assert!(looks_like_file_ref("/etc/hosts"));
    }

    #[test]
    fn markdown_link_extracts_label_target_and_length() {
        let link = parse_markdown_link("[Title](https://x.dev/path) trailing").unwrap();
        assert_eq!(link.label, "Title");
        assert_eq!(link.target, "https://x.dev/path");
        assert_eq!(link.consumed, "[Title](https://x.dev/path)".len());
    }

    #[test]
    fn table_row_trimmed_splits_pipe_separated_cells() {
        let row = parse_table_row_trimmed("| foo | bar |").unwrap();
        assert_eq!(row, vec!["foo".to_string(), "bar".to_string()]);
        assert!(is_table_separator_trimmed(&[
            "---".to_string(),
            ":--:".to_string()
        ]));
    }

    #[test]
    fn table_row_ignores_pipes_inside_code_and_escaped_pipes() {
        assert_eq!(
            parse_table_row_trimmed("ordinary prose with `` `left | right` ``"),
            None
        );
        assert_eq!(
            parse_table_row_trimmed("| `left | right` | a \\| b |"),
            Some(vec!["`left | right`".into(), "a | b".into()])
        );
        assert_eq!(
            parse_table_row_trimmed("unclosed ` code | still a delimiter"),
            Some(vec!["unclosed ` code".into(), "still a delimiter".into()])
        );
    }

    #[test]
    fn table_delimiter_must_match_header_width_and_contain_dashes() {
        let header = parse_table_row_trimmed("| one | two |").unwrap();
        let valid = parse_table_row_trimmed("| :--- | ---: |").unwrap();
        let too_short = parse_table_row_trimmed("| --- |").unwrap_or_default();
        let colon_only = vec![":".into(), ":".into()];

        assert!(is_table_delimiter_for_header(&header, &valid));
        assert!(!is_table_delimiter_for_header(&header, &too_short));
        assert!(!is_table_delimiter_for_header(&header, &colon_only));

        let one_column_header = parse_table_row_trimmed("| one |").unwrap();
        let one_column_delimiter = parse_table_row_trimmed("| --- |").unwrap();
        assert!(is_table_delimiter_for_header(
            &one_column_header,
            &one_column_delimiter
        ));
    }
}
