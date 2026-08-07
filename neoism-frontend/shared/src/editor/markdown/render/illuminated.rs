use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::ide_theme::IdeTheme;

use super::draw::{
    draw_if_visible, draw_rect_clipped, draw_rounded_rect_clipped, line_height,
};
use super::types::{DEPTH, ORDER_BG};

const UNIFRAKTUR_COOK_BOLD: &[u8] = include_bytes!(
    "../../../../assets/illuminated/fonts/google-ofl/UnifrakturCook-Bold.ttf"
);
const UNIFRAKTUR_MAGUNTIA_BOOK: &[u8] = include_bytes!(
    "../../../../assets/illuminated/fonts/google-ofl/UnifrakturMaguntia-Book.ttf"
);
const CINZEL_DECORATIVE_BOLD: &[u8] = include_bytes!(
    "../../../../assets/illuminated/fonts/google-ofl/CinzelDecorative-Bold.ttf"
);
const CINZEL_DECORATIVE_BLACK: &[u8] = include_bytes!(
    "../../../../assets/illuminated/fonts/google-ofl/CinzelDecorative-Black.ttf"
);
const PIRATA_ONE_REGULAR: &[u8] = include_bytes!(
    "../../../../assets/illuminated/fonts/google-ofl/PirataOne-Regular.ttf"
);
const MEDIEVAL_SHARP_REGULAR: &[u8] =
    include_bytes!("../../../../assets/illuminated/fonts/google-ofl/MedievalSharp.ttf");

const ILLUMINATED_DEFAULT_LINES: f32 = 1.2;
const ILLUMINATED_MIN_LINES: f32 = 1.0;
const ILLUMINATED_MAX_LINES: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IlluminatedStyleKind {
    Fraktur,
    Maguntia,
    Cinzel,
    CinzelBlack,
    Pirata,
    Medieval,
    Manuscript,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IlluminatedColorMode {
    Text,
    White,
    Gold,
    Native,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct IlluminatedToken {
    pub(super) letter: char,
    pub(super) style: IlluminatedStyleKind,
    pub(super) color: IlluminatedColorMode,
    pub(super) lines: f32,
    pub(super) source_len: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct IlluminatedDrawMetrics {
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) baseline_lift: f32,
}

pub(super) fn parse_illuminate_token(source: &str) -> Option<IlluminatedToken> {
    let rest = source.strip_prefix("::illuminate[")?;
    let close = rest.find(']')?;
    let letter = rest[..close]
        .trim()
        .chars()
        .find(|ch| !ch.is_whitespace())?;
    let mut token = IlluminatedToken {
        letter,
        style: IlluminatedStyleKind::Fraktur,
        color: IlluminatedColorMode::Text,
        lines: ILLUMINATED_DEFAULT_LINES,
        source_len: "::illuminate[".len() + close + 1,
    };
    let after = &rest[close + 1..];
    if let Some((attrs_len, style, color, lines)) = parse_attrs(after) {
        token.source_len += attrs_len;
        token.style = style.unwrap_or(token.style);
        token.color = color.unwrap_or(token.color);
        token.lines = lines.unwrap_or(token.lines);
    }
    Some(token)
}

pub(super) fn illuminated_inline_metrics(
    sugarloaf: &mut Sugarloaf,
    token: &IlluminatedToken,
    opts: &DrawOpts,
) -> IlluminatedDrawMetrics {
    let line_h = line_height(opts);
    let font_size = illuminated_font_size(opts, token.lines);
    let draw_opts =
        illuminated_draw_opts(sugarloaf, token.style, token.color, opts, font_size);
    let letter = token.letter.to_string();
    let measured = sugarloaf.text_mut().measure(&letter, &draw_opts);
    let height = line_h * token.lines;
    IlluminatedDrawMetrics {
        width: measured.max(font_size * 0.72) + font_size * 0.22,
        height,
        baseline_lift: -(font_size * 0.14).min(height * 0.18),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_illuminated_inline(
    sugarloaf: &mut Sugarloaf,
    token: &IlluminatedToken,
    x: f32,
    text_y: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    occlusions: &[[f32; 4]],
) -> f32 {
    let metrics = illuminated_inline_metrics(sugarloaf, token, opts);
    let font_size = illuminated_font_size(opts, token.lines);
    let box_y = text_y;
    let text_y = text_y + metrics.baseline_lift;
    if matches!(token.style, IlluminatedStyleKind::Manuscript) {
        draw_manuscript_tile(
            sugarloaf,
            token.letter,
            x,
            box_y + metrics.baseline_lift - metrics.height * 0.04,
            metrics.width,
            metrics.height * 0.9,
            theme,
            clip,
        );
    }
    let letter = token.letter.to_string();
    let draw_opts =
        illuminated_draw_opts(sugarloaf, token.style, token.color, opts, font_size);
    draw_if_visible(
        sugarloaf,
        x + font_size * 0.07,
        text_y,
        &letter,
        &draw_opts,
        clip_top,
        clip_bottom,
        occlusions,
    );
    metrics.width
}

pub(super) fn illuminated_style_from_attr(value: &str) -> Option<IlluminatedStyleKind> {
    match normalize_attr(value).as_str() {
        "fraktur" | "blackletter" | "gothic" | "unifraktur" | "unifrakturcook" => {
            Some(IlluminatedStyleKind::Fraktur)
        }
        "maguntia" | "unifrakturmaguntia" | "book" | "storybook" => {
            Some(IlluminatedStyleKind::Maguntia)
        }
        "cinzel" | "cinzeldecorative" | "roman" | "engraved" => {
            Some(IlluminatedStyleKind::Cinzel)
        }
        "cinzelblack" | "poster" | "capitals" => Some(IlluminatedStyleKind::CinzelBlack),
        "pirata" | "pirataone" | "pirate" | "woodcut" => {
            Some(IlluminatedStyleKind::Pirata)
        }
        "medieval" | "medievalsharp" | "sharp" | "scribe" => {
            Some(IlluminatedStyleKind::Medieval)
        }
        "manuscript" | "gutenberg" | "reusableart" | "reusable-art" | "goldleaf"
        | "art" => Some(IlluminatedStyleKind::Manuscript),
        _ => None,
    }
}

fn illuminated_color_from_attr(value: &str) -> Option<IlluminatedColorMode> {
    match normalize_attr(value).as_str() {
        "text" | "current" | "fg" => Some(IlluminatedColorMode::Text),
        "white" => Some(IlluminatedColorMode::White),
        "gold" | "gilded" => Some(IlluminatedColorMode::Gold),
        "native" | "source" | "original" => Some(IlluminatedColorMode::Native),
        _ => None,
    }
}

fn parse_attrs(
    source: &str,
) -> Option<(
    usize,
    Option<IlluminatedStyleKind>,
    Option<IlluminatedColorMode>,
    Option<f32>,
)> {
    let leading_ws = source.len().saturating_sub(source.trim_start().len());
    let rest = source.trim_start().strip_prefix('{')?;
    let close = rest.find('}')?;
    let attrs = &rest[..close];
    let mut style = None;
    let mut color = None;
    let mut lines = None;
    for part in attrs.split_whitespace() {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let key = normalize_attr(key);
        let value = value.trim_matches(['"', '\'']);
        if key == "style" || key == "font" || key == "pack" {
            style = illuminated_style_from_attr(value).or(style);
        } else if key == "color" || key == "tone" || key == "mode" {
            color = illuminated_color_from_attr(value).or(color);
        } else if key == "size" || key == "lines" || key == "height" || key == "drop" {
            lines = illuminated_lines_from_attr(value).or(lines);
        }
    }
    Some((leading_ws + close + 2, style, color, lines))
}

fn illuminated_lines_from_attr(value: &str) -> Option<f32> {
    let normalized = normalize_attr(value);
    let lines = match normalized.as_str() {
        "inline" | "small" | "s" => 1.0,
        "medium" | "m" => 2.0,
        "large" | "big" | "l" => 3.0,
        "huge" | "xl" => 4.0,
        _ => value
            .trim()
            .trim_matches(['\"', '\''])
            .trim_end_matches("lines")
            .trim_end_matches("line")
            .trim_end_matches('x')
            .parse::<f32>()
            .ok()?,
    };
    Some(lines.clamp(ILLUMINATED_MIN_LINES, ILLUMINATED_MAX_LINES))
}

fn illuminated_font_size(opts: &DrawOpts, lines: f32) -> f32 {
    if lines <= 1.25 {
        return (opts.font_size * 1.42).max(opts.font_size + 6.0);
    }
    (line_height(opts) * lines * 0.72).max(opts.font_size + 6.0)
}

fn normalize_attr(value: &str) -> String {
    value
        .trim()
        .trim_matches(['"', '\''])
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect()
}

fn illuminated_draw_opts(
    sugarloaf: &mut Sugarloaf,
    style: IlluminatedStyleKind,
    color: IlluminatedColorMode,
    base: &DrawOpts,
    font_size: f32,
) -> DrawOpts {
    let font_id = match style {
        IlluminatedStyleKind::Fraktur => {
            sugarloaf.ensure_static_font(UNIFRAKTUR_COOK_BOLD)
        }
        IlluminatedStyleKind::Maguntia | IlluminatedStyleKind::Manuscript => {
            sugarloaf.ensure_static_font(UNIFRAKTUR_MAGUNTIA_BOOK)
        }
        IlluminatedStyleKind::Cinzel => {
            sugarloaf.ensure_static_font(CINZEL_DECORATIVE_BOLD)
        }
        IlluminatedStyleKind::CinzelBlack => {
            sugarloaf.ensure_static_font(CINZEL_DECORATIVE_BLACK)
        }
        IlluminatedStyleKind::Pirata => sugarloaf.ensure_static_font(PIRATA_ONE_REGULAR),
        IlluminatedStyleKind::Medieval => {
            sugarloaf.ensure_static_font(MEDIEVAL_SHARP_REGULAR)
        }
    };
    DrawOpts {
        font_size,
        color: illuminated_color(color, base),
        bold: false,
        italic: false,
        extrude: false,
        font_id,
        clip_rect: base.clip_rect,
    }
}

fn illuminated_color(color: IlluminatedColorMode, base: &DrawOpts) -> [u8; 4] {
    match color {
        IlluminatedColorMode::Text => base.color,
        IlluminatedColorMode::White => [248, 244, 232, base.color[3]],
        IlluminatedColorMode::Gold | IlluminatedColorMode::Native => {
            [238, 194, 89, base.color[3]]
        }
    }
}

fn draw_manuscript_tile(
    sugarloaf: &mut Sugarloaf,
    letter: char,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    theme: &IdeTheme,
    clip: [f32; 4],
) {
    let bg = [0.42, 0.065, 0.09, 0.92];
    let gold = [0.94, 0.63, 0.18, 0.95];
    let ink = theme.f32_alpha(theme.surface, 0.68);
    draw_rounded_rect_clipped(sugarloaf, clip, x, y, w, h, 5.0, bg, DEPTH, ORDER_BG + 2);
    draw_rect_clipped(
        sugarloaf,
        clip,
        x + 3.0,
        y + 3.0,
        w - 6.0,
        2.0,
        gold,
        DEPTH,
        ORDER_BG + 3,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        x + 3.0,
        y + h - 5.0,
        w - 6.0,
        2.0,
        gold,
        DEPTH,
        ORDER_BG + 3,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        x + 3.0,
        y + 3.0,
        2.0,
        h - 6.0,
        gold,
        DEPTH,
        ORDER_BG + 3,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        x + w - 5.0,
        y + 3.0,
        2.0,
        h - 6.0,
        gold,
        DEPTH,
        ORDER_BG + 3,
    );
    let seed = letter.to_ascii_uppercase() as u32;
    for ix in 0..3 {
        let fx = x + 7.0 + ((seed + ix * 17) % 29) as f32 / 29.0 * (w - 18.0).max(1.0);
        let fy = y + 7.0 + ((seed + ix * 11) % 31) as f32 / 31.0 * (h - 18.0).max(1.0);
        draw_rounded_rect_clipped(
            sugarloaf,
            clip,
            fx,
            fy,
            (w * 0.12).max(4.0),
            (h * 0.06).max(3.0),
            8.0,
            ink,
            DEPTH,
            ORDER_BG + 3,
        );
    }
}
