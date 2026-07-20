//! Status line — cross-platform port of the original
//! `frontends/neoism/src/chrome/panels/status_line/` module set.
//!
//! Migrated verbatim from the native chrome (`types.rs`, `state.rs`,
//! `update.rs`, `segments.rs`, `helpers.rs`, `render.rs`) into a single
//! file so it can run unchanged on web through sugarloaf. The only
//! shape changes relative to the original:
//!
//! - The legacy `render` method used `&IdeTheme` from
//!   `frontends/neoism/src/chrome/primitives/theme.rs`. That type is
//!   native-only (depends on `neoism_backend`). Here we expose
//!   [`StatusPalette`], a structurally-identical struct with the
//!   subset of fields the status line actually reads, plus the same
//!   `f32(u32)` / `u8(u32)` helpers. The native shim converts its
//!   `IdeTheme` to a `StatusPalette` per frame.
//! - `GitChangeSummary` is redefined here (same shape as
//!   `frontends/neoism/src/chrome/panels/git_branch.rs`) so this crate
//!   doesn't have to reach back into the native frontend.
//! - A `Panel` impl is added on top of the legacy `render` API. The
//!   status line is read-only — `handle_event` swallows everything;
//!   `draw` requires the host to refresh the panel's snapshot via
//!   `set_info` between frames (existing API) and call `render` with
//!   the resolved palette. The trait `draw` is a thin shim that
//!   delegates to `render` once the host has stashed an
//!   `StatusInfo`-supplying closure / context on the struct.
//!
//! See `docs/NEOISM_UI_DESIGN.md` for the broader migration plan.

use web_time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use web_time::Instant;

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug)]
struct Instant(f64);

#[cfg(target_arch = "wasm32")]
impl Instant {
    fn now() -> Self {
        Self(js_sys::Date::now())
    }

    fn elapsed(&self) -> Duration {
        Duration::from_secs_f64(((js_sys::Date::now() - self.0) / 1000.0).max(0.0))
    }
}

#[cfg(target_arch = "wasm32")]
impl std::ops::Sub<Duration> for Instant {
    type Output = Instant;

    fn sub(self, rhs: Duration) -> Self::Output {
        Instant(self.0 - rhs.as_secs_f64() * 1000.0)
    }
}

use serde::{Deserialize, Serialize};
use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::event::UiEvent;
use crate::layout::PanelLayout;
use crate::panels::{Panel, PanelContext};
use crate::primitives::IdeTheme;

// ─── Types / constants ────────────────────────────────────────────────

pub const STATUS_LINE_HEIGHT: f32 = 22.0;
const FONT_SIZE: f32 = 12.0;
/// Faux-bold offset (logical px). Each text draw is re-drawn at +X to
/// thicken the strokes when the loaded font's bold variant is too thin.
const FAUX_BOLD_OFFSET: f32 = 0.6;
const PILL_VPAD: f32 = 2.0;
const SECTION_PAD_X: f32 = 8.0;
const SCALE_MIN: f32 = 0.5;
const SCALE_MAX: f32 = 3.0;
/// Mode bg cross-fade duration AND scramble-text duration.
const TRANSITION_MS: f32 = 320.0;
/// How long a new dir-pill label must stay put before it paints.
const CWD_LABEL_SETTLE_MS: f32 = 250.0;
const BRANCH_HOVER_MS: f32 = 140.0;
/// How far the mode pill's rounded tail tucks under the file pill,
/// expressed as a fraction of the pill radius. The visible portion of
/// the curve protrudes from behind the file pill's flat-left edge.
const MODE_TAIL_OVERLAP_FRAC: f32 = 0.55;

const DEPTH: f32 = 0.0;
// Above the command composer (12-15) so the composer can sit flush
// against the bottom chrome without painting over the status line.
const ORDER_BG: u8 = 16;
const ORDER_PILL_BACK: u8 = 17;
const ORDER_PILL: u8 = 18;

// Nerd-font glyphs. Mode glyph matches NvChad's hardcoded `\u{e7c5}`
// (Vim/NvChad logo). Devicon defaults are used for file/cwd/cursor.
const GLYPH_MODE: &str = "\u{e7c5}"; //
const GLYPH_FILE: &str = "\u{f15b}"; //  (devicons default in stl/utils.lua)
const GLYPH_TERMINAL: &str = "\u{f120}"; // terminal prompt
const GLYPH_BRANCH: &str = "\u{e725}"; //
#[allow(dead_code)]
const GLYPH_WORKSPACE: &str = "\u{f0e8}"; // sitemap/workspace
const GLYPH_FOLDER: &str = "\u{f07b}"; //  (folder closed)
#[allow(dead_code)]
const GLYPH_HOME: &str = "\u{f015}"; //  (home — unused after cwd pill switched to folder)
#[allow(dead_code)]
const GLYPH_GITHUB: &str = "\u{f09b}"; //  (FA github — repo pill on right cluster)
const GLYPH_LINES: &str = "\u{f0c9}"; //  (line index)
                                      // Severity + LSP glyphs are shared with the diagnostics/LSP popups —
                                      // single source in `primitives::icons` so the panels never drift.
const GLYPH_LSP: &str = crate::primitives::icons::GLYPH_LSP;
const GLYPH_SPLIT: &str = "\u{eb56}"; // codicon split-horizontal
const GLYPH_ERROR: &str = crate::primitives::icons::GLYPH_ERROR;
const GLYPH_WARN: &str = crate::primitives::icons::GLYPH_WARN;

/// Resolve a mash-up pack / user icon override (`[icons]` table) for
/// one of the status-line glyphs. Glyph only — pill colors stay
/// theme-driven. With no override registered this returns `default`,
/// so stock rendering is unchanged.
fn status_glyph(key: &str, default: &'static str) -> &'static str {
    crate::primitives::look::themed_glyph(key, default)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    Replace,
    Cmd,
    Terminal,
    Markdown,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PrimaryKind {
    File,
    #[default]
    Terminal,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LspStatus {
    Initializing,
    Active,
    Missing,
}

/// Diagnostic counts the lua side already aggregated for the buffer the
/// editor is on. Stored alongside `StatusInfo` so the pills, the popup
/// title, and the hit-test all read the same numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticCounts {
    pub error: u64,
    pub warn: u64,
    pub info: u64,
    pub hint: u64,
}

/// Line-level change totals for the active repo. Migrated from
/// `frontends/neoism/src/chrome/panels/git_branch.rs` so this crate
/// doesn't reach back into the native frontend; the native shim
/// converts the original `git_branch::GitChangeSummary` to this struct
/// inside its `set_info` wrapper.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitChangeSummary {
    pub added: u64,
    pub deleted: u64,
}

impl GitChangeSummary {
    pub fn is_empty(self) -> bool {
        self.added == 0 && self.deleted == 0
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct StatusInfo {
    pub mode: Mode,
    pub primary: String,
    pub primary_kind: PrimaryKind,
    pub branch: Option<String>,
    pub git_changes: Option<GitChangeSummary>,
    pub workspace: Option<String>,
    pub lsp_status: Option<LspStatus>,
    /// Short server label for the LSP pill, e.g. `rust-analyzer` or
    /// `pyright+1`. When absent, the pill falls back to `LSP`.
    pub lsp_label: Option<String>,
    pub project: Option<String>,
    /// Vim-style ruler for the right cluster: cursor line / total
    /// lines of the active *editor* surface (code buffer or markdown
    /// pane). `None` on terminal panes — a terminal has no meaningful
    /// line position, so the pill hides entirely.
    pub cursor_lines: Option<(usize, usize)>,
    /// nvim-style showcmd: in-progress vim keys appended to the mode
    /// chip (`NORMAL · 2d`). `None`/empty hides the segment.
    pub pending_keys: Option<String>,
    pub diagnostics: DiagnosticCounts,
    /// Directory shown in the cwd pill on the left cluster. Populated
    /// for every active-pane kind (editor / markdown / terminal) — the
    /// caller is responsible for picking the right path (active file's
    /// parent for editors, terminal cwd for shells) and abbreviating
    /// it zsh-style (`~`, `~/sub`, or absolute).
    pub cwd_label: Option<String>,
    /// Measured frames-per-second of the host window's render loop,
    /// already smoothed by the host. `None` hides the pill (config off
    /// or no measurement yet). Purely display — the pill never drives
    /// redraws itself, so an idle window just freezes at the last
    /// measured burst.
    pub fps: Option<u32>,
}

/// Rectangle of one diagnostic pill in window-logical coordinates.
/// Stored after each render so the screen layer can hit-test mouse
/// presses without re-running the layout pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct PillRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PillRect {
    pub fn contains(&self, mx: f32, my: f32) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && mx >= self.x
            && mx <= self.x + self.w
            && my >= self.y
            && my <= self.y + self.h
    }
}

/// Which diagnostic pill the user clicked. The popup title and the
/// initial filter both follow from this so the user can pop a list of
/// just errors or just warnings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticPill {
    Error,
    Warn,
}

/// Shared status-line click classification. Hosts still execute the
/// side effect (open git diff, toggle split strip, open diagnostics),
/// but all hit testing stays with the lifted status-line geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatusLineClickAction {
    ToggleSplit,
    ToggleGitDiff,
    Diagnostics { pill: DiagnosticPill },
    ToggleLspPopup,
}

/// Subset of the native `IdeTheme` the status line actually reads.
/// Stored as packed `u32` rgb values so the conversion helpers can
/// mirror `IdeTheme::f32` / `IdeTheme::u8` bit-for-bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusPalette {
    pub bg: u32,
    pub surface: u32,
    pub muted: u32,
    pub red: u32,
    pub green: u32,
    pub yellow: u32,
    pub blue: u32,
    pub magenta: u32,
    pub cyan: u32,
    pub black: u32,
}

impl StatusPalette {
    pub fn f32(self, color: u32) -> [f32; 4] {
        let r = ((color >> 16) & 0xff) as f32 / 255.0;
        let g = ((color >> 8) & 0xff) as f32 / 255.0;
        let b = (color & 0xff) as f32 / 255.0;
        [r, g, b, 1.0]
    }

    pub fn u8(self, color: u32) -> [u8; 4] {
        [
            ((color >> 16) & 0xff) as u8,
            ((color >> 8) & 0xff) as u8,
            (color & 0xff) as u8,
            255,
        ]
    }
}

// ─── Segment helpers ──────────────────────────────────────────────────

struct TwoTonePill {
    icon_glyph: &'static str,
    label: String,
    icon_bg: [f32; 4],
    icon_fg: [u8; 4],
    text_bg: [f32; 4],
    text_fg: [u8; 4],
}

/// Right-cluster branch pill — needs a custom render because its
/// label is split into three differently-colored segments (branch
/// name, `+N` in green, `-N` in red), which the generic two-tone
/// renderer can't express.
struct BranchRightMeta {
    branch_label: String,
    added_str: Option<String>,
    deleted_str: Option<String>,
    #[allow(dead_code)]
    icon_w: f32,
    branch_w: f32,
    added_w: f32,
    #[allow(dead_code)]
    deleted_w: f32,
    icon_section_w: f32,
    #[allow(dead_code)]
    text_section_w: f32,
    base_w: f32,
    icon_bg: [f32; 4],
    icon_fg: [u8; 4],
    text_fg: [u8; 4],
}

struct InlineIconText {
    icon: &'static str,
    label: String,
    opts: DrawOpts,
    width: f32,
}

/// One diagnostic count pill (error or warn).
struct DiagPillSpec {
    kind: DiagnosticPill,
    glyph: &'static str,
    count: u64,
    bg: [f32; 4],
    fg: [u8; 4],
}

/// Which edge of the strip the pill belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

fn corner_radii(side: Side, r: f32) -> [f32; 4] {
    // [top_left, top_right, bottom_right, bottom_left]
    match side {
        Side::Left => [0.0, r, r, 0.0],
        Side::Right => [r, 0.0, 0.0, r],
    }
}

// ─── Pure helpers ─────────────────────────────────────────────────────

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn draw_thick(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    scale: f32,
) {
    let ui = sugarloaf.text_mut();
    ui.draw(x, y, text, opts);
    ui.draw(x + FAUX_BOLD_OFFSET * scale, y, text, opts);
}

/// Vertical position for icon glyphs. Both the web fallback font and
/// CoreText's patched-font metrics place Nerd Font / Font Awesome
/// glyphs a little higher than the surrounding status text. Keep the
/// correction proportional to the live chrome zoom so Retina and
/// non-Retina displays share the same logical baseline.
fn icon_baseline_y(y: f32, font_size: f32) -> f32 {
    y + font_size
        * icon_baseline_shift_em(cfg!(target_arch = "wasm32"), cfg!(target_os = "macos"))
}

fn icon_baseline_shift_em(is_web: bool, is_macos: bool) -> f32 {
    if is_web {
        0.12
    } else if is_macos {
        // 0.96 logical px at the status line's 12 px baseline.
        0.08
    } else {
        0.0
    }
}

fn icon_gap_text_width(
    sugarloaf: &mut Sugarloaf,
    icon: &str,
    text: &str,
    opts: &DrawOpts,
) -> f32 {
    sugarloaf.text_mut().measure(icon, opts)
        + sugarloaf.text_mut().measure("  ", opts)
        + if text.is_empty() {
            0.0
        } else {
            sugarloaf.text_mut().measure(text, opts)
        }
}

fn draw_gap_text_thick(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    icon: &str,
    text: &str,
    opts: &DrawOpts,
    scale: f32,
) {
    let icon_w = sugarloaf.text_mut().measure(icon, opts);
    let gap_w = sugarloaf.text_mut().measure("  ", opts);
    draw_thick(
        sugarloaf,
        x,
        icon_baseline_y(y, opts.font_size),
        icon,
        opts,
        scale,
    );
    if !text.is_empty() {
        draw_thick(sugarloaf, x + icon_w + gap_w, y, text, opts, scale);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_scramble_label(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    target: &str,
    base_opts: &DrawOpts,
    t: f32,
    elapsed_ms: f32,
    scale: f32,
) {
    let chars: Vec<char> = target.chars().collect();
    let n = chars.len().max(1) as f32;
    let mut cursor = x;

    // Frame-level scramble seed. Cycles every ~30ms so the random
    // glyphs visibly churn through several values mid-transition.
    let seed = (elapsed_ms / 30.0) as u64;

    for (i, &target_ch) in chars.iter().enumerate() {
        // Each character locks at its own threshold: char 0 settles
        // first, then 1, then 2, …, with the last char locking at t=1.
        let lock_threshold = (i as f32 + 1.0) / n;
        let locked = t >= lock_threshold;

        let (display_ch, color) = if locked {
            (target_ch, base_opts.color)
        } else {
            let h = mix_seed(seed, i);
            let scrambled = (b'A' + (h % 26) as u8) as char;
            let hue = (elapsed_ms * 0.6 + i as f32 * 47.0) % 360.0;
            (scrambled, hsl_to_u8(hue, 1.0, 0.65))
        };

        let mut buf = [0u8; 4];
        let s_str = display_ch.encode_utf8(&mut buf);
        let opts = DrawOpts {
            color,
            ..*base_opts
        };
        let w = sugarloaf.text_mut().measure(s_str, &opts);
        draw_thick(sugarloaf, cursor, y, s_str, &opts, scale);
        cursor += w;
    }
}

fn mix_seed(seed: u64, idx: usize) -> u32 {
    let mut h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h.rotate_right(27).wrapping_add(idx as u64 * 31);
    h ^= h >> 33;
    h as u32
}

fn hsl_to_u8(h: f32, s: f32, l: f32) -> [u8; 4] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        255,
    ]
}

fn mode_bg(mode: Mode, palette: &StatusPalette) -> [f32; 4] {
    let rgb = match mode {
        Mode::Normal => palette.blue,
        Mode::Insert => palette.magenta,
        Mode::Visual => palette.cyan,
        Mode::Replace => palette.yellow,
        Mode::Cmd => palette.green,
        Mode::Terminal => palette.green,
        Mode::Markdown => palette.cyan,
        Mode::Agent => palette.red,
    };
    palette.f32(rgb)
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Visual => "VISUAL",
        Mode::Replace => "REPLACE",
        Mode::Cmd => "COMMAND",
        Mode::Terminal => "TERMINAL",
        Mode::Markdown => "MARKDOWN",
        Mode::Agent => "AGENT",
    }
}

#[allow(dead_code)]
fn primary_glyph(kind: PrimaryKind) -> &'static str {
    match kind {
        PrimaryKind::File => status_glyph("status.file", GLYPH_FILE),
        PrimaryKind::Terminal => status_glyph("status.terminal", GLYPH_TERMINAL),
        PrimaryKind::Agent => status_glyph("status.mode", GLYPH_MODE),
    }
}

fn two_tone_width(
    sugarloaf: &mut Sugarloaf,
    font_size: f32,
    section_pad: f32,
    spec: &TwoTonePill,
    extra_right_pad: f32,
) -> f32 {
    let icon_opts = DrawOpts {
        font_size,
        color: spec.icon_fg,
        bold: true,
        ..DrawOpts::default()
    };
    let text_opts = DrawOpts {
        font_size,
        color: spec.text_fg,
        bold: true,
        ..DrawOpts::default()
    };
    let ui = sugarloaf.text_mut();
    let icon_w = ui.measure(spec.icon_glyph, &icon_opts);
    let text_w = ui.measure(&spec.label, &text_opts);
    let icon_section_w = icon_w + section_pad * 2.0;
    let text_section_w = text_w + section_pad * 2.0;
    icon_section_w + text_section_w + extra_right_pad
}

#[allow(clippy::too_many_arguments)]
fn draw_two_tone_pill(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    pill_y: f32,
    pill_h: f32,
    body_y: f32,
    font_size: f32,
    radius: f32,
    section_pad: f32,
    spec: &TwoTonePill,
    side: Side,
    scale: f32,
    outer_order: u8,
    icon_order: u8,
    extra_right_pad: f32,
) {
    let icon_opts = DrawOpts {
        font_size,
        color: spec.icon_fg,
        bold: true,
        ..DrawOpts::default()
    };
    let text_opts = DrawOpts {
        font_size,
        color: spec.text_fg,
        bold: true,
        ..DrawOpts::default()
    };
    let (icon_w, text_w) = {
        let ui = sugarloaf.text_mut();
        (
            ui.measure(spec.icon_glyph, &icon_opts),
            ui.measure(&spec.label, &text_opts),
        )
    };
    let icon_section_w = icon_w + section_pad * 2.0;
    let text_section_w = text_w + section_pad * 2.0;
    let pill_w = icon_section_w + text_section_w + extra_right_pad;

    let outer = corner_radii(side, radius);
    let (icon_radii, _text_radii) = match side {
        Side::Right => ([radius, 0.0, 0.0, radius], [0.0, 0.0, 0.0, 0.0]),
        Side::Left => ([0.0, 0.0, 0.0, 0.0], [0.0, radius, radius, 0.0]),
    };

    sugarloaf.quad(
        None,
        x,
        pill_y,
        pill_w,
        pill_h,
        spec.text_bg,
        outer,
        DEPTH,
        outer_order,
    );
    sugarloaf.quad(
        None,
        x,
        pill_y,
        icon_section_w,
        pill_h,
        spec.icon_bg,
        icon_radii,
        DEPTH,
        icon_order,
    );
    draw_thick(
        sugarloaf,
        x + section_pad,
        icon_baseline_y(body_y, font_size),
        spec.icon_glyph,
        &icon_opts,
        scale,
    );
    draw_thick(
        sugarloaf,
        x + icon_section_w + section_pad,
        body_y,
        &spec.label,
        &text_opts,
        scale,
    );
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

fn lerp_u8(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t).round() as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t).round() as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t).round() as u8,
        (a[3] as f32 + (b[3] as f32 - a[3] as f32) * t).round() as u8,
    ]
}

// ─── StatusLine state ─────────────────────────────────────────────────

pub struct StatusLine {
    visible: bool,
    info: StatusInfo,
    scale: f32,
    prev_mode: Mode,
    transition_started: Instant,
    /// Debounced dir pill (see `set_info`): what is actually painted,
    /// plus the not-yet-committed candidate and when it first appeared.
    cwd_label_display: Option<String>,
    cwd_label_candidate: Option<(Option<String>, Instant)>,
    error_pill_rect: PillRect,
    warn_pill_rect: PillRect,
    branch_rect: PillRect,
    branch_hovered: bool,
    branch_hover_started: Instant,
    branch_hover_from: f32,
    split_toggle_rect: PillRect,
    split_toggle_enabled: bool,
    split_toggle_hidden: bool,
    /// Hit-rect for the LSP pill (lightning glyph + status label) so
    /// the host can pop a per-buffer LSP overview when clicked. Reset
    /// to default each frame; populated only when the pill is painted.
    lsp_pill_rect: PillRect,
}

impl StatusLine {
    pub fn new() -> Self {
        StatusLine {
            visible: true,
            info: StatusInfo::default(),
            scale: 1.0,
            prev_mode: Mode::default(),
            transition_started: Instant::now()
                - Duration::from_millis(TRANSITION_MS as u64 + 1),
            cwd_label_display: None,
            cwd_label_candidate: None,
            error_pill_rect: PillRect::default(),
            warn_pill_rect: PillRect::default(),
            branch_rect: PillRect::default(),
            branch_hovered: false,
            branch_hover_started: Instant::now()
                - Duration::from_millis(BRANCH_HOVER_MS as u64 + 1),
            branch_hover_from: 0.0,
            split_toggle_rect: PillRect::default(),
            split_toggle_enabled: false,
            split_toggle_hidden: false,
            lsp_pill_rect: PillRect::default(),
        }
    }

    pub fn lsp_pill_at(&self, mx: f32, my: f32) -> bool {
        self.info.lsp_status.is_some() && self.lsp_pill_rect.contains(mx, my)
    }

    pub fn lsp_pill_rect(&self) -> Option<PillRect> {
        if self.info.lsp_status.is_some() && self.lsp_pill_rect.w > 0.0 {
            Some(self.lsp_pill_rect)
        } else {
            None
        }
    }

    pub fn scaled_height(&self) -> f32 {
        STATUS_LINE_HEIGHT * self.scale
    }

    /// Read-only access to the current `StatusInfo` snapshot. Used by
    /// tests + occasional host probes.
    pub fn info(&self) -> &StatusInfo {
        &self.info
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    // ── Update / hit-test API (was `update.rs`) ──────────────────────

    /// Hit-test a mouse press in window-logical coordinates against the
    /// last-rendered diagnostic pills.
    pub fn diagnostic_pill_at(&self, mx: f32, my: f32) -> Option<DiagnosticPill> {
        if self.info.diagnostics.error > 0 && self.error_pill_rect.contains(mx, my) {
            return Some(DiagnosticPill::Error);
        }
        if self.info.diagnostics.warn > 0 && self.warn_pill_rect.contains(mx, my) {
            return Some(DiagnosticPill::Warn);
        }
        None
    }

    pub fn git_branch_at(&self, mx: f32, my: f32) -> bool {
        self.info
            .branch
            .as_deref()
            .is_some_and(|branch| !branch.is_empty())
            && self.branch_rect.contains(mx, my)
    }

    pub fn git_branch_hovered(&self) -> bool {
        self.branch_hovered
    }

    pub fn set_git_branch_hovered(&mut self, hovered: bool) -> bool {
        let hovered = hovered
            && self
                .info
                .branch
                .as_deref()
                .is_some_and(|branch| !branch.is_empty());
        if self.branch_hovered == hovered {
            return false;
        }
        self.branch_hover_from = self.branch_hover_progress();
        self.branch_hovered = hovered;
        self.branch_hover_started = Instant::now();
        true
    }

    pub fn diagnostic_pill_anchor(&self, pill: DiagnosticPill) -> Option<(f32, f32)> {
        let r = match pill {
            DiagnosticPill::Error => self.error_pill_rect,
            DiagnosticPill::Warn => self.warn_pill_rect,
        };
        if r.w <= 0.0 {
            None
        } else {
            Some((r.x + r.w / 2.0, r.y))
        }
    }

    pub fn set_split_toggle(&mut self, enabled: bool, hidden: bool) {
        self.split_toggle_enabled = enabled;
        self.split_toggle_hidden = hidden;
        if !enabled {
            self.split_toggle_rect = PillRect::default();
        }
    }

    pub fn split_toggle_at(&self, mx: f32, my: f32) -> bool {
        self.split_toggle_enabled && self.split_toggle_rect.contains(mx, my)
    }

    pub fn click_action_at(&self, mx: f32, my: f32) -> Option<StatusLineClickAction> {
        if self.split_toggle_at(mx, my) {
            return Some(StatusLineClickAction::ToggleSplit);
        }
        if self.git_branch_at(mx, my) {
            return Some(StatusLineClickAction::ToggleGitDiff);
        }
        if self.lsp_pill_at(mx, my) {
            return Some(StatusLineClickAction::ToggleLspPopup);
        }
        self.diagnostic_pill_at(mx, my)
            .map(|pill| StatusLineClickAction::Diagnostics { pill })
    }

    #[allow(dead_code)]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    #[allow(dead_code)]
    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
    }

    pub fn set_info(&mut self, info: StatusInfo) {
        if info.mode != self.info.mode {
            self.prev_mode = self.info.mode;
            self.transition_started = Instant::now();
        }
        // Debounce the dir pill: `cwd_label` is re-derived every frame
        // from two sources (active buffer's parent vs pane cwd) that can
        // alternate during scroll-repeat, which read as flicker. A new
        // label only commits once it has stayed put for a beat; a real
        // `cd` or pane switch still lands fast, single-frame flappers
        // never paint.
        let incoming = info.cwd_label.clone();
        self.info = info;
        if incoming == self.cwd_label_display {
            self.cwd_label_candidate = None;
        } else if self.cwd_label_display.is_none()
            || self
                .cwd_label_candidate
                .as_ref()
                .is_some_and(|(candidate, since)| {
                    *candidate == incoming
                        && since.elapsed().as_secs_f32() * 1000.0 >= CWD_LABEL_SETTLE_MS
                })
        {
            self.cwd_label_display = incoming;
            self.cwd_label_candidate = None;
        } else if self
            .cwd_label_candidate
            .as_ref()
            .map(|(candidate, _)| candidate != &incoming)
            .unwrap_or(true)
        {
            self.cwd_label_candidate = Some((incoming, Instant::now()));
        }
    }

    pub fn is_animating(&self) -> bool {
        self.transition_started.elapsed().as_secs_f32() * 1000.0 < TRANSITION_MS
            || self.branch_hover_started.elapsed().as_secs_f32() * 1000.0
                < BRANCH_HOVER_MS
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(SCALE_MIN, SCALE_MAX);
    }

    fn effective_mode_color(&self, palette: &StatusPalette) -> [f32; 4] {
        let target = mode_bg(self.info.mode, palette);
        let prev = mode_bg(self.prev_mode, palette);
        let elapsed_ms = self.transition_started.elapsed().as_secs_f32() * 1000.0;
        let t = (elapsed_ms / TRANSITION_MS).clamp(0.0, 1.0);
        let eased = ease_out_cubic(t);
        lerp4(prev, target, eased)
    }

    fn branch_hover_progress(&self) -> f32 {
        let target = if self.branch_hovered { 1.0 } else { 0.0 };
        let t = (self.branch_hover_started.elapsed().as_secs_f32() * 1000.0
            / BRANCH_HOVER_MS)
            .clamp(0.0, 1.0);
        self.branch_hover_from + (target - self.branch_hover_from) * ease_out_cubic(t)
    }

    // ── Render (was `render.rs`) ─────────────────────────────────────

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        width: f32,
        palette: &StatusPalette,
    ) {
        if !self.visible || width <= 0.0 {
            return;
        }

        let s = self.scale;
        let strip_h = STATUS_LINE_HEIGHT * s;
        let font_size = FONT_SIZE * s;
        let section_pad = SECTION_PAD_X * s;
        let pill_vpad = PILL_VPAD * s;

        // Background reaches above the strip's nominal top by 1
        // logical pixel — just enough to catch the sub-pixel hairline
        // from rich-text rendering at un-rounded `cell_h` without
        // visibly clipping the bottom of descender glyphs (g, y, p,
        // q) in the editor's last row.
        let bg_y = (y_top - 1.0 * s).max(0.0);
        sugarloaf.rect(
            None,
            x_left,
            bg_y,
            width,
            strip_h + (y_top - bg_y),
            palette.f32(palette.bg),
            DEPTH,
            ORDER_BG,
        );

        // Top border — matches the file tree's frame exactly: `surface`
        // color at `FRAME_STROKE` thickness, so the full-width status
        // bar reads with the same edge as the side panels.
        let border_thickness = (crate::panels::file_tree::FRAME_STROKE * s).max(2.0);
        sugarloaf.rect(
            None,
            x_left,
            y_top,
            width,
            border_thickness,
            palette.f32(palette.surface),
            DEPTH,
            ORDER_BG + 1,
        );

        let pill_h = (strip_h - pill_vpad * 2.0).max(2.0);
        let pill_y = y_top + pill_vpad;
        let radius = pill_h / 2.0;
        let body_y = pill_y + (pill_h - font_size) / 2.0 - 2.0 * s;

        let mode_color = self.effective_mode_color(palette);

        let mut x = x_left;

        let mut mode_label_text = mode_label(self.info.mode).to_string();
        if let Some(pending) = self
            .info
            .pending_keys
            .as_deref()
            .filter(|pending| !pending.is_empty())
        {
            mode_label_text.push_str(" · ");
            mode_label_text.push_str(pending);
        }
        let mode_text_opts = DrawOpts {
            font_size,
            color: palette.u8(palette.black),
            bold: true,
            ..DrawOpts::default()
        };
        let glyph_mode = status_glyph("status.mode", GLYPH_MODE);
        let glyph_prefix_w =
            icon_gap_text_width(sugarloaf, glyph_mode, "", &mode_text_opts);
        let label_w = sugarloaf
            .text_mut()
            .measure(&mode_label_text, &mode_text_opts);
        let mode_pill_w = glyph_prefix_w + label_w + section_pad * 2.0;
        let mode_x = x;
        let tail_overlap = radius * MODE_TAIL_OVERLAP_FRAC;
        let chain_breather = 4.0 * s;

        let workspace_spec: Option<InlineIconText> = None;
        let primary_spec: Option<InlineIconText> = None;

        let workspace_x = mode_x + mode_pill_w - tail_overlap;
        let workspace_w = workspace_spec
            .as_ref()
            .map(|spec| spec.width + section_pad * 2.0 + tail_overlap + chain_breather)
            .unwrap_or(0.0);
        let primary_x = if workspace_spec.is_some() {
            workspace_x + workspace_w - tail_overlap
        } else {
            mode_x + mode_pill_w - tail_overlap
        };

        let cwd_label_raw = self.cwd_label_display.as_deref().filter(|s| !s.is_empty());
        // The directory is context, not the primary status payload. Keep it
        // width-aware so a deep workspace cannot evict diagnostics/LSP/cursor
        // pills from the opposite edge. Preserve the path root and the most
        // useful trailing components (for example `~/…/typescript`).
        let cwd_measure_opts = DrawOpts {
            font_size,
            color: palette.u8(palette.red),
            bold: true,
            ..DrawOpts::default()
        };
        let cwd_text_budget = (width * 0.30).min(360.0 * s).max(96.0 * s);
        let cwd_label_compact = cwd_label_raw.map(|label| {
            let ui = sugarloaf.text_mut();
            compact_path_label(label, cwd_text_budget, |candidate| {
                ui.measure(candidate, &cwd_measure_opts)
            })
        });
        let cwd_label = cwd_label_compact.as_deref();
        let glyph_folder = status_glyph("status.folder", GLYPH_FOLDER);
        let cwd_text_inset_left = tail_overlap + chain_breather;
        let (cwd_pill_w, cwd_text_section_w, cwd_icon_section_w, cwd_icon_w, cwd_text_w) =
            if let Some(label) = cwd_label {
                let icon_opts = DrawOpts {
                    font_size,
                    color: palette.u8(palette.black),
                    bold: true,
                    ..DrawOpts::default()
                };
                let text_opts = DrawOpts {
                    font_size,
                    color: palette.u8(palette.red),
                    bold: true,
                    ..DrawOpts::default()
                };
                let (icon_w, text_w) = {
                    let ui = sugarloaf.text_mut();
                    (
                        ui.measure(glyph_folder, &icon_opts),
                        ui.measure(label, &text_opts),
                    )
                };
                let icon_pad_left = section_pad;
                let icon_pad_right = section_pad + pill_h * 0.25;
                let text_section_w = cwd_text_inset_left + text_w + section_pad;
                let icon_section_w = icon_pad_left + icon_w + icon_pad_right;
                let pill_w = text_section_w + icon_section_w;
                (pill_w, text_section_w, icon_section_w, icon_w, text_w)
            } else {
                (0.0, 0.0, 0.0, 0.0, 0.0)
            };
        let cwd_x = mode_x + mode_pill_w - tail_overlap;
        self.branch_rect = PillRect::default();

        if let Some(label) = cwd_label {
            let icon_opts = DrawOpts {
                font_size,
                color: palette.u8(palette.black),
                bold: true,
                ..DrawOpts::default()
            };
            let text_opts = DrawOpts {
                font_size,
                color: palette.u8(palette.red),
                bold: true,
                ..DrawOpts::default()
            };
            let _ = (cwd_icon_w, cwd_text_w);

            sugarloaf.quad(
                None,
                cwd_x,
                pill_y,
                cwd_pill_w,
                pill_h,
                palette.f32(palette.surface),
                corner_radii(Side::Left, radius),
                DEPTH,
                ORDER_PILL + 1,
            );
            sugarloaf.quad(
                None,
                cwd_x + cwd_text_section_w,
                pill_y,
                cwd_icon_section_w,
                pill_h,
                palette.f32(palette.red),
                [0.0, radius, radius, 0.0],
                DEPTH,
                ORDER_PILL + 2,
            );
            draw_thick(
                sugarloaf,
                cwd_x + cwd_text_inset_left,
                body_y,
                label,
                &text_opts,
                s,
            );
            draw_thick(
                sugarloaf,
                cwd_x + cwd_text_section_w + section_pad,
                icon_baseline_y(body_y, font_size),
                glyph_folder,
                &icon_opts,
                s,
            );
        }

        if let Some(spec) = primary_spec.as_ref() {
            let extra_left_pad = tail_overlap + chain_breather;
            let pill_w = spec.width + section_pad * 2.0 + extra_left_pad;
            sugarloaf.quad(
                None,
                primary_x,
                pill_y,
                pill_w,
                pill_h,
                palette.f32(palette.surface),
                corner_radii(Side::Left, radius),
                DEPTH,
                ORDER_PILL_BACK,
            );
            draw_gap_text_thick(
                sugarloaf,
                primary_x + section_pad + extra_left_pad,
                body_y,
                spec.icon,
                &spec.label,
                &spec.opts,
                s,
            );
            x = primary_x + pill_w;
        } else if workspace_spec.is_some() {
            x = workspace_x + workspace_w;
        } else {
            x = mode_x + mode_pill_w;
        }

        if let Some(spec) = workspace_spec.as_ref() {
            let extra_left_pad = tail_overlap + chain_breather;
            let pill_w = spec.width + section_pad * 2.0 + extra_left_pad;
            sugarloaf.quad(
                None,
                workspace_x,
                pill_y,
                pill_w,
                pill_h,
                palette.f32(palette.surface),
                corner_radii(Side::Left, radius),
                DEPTH,
                ORDER_PILL,
            );
            draw_gap_text_thick(
                sugarloaf,
                workspace_x + section_pad + extra_left_pad,
                body_y,
                spec.icon,
                &spec.label,
                &spec.opts,
                s,
            );
        }

        sugarloaf.quad(
            None,
            mode_x,
            pill_y,
            mode_pill_w,
            pill_h,
            mode_color,
            corner_radii(Side::Left, radius),
            DEPTH,
            ORDER_PILL + 3,
        );
        draw_thick(
            sugarloaf,
            mode_x + section_pad,
            icon_baseline_y(body_y, font_size),
            glyph_mode,
            &mode_text_opts,
            s,
        );
        let elapsed_ms = self.transition_started.elapsed().as_secs_f32() * 1000.0;
        let t = (elapsed_ms / TRANSITION_MS).clamp(0.0, 1.0);
        render_scramble_label(
            sugarloaf,
            mode_x + section_pad + glyph_prefix_w,
            body_y,
            &mode_label_text,
            &mode_text_opts,
            t,
            elapsed_ms,
            s,
        );

        let _ = x;

        let left_end_x = if cwd_label.is_some() {
            cwd_x + cwd_pill_w
        } else {
            mode_x + mode_pill_w
        };

        self.error_pill_rect = PillRect::default();
        self.warn_pill_rect = PillRect::default();
        self.split_toggle_rect = PillRect::default();
        self.lsp_pill_rect = PillRect::default();

        // LSP indicator is now a TwoTonePill in the right chain (see
        // `right` vec construction below) so it looks like the other
        // right-cluster pills (tab position, branch). The legacy
        // `lsp_inline` slot stays as a None placeholder so the existing
        // breathing/overflow-budget arithmetic continues to work
        // without rewiring every offset by hand.
        let mut lsp_inline: Option<(&str, DrawOpts, f32)> = None;

        let glyph_split = status_glyph("status.split", GLYPH_SPLIT);
        let mut split_toggle = self.split_toggle_enabled.then(|| {
            let opts = DrawOpts {
                font_size,
                color: if self.split_toggle_hidden {
                    palette.u8(palette.muted)
                } else {
                    palette.u8(palette.cyan)
                },
                bold: true,
                ..DrawOpts::default()
            };
            let icon_w = sugarloaf.text_mut().measure(glyph_split, &opts);
            let hit_w = icon_w + section_pad * 2.0;
            (opts, icon_w, hit_w)
        });

        let mut diag_pills: Vec<DiagPillSpec> = Vec::with_capacity(2);
        if self.info.diagnostics.error > 0 {
            diag_pills.push(DiagPillSpec {
                kind: DiagnosticPill::Error,
                glyph: status_glyph("status.error", GLYPH_ERROR),
                count: self.info.diagnostics.error,
                bg: palette.f32(palette.red),
                fg: palette.u8(palette.black),
            });
        }
        if self.info.diagnostics.warn > 0 {
            diag_pills.push(DiagPillSpec {
                kind: DiagnosticPill::Warn,
                glyph: status_glyph("status.warn", GLYPH_WARN),
                count: self.info.diagnostics.warn,
                bg: palette.f32(palette.yellow),
                fg: palette.u8(palette.black),
            });
        }
        let mut diag_widths = Vec::with_capacity(diag_pills.len());
        let mut diag_total_w = 0.0;
        let diag_inner_pad = section_pad * 0.9;
        let diag_gap = 6.0 * s;
        for spec in &diag_pills {
            let opts = DrawOpts {
                font_size,
                color: spec.fg,
                bold: true,
                ..DrawOpts::default()
            };
            let count = spec.count.to_string();
            let w = icon_gap_text_width(sugarloaf, spec.glyph, &count, &opts)
                + diag_inner_pad * 2.0;
            diag_widths.push(w);
            diag_total_w += w;
        }
        let mut diag_visible_total = if diag_pills.is_empty() {
            0.0
        } else {
            diag_total_w + diag_gap * (diag_pills.len() - 1) as f32
        };
        let mut right: Vec<TwoTonePill> = Vec::with_capacity(4);
        let mut lsp_pill_index: Option<usize> = None;
        if let Some(status) = self.info.lsp_status {
            // Pill colors mirror the tab-position / branch pills: solid
            // colored icon section + surface text section. Status drives
            // the icon background so the user reads the state at a
            // glance (green = attached, yellow = warming, red = none for
            // this filetype). Label is just "LSP" — the color carries
            // the meaning, like Zed's bolt pill.
            let (icon_bg, text_fg) = match status {
                LspStatus::Active => {
                    (palette.f32(palette.green), palette.u8(palette.green))
                }
                LspStatus::Initializing => {
                    (palette.f32(palette.yellow), palette.u8(palette.yellow))
                }
                LspStatus::Missing => (palette.f32(palette.red), palette.u8(palette.red)),
            };
            lsp_pill_index = Some(right.len());
            let label = self
                .info
                .lsp_label
                .as_deref()
                .filter(|label| !label.is_empty())
                .unwrap_or("LSP");
            right.push(TwoTonePill {
                icon_glyph: status_glyph("status.lsp", GLYPH_LSP),
                label: format!(" {label}"),
                icon_bg,
                icon_fg: palette.u8(palette.black),
                text_bg: palette.f32(palette.surface),
                text_fg,
            });
        }
        if let Some((cur, total)) = self.info.cursor_lines {
            right.push(TwoTonePill {
                icon_glyph: status_glyph("status.lines", GLYPH_LINES),
                label: format!(" {cur}/{total}"),
                icon_bg: palette.f32(palette.green),
                icon_fg: palette.u8(palette.black),
                text_bg: palette.f32(palette.surface),
                text_fg: palette.u8(palette.green),
            });
        }
        // Frame-rate pill. The colored section carries the literal
        // "FPS" tag (packs can still swap it via the icon key) and the
        // surface section carries just the number. Last in the vec so
        // the overflow loop drops it before the tab-position / LSP
        // pills when width gets tight.
        if let Some(fps) = self.info.fps {
            right.push(TwoTonePill {
                icon_glyph: status_glyph("status.fps", "FPS"),
                label: format!(" {fps}"),
                icon_bg: palette.f32(palette.cyan),
                icon_fg: palette.u8(palette.black),
                text_bg: palette.f32(palette.surface),
                text_fg: palette.u8(palette.cyan),
            });
        }

        let glyph_branch = status_glyph("status.branch", GLYPH_BRANCH);
        let mut branch_right_meta: Option<BranchRightMeta> = self
            .info
            .branch
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|branch| {
                let hover_t = self.branch_hover_progress();
                let icon_bg = lerp4(
                    palette.f32(palette.blue),
                    palette.f32(palette.cyan),
                    hover_t * 0.7,
                );
                let text_fg = lerp_u8(
                    palette.u8(palette.blue),
                    palette.u8(palette.cyan),
                    hover_t * 0.7,
                );
                let icon_opts = DrawOpts {
                    font_size,
                    color: palette.u8(palette.black),
                    bold: true,
                    ..DrawOpts::default()
                };
                let text_opts = DrawOpts {
                    font_size,
                    color: text_fg,
                    bold: true,
                    ..DrawOpts::default()
                };
                let added_str = self
                    .info
                    .git_changes
                    .filter(|c| c.added > 0)
                    .map(|c| format!(" +{}", c.added));
                let deleted_str = self
                    .info
                    .git_changes
                    .filter(|c| c.deleted > 0)
                    .map(|c| format!(" -{}", c.deleted));
                let added_opts = DrawOpts {
                    font_size,
                    color: palette.u8(palette.green),
                    bold: true,
                    ..DrawOpts::default()
                };
                let deleted_opts = DrawOpts {
                    font_size,
                    color: palette.u8(palette.red),
                    bold: true,
                    ..DrawOpts::default()
                };
                let (icon_w, branch_w, added_w, deleted_w) = {
                    let ui = sugarloaf.text_mut();
                    (
                        ui.measure(glyph_branch, &icon_opts),
                        ui.measure(branch, &text_opts),
                        added_str
                            .as_deref()
                            .map(|s| ui.measure(s, &added_opts))
                            .unwrap_or(0.0),
                        deleted_str
                            .as_deref()
                            .map(|s| ui.measure(s, &deleted_opts))
                            .unwrap_or(0.0),
                    )
                };
                let text_w = branch_w + added_w + deleted_w;
                let icon_section_w = icon_w + section_pad * 2.0;
                let text_section_w = text_w + section_pad * 2.0;
                let base_w = icon_section_w + text_section_w;
                BranchRightMeta {
                    branch_label: branch.to_string(),
                    added_str,
                    deleted_str,
                    icon_w,
                    branch_w,
                    added_w,
                    deleted_w,
                    icon_section_w,
                    text_section_w,
                    base_w,
                    icon_bg,
                    icon_fg: palette.u8(palette.black),
                    text_fg,
                }
            });

        let tail_overlap_r = radius * MODE_TAIL_OVERLAP_FRAC;
        let overlap_breather_r = 6.0 * s;
        let right_base_widths: Vec<f32> = right
            .iter()
            .map(|spec| two_tone_width(sugarloaf, font_size, section_pad, spec, 0.0))
            .collect();
        let branch_base_w = branch_right_meta.as_ref().map(|m| m.base_w).unwrap_or(0.0);
        let lsp_w_full = lsp_inline.as_ref().map_or(0.0, |(_, _, w)| *w);
        let split_w_full = split_toggle.as_ref().map_or(0.0, |(_, _, w)| *w);
        let diag_total_full = diag_visible_total;
        let measure_total = |right_n: usize,
                             branch_active: bool,
                             keep_diag: bool,
                             keep_lsp: bool,
                             keep_split: bool|
         -> f32 {
            let chain_n = right_n + (branch_active as usize);
            let pills_visible = if chain_n == 0 {
                0.0
            } else {
                let mut sum: f32 = right_base_widths.iter().take(right_n).sum();
                if branch_active {
                    sum += branch_base_w;
                }
                sum + (chain_n as f32 - 1.0) * (overlap_breather_r - tail_overlap_r)
            };
            let lsp_w = if keep_lsp { lsp_w_full } else { 0.0 };
            let split_w = if keep_split { split_w_full } else { 0.0 };
            let diag_w = if keep_diag { diag_total_full } else { 0.0 };
            let split_gap = if keep_split && (keep_lsp || chain_n > 0 || keep_diag) {
                section_pad
            } else {
                0.0
            };
            let lsp_gap = if keep_lsp && (chain_n > 0 || keep_diag) {
                section_pad * 2.0
            } else {
                0.0
            };
            let diag_section_pad = if keep_diag && chain_n > 0 {
                section_pad
            } else {
                0.0
            };
            split_w
                + split_gap
                + lsp_w
                + lsp_gap
                + diag_w
                + diag_section_pad
                + pills_visible
        };
        let available_for_right = (x_left + width - left_end_x - section_pad).max(0.0);
        let mut right_n = right.len();
        let mut branch_active = branch_right_meta.is_some();
        let mut keep_diag = !diag_pills.is_empty();
        let mut keep_lsp = lsp_inline.is_some();
        let mut keep_split = split_toggle.is_some();
        loop {
            let total =
                measure_total(right_n, branch_active, keep_diag, keep_lsp, keep_split);
            if total <= available_for_right {
                break;
            }
            if right_n > 0 {
                right_n -= 1;
            } else if branch_active {
                branch_active = false;
            } else if keep_diag {
                keep_diag = false;
            } else if keep_lsp {
                keep_lsp = false;
            } else if keep_split {
                keep_split = false;
            } else {
                break;
            }
        }
        right.truncate(right_n);
        if !branch_active {
            branch_right_meta = None;
        }
        if !keep_diag {
            diag_pills.clear();
            diag_widths.clear();
            diag_total_w = 0.0;
            diag_visible_total = 0.0;
        }
        if !keep_lsp {
            lsp_inline = None;
        }
        if !keep_split {
            split_toggle = None;
        }
        let _ = diag_total_w;

        if split_toggle.is_some()
            || lsp_inline.is_some()
            || !diag_pills.is_empty()
            || !right.is_empty()
            || branch_right_meta.is_some()
        {
            let tail_overlap = radius * MODE_TAIL_OVERLAP_FRAC;
            let overlap_breather = 6.0 * s;
            let chain_has_branch = branch_right_meta.is_some();
            let chain_n = right.len() + chain_has_branch as usize;
            let chain_last_idx = chain_n.saturating_sub(1);
            let branch_chain_pos = 0;
            let right_chain_offset = chain_has_branch as usize;
            let branch_extra = if chain_has_branch && branch_chain_pos != chain_last_idx {
                overlap_breather
            } else {
                0.0
            };
            let branch_pill_w = branch_right_meta
                .as_ref()
                .map(|m| m.base_w + branch_extra)
                .unwrap_or(0.0);
            let mut widths = Vec::with_capacity(right.len());
            let mut total_w = 0.0;
            for (i, spec) in right.iter().enumerate() {
                let chain_pos = i + right_chain_offset;
                let extra = if chain_pos == chain_last_idx {
                    0.0
                } else {
                    overlap_breather
                };
                let w = two_tone_width(sugarloaf, font_size, section_pad, spec, extra);
                widths.push(w);
                total_w += w;
            }
            let pills_visible_total = if chain_n == 0 {
                0.0
            } else {
                let raw = total_w + branch_pill_w;
                raw - tail_overlap * (chain_n - 1) as f32
            };
            let chain_non_empty = chain_n > 0;
            let lsp_width = lsp_inline.as_ref().map_or(0.0, |(_, _, w)| *w);
            let split_width = split_toggle.as_ref().map_or(0.0, |(_, _, w)| *w);
            let split_gap = if split_toggle.is_some()
                && (lsp_inline.is_some() || chain_non_empty || !diag_pills.is_empty())
            {
                section_pad
            } else {
                0.0
            };
            let lsp_gap =
                if lsp_inline.is_some() && (chain_non_empty || !diag_pills.is_empty()) {
                    section_pad * 2.0
                } else {
                    0.0
                };
            let diag_section_pad = if !diag_pills.is_empty() && chain_non_empty {
                section_pad
            } else {
                0.0
            };
            let visible_total = split_width
                + split_gap
                + lsp_width
                + lsp_gap
                + diag_visible_total
                + diag_section_pad
                + pills_visible_total;
            let mut rx = x_left + width - visible_total;

            if let Some((opts, icon_w, w)) = split_toggle.as_ref() {
                draw_thick(
                    sugarloaf,
                    rx + (*w - *icon_w) / 2.0,
                    icon_baseline_y(body_y, font_size),
                    glyph_split,
                    opts,
                    s,
                );
                self.split_toggle_rect = PillRect {
                    x: rx,
                    y: pill_y,
                    w: *w,
                    h: pill_h,
                };
                rx += *w + split_gap;
            }

            // LSP indicator moved into the right-chain TwoTonePill
            // loop below; nothing inline to paint here.
            let _ = lsp_inline;
            let _ = lsp_gap;

            for (i, spec) in diag_pills.iter().enumerate() {
                let pw = diag_widths[i];
                let opts = DrawOpts {
                    font_size,
                    color: spec.fg,
                    bold: true,
                    ..DrawOpts::default()
                };
                let count = spec.count.to_string();
                sugarloaf.rounded_rect(
                    None, rx, pill_y, pw, pill_h, spec.bg, DEPTH, radius, ORDER_PILL,
                );
                draw_gap_text_thick(
                    sugarloaf,
                    rx + diag_inner_pad,
                    body_y,
                    spec.glyph,
                    &count,
                    &opts,
                    s,
                );
                let rect = PillRect {
                    x: rx,
                    y: pill_y,
                    w: pw,
                    h: pill_h,
                };
                match spec.kind {
                    DiagnosticPill::Error => self.error_pill_rect = rect,
                    DiagnosticPill::Warn => self.warn_pill_rect = rect,
                }
                rx += pw + diag_gap;
            }
            if !diag_pills.is_empty() {
                rx = rx - diag_gap + diag_section_pad;
            }

            if let Some(meta) = branch_right_meta.as_ref() {
                let icon_opts = DrawOpts {
                    font_size,
                    color: meta.icon_fg,
                    bold: true,
                    ..DrawOpts::default()
                };
                let text_opts = DrawOpts {
                    font_size,
                    color: meta.text_fg,
                    bold: true,
                    ..DrawOpts::default()
                };
                let added_opts = DrawOpts {
                    font_size,
                    color: palette.u8(palette.green),
                    bold: true,
                    ..DrawOpts::default()
                };
                let deleted_opts = DrawOpts {
                    font_size,
                    color: palette.u8(palette.red),
                    bold: true,
                    ..DrawOpts::default()
                };
                let pill_w_total = meta.base_w + branch_extra;
                let text_section_w_full = pill_w_total - meta.icon_section_w;
                sugarloaf.quad(
                    None,
                    rx,
                    pill_y,
                    pill_w_total,
                    pill_h,
                    palette.f32(palette.surface),
                    corner_radii(Side::Right, radius),
                    DEPTH,
                    ORDER_PILL_BACK,
                );
                sugarloaf.quad(
                    None,
                    rx,
                    pill_y,
                    meta.icon_section_w,
                    pill_h,
                    meta.icon_bg,
                    [radius, 0.0, 0.0, radius],
                    DEPTH,
                    ORDER_PILL,
                );
                draw_thick(
                    sugarloaf,
                    rx + section_pad,
                    icon_baseline_y(body_y, font_size),
                    glyph_branch,
                    &icon_opts,
                    s,
                );
                let mut label_x = rx + meta.icon_section_w + section_pad;
                draw_thick(
                    sugarloaf,
                    label_x,
                    body_y,
                    &meta.branch_label,
                    &text_opts,
                    s,
                );
                label_x += meta.branch_w;
                if let Some(added) = meta.added_str.as_deref() {
                    draw_thick(sugarloaf, label_x, body_y, added, &added_opts, s);
                    label_x += meta.added_w;
                }
                if let Some(deleted) = meta.deleted_str.as_deref() {
                    draw_thick(sugarloaf, label_x, body_y, deleted, &deleted_opts, s);
                }
                self.branch_rect = PillRect {
                    x: rx,
                    y: pill_y,
                    w: pill_w_total,
                    h: pill_h,
                };
                let _ = text_section_w_full;
                rx += pill_w_total - tail_overlap;
            }

            for (i, spec) in right.iter().enumerate() {
                let chain_pos = i + right_chain_offset;
                let layer = chain_pos as u8;
                let extra = if chain_pos == chain_last_idx {
                    0.0
                } else {
                    overlap_breather
                };
                let pill_x = rx;
                draw_two_tone_pill(
                    sugarloaf,
                    rx,
                    pill_y,
                    pill_h,
                    body_y,
                    font_size,
                    radius,
                    section_pad,
                    spec,
                    Side::Right,
                    s,
                    ORDER_PILL_BACK + layer,
                    ORDER_PILL + layer,
                    extra,
                );
                // Capture the LSP pill's geometry while we're inside
                // the loop that knows the per-pill x — the click
                // dispatcher needs the rect to hit-test the popup
                // trigger.
                if lsp_pill_index == Some(i) {
                    self.lsp_pill_rect = PillRect {
                        x: pill_x,
                        y: pill_y,
                        w: widths[i] + extra,
                        h: pill_h,
                    };
                }
                rx += widths[i] - tail_overlap;
            }
        }
    }

    /// `IdeTheme`-flavored render entry point. Lifted from the desktop
    /// golden shim (`frontends/neoism/src/chrome/panels/status_line.rs`)
    /// so call sites that already carry an `IdeTheme` can drop the
    /// shim and target this crate directly. Internally packs the ten
    /// colors the status line reads into a `StatusPalette` and
    /// forwards to the inherent `render` above.
    pub fn render_with_ide_theme(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        width: f32,
        theme: &IdeTheme,
    ) {
        let palette = StatusPalette {
            bg: theme.bg,
            surface: theme.surface,
            muted: theme.muted,
            red: theme.red,
            green: theme.green,
            yellow: theme.yellow,
            blue: theme.blue,
            magenta: theme.magenta,
            cyan: theme.cyan,
            black: theme.black,
        };
        self.render(sugarloaf, x_left, y_top, width, &palette);
    }
}

/// Collapse a path to its root plus the longest trailing component sequence
/// that fits the supplied pixel budget. The measurer keeps this independent
/// of any particular font metrics and makes the policy directly testable.
fn compact_path_label(
    label: &str,
    max_width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> String {
    if label.is_empty() || measure(label) <= max_width {
        return label.to_string();
    }

    let separator = if label.contains('/') { '/' } else { '\\' };
    let separator_text = separator.to_string();
    let (prefix, body) = if label.starts_with("~/") || label.starts_with("~\\") {
        (&label[..2], &label[2..])
    } else if label.starts_with('/') || label.starts_with('\\') {
        (&label[..1], &label[1..])
    } else if label.len() >= 3
        && label.as_bytes()[1] == b':'
        && matches!(label.as_bytes()[2], b'/' | b'\\')
    {
        (&label[..3], &label[3..])
    } else {
        ("", label)
    };
    let components = body
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.is_empty() {
        return "…".to_string();
    }

    let marker = if prefix.is_empty() {
        format!("…{separator}")
    } else {
        format!("{prefix}…{separator}")
    };
    for start in 0..components.len() {
        let candidate = format!(
            "{marker}{}",
            components[start..].join(&separator_text)
        );
        if measure(&candidate) <= max_width {
            return candidate;
        }
    }

    // An individual final component can itself be wider than the budget.
    // Retain its suffix (usually the differentiating part) rather than
    // allowing it to overlap the right cluster.
    let tail = components.last().copied().unwrap_or_default();
    let mut suffix = String::new();
    for ch in tail.chars().rev() {
        let candidate_suffix = format!("{ch}{suffix}");
        let candidate = format!("…{candidate_suffix}");
        if measure(&candidate) > max_width {
            break;
        }
        suffix = candidate_suffix;
    }
    if suffix.is_empty() {
        "…".to_string()
    } else {
        format!("…{suffix}")
    }
}

impl Default for StatusLine {
    fn default() -> Self {
        StatusLine::new()
    }
}

// ─── Panel trait impl ────────────────────────────────────────────────

impl Panel for StatusLine {
    fn handle_event(&mut self, event: &UiEvent, _ctx: &mut PanelContext) {
        // The status line is read-only: clicks on its pills are routed
        // by the host through the dedicated hit-test methods (e.g.
        // `diagnostic_pill_at`, `git_branch_at`, `split_toggle_at`),
        // not through `handle_event`. Only events that need to reach
        // the panel's own state are theme-change (palette refresh,
        // handled by the host passing a new `StatusPalette` next
        // frame) and resize/tick (no state to update — the strip is
        // re-laid-out every paint).
        match event {
            UiEvent::Key(_)
            | UiEvent::Text(_)
            | UiEvent::Composition(_)
            | UiEvent::PointerMove { .. }
            | UiEvent::PointerDown { .. }
            | UiEvent::PointerUp { .. }
            | UiEvent::PointerLeave
            | UiEvent::Wheel { .. }
            | UiEvent::Focus(_)
            | UiEvent::Resize { .. }
            | UiEvent::Theme(_)
            | UiEvent::Tick(_)
            | UiEvent::ServiceReply { .. } => {}
        }
    }

    /// The trait `draw` is a no-op for this panel: the host calls the
    /// inherent `render` method directly with the resolved
    /// `StatusPalette` (chrome's full palette is wider than
    /// `ChromeTheme`'s small token set, so we can't derive the status
    /// line's color budget from `ctx.theme` alone yet). Once
    /// `ChromeTheme` grows to mirror `IdeTheme` end-to-end this method
    /// will pick up the work.
    fn draw(
        &self,
        _sugarloaf: &mut Sugarloaf,
        _layout: &PanelLayout,
        _ctx: &PanelContext,
    ) {
    }

    fn name(&self) -> &str {
        "status_line"
    }
}

#[cfg(test)]
mod tests {
    use super::{compact_path_label, icon_baseline_shift_em};

    #[test]
    fn status_icons_use_platform_font_metric_corrections() {
        assert_eq!(icon_baseline_shift_em(false, false), 0.0);
        assert_eq!(icon_baseline_shift_em(false, true), 0.08);
        assert_eq!(icon_baseline_shift_em(true, false), 0.12);
        assert_eq!(icon_baseline_shift_em(true, true), 0.12);
    }

    #[test]
    fn deep_status_paths_keep_the_root_and_useful_tail() {
        let measured_chars = |text: &str| text.chars().count() as f32;
        assert_eq!(
            compact_path_label(
                "~/projects/neoism/fixtures/editor-diagnostics/typescript",
                20.0,
                measured_chars,
            ),
            "~/…/typescript"
        );
        assert_eq!(
            compact_path_label("/workspace/packages/api/src", 19.0, measured_chars),
            "/…/packages/api/src"
        );
        assert_eq!(
            compact_path_label("short/path", 20.0, measured_chars),
            "short/path"
        );
    }
}
