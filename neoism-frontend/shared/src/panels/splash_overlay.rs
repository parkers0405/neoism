// GPU overlay for the splash banner. The cell-based splash
// (`terminal_splash::splash_bytes`) reserves three vertical
// bands of blank rows; this module paints, in order:
//
//   1. An opaque pane backdrop that prevents shell startup output
//      from bleeding through the launch screen.
//   2. A multi-pass glow render of the rasterised wordmark PNG
//      — base image at full alpha plus several offset copies
//      at low alpha to make the wordmark *itself* breathe (the
//      transparent PNG background means the glow stays masked
//      to the white pixels — no rectangle washing the bg).
//   3. The "neoism · terminal" tagline, rendered via sugarloaf
//      text inside the tagline band.
//   4. Four rounded-rect menu buttons (Open file tree / Neoism Agent
//      / Search / Command palette) inside the menu band, each with hover
//      + click states and click-to-shortcut wired through the
//      input layer.
//   5. The opencode-style click "fidget" on the wordmark — a
//      brief image squash, staggered expanding rings, and a
//      soft white burst at the click point.
//
// Visibility predicate (mirrored in `Renderer::run` and
// `Screen` button hit-tests): pane is terminal, alt-screen off,
// `terminal.history_size() == 0`, `splash_injection` recorded.

// Native frontend consumes this module via a re-export shim at
// `frontends/neoism/src/neoism/splash_overlay.rs`; both web and
// native callers go through the same `SplashOverlay` /
// `SplashInjection` types defined here.
use web_time::Instant;

use sugarloaf::text::DrawOpts;
use sugarloaf::{
    ColorType, GraphicData, GraphicDataEntry, GraphicId, GraphicOverlay, Sugarloaf,
};

use crate::panels::command_palette::commands::CommandService;
use crate::primitives::IdeTheme;

const WORDMARK_PNG: &[u8] = include_bytes!("../../assets/splash/neoism-wordmark.png");

pub const SPLASH_WORDMARK_IMAGE_ID: u32 = 0xA0DF_0001;

/// Synthetic panel id for splash image overlays. Image overlays
/// whose panel id is absent from `state.content.states` default
/// to visible — using a value no rich-text panel ever takes
/// keeps the wordmark painting independent of any pane's
/// rich-text state.
pub const SPLASH_PANEL_ID: usize = usize::MAX - 9;

/// POD mirror of the native `crate::context::SplashInjection`
/// type — carries only the fields this module actually reads
/// from the injection (`wordmark_row`, `wordmark_cells_h`,
/// `gap_cells_h`, `menu_cells_h`). The native frontend converts
/// its full `SplashInjection` into this struct at the call site.
#[derive(Clone, Copy, Debug, Default)]
pub struct SplashInjection {
    /// Row index in the *terminal grid* (not absolute scrollback)
    /// where the wordmark's first row lives at injection time.
    pub wordmark_row: usize,
    /// Wordmark cell height — used to compute pixel extents from
    /// cell coords later.
    pub wordmark_cells_h: usize,
    /// Cell rows of breathing room between the wordmark band and
    /// the menu band. Captured at inject time because the adaptive
    /// layout might use a smaller value than the
    /// `WORDMARK_TO_MENU_GAP_ROWS` const on a small pane.
    pub gap_cells_h: usize,
    /// Cell rows reserved for the menu buttons. Same reason —
    /// adaptive layout can shrink this on small panes.
    pub menu_cells_h: usize,
}

const RIPPLE_SEGMENTS: usize = 48;

/// Image squash on click — depth and recovery time. Springy:
/// snaps inward fast then overshoots outward before settling.
const SQUASH_DEPTH: f32 = 0.10;
const SQUASH_LIFE_MS: f32 = 460.0;

/// Soft white burst at click point. Sized as a fraction of
/// wordmark height and clamped to the letter band so it never
/// extends past the top/bottom of the letters — reads as a
/// small tap that echoes through the letter, not a screen-wide
/// ring.
const BURST_RADIUS_FACTOR: f32 = 0.32;
const BURST_LIFE_MS: f32 = 280.0;

/// How long the splash takes to fade + scale + drift up out of
/// view once the user runs their first command. Long enough to
/// read as an animation, short enough to not get in the way.
const DISMISS_LIFE_MS: f32 = 420.0;
/// How far the wordmark / menu drifts upward during dismiss,
/// expressed as a fraction of the band height.
const DISMISS_RISE_FACTOR: f32 = 0.35;
/// How small the wordmark scales to at the end of dismiss.
const DISMISS_SCALE_END: f32 = 0.84;

/// Number of letters in "neoism".
const LETTER_COUNT: usize = 6;
/// Per-letter hover spring damping — lower = snappier in,
/// higher = slower easing out. Frame-rate independent because
/// we step against the actual elapsed time.
const LETTER_HOVER_RATE: f32 = 14.0;
/// Maximum extra scale applied to a hovered letter (1.0 + this).
const LETTER_HOVER_SCALE: f32 = 0.18;
/// Vertical lift of a hovered letter, fraction of letter height.
const LETTER_HOVER_LIFT: f32 = 0.18;
/// Idle shimmer — per-letter scale modulation amplitude. Staggered
/// phases across letters create a wave reading.
const LETTER_SHIMMER_AMP: f32 = 0.025;
/// Period of the idle shimmer in seconds.
const LETTER_SHIMMER_PERIOD: f32 = 3.4;

/// Slow ambient breathing on the wordmark itself. Modulates the
/// alpha of a stacked glow halo around the white pixels.
const BREATH_PERIOD_SECS: f32 = 4.6;
const BREATH_HALO_MIN: f32 = 0.05;
const BREATH_HALO_MAX: f32 = 0.18;

const ORDER: u8 = 7;
const BACKDROP_ORDER: u8 = ORDER - 1;
const DEPTH: f32 = 0.0;

// Icons resolve through `CommandService::icon_themed()` at draw time
// so the splash, command sheet, and palette share one canonical glyph
// per service (and Mash Up Pack `palette.*` overrides reach all
// three). Search has no owning service — it keeps its own glyph.
const MENU: [MenuSpec; 5] = [
    MenuSpec {
        icon: MenuIcon::Service(CommandService::Workspace),
        label: "Open file tree",
        keybind: "Alt + E",
    },
    MenuSpec {
        icon: MenuIcon::Service(CommandService::Markdown),
        label: "Notes",
        keybind: "Alt + N",
    },
    MenuSpec {
        icon: MenuIcon::Service(CommandService::Agent),
        label: "Neoism Agent",
        keybind: "Alt + A",
    },
    MenuSpec {
        icon: MenuIcon::Glyph("\u{f002}"),
        label: "Search",
        keybind: "Alt + S",
    },
    MenuSpec {
        icon: MenuIcon::Service(CommandService::Neoism),
        label: "Command palette",
        keybind: "Alt + P",
    },
];

const MENU_BTN_H: f32 = 42.0;
const MENU_BTN_GAP: f32 = 8.0;
const MENU_RADIUS: f32 = 10.0;
const MENU_BTN_PAD: f32 = 22.0;
const MENU_ICON_SLOT: f32 = 22.0;
const MENU_LABEL_GAP: f32 = 14.0;
const MENU_KEY_GAP: f32 = 32.0;
const MENU_LABEL_FONT: f32 = 17.0;
const MENU_KEY_FONT: f32 = 16.0;
const MENU_ICON_FONT: f32 = 17.0;

#[derive(Clone, Copy, Debug)]
struct Click {
    cx: f32,
    cy: f32,
    started: Instant,
}

#[derive(Default)]
pub struct SplashOverlay {
    started: Option<Instant>,
    click: Option<Click>,
    image_registered: bool,
    /// Letter tint cycle the wordmark pixels were last uploaded with
    /// (the pack's `[wordmark] colors`, or `[theme.fg]`). The PNG is a
    /// white glyph; the tint keeps the splash legible on light themes
    /// and lets packs paint the letters. A change forces a re-upload —
    /// the texture cache keys on `transmit_time`.
    wordmark_tint: Option<Vec<u32>>,
    /// Mouse position from the last hover event, in window-local
    /// logical pixels. Used for menu-button hover state.
    mouse: Option<(f32, f32)>,
    /// Wordmark image rect cached on the last render — used by
    /// the input layer to translate wordmark clicks into clicks
    /// at this position for the fidget.
    wordmark_rect: Option<[f32; 4]>,
    /// Per-menu-button rects cached on the last render — used by
    /// the input layer to translate clicks into shortcut
    /// actions.
    menu_rects: [Option<[f32; 4]>; 5],
    /// `Some(t)` for the duration of the dismiss animation
    /// (when the user runs their first command and the splash
    /// fades + scales + drifts up out of view). `None` while the
    /// splash is fully visible or fully gone.
    dismiss_started: Option<Instant>,
    /// Per-letter hover progress in [0, 1]. Springs toward 1
    /// when the cursor is over the letter and back toward 0
    /// when it leaves. Drives per-letter scale + lift each
    /// frame, giving the wordmark a tactile "letter pops up
    /// under the mouse" reading.
    letter_hover: [f32; LETTER_COUNT],
    /// Frame-time accumulator for the spring update — lets
    /// hover easing run frame-rate independent.
    last_frame_at: Option<Instant>,
}

#[derive(Clone, Copy)]
struct MenuSpec {
    icon: MenuIcon,
    label: &'static str,
    keybind: &'static str,
}

#[derive(Clone, Copy)]
enum MenuIcon {
    /// Canonical per-service glyph (palette/sheet/splash stay in sync).
    Service(CommandService),
    /// A glyph with no owning command service.
    Glyph(&'static str),
}

impl MenuIcon {
    fn resolve(self) -> &'static str {
        match self {
            MenuIcon::Service(service) => service.icon_themed(),
            MenuIcon::Glyph(glyph) => glyph,
        }
    }
}

/// Per-frame, zoom-scaled dimensions for the menu buttons. We
/// compute these once in `render` (multiplying every const by
/// the live `chrome_scale`) and pass them down to
/// `draw_menu_button` so it doesn't have to recompute the same
/// math three times per frame.
#[derive(Clone, Copy)]
struct MenuDims {
    label_font: f32,
    key_font: f32,
    icon_font: f32,
    pad: f32,
    icon_slot: f32,
    label_gap: f32,
    radius: f32,
}

impl SplashOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_mouse(&mut self, pos: Option<(f32, f32)>) {
        self.mouse = pos;
    }

    /// Returns the menu-button index under (x, y), if any.
    pub fn menu_hit(&self, x: f32, y: f32) -> Option<usize> {
        for (i, slot) in self.menu_rects.iter().enumerate() {
            if let Some([rx, ry, rw, rh]) = *slot {
                if x >= rx && x <= rx + rw && y >= ry && y <= ry + rh {
                    return Some(i);
                }
            }
        }
        None
    }

    pub fn wordmark_hit(&self, x: f32, y: f32) -> bool {
        let Some([rx, ry, rw, rh]) = self.wordmark_rect else {
            return false;
        };
        x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
    }

    /// Pop the opencode-style fidget at (x, y). Replaces any
    /// in-flight fidget — chained clicks just reset the
    /// animation.
    pub fn pop_click(&mut self, x: f32, y: f32) {
        self.click = Some(Click {
            cx: x,
            cy: y,
            started: Instant::now(),
        });
    }

    pub fn is_animating(&self) -> bool {
        self.started.is_some() || self.click.is_some() || self.dismiss_started.is_some()
    }

    /// True while the dismiss-out animation is still in flight.
    /// `Renderer::run` checks this so it keeps invoking
    /// `render` even after the splash should otherwise be gone,
    /// letting the fade/scale/drift play out fully.
    pub fn is_dismissing(&self) -> bool {
        match self.dismiss_started {
            None => false,
            Some(t) => {
                Instant::now().saturating_duration_since(t).as_secs_f32() * 1000.0
                    < DISMISS_LIFE_MS
            }
        }
    }

    pub fn reset(&mut self) {
        self.click = None;
        self.wordmark_rect = None;
        self.menu_rects = [None; 5];
        self.dismiss_started = None;
        // Keep image cache + start time so re-showing the splash
        // doesn't reset the breathing phase.
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        injection: &SplashInjection,
        pane_origin: (f32, f32),
        pane_size: (f32, f32),
        cell_w: f32,
        cell_h: f32,
        theme: &IdeTheme,
        chrome_scale: f32,
        wants_visible: bool,
        occlusion_rects: &[[f32; 4]],
    ) {
        let _ = cell_w;
        let tint = crate::primitives::look::wordmark_colors_or(theme.fg);
        if !self.image_registered || self.wordmark_tint.as_deref() != Some(&tint) {
            self.image_registered = register_wordmark(sugarloaf, &tint);
            if !self.image_registered {
                return;
            }
            self.wordmark_tint = Some(tint);
        }

        // Clear our own image-overlay queue for this panel before
        // emitting this frame's batch. `image_overlays` is an
        // accumulator — `push_image_overlay` appends, it doesn't
        // replace — so without this drop the per-letter + halo
        // entries from previous frames stay in the vec and the
        // renderer composites them every frame on top of the new
        // batch. On native the host (frontends/neoism's
        // `chrome/renderer/run.rs`) clears this for us before
        // calling render; the neoism-ui Chrome path (web/wasm
        // frontend) does not, so without the self-clear here the
        // wordmark renders as duplicated, smeared letters
        // ("neoisismshrshrshr…"). Self-clearing is the right
        // invariant anyway: this module is the sole producer of
        // SPLASH_PANEL_ID overlays, and the external clear on
        // native becomes idempotent (no-op on an already-empty
        // vec).
        sugarloaf.clear_image_overlays_for(SPLASH_PANEL_ID);

        let now = Instant::now();
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_duration_since(started).as_secs_f32();

        // Dismiss bookkeeping. Visibility flipped to false →
        // start the dismiss timer (or keep counting). Visibility
        // returned to true → cancel any in-flight dismiss so the
        // splash snaps back. After the dismiss life elapses we
        // drop the timer and the renderer-side gate stops
        // calling us.
        if wants_visible {
            self.dismiss_started = None;
        } else if self.dismiss_started.is_none() {
            self.dismiss_started = Some(now);
        }
        let dismiss_t = self
            .dismiss_started
            .map(|t| {
                (now.saturating_duration_since(t).as_secs_f32() * 1000.0
                    / DISMISS_LIFE_MS)
                    .clamp(0.0, 1.0)
            })
            .unwrap_or(0.0);
        if dismiss_t >= 1.0 {
            // Animation done — paint nothing this frame, the
            // renderer will stop calling us next.
            self.wordmark_rect = None;
            self.menu_rects = [None; 5];
            return;
        }
        // Ease-in cubic on the dismiss timeline so it eases out
        // of view rather than starting fast.
        let dismiss_eased = dismiss_t * dismiss_t * (3.0 - 2.0 * dismiss_t);
        let dismiss_alpha = 1.0 - dismiss_eased;
        let dismiss_scale = 1.0 - (1.0 - DISMISS_SCALE_END) * dismiss_eased;
        let dismiss_rise = 0.0; // computed below once band height known

        // The launch screen is a real surface, not transparent chrome over
        // a live shell. In particular, macOS login shells can print
        // "Last login ..." before the splash injection lands; covering the
        // complete terminal pane here prevents those cells (and cleared
        // commands from a restored PTY) from showing through. When a modal
        // is present we retain the old below-modal composition instead of
        // allowing the splash wordmark's above-quad image pass to cover it.
        if occlusion_rects.is_empty() {
            sugarloaf.rect(
                None,
                pane_origin.0,
                pane_origin.1,
                pane_size.0,
                pane_size.1,
                theme.f32(theme.bg),
                DEPTH,
                BACKDROP_ORDER,
            );
        }

        // Compute the reserved bands (in logical pixels) from
        // the cell row anchor. Sizes come from the injection,
        // not from constants — the adaptive layout shrinks
        // these bands proportionally on small panes so the
        // splash never refuses to render.
        let band_top = pane_origin.1 + injection.wordmark_row as f32 * cell_h;
        let wordmark_h = injection.wordmark_cells_h as f32 * cell_h;
        let gap_h = injection.gap_cells_h as f32 * cell_h;
        let menu_top = band_top + wordmark_h + gap_h;

        // Wordmark sized to fill the wordmark band's height,
        // capped at ~50% of pane width so it reads as a logo
        // not a billboard. The PNG is auto-trimmed (no gutter)
        // so band height = visible letter height.
        // Slightly smaller logo per user — 80 % band height,
        // capped at 42 % pane width (was 95 / 50). Reads as a
        // refined header, not a pane-spanning banner.
        let target_h = wordmark_h * 0.80;
        let aspect = crate::panels::terminal_splash::WORDMARK_ASPECT;
        let mut img_w = target_h * aspect;
        let max_w = pane_size.0 * 0.42;
        if img_w > max_w {
            img_w = max_w;
        }
        let img_h = img_w / aspect;
        let dismiss_rise_px = wordmark_h * DISMISS_RISE_FACTOR * dismiss_eased;
        let _ = dismiss_rise; // shadowed by per-band drift

        // Compute squash on click — image briefly compresses
        // toward its centre then bounces back, giving the
        // wordmark a tactile "press" feel.
        let (scale_factor_anim, click_alpha_extra) = self.compute_squash_and_flash(now);
        // Combine click-squash scale with dismiss-shrink scale
        // so both animations play nicely together.
        let combined_scale = (1.0 - SQUASH_DEPTH * scale_factor_anim) * dismiss_scale;
        let scaled_w = img_w * combined_scale;
        let scaled_h = img_h * combined_scale;
        let img_x = pane_origin.0 + (pane_size.0 - scaled_w) / 2.0;
        let img_y = band_top + (wordmark_h - scaled_h) / 2.0 - dismiss_rise_px;
        self.wordmark_rect = Some([img_x, img_y, scaled_w, scaled_h]);

        // Compute the breathing phase up-front so both the
        // grey card pulse and the halo pass can read it.
        let phase = (elapsed / BREATH_PERIOD_SECS) * std::f32::consts::TAU;
        let breath = (phase.sin() * 0.5 + 0.5).clamp(0.0, 1.0);
        let halo_strength = BREATH_HALO_MIN
            + (BREATH_HALO_MAX - BREATH_HALO_MIN) * breath
            + click_alpha_extra * 0.6;

        let scale = sugarloaf.scale_factor();

        // (No bg card — user wants the wordmark floating, not on
        // a tinted plate. Halo + image only.)

        // Halo passes — render the image multiple times offset
        // diagonally with low alpha. Because the image has a
        // transparent background, the halo only appears around
        // the white pixels (the wordmark itself), giving us a
        // breathing glow that stays masked to the text.

        // Per-letter rendering. The wordmark PNG is sliced into
        // LETTER_COUNT equal vertical strips via `source_rect`;
        // each letter gets its own image_overlay with its own
        // hover spring + idle shimmer driving scale and lift.
        // Mouse moves over a letter → that letter pops up; idle
        // → letters wave gently in a staggered shimmer.
        let letter_w = scaled_w / LETTER_COUNT as f32;
        let wordmark_z_index = splash_wordmark_z_index(occlusion_rects);

        // Frame-time delta for the spring update (frame-rate
        // independent easing).
        let dt = match self.last_frame_at {
            Some(prev) => now
                .saturating_duration_since(prev)
                .as_secs_f32()
                .clamp(0.0, 0.10),
            None => 0.0,
        };
        self.last_frame_at = Some(now);

        // Step every letter's hover spring against the live
        // mouse position. Done in a separate loop so the draw
        // loop sees the freshly-eased value.
        let mouse = self.mouse;
        for i in 0..LETTER_COUNT {
            let lx = img_x + i as f32 * letter_w;
            let ly = img_y;
            let lh = scaled_h;
            let mouse_in = mouse
                .map(|(mx, my)| {
                    mx >= lx && mx <= lx + letter_w && my >= ly && my <= ly + lh
                })
                .unwrap_or(false);
            let target = if mouse_in { 1.0 } else { 0.0 };
            let alpha = 1.0 - (-LETTER_HOVER_RATE * dt).exp();
            self.letter_hover[i] += (target - self.letter_hover[i]) * alpha;
        }

        for i in 0..LETTER_COUNT {
            let hov = self.letter_hover[i].clamp(0.0, 1.0);
            // Idle shimmer — staggered phase per letter so the
            // wordmark "ripples" left-to-right.
            let shimmer_phase = (elapsed / LETTER_SHIMMER_PERIOD + i as f32 * 0.16)
                * std::f32::consts::TAU;
            let shimmer = shimmer_phase.sin() * LETTER_SHIMMER_AMP;
            let extra_scale = 1.0 + hov * LETTER_HOVER_SCALE + shimmer;
            let lift = -hov * LETTER_HOVER_LIFT * scaled_h;

            let centre_x = img_x + (i as f32 + 0.5) * letter_w;
            let centre_y = img_y + scaled_h * 0.5 + lift;
            let lw = letter_w * extra_scale;
            let lh = scaled_h * extra_scale;
            let lx = centre_x - lw * 0.5;
            let ly = centre_y - lh * 0.5;

            let u0 = i as f32 / LETTER_COUNT as f32;
            let u1 = (i as f32 + 1.0) / LETTER_COUNT as f32;
            sugarloaf.push_image_overlay(
                SPLASH_PANEL_ID,
                GraphicOverlay {
                    image_id: SPLASH_WORDMARK_IMAGE_ID,
                    x: lx * scale,
                    y: ly * scale,
                    width: lw * scale,
                    height: lh * scale,
                    z_index: wordmark_z_index,
                    source_rect: [u0, 0.0, u1, 1.0],
                },
            );

            // Per-letter halo — a slightly larger duplicate
            // behind the letter, sized off the letter's hover
            // strength so hovering a letter glows specifically
            // that letter rather than the whole wordmark.
            let halo_letter = halo_strength + hov * 0.4;
            let glow_extra = 1.0 + 0.05 * halo_letter;
            let gw = lw * glow_extra;
            let gh = lh * glow_extra;
            let gx = centre_x - gw * 0.5;
            let gy = centre_y - gh * 0.5;
            sugarloaf.push_image_overlay(
                SPLASH_PANEL_ID,
                GraphicOverlay {
                    image_id: SPLASH_WORDMARK_IMAGE_ID,
                    x: gx * scale,
                    y: gy * scale,
                    width: gw * scale,
                    height: gh * scale,
                    z_index: wordmark_z_index,
                    source_rect: [u0, 0.0, u1, 1.0],
                },
            );
        }
        let _ = click_alpha_extra;

        // Menu buttons — every dimension scaled by chrome_scale
        // so Ctrl + / Ctrl - zoom grows / shrinks the splash
        // alongside the rest of the IDE shell. Width is still
        // computed from each row's measured text plus padding
        // so the bg always covers the labels exactly.
        let menu_band_h = injection.menu_cells_h as f32 * cell_h;
        let menu_side_pad =
            (24.0 * chrome_scale.max(0.1)).min((pane_size.0 * 0.08).max(0.0));
        let max_menu_w = (pane_size.0 - menu_side_pad * 2.0).max(1.0);

        let mut s = chrome_scale.max(0.1);
        let mut menu_label_font = MENU_LABEL_FONT * s;
        let mut menu_key_font = MENU_KEY_FONT * s;
        let mut menu_icon_font = MENU_ICON_FONT * s;
        let mut menu_btn_h = MENU_BTN_H * s;
        let mut menu_btn_gap = MENU_BTN_GAP * s;
        let mut menu_btn_pad = MENU_BTN_PAD * s;
        let mut menu_icon_slot = MENU_ICON_SLOT * s;
        let mut menu_label_gap = MENU_LABEL_GAP * s;
        let mut menu_radius = MENU_RADIUS * s;

        // First pass at the live zoom scale, then shrink the menu as a
        // unit if either width or the reserved menu band is too small.
        // This keeps the splash controls contained in the terminal pane
        // when the tree is open or the viewport is narrow.
        let widest_text_at_scale = |s: f32, sugarloaf: &mut Sugarloaf| -> f32 {
            let label_font = MENU_LABEL_FONT * s;
            let key_font = MENU_KEY_FONT * s;
            let btn_pad = MENU_BTN_PAD * s;
            let icon_slot = MENU_ICON_SLOT * s;
            let label_gap = MENU_LABEL_GAP * s;
            let key_gap = MENU_KEY_GAP * s;
            let ui = sugarloaf.text_mut();
            let mut widest: f32 = 0.0;
            for spec in MENU.iter() {
                let label_w = ui.measure(
                    spec.label,
                    &DrawOpts {
                        font_size: label_font,
                        bold: true,
                        ..DrawOpts::default()
                    },
                );
                let key_w = ui.measure(
                    &format!("[{}]", spec.keybind),
                    &DrawOpts {
                        font_size: key_font,
                        bold: true,
                        ..DrawOpts::default()
                    },
                );
                let row_w =
                    btn_pad + icon_slot + label_gap + label_w + key_gap + key_w + btn_pad;
                widest = widest.max(row_w);
            }
            widest
        };

        let base_widest_text = widest_text_at_scale(s, sugarloaf);
        let base_menu_btn_w = scaled_w.max(base_widest_text);
        let base_total_menu_h =
            MENU.len() as f32 * menu_btn_h + (MENU.len() - 1) as f32 * menu_btn_gap;
        let width_fit = if base_menu_btn_w > max_menu_w {
            max_menu_w / base_menu_btn_w
        } else {
            1.0
        };
        let height_fit = if base_total_menu_h > menu_band_h && base_total_menu_h > 0.0 {
            menu_band_h / base_total_menu_h
        } else {
            1.0
        };
        let fit = width_fit.min(height_fit).clamp(0.20, 1.0);
        if fit < 1.0 {
            s *= fit;
            menu_label_font = MENU_LABEL_FONT * s;
            menu_key_font = MENU_KEY_FONT * s;
            menu_icon_font = MENU_ICON_FONT * s;
            menu_btn_h = MENU_BTN_H * s;
            menu_btn_gap = MENU_BTN_GAP * s;
            menu_btn_pad = MENU_BTN_PAD * s;
            menu_icon_slot = MENU_ICON_SLOT * s;
            menu_label_gap = MENU_LABEL_GAP * s;
            menu_radius = MENU_RADIUS * s;
        }

        // Menu width matches the logo when possible, but never exceeds
        // the terminal pane. At tight widths the text scale shrinks
        // first, then the row clips to its own rect as a final guard.
        let widest_text = widest_text_at_scale(s, sugarloaf);
        let menu_btn_w = scaled_w.max(widest_text).min(max_menu_w);
        let total_menu_h =
            MENU.len() as f32 * menu_btn_h + (MENU.len() - 1) as f32 * menu_btn_gap;
        // Menu buttons drift up alongside the wordmark during
        // dismiss, with a slightly bigger rise so they "leave"
        // last (reads as: wordmark first, then menu peels off).
        let menu_dismiss_rise = wordmark_h * (DISMISS_RISE_FACTOR + 0.15) * dismiss_eased;
        let raw_menu_block_top =
            menu_top + (menu_band_h - total_menu_h) / 2.0 - menu_dismiss_rise;
        let menu_block_top = if dismiss_t > 0.0 {
            raw_menu_block_top
        } else {
            let max_top = menu_top + (menu_band_h - total_menu_h).max(0.0);
            raw_menu_block_top.clamp(menu_top, max_top)
        };
        let raw_menu_x = pane_origin.0 + (pane_size.0 - menu_btn_w) / 2.0;
        let max_x = pane_origin.0 + (pane_size.0 - menu_btn_w).max(0.0);
        let menu_x = raw_menu_x.clamp(pane_origin.0, max_x);
        let dims = MenuDims {
            label_font: menu_label_font,
            key_font: menu_key_font,
            icon_font: menu_icon_font,
            pad: menu_btn_pad,
            icon_slot: menu_icon_slot,
            label_gap: menu_label_gap,
            radius: menu_radius,
        };
        for (i, spec) in MENU.iter().enumerate() {
            let y = menu_block_top + i as f32 * (menu_btn_h + menu_btn_gap);
            let rect = [menu_x, y, menu_btn_w, menu_btn_h];
            // Cancel hit-test rects during dismiss so a stray
            // click as the splash fades doesn't fire a shortcut.
            self.menu_rects[i] = if dismiss_t > 0.0 { None } else { Some(rect) };
            let hovered = self
                .mouse
                .map(|(mx, my)| {
                    dismiss_t == 0.0
                        && mx >= menu_x
                        && mx <= menu_x + menu_btn_w
                        && my >= y
                        && my <= y + menu_btn_h
                })
                .unwrap_or(false);
            // Skip hover state when this row is fully covered
            // by an opaque modal — clicking it wouldn't reach
            // the splash anyway.
            let fully_covered =
                occlusion_rects.iter().any(|m| rect_contains_rect(*m, rect));
            let row_hovered = hovered && !fully_covered;
            self.draw_menu_button(
                sugarloaf,
                rect,
                spec,
                row_hovered,
                theme,
                &dims,
                dismiss_alpha,
                occlusion_rects,
            );
        }

        // Click fidget — small burst at the click point clipped
        // to the wordmark rect so it never spills past the
        // letters. Image squash gives the tactile bounce.
        if let Some(click) = self.click {
            let life =
                now.saturating_duration_since(click.started).as_secs_f32() * 1000.0;
            let max_life = BURST_LIFE_MS.max(SQUASH_LIFE_MS);
            if life >= max_life {
                self.click = None;
            } else {
                self.draw_click_fidget(
                    sugarloaf,
                    click,
                    life,
                    scaled_h,
                    [img_x, img_y, scaled_w, scaled_h],
                );
            }
        }
    }

    #[allow(dead_code)]
    fn squash_active(&self, now: Instant) -> bool {
        match self.click {
            None => false,
            Some(c) => {
                let life =
                    now.saturating_duration_since(c.started).as_secs_f32() * 1000.0;
                life < SQUASH_LIFE_MS
            }
        }
    }

    #[allow(dead_code)]
    fn burst_active(&self, now: Instant) -> bool {
        match self.click {
            None => false,
            Some(c) => {
                let life =
                    now.saturating_duration_since(c.started).as_secs_f32() * 1000.0;
                life < BURST_LIFE_MS
            }
        }
    }

    fn compute_squash_and_flash(&self, now: Instant) -> (f32, f32) {
        let Some(click) = self.click else {
            return (0.0, 0.0);
        };
        let life = now.saturating_duration_since(click.started).as_secs_f32() * 1000.0;
        if life >= SQUASH_LIFE_MS {
            return (0.0, 0.0);
        }
        let t = life / SQUASH_LIFE_MS;
        // Spring profile: snap inward by t≈0.18, overshoot
        // outward to ~−0.4 at t≈0.45, then ring back to zero
        // by t=1.0. This reads as a tactile "press → pop"
        // bounce on the wordmark instead of a flat shrink.
        let damping = (-3.5 * t).exp();
        let phase = (t - 0.18) * std::f32::consts::PI / 0.42;
        let scale_offset = if t < 0.18 {
            // Compress phase: ease into the press.
            let u = t / 0.18;
            u * u * (3.0 - 2.0 * u)
        } else {
            // Release phase: damped sin overshoot. Negative
            // values mean the image briefly grows past 1.0 —
            // gives the wordmark its "snap back" pop.
            damping * phase.cos()
        };
        // Click flash — bumps halo brightness for the first
        // ~120 ms of the press.
        let flash = (1.0 - t * 4.0).max(0.0).powi(2);
        (scale_offset, flash)
    }

    fn draw_menu_button(
        &self,
        sugarloaf: &mut Sugarloaf,
        rect: [f32; 4],
        spec: &MenuSpec,
        hovered: bool,
        theme: &IdeTheme,
        dims: &MenuDims,
        alpha_mul: f32,
        occlusion_rects: &[[f32; 4]],
    ) {
        let [x, y, w, h] = rect;
        // Bg is hidden until hover — keeps the splash visually
        // clean (just the logo + three text rows lined up under
        // it). Skip the hover chip when an opaque modal fully
        // covers this row; otherwise it would punch through the
        // modal panel.
        let row_rect = [x, y, w, h];
        let hover_visible = hovered
            && !occlusion_rects
                .iter()
                .any(|m| rect_contains_rect(*m, row_rect));
        if hover_visible {
            let bg = theme.f32_alpha(theme.hover, 0.95 * alpha_mul);
            sugarloaf.rounded_rect(None, x, y, w, h, bg, DEPTH, dims.radius, ORDER);
        }

        let a = alpha_mul.clamp(0.0, 1.0);
        let icon_color = if hovered {
            theme.u8_alpha(theme.fg, a)
        } else {
            theme.u8_alpha(theme.dim, a)
        };
        let label_color = if hovered {
            theme.u8_alpha(theme.fg, a)
        } else {
            theme.u8_alpha(theme.dim, a)
        };
        let key_color: [u8; 4] = theme.u8_alpha(theme.green, a);
        // sugarloaf text::draw treats `y` as the TOP of the
        // glyph box, not the baseline. Center each glyph type
        // by subtracting its font_size from the row height.
        let label_y = y + (h - dims.label_font) / 2.0;
        let icon_y = y + (h - dims.icon_font) / 2.0;
        let key_y = y + (h - dims.key_font) / 2.0;

        // Use the row rect itself as the base clip — text gets
        // sliced into segments that avoid occlusion rects (the
        // same trick `file_tree::draw_text_with_occlusion`
        // uses, only inlined here so we don't need to make that
        // helper public).
        let row_clip = [x, y, w, h];

        draw_text_clipped(
            sugarloaf,
            x + dims.pad,
            icon_y,
            spec.icon.resolve(),
            &DrawOpts {
                font_size: dims.icon_font,
                color: icon_color,
                bold: true,
                clip_rect: Some(row_clip),
                ..DrawOpts::default()
            },
            occlusion_rects,
        );
        let label_x = x + dims.pad + dims.icon_slot + dims.label_gap;
        draw_text_clipped(
            sugarloaf,
            label_x,
            label_y,
            spec.label,
            &DrawOpts {
                font_size: dims.label_font,
                color: label_color,
                bold: true,
                clip_rect: Some(row_clip),
                ..DrawOpts::default()
            },
            occlusion_rects,
        );
        let key_text = format!("[{}]", spec.keybind);
        let key_measure_opts = DrawOpts {
            font_size: dims.key_font,
            bold: true,
            ..DrawOpts::default()
        };
        let key_w = sugarloaf.text_mut().measure(&key_text, &key_measure_opts);
        draw_text_clipped(
            sugarloaf,
            x + w - dims.pad - key_w,
            key_y,
            &key_text,
            &DrawOpts {
                font_size: dims.key_font,
                color: key_color,
                bold: true,
                clip_rect: Some(row_clip),
                ..DrawOpts::default()
            },
            occlusion_rects,
        );
    }

    fn draw_click_fidget(
        &self,
        sugarloaf: &mut Sugarloaf,
        click: Click,
        life_ms: f32,
        wordmark_size: f32,
        wordmark_rect: [f32; 4],
    ) {
        if life_ms >= BURST_LIFE_MS {
            return;
        }
        let t = life_ms / BURST_LIFE_MS;
        let alpha = (1.0 - t).powi(2) * 0.85;
        // Burst radius sized off wordmark height, capped at
        // half the height so the disc never extends past the
        // top/bottom of the letter band — that's what the user
        // means by "echo through the letter": stay inside the
        // letterforms vertically.
        let max_radius = (wordmark_size * 0.5).max(1.0);
        let radius =
            (wordmark_size * BURST_RADIUS_FACTOR * (0.4 + 0.6 * t)).min(max_radius);
        // Build a polygon disc clipped to the wordmark rect.
        // Anything that would fall outside the rect is snapped
        // to the rect boundary, so the disc visually crops at
        // the edge of the letters.
        let pts =
            clipped_disc(click.cx, click.cy, radius, wordmark_rect, RIPPLE_SEGMENTS);
        if pts.len() >= 3 {
            sugarloaf.polygon(&pts, DEPTH, [1.0, 1.0, 1.0, alpha]);
        }
    }

    pub fn clear_image_overlays(sugarloaf: &mut Sugarloaf) {
        sugarloaf.clear_image_overlays_for(SPLASH_PANEL_ID);
    }
}

/// Build a polygon approximating a disc centered at `(cx, cy)`
/// with `radius`, with every point clamped to lie inside
/// `rect = [x, y, w, h]`. The clamp is per-point — the resulting
/// polygon hugs the rect edges anywhere the unclipped disc would
/// have escaped, which gives the visual effect of a circular
/// pulse cropped to the wordmark rectangle.
fn clipped_disc(
    cx: f32,
    cy: f32,
    radius: f32,
    rect: [f32; 4],
    segments: usize,
) -> Vec<(f32, f32)> {
    if radius <= 0.0 {
        return Vec::new();
    }
    let [rx, ry, rw, rh] = rect;
    let xmin = rx;
    let xmax = rx + rw;
    let ymin = ry;
    let ymax = ry + rh;
    let mut points = Vec::with_capacity(segments + 2);
    points.push((cx.clamp(xmin, xmax), cy.clamp(ymin, ymax)));
    for i in 0..=segments {
        let a = i as f32 / segments as f32 * std::f32::consts::TAU;
        let px = (cx + radius * a.cos()).clamp(xmin, xmax);
        let py = (cy + radius * a.sin()).clamp(ymin, ymax);
        points.push((px, py));
    }
    points
}

/// Slice `text` against the occlusion rects so it draws only in
/// the parts of the base clip rect that aren't covered by an
/// opaque modal panel. Same algorithm `file_tree.rs::draw_text_
/// with_occlusion` uses — inlined here so we don't need to
/// touch its visibility. Without a `clip_rect` on the input
/// opts we can't carve, so just draw straight through.
fn draw_text_clipped(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
) {
    if occlusion_rects.is_empty() {
        sugarloaf.text_mut().draw(x, y, text, opts);
        return;
    }
    let Some(base_clip) = opts.clip_rect else {
        sugarloaf.text_mut().draw(x, y, text, opts);
        return;
    };
    let width = sugarloaf.text_mut().measure(text, opts);
    if width <= 0.0 {
        return;
    }
    let text_h = (opts.font_size * 1.8).max(opts.font_size + 8.0);
    let text_rect = [x, y - 4.0, width, text_h];
    let mut intervals = vec![(base_clip[0], base_clip[0] + base_clip[2])];

    for rect in occlusion_rects {
        if !rects_intersect(text_rect, *rect) {
            continue;
        }
        let cut_start = rect[0].max(base_clip[0]);
        let cut_end = (rect[0] + rect[2]).min(base_clip[0] + base_clip[2]);
        if cut_end <= cut_start {
            continue;
        }
        let mut next = Vec::with_capacity(intervals.len() + 1);
        for (start, end) in intervals {
            if cut_end <= start || cut_start >= end {
                next.push((start, end));
                continue;
            }
            if cut_start > start {
                next.push((start, cut_start));
            }
            if cut_end < end {
                next.push((cut_end, end));
            }
        }
        intervals = next;
        if intervals.is_empty() {
            return;
        }
    }

    for (start, end) in intervals {
        let clip_w = end - start;
        if clip_w <= 0.0 {
            continue;
        }
        let mut clipped = *opts;
        clipped.clip_rect = Some([start, base_clip[1], clip_w, base_clip[3]]);
        sugarloaf.text_mut().draw(x, y, text, &clipped);
    }
}

fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    let (ax1, ay1, ax2, ay2) = (a[0], a[1], a[0] + a[2], a[1] + a[3]);
    let (bx1, by1, bx2, by2) = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
    ax1 < bx2 && ax2 > bx1 && ay1 < by2 && ay2 > by1
}

/// True when `outer` fully contains `inner` (used to drop the
/// hover chip on rows that are entirely under a modal panel).
fn rect_contains_rect(outer: [f32; 4], inner: [f32; 4]) -> bool {
    let (ox1, oy1, ox2, oy2) =
        (outer[0], outer[1], outer[0] + outer[2], outer[1] + outer[3]);
    let (ix1, iy1, ix2, iy2) =
        (inner[0], inner[1], inner[0] + inner[2], inner[1] + inner[3]);
    ox1 <= ix1 && oy1 <= iy1 && ox2 >= ix2 && oy2 >= iy2
}

fn splash_wordmark_z_index(occlusion_rects: &[[f32; 4]]) -> i32 {
    if occlusion_rects.is_empty() {
        // The opaque backdrop is a rich-text quad. Keep the wordmark in the
        // above-quad image pass on an unobstructed splash, then let the UI
        // text pass paint its labels over both. If a modal is present, move
        // the image below the quad pass so the modal remains authoritative.
        1
    } else {
        -1
    }
}

/// Decode + upload the wordmark with the letter tint cycle baked into
/// its vertical strips (letters render as `source_rect` strips, so
/// per-letter color needs no draw-side change). The source PNG is a
/// white glyph on transparency. Callers re-invoke on tint change; the
/// fresh `transmit_time` below invalidates the cached texture.
fn register_wordmark(sugarloaf: &mut Sugarloaf, tint: &[u32]) -> bool {
    let img = match image_rs::load_from_memory(WORDMARK_PNG) {
        Ok(i) => i.to_rgba8(),
        Err(_) => return false,
    };
    let (w, h) = img.dimensions();
    let mut pixels = img.into_raw();
    crate::primitives::look::tint_wordmark_pixels(
        &mut pixels,
        w as usize,
        LETTER_COUNT,
        tint,
    );
    let entry = GraphicDataEntry::from_graphic_data(GraphicData {
        id: GraphicId::new(SPLASH_WORDMARK_IMAGE_ID as u64),
        width: w as usize,
        height: h as usize,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: false,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: web_time::Instant::now(),
    });
    sugarloaf.image_data.insert(SPLASH_WORDMARK_IMAGE_ID, entry);
    true
}
