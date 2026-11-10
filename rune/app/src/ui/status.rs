//! The status line (`shared/src/panels/status_line.rs`): a 22 pt strip with a 2 pt top rule.
//! Left, a mode pill whose rounded tail overlaps the cwd pill (surface text section, red folder
//! section). Right, overlapping two-tone pills: branch (with `+added -deleted`), then FPS. Every
//! label is bold, drawn twice 0.6 pt apart, over two dark extrusion copies.

use super::theme::rgb;
use crate::app::App;
use crate::tabs::TabKind;
use rune::tok::{Family, TextStyle};
use rune::{Color, El, el, row, text};

const HEIGHT: f32 = 22.0;
const FONT: f32 = 12.0;
const PILL_VPAD: f32 = 2.0;
const SECTION_PAD: f32 = 8.0;
const TAIL_OVERLAP: f32 = 0.55;
const CHAIN_BREATHER: f32 = 4.0;
const ADVANCE: f32 = 0.6;

const GLYPH_MODE: &str = "\u{e7c5}";
const GLYPH_FOLDER: &str = "\u{f07b}";
const GLYPH_BRANCH: &str = "\u{e725}";
const GLYPH_LINES: &str = "\u{f0c9}";

/// Git branch and working-tree line counts for the branch pill.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitSummary {
    pub branch: String,
    pub added: u64,
    pub deleted: u64,
}

pub fn height(app: &App) -> f32 { HEIGHT * app.ty.scale() }

struct Metrics {
    s: f32,
    font: f32,
    pill_y: f32,
    pill_h: f32,
    radius: f32,
    pad: f32,
}

impl Metrics {
    fn text_w(&self, text: &str) -> f32 { text.chars().count() as f32 * self.font * ADVANCE }
    /// Symbols Nerd Font Mono icons are one em wide.
    fn icon_w(&self) -> f32 { self.font }
}

/// Bold text with the faux-bold copy and the two extrusion shadows (`draw_thick` + `extrude`).
fn thick(m: &Metrics, label: &str, family: Option<Family>, color: Color) -> El {
    let style = || {
        let style = TextStyle::new(m.font, 700.0);
        match family { Some(family) => style.family(family), None => style.mono() }
    };
    let layer = |dx: f32, dy: f32, ink: Color| text(label.to_owned()).font(style()).color(ink).absolute(dx, dy);
    let alpha = color.a;
    el().child(layer(1.5 * m.s, 1.5 * m.s, Color { r: 0.003, g: 0.003, b: 0.005, a: alpha * 5.0 / 6.0 }))
        .child(layer(0.75 * m.s, 0.75 * m.s, rgb(0x3e3e48)))
        .child(layer(0.6 * m.s, 0.0, color))
        .child(text(label.to_owned()).font(style()).color(color))
}

fn icon(m: &Metrics, glyph: &str, color: Color, width: f32) -> El {
    el().w(width).h(m.pill_h).center().child(thick(m, glyph, Some(super::theme::NERD), color))
}

/// A pill section: `fill`, `corners` [tl, tr, br, bl], at `x` from the left.
fn section(m: &Metrics, x: f32, width: f32, fill: Color, corners: [f32; 4]) -> El {
    el().w(width).h(m.pill_h).fill(fill).corners(corners).absolute(x, m.pill_y)
}

/// A pill section whose right edge is `right` from the strip's right edge.
fn section_right(m: &Metrics, right: f32, width: f32, fill: Color, corners: [f32; 4]) -> El {
    el().w(width).h(m.pill_h).fill(fill).corners(corners).pin(None, Some(m.pill_y), Some(right), None)
}

pub fn view(app: &App) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let strip_h = HEIGHT * s;
    let pill_h = strip_h - PILL_VPAD * 2.0 * s;
    let m = Metrics { s, font: FONT * s, pill_y: PILL_VPAD * s, pill_h, radius: pill_h / 2.0, pad: SECTION_PAD * s };
    let r = m.radius;
    let tail = r * TAIL_OVERLAP;
    let breather = CHAIN_BREATHER * s;
    let black = rgb(p.black);

    let (mode, mode_color) = match app.active_tab().kind {
        TabKind::Agent { .. } => ("AGENT", p.red),
        TabKind::Terminal => ("TERMINAL", p.green),
        TabKind::Editor { .. } if app.active_buffer().is_some_and(|b| b.prompt.is_some()) => ("COMMAND", p.green),
        TabKind::Editor { .. } => match app.editor_mode() {
            Some(crate::editor::document::Mode::Insert) => ("INSERT", p.magenta),
            Some(crate::editor::document::Mode::Visual) => ("VISUAL", p.cyan),
            _ => ("NORMAL", p.blue),
        },
        TabKind::Home => ("NORMAL", p.blue),
    };
    let mut bar = el().key("original-statusline").w_full().h(strip_h).fill(rgb(p.bg))
        .child(el().w_full().h((2.0 * s).max(2.0)).fill(rgb(p.surface)).absolute(0.0, 0.0));

    // Left: cwd pill first (under), then the mode pill over its left end.
    let gap = m.text_w("  ");
    let mode_w = m.pad * 2.0 + m.icon_w() + gap + m.text_w(mode);
    let cwd = app.tab_cwd(app.active_tab());
    let cwd_x = mode_w - tail;
    let cwd_text_w = tail + breather + m.text_w(&cwd) + m.pad;
    let cwd_icon_w = m.pad + m.icon_w() + m.pad + pill_h * 0.25;
    bar = bar
        .child(section(&m, cwd_x, cwd_text_w + cwd_icon_w, rgb(p.surface), [0.0, r, r, 0.0])
            .child(row_at(&m, tail + breather, thick(&m, &cwd, None, rgb(p.red)))))
        .child(section(&m, cwd_x + cwd_text_w, cwd_icon_w, rgb(p.red), [0.0, r, r, 0.0])
            .child(icon(&m, GLYPH_FOLDER, black, cwd_icon_w).absolute(0.0, 0.0)))
        .child(section(&m, 0.0, mode_w, rgb(mode_color), [0.0, r, r, 0.0])
            .child(row_at(&m, m.pad, row().h(pill_h).center_y()
                .child(icon(&m, GLYPH_MODE, black, m.icon_w()))
                .child(el().ml(gap).child(thick(&m, mode, None, black))))));

    // Right: pills chain leftward from the window edge; later pills sit on top.
    struct Pill { icon: (String, Option<Family>), icon_bg: u32, text: Vec<(String, u32)> }
    let mut pills = Vec::new();
    if let Some(git) = &app.git {
        let mut parts = vec![(git.branch.clone(), p.blue)];
        if git.added > 0 { parts.push((format!(" +{}", git.added), p.green)); }
        if git.deleted > 0 { parts.push((format!(" -{}", git.deleted), p.red)); }
        pills.push(Pill { icon: (GLYPH_BRANCH.into(), Some(super::theme::NERD)), icon_bg: p.blue, text: parts });
    }
    if let Some(buffer) = app.active_buffer().filter(|b| b.loaded) {
        let label = format!(" {}/{}", buffer.doc.cursor_line + 1, buffer.doc.lines.len());
        pills.push(Pill { icon: (GLYPH_LINES.into(), Some(super::theme::NERD)), icon_bg: p.green, text: vec![(label, p.green)] });
    }
    if let Some(fps) = app.fps {
        pills.push(Pill { icon: ("FPS".into(), None), icon_bg: p.cyan, text: vec![(format!(" {fps}"), p.cyan)] });
    }
    let widths: Vec<(f32, f32)> = pills.iter().enumerate().map(|(i, pill)| {
        let icon_w = if pill.icon.1.is_some() { m.icon_w() } else { m.text_w(&pill.icon.0) };
        let text_w: f32 = pill.text.iter().map(|(t, _)| m.text_w(t)).sum();
        let extra = if i + 1 == pills.len() { 0.0 } else { tail + breather };
        (icon_w + m.pad * 2.0, text_w + m.pad * 2.0 + extra)
    }).collect();
    let total: f32 = widths.iter().map(|(a, b)| a + b).sum::<f32>() - tail * pills.len().saturating_sub(1) as f32;
    // `x` runs from the cluster's left edge; sections pin by their distance from the right.
    let mut x = 0.0;
    for (pill, (icon_w, text_w)) in pills.iter().zip(&widths) {
        let mut label = row().h(pill_h).center_y();
        for (part, color) in &pill.text {
            label = label.child(thick(&m, part, None, rgb(*color)));
        }
        let glyph = el().w(*icon_w).h(pill_h).center().child(thick(&m, &pill.icon.0, pill.icon.1, black));
        bar = bar
            .child(section_right(&m, total - (x + icon_w + text_w), icon_w + text_w, rgb(p.surface), [r, 0.0, 0.0, r])
                .child(row_at(&m, icon_w + m.pad, label)))
            .child(section_right(&m, total - (x + icon_w), *icon_w, rgb(pill.icon_bg), [r, 0.0, 0.0, r]).child(glyph.absolute(0.0, 0.0)));
        x += icon_w + text_w - tail;
    }
    bar
}

/// Vertically centre a label inside a pill section, `x` from its left edge.
fn row_at(m: &Metrics, x: f32, child: El) -> El {
    el().h(m.pill_h).center_y().absolute(x, 0.0).child(child)
}
