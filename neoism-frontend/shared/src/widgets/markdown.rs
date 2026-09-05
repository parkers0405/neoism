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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownBlockKind {
    Empty,
    Heading(u8),
    Paragraph,
    Task { checked: bool, depth: usize },
    Bullet { depth: usize },
    Ordered { depth: usize },
    CodeFence,
    Code,
    Callout { kind: MarkdownCalloutKind },
    Quote,
    Divider,
}

/// Obsidian-compatible callout families. Aliases such as `summary`, `hint`,
/// and `error` collapse onto the same visual family while the source label is
/// preserved by [`MarkdownCallout`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownCalloutKind {
    Note,
    Abstract,
    Info,
    Todo,
    Tip,
    Important,
    Success,
    Question,
    Warning,
    Failure,
    Danger,
    Bug,
    Example,
    Quote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkdownCallout<'a> {
    pub kind: MarkdownCalloutKind,
    /// The source callout name, without `[!]` (for example `IMPORTANT`).
    pub label: &'a str,
    /// An optional custom title following the marker.
    pub custom_title: Option<&'a str>,
    /// Bytes hidden before the rendered title.
    pub marker_len: usize,
}

impl<'a> MarkdownCallout<'a> {
    pub fn title(&self) -> &'a str {
        self.custom_title.unwrap_or(self.label)
    }
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

    if let Some(callout) = parse_callout_line(line) {
        return ParsedLine {
            kind: MarkdownBlockKind::Callout { kind: callout.kind },
            text: callout.title(),
            marker_len: callout.marker_len,
            list_marker: None,
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

/// Parse an Obsidian callout header such as `> [!IMPORTANT]` or
/// `> [!WARNING]- Custom title`. Ordinary blockquotes return `None`.
pub fn parse_callout_line(line: &str) -> Option<MarkdownCallout<'_>> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let after_quote = trimmed.strip_prefix('>')?;
    let quote_space = after_quote.len() - after_quote.trim_start().len();
    let marker = after_quote.trim_start().strip_prefix("[!")?;
    let close = marker.find(']')?;
    let label = marker[..close].trim();
    if label.is_empty()
        || !label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return None;
    }
    let kind = match label.to_ascii_lowercase().as_str() {
        "note" => MarkdownCalloutKind::Note,
        "abstract" | "summary" | "tldr" => MarkdownCalloutKind::Abstract,
        "info" => MarkdownCalloutKind::Info,
        "todo" => MarkdownCalloutKind::Todo,
        "tip" | "hint" => MarkdownCalloutKind::Tip,
        "important" => MarkdownCalloutKind::Important,
        "success" | "check" | "done" => MarkdownCalloutKind::Success,
        "question" | "help" | "faq" => MarkdownCalloutKind::Question,
        "warning" | "caution" | "attention" => MarkdownCalloutKind::Warning,
        "failure" | "fail" | "missing" => MarkdownCalloutKind::Failure,
        "danger" | "error" => MarkdownCalloutKind::Danger,
        "bug" => MarkdownCalloutKind::Bug,
        "example" => MarkdownCalloutKind::Example,
        "quote" | "cite" => MarkdownCalloutKind::Quote,
        _ => MarkdownCalloutKind::Note,
    };
    let after_marker = &marker[close + 1..];
    let after_fold = after_marker
        .strip_prefix(['+', '-'])
        .unwrap_or(after_marker);
    let title_space = after_fold.len() - after_fold.trim_start().len();
    let custom_title = (!after_fold.trim().is_empty()).then(|| after_fold.trim());
    let marker_len = indent
        + 1
        + quote_space
        + 2
        + close
        + 1
        + (after_marker.len() - after_fold.len())
        + title_space;
    Some(MarkdownCallout {
        kind,
        label,
        custom_title,
        marker_len,
    })
}

/// Resolve the callout family for any line in one contiguous quoted callout
/// block. This lets continuation `> body` lines inherit the header's accent
/// and upright body style without carrying parser state into editor models.
pub fn callout_kind_for_quote_line(
    lines: &[String],
    line_ix: usize,
) -> Option<MarkdownCalloutKind> {
    if quote_prefix_len(lines.get(line_ix)?).is_none() {
        return None;
    }
    for line in lines[..=line_ix].iter().rev() {
        if let Some(callout) = parse_callout_line(line) {
            return Some(callout.kind);
        }
        if quote_prefix_len(line).is_none() {
            break;
        }
    }
    None
}

fn quote_prefix_len(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let rest = trimmed.strip_prefix('>')?;
    let space = rest
        .chars()
        .next()
        .filter(|ch| ch.is_whitespace())
        .map(char::len_utf8)
        .unwrap_or(0);
    Some(indent + 1 + space)
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
    let delimiter = trimmed_start[delimiter_ix..].chars().next()?;
    // A multi-letter `word. Rest` is overwhelmingly a sentence, not a list.
    // Keep alphabetic lists with `)` (including Excel-style `AA)`) and the
    // conventional single-letter `a.` form without stealing sentence openers.
    let alphabetic_marker = all_letters && (delimiter == ')' || token.len() == 1);
    if !all_digits && !alphabetic_marker {
        return None;
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebLinkSpan {
    /// Full clickable source span. For Markdown links this includes
    /// `[label](target)`; for a bare URL it is the URL itself.
    pub raw_start: usize,
    pub raw_end: usize,
    /// Visible label inside the source span.
    pub label_start: usize,
    pub label_end: usize,
    pub target: String,
}

/// Parse a leading `[label](target)` link from `value`. Returns the literal
/// label and target slices plus how many bytes the whole link consumed.
pub fn parse_markdown_link(value: &str) -> Option<MarkdownLink<'_>> {
    let rest = value.strip_prefix('[')?;
    let label_end = rest.find(']')?;
    let label = &rest[..label_end];
    let rest = &rest[label_end + 1..];
    let rest = rest.strip_prefix('(')?;
    let mut depth = 1usize;
    let mut escaped = false;
    let mut target_end = None;
    for (index, ch) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    target_end = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let target_end = target_end?;
    let target = &rest[..target_end];
    Some(MarkdownLink {
        label,
        target,
        consumed: label_end + target_end + 4,
    })
}

/// Return a clean external link target while preserving balanced URL parentheses.
/// Ambient prose punctuation (`https://x.dev).`) is excluded from the target.
pub fn web_url_target(value: &str) -> Option<&str> {
    let value = value.trim_start_matches(|ch: char| {
        matches!(ch, '<' | '(' | '[' | '{' | '\'' | '"' | '`')
    });
    if !(value.starts_with("https://")
        || value.starts_with("http://")
        || value.starts_with("mailto:")
        || value.starts_with("tel:"))
    {
        return None;
    }
    let mut end = value.len();
    loop {
        let Some(ch) = value[..end].chars().next_back() else {
            return None;
        };
        let trim = matches!(
            ch,
            '.' | ',' | ';' | ':' | '!' | '?' | '>' | '\'' | '"' | '`'
        ) || (ch == ')'
            && value[..end].chars().filter(|ch| *ch == ')').count()
                > value[..end].chars().filter(|ch| *ch == '(').count())
            || (ch == ']'
                && value[..end].chars().filter(|ch| *ch == ']').count()
                    > value[..end].chars().filter(|ch| *ch == '[').count())
            || (ch == '}'
                && value[..end].chars().filter(|ch| *ch == '}').count()
                    > value[..end].chars().filter(|ch| *ch == '{').count());
        if !trim {
            break;
        }
        end -= ch.len_utf8();
    }
    let scheme_len = if value.starts_with("https://") {
        8
    } else if value.starts_with("http://") || value.starts_with("mailto:") {
        7
    } else {
        4
    };
    let target = &value[..end];
    (end > scheme_len && !target.chars().any(char::is_whitespace)).then_some(target)
}

/// Validate a local `file://` URI without decoding it. Decoding belongs at
/// the filesystem boundary; keeping the encoded target here preserves exact
/// link round-tripping and makes spaces (`%20`) safe during inline parsing.
pub fn file_uri_target(value: &str) -> Option<&str> {
    let value = value.trim();
    let path = value.strip_prefix("file://")?;
    (!path.is_empty() && !value.chars().any(char::is_whitespace)).then_some(value)
}

/// Target accepted by rendered Markdown links. This deliberately remains
/// conservative for bare prose tokens while allowing explicit Markdown links
/// to point at web URLs, `file://` URIs, or recognizable local paths.
pub fn rendered_link_target(value: &str) -> Option<&str> {
    web_url_target(value)
        .or_else(|| file_uri_target(value))
        .or_else(|| looks_like_file_ref(value).then_some(value))
}

/// CommonMark backslash escape at byte zero. ASCII punctuation is the exact
/// escapable class, so `\*` becomes a literal `*` while `\n` remains `\n`.
pub fn backslash_escape_at_start(value: &str) -> Option<(char, usize)> {
    let rest = value.strip_prefix('\\')?;
    let escaped = rest.chars().next()?;
    escaped
        .is_ascii_punctuation()
        .then_some((escaped, 1 + escaped.len_utf8()))
}

/// Parse a web link beginning at byte zero. Standard Markdown links expose
/// only their label as visible text, while bare URLs use the URL itself.
pub fn web_link_at_start(text: &str) -> Option<WebLinkSpan> {
    if let Some(link) = parse_markdown_link(text) {
        if let Some(target) = web_url_target(link.target) {
            let label_start = 1;
            return Some(WebLinkSpan {
                raw_start: 0,
                raw_end: link.consumed,
                label_start,
                label_end: label_start + link.label.len(),
                target: target.to_string(),
            });
        }
    }

    if text.starts_with("https://")
        || text.starts_with("http://")
        || text.starts_with("mailto:")
        || text.starts_with("tel:")
    {
        let token_end = text
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '"' | '\'' | '`'))
            .unwrap_or(text.len());
        if let Some(target) = web_url_target(&text[..token_end]) {
            let end = target.len();
            return Some(WebLinkSpan {
                raw_start: 0,
                raw_end: end,
                label_start: 0,
                label_end: end,
                target: target.to_string(),
            });
        }
    }

    None
}

/// Find standard Markdown web links and bare HTTP(S) URLs without returning
/// the destination inside `[label](destination)` a second time.
pub fn web_link_spans(text: &str) -> Vec<WebLinkSpan> {
    let mut spans = Vec::new();
    let mut byte = 0usize;
    while byte < text.len() {
        let rest = &text[byte..];
        if let Some(mut link) = web_link_at_start(rest) {
            link.raw_start += byte;
            link.raw_end += byte;
            link.label_start += byte;
            link.label_end += byte;
            byte = link.raw_end;
            spans.push(link);
            continue;
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        byte += ch.len_utf8();
    }
    spans
}

pub fn web_link_at(text: &str, char_col: usize) -> Option<WebLinkSpan> {
    web_link_spans(text).into_iter().find(|span| {
        let start = text[..span.raw_start].chars().count();
        let end = start + text[span.raw_start..span.raw_end].chars().count();
        char_col >= start && char_col < end
    })
}

/// Find the next inline marker (`**`, `~~`, `` ` ``, `[`) in `value`.
pub fn next_inline_marker(value: &str) -> Option<usize> {
    ["**", "~~", "`", "[", "\\"]
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
    if file_uri_target(value).is_some() {
        return true;
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
    fn ordered_parse_does_not_steal_sentence_openers() {
        for sentence in [
            "Fair. Outages hit especially hard.",
            "Monkey. What's the move?",
            "Chill. OpenAI still giving you trouble?",
        ] {
            let parsed = parse_line(sentence, false);
            assert!(matches!(parsed.kind, MarkdownBlockKind::Paragraph));
            assert_eq!(parsed.text, sentence);
            assert!(parse_ordered_line(sentence).is_none());
        }

        for list in ["1. Numeric", "a. Alpha", "a) Alpha", "AA) Excel style"] {
            assert!(parse_ordered_line(list).is_some(), "{list}");
        }
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
        assert!(looks_like_file_ref("file:///tmp/Patriot%20Report.md"));
        assert!(!looks_like_file_ref("Yes/No"));
        assert!(!looks_like_file_ref("and/or"));
        assert!(looks_like_file_ref("/etc/hosts"));
    }

    #[test]
    fn obsidian_callouts_parse_common_kinds_and_custom_titles() {
        let important = parse_callout_line("> [!IMPORTANT]").unwrap();
        assert_eq!(important.kind, MarkdownCalloutKind::Important);
        assert_eq!(important.title(), "IMPORTANT");
        assert_eq!(important.marker_len, "> [!IMPORTANT]".len());

        let warning = parse_callout_line("  > [!WARNING]- Read this first").unwrap();
        assert_eq!(warning.kind, MarkdownCalloutKind::Warning);
        assert_eq!(warning.title(), "Read this first");
        assert_eq!(warning.marker_len, "  > [!WARNING]- ".len());

        let lines = vec![
            "> [!IMPORTANT]".to_string(),
            "> First body line".to_string(),
            "> Second body line".to_string(),
            "ordinary paragraph".to_string(),
        ];
        assert_eq!(
            callout_kind_for_quote_line(&lines, 2),
            Some(MarkdownCalloutKind::Important)
        );
        assert_eq!(callout_kind_for_quote_line(&lines, 3), None);
    }

    #[test]
    fn commonmark_backslash_escape_only_accepts_ascii_punctuation() {
        assert_eq!(backslash_escape_at_start(r"\* footnote"), Some(('*', 2)));
        assert_eq!(backslash_escape_at_start(r"\n"), None);
    }

    #[test]
    fn web_links_cover_markdown_labels_and_bare_urls_once() {
        let text =
            "[Search Engineer](https://jobs.example/x) and https://neoism.dev/docs).";
        let links = web_link_spans(text);
        assert_eq!(links.len(), 2);
        assert_eq!(
            &text[links[0].label_start..links[0].label_end],
            "Search Engineer"
        );
        assert_eq!(links[0].target, "https://jobs.example/x");
        assert_eq!(links[1].target, "https://neoism.dev/docs");
        assert_eq!(&text[links[1].raw_start..links[1].raw_end], links[1].target);
    }

    #[test]
    fn web_links_are_detected_inside_emphasis_markers() {
        let text = "**[Sira — Founding Engineer](https://www.ycombinator.com/companies/sira/jobs/founding-engineer)**";
        let links = web_link_spans(text);

        assert_eq!(links.len(), 1);
        assert_eq!(
            &text[links[0].label_start..links[0].label_end],
            "Sira — Founding Engineer"
        );
        assert_eq!(
            links[0].target,
            "https://www.ycombinator.com/companies/sira/jobs/founding-engineer"
        );
    }

    #[test]
    fn phone_links_support_markdown_labels_and_bare_tel_targets() {
        let text = "Call [support](tel:+15551234567) or tel:+15557654321.";
        let links = web_link_spans(text);

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "tel:+15551234567");
        assert_eq!(&text[links[0].label_start..links[0].label_end], "support");
        assert_eq!(links[1].target, "tel:+15557654321");
    }

    #[test]
    fn markdown_link_parser_keeps_balanced_url_parentheses() {
        let link = parse_markdown_link("[docs](https://x.dev/path_(one)) tail").unwrap();
        assert_eq!(link.label, "docs");
        assert_eq!(link.target, "https://x.dev/path_(one)");
        assert_eq!(link.consumed, "[docs](https://x.dev/path_(one))".len());
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
