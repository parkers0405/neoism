use super::*;
use crate::widgets::markdown::{parse_markdown_link, web_link_spans};

/// Layout retains byte spans in the visible label, never in the URL target.
/// Every wrapped fragment therefore opens the original, unmodified URL.
fn wrap_line(
    source: &str,
    kind: BodyLineKind,
    width: f32,
    scale: f32,
    target: Option<&str>,
    measure: &mut impl FnMut(&str, bool) -> f32,
) -> Vec<BodyLine> {
    let code = matches!(kind, BodyLineKind::Code(_));
    let mut text = String::new();
    let mut links = Vec::new();
    let mut consumed = 0;
    if let Some(target) = target {
        text.push_str(source);
        links.push(BodyLink { range: 0..text.len(), url: target.to_string() });
        consumed = source.len();
    } else if !code {
        for span in web_link_spans(source) {
            let target = parse_markdown_link(&source[span.raw_start..])
                .map_or(span.target.as_str(), |link| link.target);
            if !(target.starts_with("https://") || target.starts_with("http://"))
                || target.chars().any(|ch| ch.is_whitespace() || ch.is_control())
            {
                continue;
            }
            text.push_str(&source[consumed..span.raw_start]);
            let start = text.len();
            text.push_str(&source[span.label_start..span.label_end]);
            links.push(BodyLink { range: start..text.len(), url: target.to_string() });
            consumed = span.raw_end;
        }
    }
    text.push_str(&source[consumed..]);
    let inset = match kind {
        BodyLineKind::Bullet => (BODY_FONT_SIZE + 8.0) * scale,
        BodyLineKind::Code(_) => 16.0 * scale,
        _ => 0.0,
    };
    let width = (width - inset).max(1.0);
    let bold = kind == BodyLineKind::Heading;
    let mut rows = Vec::new();
    let mut start = 0;
    while start < text.len() {
        if !code {
            start += text[start..].len() - text[start..].trim_start().len();
        }
        if start == text.len() {
            break;
        }
        let rest = &text[start..];
        let boundaries: Vec<usize> = rest.char_indices().map(|(byte, _)| byte)
            .chain(std::iter::once(rest.len())).collect();
        // Binary search character boundaries, including oversize URL tokens.
        let mut lo = 0;
        let mut hi = boundaries.len() - 1;
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if measure_range(&text, &links, start..start + boundaries[mid], bold, measure) <= width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let mut end = boundaries[lo.max(1)];
        if end < rest.len() && !code {
            if let Some((byte, _)) = rest[..end].char_indices()
                .rev().find(|(byte, ch)| *byte > 0 && ch.is_whitespace())
            {
                end = byte;
            }
        }
        let row_text = if code { &rest[..end] } else { rest[..end].trim_end() };
        let row_end = start + row_text.len();
        let mut row = BodyLine::new(row_text.to_string(), kind);
        row.continuation = !rows.is_empty();
        row.links = links.iter().filter_map(|link| {
            let from = link.range.start.max(start);
            let to = link.range.end.min(row_end);
            (from < to).then(|| BodyLink {
                range: from - start..to - start,
                url: link.url.clone(),
            })
        }).collect();
        rows.push(row);
        start += end;
    }
    if rows.is_empty() {
        rows.push(BodyLine::new(String::new(), kind));
    }
    rows
}

fn measure_range(
    text: &str,
    links: &[BodyLink],
    range: std::ops::Range<usize>,
    bold: bool,
    measure: &mut impl FnMut(&str, bool) -> f32,
) -> f32 {
    let mut width = 0.0;
    let mut byte = range.start;
    for link in links {
        let from = link.range.start.max(range.start);
        let to = link.range.end.min(range.end);
        if from < to {
            width += measure(&text[byte..from], bold) + measure(&text[from..to], bold);
            byte = to;
        }
    }
    width + measure(&text[byte..range.end], bold)
}

pub(super) fn link_lines(
    label: &str,
    url: &str,
    width: f32,
    scale: f32,
    mut measure: impl FnMut(&str, bool) -> f32,
) -> Vec<BodyLine> {
    wrap_line(label, BodyLineKind::Text, width, scale, Some(url), &mut measure)
}

pub(super) fn layout_lines(
    source: &str,
    width: f32,
    scale: f32,
    invite_url: Option<&str>,
    mut measure: impl FnMut(&str, bool) -> f32,
) -> Vec<BodyLine> {
    let mut out = Vec::new();
    let mut code_info: Option<String> = None;
    let mut code_lines = Vec::new();
    for raw in source.lines() {
        let trimmed = raw.trim();
        if let Some(info) = trimmed.strip_prefix("```").or_else(|| trimmed.strip_prefix("~~~")) {
            if let Some(info) = code_info.take() {
                append_code(&mut out, &code_lines, &info, width, scale, &mut measure);
                code_lines.clear();
            } else {
                code_info = Some(info.trim().to_string());
            }
            continue;
        }
        if code_info.is_some() {
            code_lines.push(raw.to_string());
            continue;
        }
        if invite_url == Some(trimmed) {
            // Invite URLs have a distinct copy affordance, not a browser action.
            out.push(BodyLine::new(trimmed.to_string(), BodyLineKind::Text));
            continue;
        }
        let (kind, content) = if let Some(heading) = markdown_heading(raw) {
            (BodyLineKind::Heading, heading)
        } else if let Some(bullet) = markdown_bullet(raw) {
            (BodyLineKind::Bullet, bullet)
        } else {
            (BodyLineKind::Text, trimmed)
        };
        out.extend(wrap_line(content, kind, width, scale, None, &mut measure));
    }
    if let Some(info) = code_info {
        append_code(&mut out, &code_lines, &info, width, scale, &mut measure);
    }
    out
}

fn append_code(
    out: &mut Vec<BodyLine>,
    lines: &[String],
    info: &str,
    width: f32,
    scale: f32,
    measure: &mut impl FnMut(&str, bool) -> f32,
) {
    if info.eq_ignore_ascii_case("mermaid") {
        let source = lines.join("\n");
        if let Some(diagram) = parse_mermaid_diagram(&source) {
            let mut line = BodyLine::new(source, BodyLineKind::Mermaid);
            line.mermaid = Some(diagram);
            line.row_span = 8;
            out.push(line);
            return;
        }
    }
    for line in lines {
        out.extend(wrap_line(line, BodyLineKind::Code(lang_from_fence(info)), width, scale, None, measure));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(text: &str, bold: bool) -> f32 {
        text.chars().map(|ch| if ch == 'W' { 12.0 } else { 6.0 }).sum::<f32>()
            * if bold { 1.1 } else { 1.0 }
    }

    #[test]
    fn oauth_url_wraps_without_markdown_syntax_or_target_loss() {
        let url = format!("https://vercel.com/oauth/authorize?state={}&next=(callback)&code=a%2Bb#finish.", "x".repeat(240));
        let source = format!("Opened your browser to authorize this MCP server.\n\nAuthorization URL:\n[{url}]({url})");
        for scale in [0.75, 1.0, 2.0, 3.0] {
            for width in [160.0, 330.0, 520.0] {
                let rows = layout_lines(&source, width, scale, None, |text, bold| measure(text, bold) * scale);
                assert!(rows.iter().all(|row| measure(&row.text, false) * scale <= width));
                let fragments: Vec<_> = rows.iter().filter(|row| !row.links.is_empty()).collect();
                assert!(fragments.len() > 1);
                let mut visible_url = String::new();
                for row in fragments {
                    for link in &row.links {
                        assert_eq!(link.url, url);
                        visible_url.push_str(&row.text[link.range.clone()]);
                    }
                }
                assert_eq!(visible_url, url);
            }
        }
    }

    #[test]
    fn mixed_links_and_unicode_labels_keep_visible_byte_ranges() {
        let rows = layout_lines("Before [café 世界 W](https://example.com/a?x=1%2B2) after https://example.org/path.", 90.0, 1.0, None, measure);
        for row in &rows {
            assert!(measure(&row.text, false) <= 90.0);
            for link in &row.links {
                assert!(!row.text[link.range.clone()].is_empty());
            }
        }
        let label = rows.iter().flat_map(|row| row.links.iter().map(move |link| (row, link)))
            .filter(|(_, link)| link.url.contains("example.com"))
            .map(|(row, link)| &row.text[link.range.clone()]).collect::<Vec<_>>().join("");
        assert_eq!(label.replace(' ', ""), "café世界W");
        assert!(rows.iter().any(|row| row.links.iter().any(|link| link.url == "https://example.org/path")));
    }

    #[test]
    fn headings_bullets_and_code_use_their_actual_width_budget() {
        let rows = layout_lines("# WWWWWWWWWWWWWWWW\n- a longer bullet that wraps\n```text\n  https://example.com/a-long-code-token\n\n```", 90.0, 1.0, None, measure);
        for row in &rows {
            let inset = match row.kind {
                BodyLineKind::Bullet => BODY_FONT_SIZE + 8.0,
                BodyLineKind::Code(_) => 16.0,
                _ => 0.0,
            };
            assert!(measure(&row.text, row.kind == BodyLineKind::Heading) <= 90.0 - inset);
            if matches!(row.kind, BodyLineKind::Code(_)) {
                assert!(row.links.is_empty());
            }
        }
        assert!(rows.iter().any(|row| row.kind == BodyLineKind::Bullet && row.continuation));
        assert!(rows.iter().any(|row| matches!(row.kind, BodyLineKind::Code(_)) && row.text.is_empty()));
    }

    #[test]
    fn invite_keeps_distinct_copy_behavior_and_unsafe_links_are_not_active() {
        let url = "https://example.com/join?token=a%2Bb#invite";
        let rows = layout_lines(url, 50.0, 1.0, Some(url), measure);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, url);
        assert!(rows[0].links.is_empty());
        let rows = layout_lines("[bad](javascript:alert(1))", 300.0, 1.0, None, measure);
        assert!(rows.iter().all(|row| row.links.is_empty()));
    }
}
