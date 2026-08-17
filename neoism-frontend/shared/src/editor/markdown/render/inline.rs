use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::editor::markdown::{
    parse_markdown_link_parts, source_map::InlineSourceMap, MarkdownPane,
};

use super::types::{InlineTag, InlineWikiLink, DEPTH, ORDER_TEXT};
use crate::editor::markdown::render::draw::{
    cursor_cell_width, cursor_position_for_text_prefix, draw_if_visible,
    draw_rect_clipped, rects_intersect,
};
use crate::primitives::ide_theme::IdeTheme;
use crate::widgets::markdown::web_link_spans;

// Pure spellcheck primitives live next door in the sibling
// `spellcheck` module of the lifted state crate. Native callers
// (`screen/bridges/markdown.rs`) reach them via
// `crate::editor::markdown::render::{is_misspelled_word, spelling_suggestions}`
// which the parent `render/mod.rs` re-exports; the tests in
// `render/mod.rs` reach `normalized_spellcheck_word` + `spellcheck_words`
// via `super::inline::*`.
#[allow(unused_imports)]
pub use crate::editor::markdown::{
    is_misspelled_word, normalized_spellcheck_word, spellcheck_dictionary,
    spellcheck_words, spelling_suggestions, SpellcheckWord,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_spellcheck_underlines(
    sugarloaf: &mut Sugarloaf,
    text_x: f32,
    text_y: f32,
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    occlusions: &[[f32; 4]],
    text: &str,
) {
    if spellcheck_dictionary().is_none() || text.is_empty() {
        return;
    }

    for word in spellcheck_words(text) {
        if !is_misspelled_word(word.text) {
            continue;
        }
        let Some(prefix) = text.get(..word.start) else {
            continue;
        };
        let (x, y) = cursor_position_for_text_prefix(
            sugarloaf, text_x, text_y, line_h, wrap_width, opts, text, prefix,
        );
        let word_w = sugarloaf.text_mut().measure(word.text, opts);
        let remaining_w = (text_x + wrap_width - x).max(0.0);
        if word_w <= 0.0 || word_w > remaining_w + 1.0 {
            continue;
        }
        let underline_y = y + line_h - 4.0;
        if underline_y < clip_top || underline_y > clip_bottom {
            continue;
        }
        draw_spellcheck_squiggle_visible(
            sugarloaf,
            clip,
            x,
            underline_y,
            word_w,
            theme,
            occlusions,
        );
    }
}

pub(super) fn draw_spellcheck_squiggle_visible(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    x: f32,
    y: f32,
    w: f32,
    theme: &IdeTheme,
    occlusions: &[[f32; 4]],
) {
    let underline = [x, y, w, 4.0];
    let visible = spellcheck_visible_spans(underline, occlusions);
    for (start, end) in visible {
        let start = start.max(clip[0]);
        let end = end.min(clip[0] + clip[2]);
        if end <= start {
            continue;
        }
        let segment_clip = [start, clip[1], end - start, clip[3]];
        draw_spellcheck_squiggle(sugarloaf, segment_clip, x, y, w, theme);
    }
}

fn spellcheck_visible_spans(
    underline: [f32; 4],
    occlusions: &[[f32; 4]],
) -> Vec<(f32, f32)> {
    let mut visible = vec![(underline[0], underline[0] + underline[2])];
    for cover in occlusions {
        if !rects_intersect(underline, *cover) {
            continue;
        }
        let cover_start = cover[0];
        let cover_end = cover[0] + cover[2];
        let mut next = Vec::with_capacity(visible.len() + 1);
        for (start, end) in visible {
            if cover_end <= start || cover_start >= end {
                next.push((start, end));
                continue;
            }
            if cover_start > start {
                next.push((start, cover_start.min(end)));
            }
            if cover_end < end {
                next.push((cover_end.max(start), end));
            }
        }
        visible = next;
        if visible.is_empty() {
            return visible;
        }
    }
    visible
}

pub(super) fn draw_spellcheck_squiggle(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    x: f32,
    y: f32,
    w: f32,
    theme: &IdeTheme,
) {
    let mut dx = 0.0;
    let mut high = true;
    while dx < w {
        let segment_w = 2.4_f32.min(w - dx).max(0.0);
        if segment_w <= 0.0 {
            break;
        }
        draw_rect_clipped(
            sugarloaf,
            clip,
            x + dx,
            y + if high { 0.0 } else { 2.0 },
            segment_w,
            1.3,
            theme.f32_alpha(theme.red, 0.9),
            DEPTH,
            ORDER_TEXT + 2,
        );
        high = !high;
        dx += 3.2;
    }
}

#[cfg(test)]
mod spellcheck_occlusion_tests {
    use super::spellcheck_visible_spans;

    #[test]
    fn partial_overlay_only_clips_the_covered_squiggle_segment() {
        assert_eq!(
            spellcheck_visible_spans(
                [10.0, 20.0, 100.0, 4.0],
                &[[40.0, 18.0, 30.0, 20.0]],
            ),
            vec![(10.0, 40.0), (70.0, 110.0)]
        );
    }

    #[test]
    fn unrelated_overlay_does_not_clip_the_squiggle() {
        assert_eq!(
            spellcheck_visible_spans(
                [10.0, 20.0, 100.0, 4.0],
                &[[40.0, 50.0, 30.0, 20.0]],
            ),
            vec![(10.0, 110.0)]
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_inline_links_for_line(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    line: &str,
    text_x: f32,
    text_y: f32,
    marker_len: usize,
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    occlusions: &[[f32; 4]],
    active_col: Option<usize>,
) {
    let marker_len = marker_len.min(line.len());
    let Some(visible) = line.get(marker_len..) else {
        return;
    };
    let links = collect_inline_links(visible);
    let tags = collect_inline_tags(visible, &links);
    if links.is_empty() && tags.is_empty() {
        return;
    }
    let clean_visible = clean_inline_with_active_link(visible, active_col);
    let mut link_opts = opts.clone();
    link_opts.color = theme.u8(theme.blue);
    for link in &links {
        if active_col.is_some_and(|col| col > link.raw_start && col < link.raw_end) {
            continue;
        }
        let Some(prefix_source) = visible.get(..link.raw_start) else {
            continue;
        };
        let prefix_active_col = active_col.filter(|col| *col <= prefix_source.len());
        let prefix = clean_inline_with_active_link(prefix_source, prefix_active_col);
        let (link_x, link_y) = cursor_position_for_text_prefix(
            sugarloaf,
            text_x,
            text_y,
            line_h,
            wrap_width,
            opts,
            &clean_visible,
            &prefix,
        );
        let link_w = sugarloaf
            .text_mut()
            .measure(&link.label, &link_opts)
            .max(cursor_cell_width(opts));
        draw_if_visible(
            sugarloaf,
            link_x,
            link_y,
            &link.label,
            &link_opts,
            clip_top,
            clip_bottom,
            occlusions,
        );
        draw_rect_clipped(
            sugarloaf,
            clip,
            link_x,
            link_y + line_h - 3.0,
            link_w,
            1.4,
            theme.f32_alpha(theme.blue, 0.92),
            DEPTH,
            ORDER_TEXT + 1,
        );
        if let Some(target) = pane.resolve_markdown_link(&link.inner) {
            pane.register_link_rect([link_x, link_y, link_w, line_h], target);
        }
    }
    let mut tag_opts = opts.clone();
    tag_opts.color = theme.u8(theme.green);
    tag_opts.bold = true;
    for tag in tags {
        let Some(prefix_source) = visible.get(..tag.raw_start) else {
            continue;
        };
        let prefix_active_col = active_col.filter(|col| *col <= prefix_source.len());
        let prefix = clean_inline_with_active_link(prefix_source, prefix_active_col);
        let (tag_x, tag_y) = cursor_position_for_text_prefix(
            sugarloaf,
            text_x,
            text_y,
            line_h,
            wrap_width,
            opts,
            &clean_visible,
            &prefix,
        );
        let tag_w = sugarloaf
            .text_mut()
            .measure(&tag.label, &tag_opts)
            .max(cursor_cell_width(opts));
        draw_if_visible(
            sugarloaf,
            tag_x,
            tag_y,
            &tag.label,
            &tag_opts,
            clip_top,
            clip_bottom,
            occlusions,
        );
        draw_rect_clipped(
            sugarloaf,
            clip,
            tag_x,
            tag_y + line_h - 3.0,
            tag_w,
            1.2,
            theme.f32_alpha(theme.green, 0.75),
            DEPTH,
            ORDER_TEXT + 1,
        );
    }
}

pub(super) fn collect_inline_links(text: &str) -> Vec<InlineWikiLink> {
    let mut links = Vec::new();
    let mut search_from = 0;
    while let Some(start_rel) = text[search_from..].find("[[") {
        let start = search_from + start_rel;
        let inner_start = start + 2;
        let Some(end_rel) = text[inner_start..].find("]]") else {
            break;
        };
        let end = inner_start + end_rel;
        let raw_end = end + 2;
        let inner = &text[inner_start..end];
        if let Some(label) = markdown_link_label(inner) {
            links.push(InlineWikiLink {
                raw_start: start,
                raw_end,
                inner: inner.to_string(),
                label,
            });
        }
        search_from = raw_end;
    }
    let web_links: Vec<_> = web_link_spans(text)
        .into_iter()
        .filter(|web| {
            !links
                .iter()
                .any(|wiki| web.raw_start < wiki.raw_end && web.raw_end > wiki.raw_start)
        })
        .map(|link| InlineWikiLink {
            raw_start: link.raw_start,
            raw_end: link.raw_end,
            inner: link.target,
            label: text[link.label_start..link.label_end].to_string(),
        })
        .collect();
    links.extend(web_links);
    links.sort_by_key(|link| link.raw_start);
    links
}

pub(super) fn collect_inline_tags(
    text: &str,
    links: &[InlineWikiLink],
) -> Vec<InlineTag> {
    let bytes = text.as_bytes();
    let mut tags = Vec::new();
    let mut ix = 0usize;
    while ix < bytes.len() {
        if bytes[ix] != b'#'
            || links
                .iter()
                .any(|link| ix >= link.raw_start && ix < link.raw_end)
        {
            ix += 1;
            continue;
        }
        let prev_ok = ix == 0 || !is_tag_char(bytes[ix - 1] as char);
        let mut end = ix + 1;
        while end < bytes.len() && is_tag_char(bytes[end] as char) {
            end += 1;
        }
        if prev_ok && end > ix + 1 {
            tags.push(InlineTag {
                raw_start: ix,
                label: text[ix..end].to_string(),
            });
        }
        ix = end.max(ix + 1);
    }
    tags
}

fn is_tag_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/')
}

pub(super) fn markdown_link_label(inner: &str) -> Option<String> {
    let parsed = parse_markdown_link_parts(inner)?;
    if let Some(alias) = parsed.alias {
        if !alias.is_empty() {
            return Some(alias);
        }
    }
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

pub(super) fn clean_inline_with_active_link(
    text: &str,
    _active_col: Option<usize>,
) -> String {
    InlineSourceMap::new(text).visible_text()
}
