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
const GLYPH_LSP: &str = "\u{f0e7}";
const GLYPH_ERROR: &str = "\u{ea87}";
const GLYPH_WARN: &str = "\u{f071}";

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

/// The mode label mid-change (`render_scramble_label`): each letter shows a random capital in a
/// cycling rainbow until it locks in turn, the last at the end of the transition.
fn scramble(m: &Metrics, label: &str, color: Color, t: f32, elapsed_ms: f32) -> El {
    if t >= 1.0 { return thick(m, label, None, color); }
    let chars: Vec<char> = label.chars().collect();
    let n = chars.len().max(1) as f32;
    let seed = (elapsed_ms / 30.0) as u64;
    let mut out = row();
    for (i, &target) in chars.iter().enumerate() {
        let (ch, ink) = if t >= (i as f32 + 1.0) / n || target == ' ' {
            (target, color)
        } else {
            let h = mix_seed(seed, i);
            ((b'A' + (h % 26) as u8) as char, hsl((elapsed_ms * 0.6 + i as f32 * 47.0) % 360.0, 1.0, 0.65))
        };
        out = out.child(el().w(m.font * ADVANCE).child(thick(m, &ch.to_string(), None, ink)));
    }
    out
}

fn mix_seed(seed: u64, idx: usize) -> u32 {
    let mut h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h.rotate_right(27).wrapping_add(idx as u64 * 31);
    h ^= h >> 33;
    h as u32
}

fn hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as i32 { 0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x), 3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x) };
    let m = l - c / 2.0;
    Color { r: r + m, g: g + m, b: b + m, a: 1.0 }
}

/// Mode pill colour cross-fade and scramble duration (`TRANSITION_MS`).
const TRANSITION_MS: f32 = 320.0;

/// The mode pill's state across frames: the mode it shows and when it last changed.
#[derive(Default)]
pub struct ModePill {
    mode: &'static str,
    from: u32,
    /// Frame-clock seconds when the mode last changed.
    started: f64,
}

impl App {
    /// The mode the pill names and its colour (`mode_label` / `mode_bg`).
    fn mode_now(&self) -> (&'static str, u32) {
        let p = self.pal;
        match self.active_tab().kind {
            TabKind::Agent { .. } => ("AGENT", p.red),
            TabKind::Database { .. } => ("DATABASE", p.blue),
            TabKind::Terminal => ("TERMINAL", p.green),
            TabKind::Editor { .. } if self.active_buffer().is_some_and(|b| b.prompt.is_some()) => ("COMMAND", p.green),
            TabKind::Editor { .. } => match self.editor_mode() {
                Some(crate::editor::document::Mode::Insert) => ("INSERT", p.magenta),
                Some(crate::editor::document::Mode::Visual) => ("VISUAL", p.cyan),
                _ => ("NORMAL", p.blue),
            },
            TabKind::Home => ("NORMAL", p.blue),
        }
    }

    /// Start the transition when the mode changes. Returns whether it is still playing.
    pub fn tick_mode_pill(&mut self) -> bool {
        let (mode, _) = self.mode_now();
        if mode != self.mode_pill.mode {
            // The very first mode appears at rest.
            let first = self.mode_pill.mode.is_empty();
            let from = self.mode_pill_color();
            self.mode_pill = ModePill { mode, from, started: if first { f64::NEG_INFINITY } else { self.tools.clock } };
        }
        self.mode_pill_elapsed() < TRANSITION_MS
    }

    /// Milliseconds since the mode last changed, on the frame clock.
    fn mode_pill_elapsed(&self) -> f32 {
        ((self.tools.clock - self.mode_pill.started) * 1000.0).min(f32::MAX as f64) as f32
    }

    fn mode_pill_color(&self) -> u32 {
        let (_, to) = self.mode_now();
        let t = (self.mode_pill_elapsed() / TRANSITION_MS).clamp(0.0, 1.0);
        if t >= 1.0 || self.mode_pill.mode.is_empty() { return to; }
        let eased = 1.0 - (1.0 - t).powi(3);
        let lerp = |shift: u32| {
            let (a, b) = (((self.mode_pill.from >> shift) & 0xff) as f32, ((to >> shift) & 0xff) as f32);
            ((a + (b - a) * eased).round() as u32) << shift
        };
        lerp(16) | lerp(8) | lerp(0)
    }

    /// This frame's pill: its label (with nvim's showcmd, `NORMAL · 2d`), colour, transition
    /// progress and elapsed milliseconds.
    pub fn mode_pill_frame(&self) -> (String, Color, f32, f32) {
        let (mode, _) = self.mode_now();
        let pending = matches!(self.active_tab().kind, TabKind::Editor { .. })
            .then(|| self.active_buffer().filter(|b| b.prompt.is_none()).map(|b| b.doc.vim.pending.showcmd()))
            .flatten().filter(|p| !p.is_empty());
        let label = match pending { Some(keys) => format!("{mode} \u{b7} {keys}"), None => mode.to_owned() };
        let elapsed = self.mode_pill_elapsed();
        (label, rgb(self.mode_pill_color()), (elapsed / TRANSITION_MS).clamp(0.0, 1.0), elapsed)
    }
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

    let (mode, mode_color, t, elapsed) = app.mode_pill_frame();
    let mode = mode.as_str();
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
        .child(section(&m, 0.0, mode_w, mode_color, [0.0, r, r, 0.0])
            .child(row_at(&m, m.pad, row().h(pill_h).center_y()
                .child(icon(&m, GLYPH_MODE, black, m.icon_w()))
                .child(el().ml(gap).child(scramble(&m, mode, black, t, elapsed))))));

    // Right: pills chain leftward from the window edge; later pills sit on top.
    // A `solid` pill (diagnostic counts) is one colour end to end with black text.
    struct Pill { icon: (String, Option<Family>), icon_bg: u32, text: Vec<(String, u32)>, solid: bool }
    let mut pills = Vec::new();
    if let Some(git) = &app.git {
        let mut parts = vec![(git.branch.clone(), p.blue)];
        if git.added > 0 { parts.push((format!(" +{}", git.added), p.green)); }
        if git.deleted > 0 { parts.push((format!(" -{}", git.deleted), p.red)); }
        pills.push(Pill { icon: (GLYPH_BRANCH.into(), Some(super::theme::NERD)), icon_bg: p.blue, text: parts, solid: false });
    }
    // Diagnostic counts, then the LSP pill: green attached, yellow warming, red none.
    if let Some((lsp, errors, warns)) = crate::editor::lsp_view::status(app) {
        if errors > 0 {
            pills.push(Pill { icon: (GLYPH_ERROR.into(), Some(super::theme::NERD)), icon_bg: p.red, text: vec![(format!(" {errors}"), p.black)], solid: true });
        }
        if warns > 0 {
            pills.push(Pill { icon: (GLYPH_WARN.into(), Some(super::theme::NERD)), icon_bg: p.yellow, text: vec![(format!(" {warns}"), p.black)], solid: true });
        }
        if let Some(lsp) = lsp {
            let color = match lsp {
                crate::editor::lsp::Status::Active => p.green,
                crate::editor::lsp::Status::Initializing => p.yellow,
                crate::editor::lsp::Status::Missing => p.red,
            };
            pills.push(Pill { icon: (GLYPH_LSP.into(), Some(super::theme::NERD)), icon_bg: color, text: vec![(" LSP".into(), color)], solid: false });
        }
    }
    if let Some(buffer) = app.active_buffer().filter(|b| b.loaded) {
        let label = format!(" {}/{}", buffer.doc.cursor_line + 1, buffer.doc.lines.len());
        pills.push(Pill { icon: (GLYPH_LINES.into(), Some(super::theme::NERD)), icon_bg: p.green, text: vec![(label, p.green)], solid: false });
    }
    if let Some(fps) = app.fps {
        pills.push(Pill { icon: ("FPS".into(), None), icon_bg: p.cyan, text: vec![(format!(" {fps}"), p.cyan)], solid: false });
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
            .child(section_right(&m, total - (x + icon_w + text_w), icon_w + text_w, rgb(if pill.solid { pill.icon_bg } else { p.surface }), [r, 0.0, 0.0, r])
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
