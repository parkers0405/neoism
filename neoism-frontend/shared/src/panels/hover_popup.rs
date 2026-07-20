//! VS Code-style hover doc popup — a floating box anchored under the editor
//! cell the mouse is resting on. Fed by the Neoism LSP engine's `hover` result
//! (`EditorServerMessage::LspHoverResult`). Drawn on the overlay layer so it
//! composites above editor content, mirroring `inline_diagnostics`.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::ide_theme::IdeTheme;
use crate::syntax::{highlight_line, syn_color, Lang};

const DEPTH: f32 = 0.02;
const ORDER: u8 = 40;
const MAX_WIDTH_LOGICAL: f32 = 640.0;
const MAX_RENDERED_LINES: usize = 18;

/// Placement inputs, all in LOGICAL pixels (the caller divides physical geometry
/// by `scale_factor` before handing it over, matching the overlay convention).
pub struct HoverPopupLayout {
    /// Left edge of the anchored cell.
    pub anchor_x: f32,
    /// Top edge of the anchored cell.
    pub anchor_y: f32,
    /// Cell height, so the box sits just below the hovered cell.
    pub cell_h: f32,
    /// Window bounds, for clamping horizontally and flipping above/below.
    pub window_w: f32,
    pub window_h: f32,
    /// Chrome scale, for font/padding sizing.
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoverLineKind {
    Text,
    Heading,
    Bullet,
    Code(Lang),
}

#[derive(Clone, Debug)]
struct HoverLine {
    text: String,
    kind: HoverLineKind,
}

#[derive(Clone, Debug)]
struct RenderedHoverLine {
    text: String,
    kind: HoverLineKind,
}

/// Draw the hover box. `lines` are markdown-ish hover lines; fenced code blocks
/// are rendered with the shared Neoism syntax colors.
pub fn render(
    sugarloaf: &mut Sugarloaf,
    lines: &[String],
    layout: HoverPopupLayout,
    theme: &IdeTheme,
) {
    let parsed = parse_hover_lines(lines);
    if parsed.is_empty() {
        return;
    }
    let s = layout.scale.clamp(0.5, 3.0);
    let font_size = (12.0 * s).clamp(10.0, 18.0);
    let line_h = font_size + 5.0 * s;
    let pad = 9.0 * s;

    let text_opts = DrawOpts {
        font_size,
        color: theme.u8(theme.fg),
        ..DrawOpts::default()
    };
    let dim_opts = DrawOpts {
        color: theme.u8(theme.dim),
        ..text_opts
    };
    let heading_opts = DrawOpts {
        color: theme.u8(theme.fg),
        bold: true,
        ..text_opts
    };
    let code_opts = DrawOpts {
        color: theme.u8(theme.fg),
        ..text_opts
    };

    let max_text_w = (MAX_WIDTH_LOGICAL * s)
        .min(layout.window_w - 2.0 * pad - 16.0)
        .max(80.0);

    let bullet_w = sugarloaf.overlay_text_mut().measure("-", &heading_opts);
    let mut rendered: Vec<RenderedHoverLine> = Vec::with_capacity(parsed.len());
    let mut content_w = 0.0f32;
    for line in parsed {
        let (opts, budget, extra_w) = match line.kind {
            HoverLineKind::Heading => (&heading_opts, max_text_w, 0.0),
            HoverLineKind::Bullet => (
                &dim_opts,
                (max_text_w - bullet_w - 8.0 * s).max(12.0),
                bullet_w + 8.0 * s,
            ),
            HoverLineKind::Code(_) => {
                (&code_opts, (max_text_w - 16.0 * s).max(12.0), 16.0 * s)
            }
            HoverLineKind::Text => (&dim_opts, max_text_w, 0.0),
        };
        let fitted = truncate_to_fit(sugarloaf, &line.text, budget, opts);
        let w = sugarloaf.overlay_text_mut().measure(&fitted, opts) + extra_w;
        if w > content_w {
            content_w = w;
        }
        rendered.push(RenderedHoverLine {
            text: fitted,
            kind: line.kind,
        });
    }

    let box_w = content_w + pad * 2.0;
    let box_h = rendered.len() as f32 * line_h + pad * 2.0;

    // Clamp horizontally; prefer below the cell, flip above if it would spill
    // off the bottom.
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

    sugarloaf.overlay_rounded_rect(
        box_x,
        box_y,
        box_w,
        box_h,
        // Fully opaque: overlay text (including inline diagnostics) is
        // composited in a separate pass, so translucency makes unrelated
        // editor glyphs visibly bleed through the documentation surface.
        theme.f32_alpha(theme.panel_bg(), 1.0),
        DEPTH,
        6.0 * s,
        ORDER,
    );
    // Thin top border to lift it off the editor.
    sugarloaf.overlay_rounded_rect(
        box_x,
        box_y,
        box_w,
        1.0_f32.max(s),
        theme.f32(theme.border),
        DEPTH,
        6.0 * s,
        ORDER + 1,
    );

    for (i, line) in rendered.iter().enumerate() {
        let ty = box_y + pad + i as f32 * line_h;
        let clip = [
            box_x + pad,
            box_y + pad,
            (box_w - pad * 2.0).max(0.0),
            (box_h - pad * 2.0).max(0.0),
        ];
        match line.kind {
            HoverLineKind::Heading => {
                let opts = DrawOpts {
                    clip_rect: Some(clip),
                    ..heading_opts
                };
                sugarloaf
                    .overlay_text_mut()
                    .draw(box_x + pad, ty, &line.text, &opts);
            }
            HoverLineKind::Bullet => {
                let bullet_opts = DrawOpts {
                    color: theme.u8(theme.accent),
                    bold: true,
                    clip_rect: Some(clip),
                    ..text_opts
                };
                let body_opts = DrawOpts {
                    clip_rect: Some(clip),
                    ..dim_opts
                };
                let bw =
                    sugarloaf
                        .overlay_text_mut()
                        .draw(box_x + pad, ty, "-", &bullet_opts);
                sugarloaf.overlay_text_mut().draw(
                    box_x + pad + bw + 8.0 * s,
                    ty,
                    &line.text,
                    &body_opts,
                );
            }
            HoverLineKind::Code(lang) => {
                let code_x = box_x + pad;
                let code_w = (box_w - pad * 2.0).max(0.0);
                sugarloaf.overlay_rounded_rect(
                    code_x,
                    ty - 2.0 * s,
                    code_w,
                    line_h,
                    theme.f32_alpha(theme.surface, 0.58),
                    DEPTH,
                    3.0 * s,
                    ORDER + 1,
                );
                let code_clip = [
                    code_x + 8.0 * s,
                    box_y + pad,
                    (code_w - 16.0 * s).max(0.0),
                    (box_h - pad * 2.0).max(0.0),
                ];
                let mut tx = code_x + 8.0 * s;
                for (tok, slice) in highlight_line(&line.text, lang) {
                    let opts = DrawOpts {
                        font_size,
                        color: syn_color(tok, theme, false),
                        clip_rect: Some(code_clip),
                        ..DrawOpts::default()
                    };
                    tx += sugarloaf.overlay_text_mut().draw(tx, ty, slice, &opts);
                    if tx > code_x + code_w {
                        break;
                    }
                }
            }
            HoverLineKind::Text => {
                let opts = DrawOpts {
                    clip_rect: Some(clip),
                    ..dim_opts
                };
                sugarloaf
                    .overlay_text_mut()
                    .draw(box_x + pad, ty, &line.text, &opts);
            }
        }
    }
}

fn parse_hover_lines(lines: &[String]) -> Vec<HoverLine> {
    let mut out = Vec::new();
    let mut in_code = false;
    let mut code_lang = Lang::Other;
    for raw in lines {
        let lead = raw.trim_start();
        if let Some(info) = lead
            .strip_prefix("```")
            .or_else(|| lead.strip_prefix("~~~"))
        {
            if in_code {
                in_code = false;
                code_lang = Lang::Other;
            } else {
                in_code = true;
                code_lang = lang_from_fence(info);
            }
            continue;
        }

        if in_code {
            out.push(HoverLine {
                text: raw.to_string(),
                kind: HoverLineKind::Code(code_lang),
            });
            continue;
        }

        let trimmed = raw.trim();
        if trimmed == "---" || trimmed == "___" || trimmed == "***" {
            continue;
        }
        if trimmed.is_empty() {
            out.push(HoverLine {
                text: String::new(),
                kind: HoverLineKind::Text,
            });
            continue;
        }
        if let Some(heading) = markdown_heading(raw) {
            out.push(HoverLine {
                text: heading.to_string(),
                kind: HoverLineKind::Heading,
            });
        } else if let Some(bullet) = markdown_bullet(raw) {
            out.push(HoverLine {
                text: bullet.to_string(),
                kind: HoverLineKind::Bullet,
            });
        } else {
            out.push(HoverLine {
                text: trimmed.to_string(),
                kind: HoverLineKind::Text,
            });
        }
    }

    while out
        .first()
        .is_some_and(|line| line.kind == HoverLineKind::Text && line.text.is_empty())
    {
        out.remove(0);
    }
    while out
        .last()
        .is_some_and(|line| line.kind == HoverLineKind::Text && line.text.is_empty())
    {
        out.pop();
    }
    out.truncate(MAX_RENDERED_LINES);
    out
}

fn markdown_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix('#')?
        .trim_start_matches('#')
        .trim_start();
    (!rest.is_empty()).then_some(rest)
}

fn markdown_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
}

fn lang_from_fence(label: &str) -> Lang {
    let label = label
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match label.as_str() {
        "rs" | "rust" => Lang::Rust,
        "js" | "javascript" | "mjs" | "cjs" => Lang::Javascript,
        "jsx" => Lang::Jsx,
        "ts" | "typescript" => Lang::Typescript,
        "tsx" => Lang::Tsx,
        "py" | "python" => Lang::Python,
        "go" | "golang" => Lang::Go,
        "lua" => Lang::Lua,
        "toml" => Lang::Toml,
        "json" | "jsonc" => Lang::Json,
        "md" | "markdown" => Lang::Markdown,
        _ => Lang::Other,
    }
}

/// Cut a line with an ellipsis so it fits `max_w`. Lines are short (signatures
/// / a doc sentence), so the per-char shrink loop is cheap.
fn truncate_to_fit(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> String {
    if sugarloaf.overlay_text_mut().measure(text, opts) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "\u{2026}";
        if sugarloaf.overlay_text_mut().measure(&candidate, opts) <= max_w {
            return candidate;
        }
    }
    "\u{2026}".to_string()
}
