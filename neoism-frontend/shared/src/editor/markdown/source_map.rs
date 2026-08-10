use super::parse_markdown_link_parts;
use crate::widgets::markdown::{
    backslash_escape_at_start, parse_markdown_link, rendered_link_target,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineSourceMap {
    source_len: usize,
    visible_to_source: Vec<usize>,
    source_to_visible: Vec<usize>,
    visible_chars: Vec<char>,
}

impl InlineSourceMap {
    pub fn new(text: &str) -> Self {
        let mut builder = InlineSourceMapBuilder::new(text.len());
        build_inline_map(text, 0, &mut builder);
        builder.finish(text.len())
    }

    /// A map where every source character is visible (no markup stripping):
    /// `visible == source`. Used for the cursor's own line under Obsidian-style
    /// Live Preview, where the raw markup (`**`, `` ` ``, `[`, `]`, `#tag`, …) is
    /// shown verbatim so cursor/click/wrap math on the edited line is identity.
    pub fn identity(text: &str) -> Self {
        let mut builder = InlineSourceMapBuilder::new(text.len());
        for (byte, ch) in text.char_indices() {
            builder.push_visible_char(byte, byte + ch.len_utf8(), ch);
        }
        builder.finish(text.len())
    }

    pub fn visible_len(&self) -> usize {
        self.visible_chars.len()
    }

    pub fn source_for_visible(&self, visible_offset: usize) -> usize {
        self.visible_to_source
            .get(visible_offset.min(self.visible_len()))
            .copied()
            .unwrap_or(self.source_len)
    }

    pub fn visible_for_source(&self, source_offset: usize) -> usize {
        self.source_to_visible
            .get(source_offset.min(self.source_len))
            .copied()
            .unwrap_or_else(|| self.visible_len())
    }

    pub fn visible_char(&self, visible_offset: usize) -> Option<char> {
        self.visible_chars.get(visible_offset).copied()
    }

    pub fn visible_text(&self) -> String {
        self.visible_chars.iter().collect()
    }

    pub fn visible_prefix(&self, visible_len: usize) -> String {
        self.visible_chars
            .iter()
            .take(visible_len.min(self.visible_len()))
            .collect()
    }

    pub fn visible_range(&self, start: usize, end: usize) -> String {
        let start = start.min(self.visible_len());
        let end = end.min(self.visible_len()).max(start);
        self.visible_chars
            .iter()
            .skip(start)
            .take(end - start)
            .collect()
    }
}

struct InlineSourceMapBuilder {
    source_to_visible: Vec<usize>,
    visible_to_source: Vec<usize>,
    visible_chars: Vec<char>,
    visible: usize,
}

impl InlineSourceMapBuilder {
    fn new(source_len: usize) -> Self {
        Self {
            source_to_visible: vec![0; source_len + 1],
            visible_to_source: vec![0],
            visible_chars: Vec::new(),
            visible: 0,
        }
    }

    fn hide_range(&mut self, start: usize, end: usize) {
        for ix in
            start.min(self.source_to_visible.len())..end.min(self.source_to_visible.len())
        {
            self.source_to_visible[ix] = self.visible;
        }
        if let Some(source) = self.visible_to_source.get_mut(self.visible) {
            *source = (*source).max(end);
        }
    }

    fn assign_visible_range(&mut self, start: usize, end: usize) {
        for ix in
            start.min(self.source_to_visible.len())..end.min(self.source_to_visible.len())
        {
            self.source_to_visible[ix] = self.visible;
        }
    }

    fn push_visible_char(&mut self, source_start: usize, source_end: usize, ch: char) {
        self.assign_visible_range(source_start, source_end);
        self.visible += 1;
        self.visible_chars.push(ch);
        self.visible_to_source.push(source_end);
        if source_end < self.source_to_visible.len() {
            self.source_to_visible[source_end] = self.visible;
        }
    }

    fn finish(mut self, source_len: usize) -> InlineSourceMap {
        if self.visible_to_source.last().copied() != Some(source_len) {
            self.visible_to_source.push(source_len);
        }
        let mut last = 0;
        for value in &mut self.source_to_visible {
            if *value < last {
                *value = last;
            } else {
                last = *value;
            }
        }
        InlineSourceMap {
            source_len,
            visible_to_source: self.visible_to_source,
            source_to_visible: self.source_to_visible,
            visible_chars: self.visible_chars,
        }
    }
}

fn build_inline_map(text: &str, base: usize, builder: &mut InlineSourceMapBuilder) {
    let mut ix = 0usize;
    while ix < text.len() {
        let rest = &text[ix..];
        if let Some((escaped, consumed)) = backslash_escape_at_start(rest) {
            builder.hide_range(base + ix, base + ix + 1);
            builder.push_visible_char(base + ix + 1, base + ix + consumed, escaped);
            ix += consumed;
            continue;
        }
        if rest.starts_with("<!--") {
            if let Some(end_rel) = rest[4..].find("-->") {
                let end = ix + 4 + end_rel + 3;
                builder.hide_range(base + ix, base + end);
                ix = end;
                continue;
            }
            builder.hide_range(base + ix, base + text.len());
            break;
        }
        if let Some((source_len, letter_start, letter_end)) =
            illuminate_token_source_range(rest)
        {
            builder.hide_range(base + ix, base + ix + letter_start);
            emit_source_chars(text, ix + letter_start, ix + letter_end, base, builder);
            builder.hide_range(base + ix + letter_end, base + ix + source_len);
            ix += source_len;
            continue;
        }
        if let Some(link) = parse_markdown_link(rest) {
            if rendered_link_target(link.target).is_some() {
                let label_start = ix + 1;
                let label_end = label_start + link.label.len();
                let raw_end = ix + link.consumed;
                builder.hide_range(base + ix, base + label_start);
                emit_source_chars(text, label_start, label_end, base, builder);
                builder.hide_range(base + label_end, base + raw_end);
                ix = raw_end;
                continue;
            }
        }
        if rest.starts_with("[[") {
            let inner_start = ix + 2;
            if let Some(end_rel) = text[inner_start..].find("]] ".trim()) {
                let inner_end = inner_start + end_rel;
                let raw_end = inner_end + 2;
                let inner = &text[inner_start..inner_end];
                if let Some(label_source) = link_label_source_range(inner) {
                    builder.hide_range(base + ix, base + inner_start + label_source.0);
                    emit_source_chars(
                        text,
                        inner_start + label_source.0,
                        inner_start + label_source.1,
                        base,
                        builder,
                    );
                    builder
                        .hide_range(base + inner_start + label_source.1, base + raw_end);
                    ix = raw_end;
                    continue;
                }
                if let Some(label) = markdown_link_label_for_source_map(inner) {
                    builder.hide_range(base + ix, base + raw_end);
                    let mut source_cursor = base + inner_start;
                    for ch in label.chars() {
                        builder.push_visible_char(source_cursor, source_cursor, ch);
                        source_cursor = source_cursor.min(base + raw_end);
                    }
                    ix = raw_end;
                    continue;
                }
            }
        }
        if let Some((open, close)) = inline_marker_at(rest) {
            let inner_start = ix + open.len();
            if let Some(close_rel) = text[inner_start..].find(close) {
                let inner_end = inner_start + close_rel;
                if inner_end > inner_start {
                    builder.hide_range(base + ix, base + inner_start);
                    build_inline_map(
                        &text[inner_start..inner_end],
                        base + inner_start,
                        builder,
                    );
                    builder.hide_range(base + inner_end, base + inner_end + close.len());
                    ix = inner_end + close.len();
                    continue;
                }
            }
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        let next = ix + ch.len_utf8();
        builder.push_visible_char(base + ix, base + next, ch);
        ix = next;
    }
}

fn illuminate_token_source_range(source: &str) -> Option<(usize, usize, usize)> {
    let open_len = "::illuminate[".len();
    let rest = source.strip_prefix("::illuminate[")?;
    let close = rest.find(']')?;
    let letter_rel = rest[..close]
        .char_indices()
        .find_map(|(ix, ch)| (!ch.is_whitespace()).then_some((ix, ix + ch.len_utf8())))?;
    let mut source_len = open_len + close + 1;
    let after = &rest[close + 1..];
    if let Some(attrs_len) = illuminate_attrs_len(after) {
        source_len += attrs_len;
    }
    Some((source_len, open_len + letter_rel.0, open_len + letter_rel.1))
}

fn illuminate_attrs_len(after: &str) -> Option<usize> {
    let leading_ws = after.len().saturating_sub(after.trim_start().len());
    let rest = after.trim_start();
    let rest = rest.strip_prefix('{')?;
    Some(leading_ws + rest.find('}')? + 2)
}

fn emit_source_chars(
    full_text: &str,
    source_start: usize,
    source_end: usize,
    base: usize,
    builder: &mut InlineSourceMapBuilder,
) {
    let Some(text) = full_text.get(source_start..source_end) else {
        return;
    };
    let mut ix = source_start;
    for ch in text.chars() {
        let next = ix + ch.len_utf8();
        builder.push_visible_char(base + ix, base + next, ch);
        ix = next;
    }
}

fn link_label_source_range(inner: &str) -> Option<(usize, usize)> {
    let _ = parse_markdown_link_parts(inner)?;
    if let Some(pipe) = inner.rfind('|') {
        if pipe + 1 < inner.len() {
            return Some((pipe + 1, inner.len()));
        }
    }
    None
}

fn markdown_link_label_for_source_map(inner: &str) -> Option<String> {
    let parsed = parse_markdown_link_parts(inner)?;
    let leaf = std::path::Path::new(&parsed.target)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(parsed.target.as_str());
    Some(match (parsed.heading, parsed.line) {
        (Some(heading), _) => format!("{leaf}#{heading}"),
        (None, Some(line)) => format!("{leaf}:{line}"),
        (None, None) => leaf.to_string(),
    })
}

fn inline_marker_at(rest: &str) -> Option<(&'static str, &'static str)> {
    if rest.starts_with("**") {
        Some(("**", "**"))
    } else if rest.starts_with("__") {
        Some(("__", "__"))
    } else if rest.starts_with("~~") {
        Some(("~~", "~~"))
    } else if rest.starts_with('`') {
        Some(("`", "`"))
    } else if rest.starts_with('*') {
        Some(("*", "*"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_literal_space_offsets() {
        let map = InlineSourceMap::new("hello  world");
        assert_eq!(map.visible_len(), "hello  world".chars().count());
        assert_eq!(map.source_for_visible(7), 7);
        assert_eq!(map.visible_for_source(7), 7);
    }

    #[test]
    fn hides_closed_markers_but_keeps_inner_source() {
        let map = InlineSourceMap::new("**bold** text");
        assert_eq!(map.visible_len(), "bold text".chars().count());
        assert_eq!(map.source_for_visible(0), 2);
        assert_eq!(map.source_for_visible(4), 8);
        assert_eq!(map.visible_for_source(2), 0);
        assert_eq!(map.visible_for_source(6), 4);
    }

    #[test]
    fn leaves_unclosed_markers_literal() {
        let map = InlineSourceMap::new("**bold");
        assert_eq!(map.visible_len(), "**bold".chars().count());
        assert_eq!(map.source_for_visible(2), 2);
    }

    #[test]
    fn maps_wiki_alias_to_alias_source() {
        let text = "[[path/to/file.md|Label]] ok";
        let map = InlineSourceMap::new(text);
        assert_eq!(map.visible_len(), "Label ok".chars().count());
        assert_eq!(map.source_for_visible(0), 18);
        assert_eq!(map.source_for_visible(5), 25);
    }

    #[test]
    fn maps_bare_wiki_link_to_rendered_leaf_label() {
        let text = "[[docs/Guide.md-12]]";
        let map = InlineSourceMap::new(text);
        assert_eq!(map.visible_len(), "Guide.md:12".chars().count());
        assert_eq!(map.visible_text(), "Guide.md:12");
        assert_eq!(map.visible_prefix(5), "Guide");
        assert_eq!(map.visible_range(6, 11), "md:12");
        assert_eq!(map.visible_char(0), Some('G'));
        assert_eq!(map.visible_char(9), Some('1'));
    }

    #[test]
    fn maps_standard_web_link_to_its_label() {
        let text = "See [Search Engineer](https://jobs.example/one) now";
        let map = InlineSourceMap::new(text);
        assert_eq!(map.visible_text(), "See Search Engineer now");
        assert_eq!(map.visible_for_source(text.find('[').unwrap()), 4);
    }

    #[test]
    fn maps_standard_file_uri_link_to_its_label() {
        let text = "[Report](file:///tmp/Patriot%20Report.md)";
        assert_eq!(InlineSourceMap::new(text).visible_text(), "Report");
    }

    #[test]
    fn commonmark_backslash_escapes_hide_only_the_backslash() {
        let map = InlineSourceMap::new(r"\* note and \** partial");
        assert_eq!(map.visible_text(), "* note and ** partial");
        assert_eq!(map.source_for_visible(0), 1);
        assert_eq!(map.visible_for_source(0), 0);

        assert_eq!(InlineSourceMap::new(r"\n").visible_text(), r"\n");
    }

    #[test]
    fn maps_illuminated_token_to_letter_source() {
        let text = "::illuminate[A]{style=pirata size=3} tale";
        let map = InlineSourceMap::new(text);
        assert_eq!(map.visible_text(), "A tale");
        assert_eq!(map.source_for_visible(0), "::illuminate[".len());
        assert_eq!(map.source_for_visible(1), 36);
    }

    #[test]
    fn maps_nested_illuminated_token_with_base_offset() {
        let text = "**::illuminate[B]{lines=2}**old";
        let map = InlineSourceMap::new(text);
        assert_eq!(map.visible_text(), "Bold");
        assert_eq!(map.source_for_visible(0), 2 + "::illuminate[".len());
        assert_eq!(map.visible_for_source(2), 0);
    }
}
