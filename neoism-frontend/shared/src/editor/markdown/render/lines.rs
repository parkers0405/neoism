//! Editor-pane wrapper over the shared markdown widget.
//!
//! Block parsing, list-marker arithmetic, and code-fence walks now live in
//! `chrome::widgets::markdown`. This module translates the widget's
//! `MarkdownBlockKind` into the editor's `RenderLineKind` (which carries
//! editor-only metadata like `list_marker` on ordered items) and keeps a
//! couple of neighbor-aware helpers the renderer needs.

use crate::widgets::markdown as md;

use super::types::{ParsedRenderLine, RenderLineKind};

pub(super) fn is_same_paragraph_neighbor(
    lines: &[ParsedRenderLine<'_>],
    line_ix: usize,
    offset: isize,
) -> bool {
    let Some(neighbor_ix) = line_ix.checked_add_signed(offset) else {
        return false;
    };
    lines
        .get(neighbor_ix)
        .is_some_and(|line| matches!(line.kind, RenderLineKind::Paragraph))
}

pub(super) fn heading_section_tasks_complete(
    lines: &[ParsedRenderLine<'_>],
    heading_ix: usize,
    heading_level: u8,
) -> bool {
    let mut saw_task = false;
    for line in lines.iter().skip(heading_ix + 1) {
        match line.kind {
            RenderLineKind::Heading(level) if level <= heading_level => break,
            RenderLineKind::Task { checked, .. } => {
                saw_task = true;
                if !checked {
                    return false;
                }
            }
            _ => {}
        }
    }
    saw_task
}

pub(super) fn is_closing_code_fence(
    lines: &[ParsedRenderLine<'_>],
    line_ix: usize,
) -> bool {
    lines[..line_ix]
        .iter()
        .filter(|line| matches!(line.kind, RenderLineKind::CodeFence))
        .count()
        % 2
        == 1
}

/// Parse one editor line. Delegates to the shared widget and translates the
/// widget's kind enum into the editor's variant that carries `list_marker`.
pub(super) fn parse_render_line(line: &str, in_code: bool) -> ParsedRenderLine<'_> {
    let parsed = md::parse_line(line, in_code);
    let kind = match parsed.kind {
        md::MarkdownBlockKind::Empty => RenderLineKind::Empty,
        md::MarkdownBlockKind::Heading(level) => RenderLineKind::Heading(level),
        md::MarkdownBlockKind::Paragraph => RenderLineKind::Paragraph,
        md::MarkdownBlockKind::Task { checked, depth } => {
            RenderLineKind::Task { checked, depth }
        }
        md::MarkdownBlockKind::Bullet { depth } => RenderLineKind::Bullet { depth },
        md::MarkdownBlockKind::Ordered { depth } => RenderLineKind::Ordered { depth },
        md::MarkdownBlockKind::CodeFence => RenderLineKind::CodeFence,
        md::MarkdownBlockKind::Code => RenderLineKind::Code,
        md::MarkdownBlockKind::Callout { kind } => RenderLineKind::Callout { kind },
        md::MarkdownBlockKind::Quote => RenderLineKind::Quote,
        md::MarkdownBlockKind::Divider => RenderLineKind::Divider,
    };
    ParsedRenderLine {
        kind,
        text: parsed.text,
        marker_len: parsed.marker_len,
        list_marker: parsed.list_marker,
    }
}

pub(super) fn code_block_end(lines: &[String], start: usize) -> usize {
    md::code_block_end(lines, start)
}
