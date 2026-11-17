//! Line-local live preview. Offsets are UTF-8 bytes in the original source line.
//!
//! At a hidden span, the shared display boundary is biased toward the next
//! visible character. The final boundary always reaches the source end, even
//! when trailing syntax is hidden. This keeps clicks and end-of-line editing
//! in source coordinates without modifying the document.

use std::ops::RangeInclusive;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Prose,
    Heading(u8),
    Quote,
    List,
    Task { checked: bool, marker: usize },
    Rule,
    Fence,
    Code { language: String },
    Table,
    Frontmatter,
}

#[derive(Clone, Debug)]
pub(super) struct Mark {
    pub start: usize,
    pub end: usize,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Line {
    pub text: String,
    pub source: Vec<usize>,
    pub marks: Vec<Mark>,
    pub kind: Kind,
    /// Leading source indentation, measured in bytes (tabs count as one).
    pub indent: usize,
}

#[derive(Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

impl Line {
    fn raw(raw: &str, kind: Kind, indent: usize) -> Self {
        Self {
            text: raw.to_owned(),
            source: (0..=raw.len()).collect(),
            marks: Vec::new(),
            kind,
            indent,
        }
    }

    fn append(&mut self, text: &str, offset: usize, style: Style) {
        let start = self.text.len();
        *self.source.last_mut().unwrap() = offset;
        self.text.push_str(text);
        self.source.extend((1..=text.len()).map(|n| offset + n));
        if !text.is_empty()
            && (style.bold || style.italic || style.strike || style.code || style.link)
        {
            if let Some(last) = self.marks.last_mut() {
                if last.end == start && last.bold == style.bold && last.italic == style.italic
                    && last.strike == style.strike && last.code == style.code && last.link == style.link
                {
                    last.end = self.text.len();
                    return;
                }
            }
            self.marks.push(Mark {
                start,
                end: self.text.len(),
                bold: style.bold,
                italic: style.italic,
                strike: style.strike,
                code: style.code,
                link: style.link,
            });
        }
    }

    // Synthetic glyphs map their interior bytes to the source marker start.
    fn glyph(&mut self, glyph: &str, start: usize, end: usize) {
        *self.source.last_mut().unwrap() = start;
        self.text.push_str(glyph);
        self.source.extend(std::iter::repeat_n(start, glyph.len()));
        *self.source.last_mut().unwrap() = end;
    }
}

fn escaped(text: &str, at: usize) -> bool {
    text.as_bytes()[..at].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

/// Find a matching bracket/parenthesis, ignoring escaped delimiters.
fn balanced(text: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0;
    for (i, &b) in text.as_bytes().iter().enumerate().skip(start) {
        if escaped(text, i) {
            continue;
        }
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn inline(out: &mut Line, raw: &str, start: usize, end: usize, style: Style, depth: u8, images: &[std::ops::Range<usize>]) {
    // Malformed/deep input remains editable literal text rather than recursing
    // without bound. This is intentionally not a full CommonMark parser.
    if depth > 24 {
        out.append(&raw[start..end], start, style);
        return;
    }
    let mut i = start;
    while i < end {
        // Native image rows own these bytes in preview. Retain surrounding text
        // source stops; the shared boundary follows the next visible character.
        // Empty/image-only lines keep the normal final source-end anchor.
        if let Some(image) = images.iter().find(|image| image.start == i && image.end <= end) {
            i = image.end;
            continue;
        }
        let tail = &raw[i..end];
        let b = raw.as_bytes()[i];
        if b == b'\\' && i + 1 < end && raw.as_bytes()[i + 1].is_ascii_punctuation() {
            out.append(&raw[i + 1..i + 2], i + 1, style);
            i += 2;
            continue;
        }
        if b == b'`' {
            let n = tail.bytes().take_while(|&c| c == b'`').count();
            let token = &raw[i..i + n];
            let mut close = None;
            let mut p = i + n;
            while p < end {
                if raw.as_bytes()[p] == b'`' {
                    let run = raw[p..end].bytes().take_while(|&c| c == b'`').count();
                    if run == n {
                        close = Some(p);
                        break;
                    }
                    p += run;
                } else {
                    p += 1;
                }
            }
            if let Some(close) = close {
                let mut code = style;
                code.code = true;
                out.append(&raw[i + token.len()..close], i + n, code);
                i = close + n;
                continue;
            }
        }
        if tail.starts_with("[[") {
            if let Some(relative) = tail[2..].match_indices("]]").find_map(|(p, _)| {
                (!escaped(raw, i + 2 + p)).then_some(p)
            }) {
                let close = i + 2 + relative;
                let body = &raw[i + 2..close];
                let label = body.match_indices('|').find_map(|(p, _)| {
                    (!escaped(raw, i + 2 + p)).then_some(i + 3 + p)
                }).unwrap_or(i + 2);
                let mut linked = style;
                linked.link = true;
                inline(out, raw, label, close, linked, depth + 1, images);
                i = close + 2;
                continue;
            }
        } else if b == b'[' {
            if let Some(close) = balanced(&raw[..end], i, b'[', b']') {
                if raw.as_bytes().get(close + 1) == Some(&b'(') && close + 1 < end {
                    if let Some(destination) = balanced(&raw[..end], close + 1, b'(', b')') {
                        let mut linked = style;
                        linked.link = true;
                        inline(out, raw, i + 1, close, linked, depth + 1, images);
                        i = destination + 1;
                        continue;
                    }
                }
            }
        }
        if matches!(b, b'*' | b'_' | b'~') {
            let run = tail.bytes().take_while(|&c| c == b).count();
            let n = if b == b'~' { 2 } else { run.min(3) };
            let previous = raw[..i].chars().next_back();
            let next = raw[i + run..end].chars().next();
            let can_open = run >= n && (b != b'~' || run == 2)
                && next.is_some_and(|c| !c.is_whitespace())
                && !(b == b'_' && previous.is_some_and(char::is_alphanumeric));
            if can_open {
                let token = &raw[i..i + n];
                let close = raw[i + n..end].char_indices().find_map(|(p, _)| {
                    let p = i + n + p;
                    if !raw[p..end].starts_with(token) { return None; }
                    let run_start = raw.as_bytes()[..p].iter().rev().take_while(|&&c| c == b).count();
                    let run_end = raw[p..end].bytes().take_while(|&c| c == b).count();
                    // A single emphasis delimiter must not consume one half
                    // of an inner strong delimiter. A triple run can close
                    // both, with the inner delimiter handled recursively.
                    if n == 1 && run_start + run_end == 2 { return None; }
                    let before = raw[..p].chars().next_back();
                    let after = raw[p + n..end].chars().next();
                    (!escaped(raw, p)
                        && before.is_some_and(|c| !c.is_whitespace())
                        && !(b == b'_' && after.is_some_and(char::is_alphanumeric))
                        && (p + n == end || raw.as_bytes().get(p + n) != Some(&b)))
                        .then_some(p)
                });
                if let Some(close) = close {
                    let mut nested = style;
                    if b == b'~' { nested.strike = true; }
                    else {
                        nested.bold |= n >= 2;
                        nested.italic |= n == 1 || n == 3;
                    }
                    inline(out, raw, i + n, close, nested, depth + 1, images);
                    i = close + n;
                    continue;
                }
            }
        }
        let n = tail.chars().next().unwrap().len_utf8();
        out.append(&raw[i..i + n], i, style);
        i += n;
    }
}

fn fence(text: &str) -> Option<(u8, usize, &str)> {
    let marker = *text.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') { return None; }
    let count = text.bytes().take_while(|&b| b == marker).count();
    (count >= 3).then_some((marker, count, text[count..].trim()))
}

fn rule(text: &str) -> bool {
    let mut chars = text.chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else { return false; };
    matches!(first, '-' | '*' | '_') && chars.clone().count() >= 2 && chars.all(|c| c == first)
}

/// Inline-only projection for table cells and other block-independent text.
/// The source map is relative to `raw`, not to the containing document line.
pub(super) fn inline_line(raw: &str) -> Line {
    let mut line = Line {
        text: String::new(),
        source: vec![0],
        marks: Vec::new(),
        kind: Kind::Prose,
        indent: 0,
    };
    // Tables use this projection but do not mount native image rows, so their
    // source stays visible instead of creating an invisible cell.
    inline(&mut line, raw, 0, raw.len(), Style::default(), 0, &[]);
    *line.source.last_mut().unwrap() = raw.len();
    line
}

pub(super) fn project(lines: &[String], reveal: RangeInclusive<usize>) -> Vec<Line> {
    let mut result = Vec::with_capacity(lines.len());
    let mut code: Option<(u8, usize, String)> = None;
    let mut frontmatter = false;
    for (row, raw) in lines.iter().enumerate() {
        let indent = raw.bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
        let trimmed = &raw[indent..];
        if row == 0 && trimmed == "---" { frontmatter = true; }
        else if frontmatter && matches!(trimmed, "---" | "...") {
            frontmatter = false;
            result.push(Line::raw(raw, Kind::Frontmatter, indent));
            continue;
        }
        if frontmatter {
            result.push(Line::raw(raw, Kind::Frontmatter, indent));
            continue;
        }
        if let Some((marker, count, language)) = &code {
            let closing = fence(trimmed).is_some_and(|(m, n, info)| m == *marker && n >= *count && info.is_empty());
            if !closing {
                result.push(Line::raw(raw, Kind::Code { language: language.clone() }, indent));
                continue;
            }
            code = None;
            let mut line = Line::raw(raw, Kind::Fence, indent);
            if !reveal.contains(&row) { line.text.clear(); line.source = vec![raw.len()]; }
            result.push(line);
            continue;
        }
        if let Some((marker, count, info)) = fence(trimmed) {
            // An unfinished fence stays visible, but its interior is still code.
            let closed = lines[row + 1..].iter().any(|s| fence(s.trim_start()).is_some_and(|(m, n, rest)| m == marker && n >= count && rest.is_empty()));
            code = Some((marker, count, info.split_whitespace().next().unwrap_or("").to_owned()));
            let mut line = Line::raw(raw, Kind::Fence, indent);
            if closed && !reveal.contains(&row) { line.text.clear(); line.source = vec![raw.len()]; }
            result.push(line);
            continue;
        }
        let mut kind = Kind::Prose;
        let mut start = indent;
        let mut bullet = None;
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && (trimmed.len() == hashes || trimmed.as_bytes()[hashes].is_ascii_whitespace()) {
            kind = Kind::Heading(hashes as u8);
            start += hashes;
            while raw.as_bytes().get(start).is_some_and(u8::is_ascii_whitespace) { start += 1; }
        } else if rule(trimmed) {
            kind = Kind::Rule;
            start = raw.len();
        } else if trimmed.starts_with('>') {
            kind = Kind::Quote;
            start += 1;
            if raw.as_bytes().get(start) == Some(&b' ') { start += 1; }
        } else {
            let unordered = matches!(trimmed.as_bytes().first(), Some(b'-' | b'+' | b'*'))
                && trimmed.as_bytes().get(1).is_some_and(u8::is_ascii_whitespace);
            let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
            let ordered = digits > 0 && matches!(trimmed.as_bytes().get(digits), Some(b'.' | b')'))
                && trimmed.as_bytes().get(digits + 1).is_some_and(u8::is_ascii_whitespace);
            if unordered || ordered {
                start += if unordered { 1 } else { digits + 1 };
                while raw.as_bytes().get(start).is_some_and(u8::is_ascii_whitespace) { start += 1; }
                kind = Kind::List;
                bullet = Some(("• ", indent, start));
                let rest = &raw[start..];
                let shorthand = rest.starts_with("[]") && (rest.len() == 2 || rest.as_bytes()[2].is_ascii_whitespace());
                if shorthand || (rest.len() >= 3 && rest.as_bytes()[0] == b'[' && rest.as_bytes()[2] == b']'
                    && matches!(rest.as_bytes()[1], b' ' | b'x' | b'X')
                    && (rest.len() == 3 || rest.as_bytes()[3].is_ascii_whitespace()))
                {
                    let checked = !shorthand && rest.as_bytes()[1] != b' ';
                    kind = Kind::Task { checked, marker: start + 1 };
                    let marker = start;
                    start += if shorthand { 2 } else { 3 };
                    while raw.as_bytes().get(start).is_some_and(u8::is_ascii_whitespace) { start += 1; }
                    bullet = Some((if checked { "☑ " } else { "☐ " }, marker, start));
                }
            } else if trimmed.match_indices('|').any(|(i, _)| !escaped(trimmed, i)) {
                kind = Kind::Table;
            }
        }
        if reveal.contains(&row) {
            result.push(Line::raw(raw, kind, indent));
            continue;
        }
        let mut line = Line { text: String::new(), source: vec![0], marks: Vec::new(), kind, indent };
        line.append(&raw[..indent], 0, Style::default());
        if let Some((glyph, from, to)) = bullet { line.glyph(glyph, from, to); }
        let images = super::image::spans(raw);
        inline(&mut line, raw, start, raw.len(), Style::default(), 0, &images);
        *line.source.last_mut().unwrap() = raw.len();
        result.push(line);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(raw: &str) -> Line {
        project(&[raw.to_owned()], usize::MAX..=usize::MAX).remove(0)
    }

    fn valid(raw: &str, line: &Line) {
        assert_eq!(line.source.len(), line.text.len() + 1);
        for (i, _) in line.text.char_indices().chain(std::iter::once((line.text.len(), '\0'))) {
            assert!(raw.is_char_boundary(line.source[i]), "{i}: {:?}", line.source);
        }
        assert!(line.source.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(line.source.last(), Some(&raw.len()));
    }

    #[test]
    fn cell_projection_never_classifies_block_syntax() {
        for raw in ["# x", "---", "- item", "> quote", "", "  # x"] {
            let line = inline_line(raw);
            assert_eq!(line.kind, Kind::Prose);
            assert_eq!(line.text, raw);
            assert_eq!(line.source, (0..=raw.len()).collect::<Vec<_>>());
            valid(raw, &line);
        }
        let raw = "# **世** [é](url)";
        let line = inline_line(raw);
        assert_eq!(line.text, "# 世 é");
        assert_eq!(line.kind, Kind::Prose);
        assert!(line.marks.iter().any(|m| m.bold));
        assert!(line.marks.iter().any(|m| m.link));
        assert_eq!(line.source[line.text.find('世').unwrap()], raw.find('世').unwrap());
        valid(raw, &line);
    }

    #[test]
    fn unicode_and_formatting_offsets() {
        let raw = "## hé **世🙂** and _é_ ~~no~~ `λ`";
        let line = preview(raw);
        assert_eq!(line.text, "hé 世🙂 and é no λ");
        assert_eq!(line.kind, Kind::Heading(2));
        valid(raw, &line);
        for needle in ["hé", "世", "🙂", "é_", "no", "λ"] {
            let display = needle.trim_end_matches('_');
            let d = line.text.rfind(display).unwrap();
            assert_eq!(line.source[d], raw.find(needle).unwrap());
        }
        assert!(line.marks.iter().any(|m| m.bold));
        assert!(line.marks.iter().any(|m| m.italic));
        assert!(line.marks.iter().any(|m| m.strike));
        assert!(line.marks.iter().any(|m| m.code));
        assert!(line.marks.iter().all(|m| line.text.is_char_boundary(m.start) && line.text.is_char_boundary(m.end)));
    }

    #[test]
    fn links_wiki_and_escapes() {
        let raw = r"[hé **世**](https://x/a(b)) [[目标|别名]] [[页面]] \*literal\*";
        let line = preview(raw);
        assert_eq!(line.text, "hé 世 别名 页面 *literal*");
        valid(raw, &line);
        for label in ["hé", "世", "别名", "页面", "literal"] {
            assert_eq!(line.source[line.text.find(label).unwrap()], raw.find(label).unwrap());
        }
        assert!(line.marks.iter().any(|m| m.link && m.bold));
    }

    #[test]
    fn nested_emphasis_and_literal_punctuation() {
        for (raw, expected) in [
            ("**bold *世界***", "bold 世界"),
            ("*outer **bold** outer*", "outer bold outer"),
            ("***世界***", "世界"),
            ("snake_case_name", "snake_case_name"),
            (r"\[not a link](x) and `\*世*`", "[not a link](x) and \\*世*"),
            ("**unfinished", "**unfinished"),
        ] {
            let line = preview(raw);
            assert_eq!(line.text, expected);
            valid(raw, &line);
        }
    }

    #[test]
    fn reveal_is_identity_and_keeps_classification() {
        let lines = vec!["# **hé**".into(), "- [x] 世".into()];
        let projected = project(&lines, 0..=1);
        for (line, raw) in projected.iter().zip(&lines) {
            assert_eq!(line.text, *raw);
            assert_eq!(line.source, (0..=raw.len()).collect::<Vec<_>>());
        }
        assert_eq!(projected[0].kind, Kind::Heading(1));
        assert_eq!(projected[1].kind, Kind::Task { checked: true, marker: 3 });
        let task = preview(&lines[1]);
        assert_eq!(task.text, "☑ 世");
        valid(&lines[1], &task);
    }

    #[test]
    fn fenced_code_and_frontmatter() {
        let lines: Vec<String> = ["---", "title: 世", "---", "```rust", "**世**", "```", "after"].into_iter().map(str::to_owned).collect();
        let projected = project(&lines, 5..=5);
        assert_eq!(projected.len(), lines.len());
        assert_eq!(projected[1].kind, Kind::Frontmatter);
        assert_eq!(projected[3].kind, Kind::Fence);
        assert_eq!(projected[3].text, "");
        assert_eq!(projected[3].source, vec![7]);
        assert_eq!(projected[4].kind, Kind::Code { language: "rust".into() });
        assert_eq!(projected[4].text, "**世**");
        assert_eq!(projected[5].text, "```");
        for (raw, line) in lines.iter().zip(&projected) { valid(raw, line); }
    }
}