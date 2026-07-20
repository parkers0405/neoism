//! Full diagnostic detail card shown from an inline diagnostic lens.
//!
//! The same-row lens deliberately stays compact. This card is the lossless
//! surface: it wraps the complete message (never replacing text with an
//! ellipsis), identifies the producer and source range, and exposes the
//! Quick Fix entry point when the user pins it.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::ide_theme::IdeTheme;

use super::inline_diagnostics::InlineDiagnosticSeverity;

const DEPTH: f32 = 0.022;
const ORDER: u8 = 44;
const MAX_WIDTH_LOGICAL: f32 = 620.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticDetailContent {
    pub severity: InlineDiagnosticSeverity,
    /// Complete, unmodified diagnostic message.
    pub message: String,
    pub source: Option<String>,
    /// One-based source line and zero-based source column.
    pub line: u64,
    pub column: u32,
    /// Zero-based exclusive range end from LSP.
    pub end_line: u64,
    pub end_column: u32,
    pub code: Option<String>,
    pub code_description: Option<String>,
    pub tags: Vec<String>,
    pub related_information: Vec<DiagnosticDetailRelatedInformation>,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticDetailRelatedInformation {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DiagnosticDetailGeometry {
    pub panel_rect: [f32; 4],
    pub quick_fix_rect: Option<[f32; 4]>,
}

#[derive(Clone, Copy, Debug)]
pub struct DiagnosticDetailLayout {
    /// Logical-pixel position of the diagnostic lens.
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub cell_h: f32,
    pub window_w: f32,
    pub window_h: f32,
    pub scale: f32,
}

pub fn render(
    sugarloaf: &mut Sugarloaf,
    content: &DiagnosticDetailContent,
    layout: DiagnosticDetailLayout,
    theme: &IdeTheme,
) -> DiagnosticDetailGeometry {
    if content.message.trim().is_empty()
        || layout.window_w <= 0.0
        || layout.window_h <= 0.0
    {
        return DiagnosticDetailGeometry::default();
    }

    let s = layout.scale.clamp(0.5, 3.0);
    let font_size = (12.0 * s).clamp(10.0, 18.0);
    let line_h = font_size + 5.0 * s;
    let pad = 10.0 * s;
    let severity_color = match content.severity {
        InlineDiagnosticSeverity::Error => theme.red,
        InlineDiagnosticSeverity::Warn => theme.yellow,
    };
    let body_opts = DrawOpts {
        font_size,
        color: theme.u8(theme.fg),
        ..DrawOpts::default()
    };
    let dim_opts = DrawOpts {
        color: theme.u8(theme.dim),
        ..body_opts
    };
    let heading_opts = DrawOpts {
        color: theme.u8(severity_color),
        bold: true,
        ..body_opts
    };

    let max_text_w = (MAX_WIDTH_LOGICAL * s)
        .min(layout.window_w - 2.0 * pad - 16.0)
        .max(120.0);
    let body_lines =
        wrap_complete_message(sugarloaf, &content.message, max_text_w, &body_opts);
    if body_lines.is_empty() {
        return DiagnosticDetailGeometry::default();
    }

    let severity = match content.severity {
        InlineDiagnosticSeverity::Error => "Error",
        InlineDiagnosticSeverity::Warn => "Warning",
    };
    let start_column = content.column.saturating_add(1);
    let end_line_one_based = content.end_line.saturating_add(1);
    let has_end = end_line_one_based > content.line
        || (end_line_one_based == content.line && content.end_column > content.column);
    let location = if !has_end {
        format!("{}:{start_column}", content.line)
    } else if end_line_one_based == content.line {
        format!(
            "{}:{start_column}-{}",
            content.line,
            content.end_column.saturating_add(1)
        )
    } else {
        format!(
            "{}:{start_column}-{}:{}",
            content.line,
            end_line_one_based,
            content.end_column.saturating_add(1)
        )
    };
    let heading = format!("{severity} at {location}");
    let source = content
        .source
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .map(ToOwned::to_owned);
    let code = content
        .code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty());
    let producer = match (source.as_deref(), code) {
        (Some(source), Some(code)) => Some(format!("Source: {source}    Code: {code}")),
        (Some(source), None) => Some(format!("Source: {source}")),
        (None, Some(code)) => Some(format!("Code: {code}")),
        (None, None) => None,
    };
    let mut metadata_lines = Vec::new();
    if let Some(producer) = producer {
        metadata_lines.extend(wrap_complete_message(
            sugarloaf, &producer, max_text_w, &dim_opts,
        ));
    }
    if !content.tags.is_empty() {
        metadata_lines.extend(wrap_complete_message(
            sugarloaf,
            &format!("Tags: {}", content.tags.join(", ")),
            max_text_w,
            &dim_opts,
        ));
    }
    if let Some(description) = content
        .code_description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        metadata_lines.extend(wrap_complete_message(
            sugarloaf,
            &format!("More: {description}"),
            max_text_w,
            &dim_opts,
        ));
    }
    let mut related_lines = Vec::new();
    for related in &content.related_information {
        let related_location = format!(
            "{}:{}:{}",
            related.path,
            related.line.saturating_add(1),
            related.column.saturating_add(1)
        );
        let line = if related.message.trim().is_empty() {
            format!("- {related_location}")
        } else {
            format!("- {related_location} — {}", related.message.trim())
        };
        related_lines.extend(wrap_complete_message(
            sugarloaf, &line, max_text_w, &body_opts,
        ));
    }
    let quick_fix_label = "Quick Fixes    Ctrl/Cmd+.";

    let mut content_w = sugarloaf
        .overlay_text_mut()
        .measure(&heading, &heading_opts);
    for line in &metadata_lines {
        content_w = content_w.max(sugarloaf.overlay_text_mut().measure(line, &dim_opts));
    }
    for line in &body_lines {
        content_w = content_w.max(sugarloaf.overlay_text_mut().measure(line, &body_opts));
    }
    for line in &related_lines {
        content_w = content_w.max(sugarloaf.overlay_text_mut().measure(line, &body_opts));
    }
    if content.pinned {
        content_w = content_w.max(
            sugarloaf
                .overlay_text_mut()
                .measure(quick_fix_label, &heading_opts),
        );
    } else {
        content_w = content_w.max(
            sugarloaf
                .overlay_text_mut()
                .measure("Click diagnostic to pin", &dim_opts),
        );
    }

    let metadata_rows = metadata_lines.len();
    let related_heading_rows = usize::from(!related_lines.is_empty());
    let related_rows = related_lines.len();
    let footer_rows = 1usize;
    let separator_h = 1.0_f32.max(s);
    let box_w = (content_w + pad * 2.0)
        .min(layout.window_w - 16.0)
        .max(160.0 * s);
    let box_h = pad * 2.0
        + (1 + metadata_rows
            + body_lines.len()
            + related_heading_rows
            + related_rows
            + footer_rows) as f32
            * line_h
        + (2 + usize::from(!related_lines.is_empty())) as f32 * (separator_h + 4.0 * s);

    let mut box_x = layout.anchor_x;
    if box_x + box_w > layout.window_w - 8.0 {
        box_x = (layout.window_w - box_w - 8.0).max(8.0);
    }
    let below_y = layout.anchor_y + layout.cell_h + 3.0 * s;
    let box_y = if below_y + box_h > layout.window_h - 8.0 {
        (layout.anchor_y - box_h - 3.0 * s).max(8.0)
    } else {
        below_y
    };
    let panel_rect = [box_x, box_y, box_w, box_h];

    sugarloaf.overlay_rounded_rect(
        box_x,
        box_y,
        box_w,
        box_h,
        theme.f32_alpha(theme.panel_bg(), 1.0),
        DEPTH,
        7.0 * s,
        ORDER,
    );
    sugarloaf.overlay_rounded_rect(
        box_x,
        box_y,
        3.0 * s,
        box_h,
        theme.f32_alpha(severity_color, 0.92),
        DEPTH,
        7.0 * s,
        ORDER + 1,
    );

    let clip = Some([
        box_x + pad,
        box_y + pad,
        (box_w - pad * 2.0).max(0.0),
        (box_h - pad * 2.0).max(0.0),
    ]);
    let mut y = box_y + pad;
    sugarloaf.overlay_text_mut().draw(
        box_x + pad,
        y,
        &heading,
        &DrawOpts {
            clip_rect: clip,
            ..heading_opts
        },
    );
    y += line_h;
    for line in &metadata_lines {
        sugarloaf.overlay_text_mut().draw(
            box_x + pad,
            y,
            line,
            &DrawOpts {
                clip_rect: clip,
                ..dim_opts
            },
        );
        y += line_h;
    }

    sugarloaf.overlay_rounded_rect(
        box_x + pad,
        y,
        (box_w - pad * 2.0).max(0.0),
        separator_h,
        theme.f32_alpha(theme.border, 0.72),
        DEPTH,
        0.0,
        ORDER + 1,
    );
    y += separator_h + 3.0 * s;
    for line in &body_lines {
        sugarloaf.overlay_text_mut().draw(
            box_x + pad,
            y,
            line,
            &DrawOpts {
                clip_rect: clip,
                ..body_opts
            },
        );
        y += line_h;
    }

    if !related_lines.is_empty() {
        y += 2.0 * s;
        sugarloaf.overlay_rounded_rect(
            box_x + pad,
            y,
            (box_w - pad * 2.0).max(0.0),
            separator_h,
            theme.f32_alpha(theme.border, 0.72),
            DEPTH,
            0.0,
            ORDER + 1,
        );
        y += separator_h + 4.0 * s;
        sugarloaf.overlay_text_mut().draw(
            box_x + pad,
            y,
            "Related locations",
            &DrawOpts {
                clip_rect: clip,
                ..heading_opts
            },
        );
        y += line_h;
        for line in &related_lines {
            sugarloaf.overlay_text_mut().draw(
                box_x + pad,
                y,
                line,
                &DrawOpts {
                    clip_rect: clip,
                    ..body_opts
                },
            );
            y += line_h;
        }
    }

    y += 2.0 * s;
    sugarloaf.overlay_rounded_rect(
        box_x + pad,
        y,
        (box_w - pad * 2.0).max(0.0),
        separator_h,
        theme.f32_alpha(theme.border, 0.72),
        DEPTH,
        0.0,
        ORDER + 1,
    );
    y += separator_h + 4.0 * s;

    let quick_fix_rect = if content.pinned {
        let rect = [
            box_x + pad,
            y - 3.0 * s,
            (box_w - pad * 2.0).max(0.0),
            line_h + 5.0 * s,
        ];
        sugarloaf.overlay_rounded_rect(
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            theme.f32_alpha(severity_color, 0.14),
            DEPTH,
            4.0 * s,
            ORDER + 1,
        );
        sugarloaf.overlay_text_mut().draw(
            box_x + pad + 6.0 * s,
            y,
            quick_fix_label,
            &DrawOpts {
                clip_rect: clip,
                ..heading_opts
            },
        );
        Some(rect)
    } else {
        sugarloaf.overlay_text_mut().draw(
            box_x + pad,
            y,
            "Click diagnostic to pin",
            &DrawOpts {
                clip_rect: clip,
                ..dim_opts
            },
        );
        None
    };

    DiagnosticDetailGeometry {
        panel_rect,
        quick_fix_rect,
    }
}

/// Wrap every paragraph to the measured width while retaining every source
/// character. Unlike the compact lens, this function never appends an
/// ellipsis or drops tail text.
fn wrap_complete_message(
    sugarloaf: &mut Sugarloaf,
    message: &str,
    max_width: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in message.lines() {
        if paragraph.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for ch in paragraph.chars() {
            let mut candidate = line.clone();
            candidate.push(ch);
            if !line.is_empty()
                && sugarloaf.overlay_text_mut().measure(&candidate, opts) > max_width
            {
                out.push(std::mem::take(&mut line));
            }
            line.push(ch);
        }
        out.push(line);
    }
    out
}

pub fn rect_contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
}

#[cfg(test)]
mod tests {
    use super::rect_contains;

    #[test]
    fn detail_hit_rect_includes_edges_and_rejects_outside() {
        let rect = [10.0, 20.0, 100.0, 40.0];
        assert!(rect_contains(rect, 10.0, 20.0));
        assert!(rect_contains(rect, 110.0, 60.0));
        assert!(!rect_contains(rect, 9.9, 20.0));
        assert!(!rect_contains(rect, 10.0, 60.1));
    }
}
