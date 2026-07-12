//! Slash-command easter eggs — tiny pixel-fella skits played over the
//! agent timeline (`/piss`, `/cuss`). Pure rect/text-sprite animation
//! on the shared draw primitives; the host pane owns the timer and
//! fires the follow-up prompt when [`prompt_at`] passes (the model
//! only hears about it once the deed is done).

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::ide_theme::IdeTheme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentFxKind {
    /// Jogs in from the right, waters the timeline leftward, jogs off.
    Piss,
    /// Storms in from the right and cusses the model out — grawlix
    /// bubble, flailing arms — then storms off.
    Cuss,
    /// Yanks an invisible cable — the pane glitches with scanline
    /// bands — plugs it back in, shrugs, leaves.
    Glitch,
    /// Disco ball descends, colored beams sweep, confetti rains, the
    /// fella dances, then moonwalks off.
    Disco,
    /// Two crews shoot it out cartoon-style; the fella's crew wins
    /// and he walks off alone through the aftermath.
    GangFight,
    /// Jesus on a golden throne, light streaming, worshipers bowing
    /// in waves while music notes rise.
    Praise,
}

/// Full runtime of a skit; hosts clear the timer past this.
pub fn total_seconds(kind: AgentFxKind) -> f32 {
    match kind {
        AgentFxKind::Piss => 7.4,
        AgentFxKind::Cuss => 6.6,
        AgentFxKind::Glitch => 4.2,
        AgentFxKind::Disco => 7.0,
        AgentFxKind::GangFight => 9.0,
        AgentFxKind::Praise => 8.0,
    }
}

/// When the follow-up prompt becomes due: after the deed is done, so
/// the reply lands as a reaction, not a play-by-play.
pub fn prompt_at(kind: AgentFxKind) -> f32 {
    match kind {
        AgentFxKind::Piss => PISS_END,
        AgentFxKind::Cuss => RANT_END,
        AgentFxKind::Glitch => GLITCH_END,
        AgentFxKind::Disco => 2.0,
        AgentFxKind::GangFight => SHOOTOUT_END,
        AgentFxKind::Praise => 5.0,
    }
}

// ── piss timeline ────────────────────────────────────────────────
const WALK_IN_END: f32 = 2.0;
const STAND_END: f32 = 2.4;
const PISS_END: f32 = 5.6;
const ZIP_END: f32 = 5.9;

// ── cuss timeline ────────────────────────────────────────────────
const CUSS_WALK_IN_END: f32 = 1.6;
const RANT_END: f32 = 4.9;

// ── glitch timeline ──────────────────────────────────────────────
const GLITCH_WALK_END: f32 = 0.9;
const YANK_END: f32 = 1.2;
const GLITCH_END: f32 = 2.6;
const PLUG_END: f32 = 3.0;

// ── disco timeline ───────────────────────────────────────────────
const BALL_DROP_END: f32 = 0.8;
const DANCE_END: f32 = 5.6;

// ── gang fight timeline ──────────────────────────────────────────
const CREWS_IN_END: f32 = 1.6;
const STANDOFF_END: f32 = 2.2;
const SHOOTOUT_END: f32 = 6.0;

// ── praise timeline ──────────────────────────────────────────────
const GATHER_END: f32 = 2.0;

const DEPTH: f32 = 0.0;
/// Over timeline content (20s), under the `/` picker (180) and modals.
const ORDER: u8 = 120;

// 12-wide × 16-tall art pixels. Drawn facing RIGHT; `flip` mirrors.
// legend: h hair · k skin · s shirt · p pants · b boots
const WALK_A: [&str; 16] = [
    "   hhhh     ",
    "  hhhhhh    ",
    "  hkkkkk    ",
    "  kkkkkk    ",
    "   kkkk     ",
    "  ssssss    ",
    " sssssss    ",
    " k ssss k   ",
    "   ssss     ",
    "   pppp     ",
    "   pppp     ",
    "  pp  pp    ",
    "  pp   pp   ",
    " pp     pp  ",
    " bb      bb ",
    "            ",
];
const WALK_B: [&str; 16] = [
    "   hhhh     ",
    "  hhhhhh    ",
    "  hkkkkk    ",
    "  kkkkkk    ",
    "   kkkk     ",
    "  ssssss    ",
    " sssssss    ",
    "  kssssk    ",
    "   ssss     ",
    "   pppp     ",
    "   pppp     ",
    "   pppp     ",
    "   p  p     ",
    "   p  p     ",
    "  bb  bb    ",
    "            ",
];
// Hands at the front — mid-business.
const PISS_POSE: [&str; 16] = [
    "   hhhh     ",
    "  hhhhhh    ",
    "  hkkkkk    ",
    "  kkkkkk    ",
    "   kkkk     ",
    "  ssssss    ",
    "  ssssss    ",
    "   ssskk    ",
    "   sskk     ",
    "   pppp     ",
    "   pppp     ",
    "   pppp     ",
    "   p  p     ",
    "   p  p     ",
    "  bb  bb    ",
    "            ",
];
// Rant poses: fists in the air / arms thrown out sideways.
const RANT_A: [&str; 16] = [
    " k hhhh k   ",
    " k hhhhhh k ",
    "  khkkkkkk  ",
    "  kkkkkk    ",
    "   kkkk     ",
    "  ssssss    ",
    "  ssssss    ",
    "  ssssss    ",
    "   ssss     ",
    "   pppp     ",
    "   pppp     ",
    "   pppp     ",
    "   p  p     ",
    "   p  p     ",
    "  bb  bb    ",
    "            ",
];
const RANT_B: [&str; 16] = [
    "   hhhh     ",
    "  hhhhhh    ",
    "  hkkkkk    ",
    "  kkkkkk    ",
    "   kkkk     ",
    "  ssssss    ",
    "kksssssskk  ",
    "  ssssss    ",
    "   ssss     ",
    "   pppp     ",
    "   pppp     ",
    "   pppp     ",
    "   p  p     ",
    "   p  p     ",
    "  bb  bb    ",
    "            ",
];

// Arm out at shoulder height, tiny piece in hand ('g'), facing right.
const SHOOT_POSE: [&str; 16] = [
    "   hhhh     ",
    "  hhhhhh    ",
    "  hkkkkk    ",
    "  kkkkkk    ",
    "   kkkk     ",
    "  ssssss    ",
    "  sssskkgg  ",
    "  ssssss    ",
    "   ssss     ",
    "   pppp     ",
    "   pppp     ",
    "   pppp     ",
    "   p  p     ",
    "   p  p     ",
    "  bb  bb    ",
    "            ",
];
// Hooded crew variant ('f' = hood, drawn in the member's hat color).
const HOOD_SHOOT: [&str; 16] = [
    "   ffff     ",
    "  ffffff    ",
    "  fkkkkf    ",
    "  fkkkkf    ",
    "   kkkk     ",
    "  ssssss    ",
    "  sssskkgg  ",
    "  ssssss    ",
    "   ssss     ",
    "   pppp     ",
    "   pppp     ",
    "   pppp     ",
    "   p  p     ",
    "   p  p     ",
    "  bb  bb    ",
    "            ",
];
// Fedora crew variant ('f' = hat crown + brim).
const FEDORA_SHOOT: [&str; 16] = [
    "   ffff     ",
    " ffffffff   ",
    "  kkkkkk    ",
    "  kkkkkk    ",
    "   kkkk     ",
    "  ssssss    ",
    "  sssskkgg  ",
    "  ssssss    ",
    "   ssss     ",
    "   pppp     ",
    "   pppp     ",
    "   pppp     ",
    "   p  p     ",
    "   p  p     ",
    "  bb  bb    ",
    "            ",
];

// Jesus: halo ('f' → gold), long hair, white robe ('s'), arms open
// in blessing.
const JESUS: [&str; 16] = [
    "   ffff     ",
    "   hhhh     ",
    "  hhhhhh    ",
    "  hkkkkh    ",
    "  hkkkkh    ",
    "   kkkk     ",
    "  ssssss    ",
    "kkssssssk k ",
    "  ssssss    ",
    "  ssssss    ",
    "  ssssss    ",
    "  ssssss    ",
    "  ssssss    ",
    " ssssssss   ",
    " ssssssss   ",
    "            ",
];
// Worshiper, kneeling upright (hands raised between bows).
const KNEEL_UP: [&str; 16] = [
    "            ",
    "            ",
    "            ",
    " k hhhh k   ",
    " k hhhhhhk  ",
    "  khkkkkk   ",
    "  kkkkkk    ",
    "   kkkk     ",
    "  ssssss    ",
    "  ssssss    ",
    "   ssss     ",
    "   pppp     ",
    "  pppppp    ",
    "  bb  bb    ",
    "            ",
    "            ",
];
// Worshiper, bowed low toward the throne (facing right).
const KNEEL_BOW: [&str; 16] = [
    "            ",
    "            ",
    "            ",
    "            ",
    "            ",
    "            ",
    "            ",
    "            ",
    "        hhh ",
    "  ss  hhkkk ",
    " sssssskkk  ",
    " ssssssk    ",
    " pppppp     ",
    " bb  bb     ",
    "            ",
    "            ",
];

const SPRITE_W: usize = 12;
const SPRITE_H: usize = 16;

const HAIR: [f32; 4] = [0.29, 0.18, 0.11, 1.0];
const SKIN: [f32; 4] = [0.91, 0.71, 0.55, 1.0];
const PANTS: [f32; 4] = [0.17, 0.29, 0.55, 1.0];
const BOOTS: [f32; 4] = [0.15, 0.13, 0.11, 1.0];
const PISS_YELLOW: [f32; 4] = [0.95, 0.82, 0.14, 0.9];
const ANGER_RED: [f32; 4] = [0.88, 0.22, 0.16, 0.95];
const GUN: [f32; 4] = [0.16, 0.16, 0.18, 1.0];
const MUZZLE: [f32; 4] = [1.0, 0.92, 0.45, 0.95];
const TRACER: [f32; 4] = [1.0, 0.95, 0.6, 0.85];
const GOLD: [f32; 4] = [0.87, 0.70, 0.22, 1.0];
const GOLD_DARK: [f32; 4] = [0.62, 0.48, 0.13, 1.0];
const ROBE_WHITE: [f32; 4] = [0.96, 0.95, 0.90, 1.0];

#[allow(clippy::too_many_arguments)]
fn draw_sprite(
    sugarloaf: &mut Sugarloaf,
    map: &[&str; SPRITE_H],
    x: f32,
    y: f32,
    px: f32,
    shirt: [f32; 4],
    flip: bool,
) {
    for (row_i, row) in map.iter().enumerate() {
        for (col_i, ch) in row.chars().enumerate() {
            let color = match ch {
                'h' => HAIR,
                'k' => SKIN,
                's' => shirt,
                'p' => PANTS,
                'b' => BOOTS,
                _ => continue,
            };
            let col = if flip { SPRITE_W - 1 - col_i } else { col_i };
            sugarloaf.rect(
                None,
                x + col as f32 * px,
                y + row_i as f32 * px,
                px,
                px,
                color,
                DEPTH,
                ORDER,
            );
        }
    }
}

fn walk_frame(t: f32, rate: f32) -> &'static [&'static str; SPRITE_H] {
    if ((t * rate) as usize) % 2 == 0 {
        &WALK_A
    } else {
        &WALK_B
    }
}

/// Per-fella palette for the crew skits.
struct SpriteStyle {
    shirt: [f32; 4],
    skin: [f32; 4],
    hat: [f32; 4],
}

/// `draw_sprite` with per-member colors, hats ('f') and guns ('g'),
/// plus a `fallen` mode that lays the sprite on its back (rotated a
/// quarter turn, feet toward its facing side).
#[allow(clippy::too_many_arguments)]
fn draw_sprite_styled(
    sugarloaf: &mut Sugarloaf,
    map: &[&str; SPRITE_H],
    x: f32,
    y: f32,
    px: f32,
    style: &SpriteStyle,
    flip: bool,
    fallen: bool,
) {
    for (row_i, row) in map.iter().enumerate() {
        for (col_i, ch) in row.chars().enumerate() {
            let color = match ch {
                'h' => HAIR,
                'k' => style.skin,
                's' => style.shirt,
                'p' => PANTS,
                'b' => BOOTS,
                'f' => style.hat,
                'g' => GUN,
                _ => continue,
            };
            let col = if flip { SPRITE_W - 1 - col_i } else { col_i };
            let (cx, cy) = if fallen {
                // Head ends up opposite the facing side; footprint
                // becomes SPRITE_H wide × SPRITE_W tall.
                (x + row_i as f32 * px, y + (SPRITE_W - 1 - col) as f32 * px)
            } else {
                (x + col as f32 * px, y + row_i as f32 * px)
            };
            sugarloaf.rect(None, cx, cy, px, px, color, DEPTH, ORDER);
        }
    }
}

/// Deterministic 0..1 hash — skits can't touch the RNG or wall clock
/// (wasm + resume safety), so all "randomness" derives from ticks.
fn hash01(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x045D_9F3B);
    x ^= x >> 16;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Draw one frame of the skit at `elapsed` seconds. Callers gate on
/// `0.0..=total_seconds(kind)`; anything outside is their cue to clear
/// the timer (also covers the wrap of the 10k-second animation clock).
pub fn render(
    kind: AgentFxKind,
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    elapsed: f32,
    scale: f32,
    theme: &IdeTheme,
) {
    match kind {
        AgentFxKind::Piss => render_piss(sugarloaf, rect, elapsed, scale, theme),
        AgentFxKind::Cuss => render_cuss(sugarloaf, rect, elapsed, scale, theme),
        AgentFxKind::Glitch => render_glitch(sugarloaf, rect, elapsed, scale, theme),
        AgentFxKind::Disco => render_disco(sugarloaf, rect, elapsed, scale, theme),
        AgentFxKind::GangFight => {
            render_gang_fight(sugarloaf, rect, elapsed, scale, theme)
        }
        AgentFxKind::Praise => render_praise(sugarloaf, rect, elapsed, scale, theme),
    }
}

/// Jesus on a golden throne at center, light streaming down, and
/// worshipers gathering from both sides to bow in staggered waves
/// while music notes float up. Reverent, 16 pixels tall.
fn render_praise(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    elapsed: f32,
    scale: f32,
    theme: &IdeTheme,
) {
    let s = scale.clamp(0.5, 3.0);
    let px = 3.0 * s;
    let guy_w = SPRITE_W as f32 * px;
    let guy_h = SPRITE_H as f32 * px;
    let floor_y = rect[1] + rect[3] - 36.0 * s;
    let cx = rect[0] + rect[2] * 0.5;

    // Dais + throne: two gold steps, tall back, seat and armrests.
    let dais_w = guy_w * 2.4;
    let dais_h = 5.0 * s;
    sugarloaf.rect(None, cx - dais_w * 0.5, floor_y - dais_h, dais_w, dais_h, GOLD_DARK, DEPTH, ORDER);
    sugarloaf.rect(None, cx - dais_w * 0.35, floor_y - dais_h * 2.0, dais_w * 0.7, dais_h, GOLD, DEPTH, ORDER);
    let throne_top = floor_y - dais_h * 2.0 - guy_h - 10.0 * s;
    sugarloaf.rounded_rect(None, cx - guy_w * 0.62, throne_top, guy_w * 1.24, guy_h + 10.0 * s, GOLD_DARK, DEPTH, 6.0 * s, ORDER);
    sugarloaf.rect(None, cx - guy_w * 0.78, floor_y - dais_h * 2.0 - 9.0 * s, guy_w * 0.16, 9.0 * s, GOLD, DEPTH, ORDER);
    sugarloaf.rect(None, cx + guy_w * 0.62, floor_y - dais_h * 2.0 - 9.0 * s, guy_w * 0.16, 9.0 * s, GOLD, DEPTH, ORDER);

    // Light from above: a soft gold column plus two breathing outer
    // bands — glory, in rects.
    let breathe = 0.5 + 0.5 * (elapsed * 1.4).sin();
    for (half_w, alpha) in [
        (guy_w * 0.9, 0.16 + 0.06 * breathe),
        (guy_w * 1.6, 0.08 + 0.04 * breathe),
        (guy_w * 2.4, 0.04 + 0.03 * breathe),
    ] {
        sugarloaf.rect(
            None,
            cx - half_w,
            rect[1],
            half_w * 2.0,
            floor_y - rect[1],
            [GOLD[0], GOLD[1], GOLD[2], alpha],
            DEPTH,
            ORDER,
        );
    }

    // Jesus, seated in the throne's frame.
    let jesus_style = SpriteStyle {
        shirt: ROBE_WHITE,
        skin: [0.80, 0.60, 0.42, 1.0],
        hat: [1.0, 0.88, 0.35, 1.0],
    };
    draw_sprite_styled(
        sugarloaf,
        &JESUS,
        cx - guy_w * 0.5,
        throne_top + 6.0 * s,
        px,
        &jesus_style,
        false,
        false,
    );

    // Worshipers gather from both sides, then bow in staggered waves.
    // Mixed folks, all facing the throne.
    let worshipers: [([f32; 4], [f32; 4], f32, bool); 4] = [
        ([0.30, 0.34, 0.46, 1.0], [0.55, 0.38, 0.26, 1.0], 0.0, true),
        ([0.42, 0.24, 0.18, 1.0], [0.91, 0.71, 0.55, 1.0], 0.45, true),
        ([0.26, 0.38, 0.28, 1.0], [0.98, 0.80, 0.64, 1.0], 0.2, false),
        ([0.36, 0.28, 0.42, 1.0], [0.76, 0.56, 0.40, 1.0], 0.65, false),
    ];
    let slots = [0.24, 0.35, 0.65, 0.76];
    for (i, ((outfit, skin, phase, from_left), slot)) in
        worshipers.iter().zip(slots.iter()).enumerate()
    {
        let style = SpriteStyle { shirt: *outfit, skin: *skin, hat: *outfit };
        let target = rect[0] + rect[2] * slot - guy_w * 0.5;
        let guy_y = floor_y - guy_h;
        if elapsed < GATHER_END {
            let u = (elapsed / GATHER_END).clamp(0.0, 1.0);
            let x = if *from_left {
                rect[0] - guy_w * (1.0 + i as f32 * 0.5)
                    + (target - rect[0] + guy_w * (1.0 + i as f32 * 0.5)) * u
            } else {
                let start = rect[0] + rect[2] + guy_w * (1.0 + i as f32 * 0.5);
                start + (target - start) * u
            };
            let bob = ((elapsed * 12.0 + phase * 8.0).sin() * 1.1 * s).abs();
            draw_sprite_styled(
                sugarloaf,
                walk_frame(elapsed + phase, 7.0),
                x,
                guy_y - bob,
                px,
                &style,
                !from_left,
                false,
            );
        } else {
            // Bow cycle: down slow, up slow, staggered per worshiper.
            let t = elapsed - GATHER_END + phase * 1.3;
            let bowed = ((t * 0.9) as usize) % 2 == 0;
            draw_sprite_styled(
                sugarloaf,
                if bowed { &KNEEL_BOW } else { &KNEEL_UP },
                target,
                guy_y,
                px,
                &style,
                !from_left,
                false,
            );
        }
    }

    // Music notes rising through the light once worship begins.
    if elapsed >= GATHER_END {
        let note_opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.yellow),
            bold: true,
            ..DrawOpts::default()
        };
        for i in 0..5u32 {
            let nx = cx + (hash01(i.wrapping_add(3)) - 0.5) * guy_w * 3.6;
            let rise_h = rect[3] * 0.5;
            let rise = (elapsed * (18.0 + hash01(i) * 22.0)
                + hash01(i.wrapping_add(9)) * rise_h)
                % rise_h;
            let sway = ((elapsed * 2.0 + i as f32) * 1.3).sin() * 4.0 * s;
            sugarloaf.text_mut().draw(
                nx + sway,
                floor_y - 30.0 * s - rise,
                if i % 2 == 0 { "♪" } else { "♩" },
                &note_opts,
            );
        }
    }
}

/// Walks in, yanks an invisible cable — the pane "loses signal" in
/// jittering scanline bands — plugs it back, shrugs, walks off.
fn render_glitch(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    elapsed: f32,
    scale: f32,
    theme: &IdeTheme,
) {
    let s = scale.clamp(0.5, 3.0);
    let px = 3.0 * s;
    let guy_w = SPRITE_W as f32 * px;
    let guy_h = SPRITE_H as f32 * px;
    let floor_y = rect[1] + rect[3] - 36.0 * s;
    let guy_y = floor_y - guy_h;
    let stop_x = rect[0] + rect[2] * 0.30;
    let shirt = theme.f32(theme.accent);

    if elapsed < GLITCH_WALK_END {
        let u = (elapsed / GLITCH_WALK_END).clamp(0.0, 1.0);
        let x = rect[0] - guy_w + (stop_x - rect[0] + guy_w) * u;
        let bob = ((elapsed * 14.0).sin() * 1.2 * s).abs();
        draw_sprite(sugarloaf, walk_frame(elapsed, 8.0), x, guy_y - bob, px, shirt, false);
        return;
    }

    let glitching = elapsed >= YANK_END && elapsed < GLITCH_END;
    // The fella jitters with the signal while he holds the cable.
    let tick = (elapsed * 30.0) as u32;
    let jitter_x = if glitching {
        (hash01(tick) - 0.5) * 4.0 * s
    } else {
        0.0
    };
    if elapsed < PLUG_END {
        // Bent over the plug (the business pose reads as "reaching
        // down" from this angle, don't overthink it).
        draw_sprite(sugarloaf, &PISS_POSE, stop_x + jitter_x, guy_y, px, shirt, false);
    } else {
        let u = ((elapsed - PLUG_END)
            / (total_seconds(AgentFxKind::Glitch) - PLUG_END))
            .clamp(0.0, 1.0);
        let x = stop_x + (rect[0] - guy_w * 2.0 - stop_x) * u;
        let bob = ((elapsed * 14.0).sin() * 1.2 * s).abs();
        draw_sprite(sugarloaf, walk_frame(elapsed, 8.0), x, guy_y - bob, px, shirt, true);
    }

    if glitching {
        // Scanline bands: torn horizontal strips in RGB-split colors
        // plus bg-colored displacement bars, reshuffled every tick.
        let band_colors = [
            [0.9, 0.2, 0.2, 0.16],
            [0.2, 0.9, 0.3, 0.14],
            [0.25, 0.4, 0.95, 0.16],
            theme.f32_alpha(theme.fg, 0.10),
        ];
        for i in 0..7u32 {
            let seed = tick.wrapping_mul(7).wrapping_add(i);
            let band_y = rect[1] + hash01(seed) * rect[3];
            let band_h = (2.0 + hash01(seed.wrapping_add(31)) * 9.0) * s;
            let color = band_colors[(seed % 4) as usize];
            sugarloaf.rect(None, rect[0], band_y, rect[2], band_h, color, DEPTH, ORDER);
        }
        for i in 0..3u32 {
            let seed = tick.wrapping_mul(13).wrapping_add(i).wrapping_add(100);
            let band_y = rect[1] + hash01(seed) * rect[3];
            let band_h = (3.0 + hash01(seed.wrapping_add(7)) * 6.0) * s;
            let shift = (hash01(seed.wrapping_add(53)) - 0.5) * 30.0 * s;
            sugarloaf.rect(
                None,
                rect[0] + shift.max(0.0),
                band_y,
                rect[2] - shift.abs(),
                band_h,
                theme.f32_alpha(theme.bg, 0.85),
                DEPTH,
                ORDER,
            );
        }
    }
}

/// Disco ball descends on a wire, colored beams sweep, floor tiles
/// pulse, confetti rains, the fella dances — then moonwalks off while
/// the ball retracts.
fn render_disco(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    elapsed: f32,
    scale: f32,
    theme: &IdeTheme,
) {
    let s = scale.clamp(0.5, 3.0);
    let px = 3.0 * s;
    let guy_w = SPRITE_W as f32 * px;
    let guy_h = SPRITE_H as f32 * px;
    let floor_y = rect[1] + rect[3] - 36.0 * s;
    let guy_y = floor_y - guy_h;
    let cx = rect[0] + rect[2] * 0.5;
    let shirt = theme.f32(theme.accent);
    let total = total_seconds(AgentFxKind::Disco);

    // Ball drop / retract on its wire.
    let ball_r = 9.0 * s;
    let ball_hang_y = rect[1] + 54.0 * s;
    let ball_y = if elapsed < BALL_DROP_END {
        let u = (elapsed / BALL_DROP_END).clamp(0.0, 1.0);
        rect[1] - ball_r * 2.0 + (ball_hang_y - rect[1] + ball_r * 2.0) * u
    } else if elapsed > DANCE_END {
        let u = ((elapsed - DANCE_END) / (total - DANCE_END)).clamp(0.0, 1.0);
        ball_hang_y - (ball_hang_y - rect[1] + ball_r * 2.0) * u
    } else {
        ball_hang_y
    };
    sugarloaf.rect(
        None,
        cx - 0.5 * s,
        rect[1],
        1.0 * s,
        (ball_y - rect[1]).max(0.0),
        theme.f32_alpha(theme.muted, 0.8),
        DEPTH,
        ORDER,
    );
    // Mirror-tile ball: pixel disc with a rotating sparkle pattern.
    let tick = (elapsed * 10.0) as i32;
    let cells = (ball_r / (2.0 * s)) as i32;
    for dy in -cells..=cells {
        for dx in -cells..=cells {
            if dx * dx + dy * dy > cells * cells {
                continue;
            }
            let shade = match (dx + dy + tick).rem_euclid(3) {
                0 => [0.92, 0.92, 0.96, 1.0],
                1 => [0.66, 0.68, 0.75, 1.0],
                _ => [0.45, 0.47, 0.55, 1.0],
            };
            sugarloaf.rect(
                None,
                cx + dx as f32 * 2.0 * s - s,
                ball_y + dy as f32 * 2.0 * s + ball_r,
                2.0 * s,
                2.0 * s,
                shade,
                DEPTH,
                ORDER,
            );
        }
    }

    let dancing = (BALL_DROP_END..DANCE_END).contains(&elapsed);
    if dancing {
        // Sweeping colored beams from the ball down to the floor.
        let beam_colors = [theme.red, theme.green, theme.blue, theme.magenta];
        for (i, color) in beam_colors.iter().enumerate() {
            let sweep = (elapsed * 0.9 + i as f32 * 1.57).sin();
            let bx = cx + sweep * rect[2] * 0.30;
            sugarloaf.rect(
                None,
                bx - 5.0 * s,
                ball_y + ball_r * 2.0,
                10.0 * s,
                (floor_y - ball_y - ball_r * 2.0).max(0.0),
                theme.f32_alpha(*color, 0.10),
                DEPTH,
                ORDER,
            );
        }
        // Pulsing dance floor.
        let tiles = 8;
        let tile_w = rect[2] * 0.5 / tiles as f32;
        let beat = (elapsed * 3.0) as usize;
        for i in 0..tiles {
            let color = [theme.red, theme.green, theme.blue, theme.yellow]
                [(i + beat) % 4];
            sugarloaf.rect(
                None,
                rect[0] + rect[2] * 0.25 + i as f32 * tile_w,
                floor_y,
                tile_w - 1.0 * s,
                5.0 * s,
                theme.f32_alpha(color, 0.35),
                DEPTH,
                ORDER,
            );
        }
        // Confetti — deterministic columns falling at their own rates.
        for i in 0..36u32 {
            let col_x = rect[0] + hash01(i) * rect[2];
            let speed = 40.0 + hash01(i.wrapping_add(99)) * 80.0;
            let drop = (elapsed * speed + hash01(i.wrapping_add(7)) * rect[3] * 2.0)
                % (rect[3] - 20.0 * s);
            let color = [theme.red, theme.green, theme.blue, theme.yellow,
                theme.magenta, theme.cyan][(i % 6) as usize];
            sugarloaf.rect(
                None,
                col_x,
                rect[1] + drop,
                2.0 * s,
                2.0 * s,
                theme.f32_alpha(color, 0.9),
                DEPTH,
                ORDER,
            );
        }
        // The dance: pose cycle with a hip wiggle.
        let dance_t = elapsed - BALL_DROP_END;
        let pose = match ((dance_t * 4.0) as usize) % 4 {
            0 => &RANT_A,
            1 => &WALK_B,
            2 => &RANT_B,
            _ => &WALK_B,
        };
        let wiggle = (dance_t * 6.0).sin() * 3.0 * s;
        let hop = if ((dance_t * 4.0) as usize) % 2 == 0 { 2.0 * s } else { 0.0 };
        draw_sprite(
            sugarloaf,
            pose,
            cx - guy_w * 0.5 + wiggle,
            guy_y - hop,
            px,
            shirt,
            false,
        );
        return;
    }

    if elapsed < BALL_DROP_END {
        // Strolls in from the left while the ball descends.
        let u = (elapsed / BALL_DROP_END).clamp(0.0, 1.0);
        let x = rect[0] - guy_w + (cx - guy_w * 0.5 - rect[0] + guy_w) * u;
        draw_sprite(sugarloaf, walk_frame(elapsed, 8.0), x, guy_y, px, shirt, false);
    } else {
        // Walks off the way he came (a moonwalk exit read as a bug,
        // not a bit — RIP).
        let u = ((elapsed - DANCE_END) / (total - DANCE_END)).clamp(0.0, 1.0);
        let x = cx - guy_w * 0.5 + (rect[0] - guy_w * 2.0 - cx + guy_w * 0.5) * u;
        draw_sprite(sugarloaf, walk_frame(elapsed, 10.0), x, guy_y, px, shirt, true);
    }
}

struct CrewMember {
    /// Standoff x as a fraction of the pane width.
    slot: f32,
    outfit: [f32; 4],
    skin: [f32; 4],
    hat: [f32; 4],
    /// When they dramatically go down (cartoon-style, no gore).
    fall_at: f32,
    from_left: bool,
}

/// Both crews are mixed — they're told apart by outfit (hoodies vs
/// fedora suits), not by who's in them.
const CREW: [CrewMember; 5] = [
    CrewMember { slot: 0.16, outfit: [0.26, 0.30, 0.42, 1.0], skin: [0.55, 0.38, 0.26, 1.0], hat: [0.19, 0.22, 0.32, 1.0], fall_at: 3.0, from_left: true },
    CrewMember { slot: 0.23, outfit: [0.45, 0.18, 0.18, 1.0], skin: [0.98, 0.80, 0.64, 1.0], hat: [0.34, 0.13, 0.13, 1.0], fall_at: 4.2, from_left: true },
    CrewMember { slot: 0.30, outfit: [0.30, 0.38, 0.24, 1.0], skin: [0.76, 0.56, 0.40, 1.0], hat: [0.22, 0.28, 0.17, 1.0], fall_at: 5.4, from_left: true },
    CrewMember { slot: 0.72, outfit: [0.22, 0.22, 0.27, 1.0], skin: [0.45, 0.30, 0.20, 1.0], hat: [0.13, 0.13, 0.16, 1.0], fall_at: 3.6, from_left: false },
    CrewMember { slot: 0.79, outfit: [0.34, 0.26, 0.19, 1.0], skin: [0.91, 0.71, 0.55, 1.0], hat: [0.13, 0.13, 0.16, 1.0], fall_at: 4.8, from_left: false },
];

/// Cartoon crew shootout. The fella leads the fedora crew in from the
/// right; the hooded crew rolls in from the left; muzzle flashes and
/// tracers fly until only he is left standing, and he walks off
/// through the aftermath.
fn render_gang_fight(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    elapsed: f32,
    scale: f32,
    theme: &IdeTheme,
) {
    let s = scale.clamp(0.5, 3.0);
    let px = 3.0 * s;
    let guy_w = SPRITE_W as f32 * px;
    let guy_h = SPRITE_H as f32 * px;
    let floor_y = rect[1] + rect[3] - 36.0 * s;
    let guy_y = floor_y - guy_h;
    let fallen_y = floor_y - SPRITE_W as f32 * px;
    let total = total_seconds(AgentFxKind::GangFight);
    let main_slot = 0.65;
    let main_style = SpriteStyle {
        shirt: theme.f32(theme.accent),
        skin: [0.91, 0.71, 0.55, 1.0],
        hat: [0.13, 0.13, 0.16, 1.0],
    };

    let member_x = |slot: f32| rect[0] + rect[2] * slot - guy_w * 0.5;
    let walk_in_x = |m: &CrewMember, u: f32| -> f32 {
        let target = member_x(m.slot);
        if m.from_left {
            rect[0] - guy_w + (target - rect[0] + guy_w) * u
        } else {
            let start = rect[0] + rect[2] + guy_w;
            start + (target - start) * u
        }
    };

    if elapsed < CREWS_IN_END {
        let u = (elapsed / CREWS_IN_END).clamp(0.0, 1.0);
        for m in &CREW {
            let style = SpriteStyle { shirt: m.outfit, skin: m.skin, hat: m.hat };
            let bob = ((elapsed * 14.0 + m.slot * 20.0).sin() * 1.2 * s).abs();
            draw_sprite_styled(
                sugarloaf,
                if m.from_left { &HOOD_SHOOT } else { &FEDORA_SHOOT },
                walk_in_x(m, u),
                guy_y - bob,
                px,
                &style,
                !m.from_left,
                false,
            );
        }
        let main_start = rect[0] + rect[2] + guy_w * 2.0;
        let main_target = member_x(main_slot);
        draw_sprite_styled(
            sugarloaf,
            walk_frame(elapsed, 9.0),
            main_start + (main_target - main_start) * u,
            guy_y,
            px,
            &main_style,
            true,
            false,
        );
        return;
    }

    let shootout = (STANDOFF_END..SHOOTOUT_END).contains(&elapsed);
    let tick = (elapsed * 12.0) as u32;

    // Crews: standing members hold/shoot, fallen ones lie where they
    // stood for the rest of the skit.
    let mut left_standing = 0;
    for (i, m) in CREW.iter().enumerate() {
        let style = SpriteStyle { shirt: m.outfit, skin: m.skin, hat: m.hat };
        let x = member_x(m.slot);
        let down = shootout_progress(elapsed) > m.fall_at;
        if !down {
            if m.from_left {
                left_standing += 1;
            }
            draw_sprite_styled(
                sugarloaf,
                if m.from_left { &HOOD_SHOOT } else { &FEDORA_SHOOT },
                x,
                guy_y,
                px,
                &style,
                !m.from_left,
                false,
            );
            // Muzzle flash, staggered per member.
            if shootout && (tick as usize + i * 3) % 5 == 0 {
                let gun_y = guy_y + 6.5 * px;
                let flash_x = if m.from_left {
                    x + 10.0 * px
                } else {
                    x + 2.0 * px - 4.0 * s
                };
                sugarloaf.rect(None, flash_x, gun_y - 1.0 * s, 4.0 * s, 4.0 * s, MUZZLE, DEPTH, ORDER);
                sugarloaf.rect(None, flash_x + 1.5 * s, gun_y - 3.0 * s, 1.5 * s, 8.0 * s, MUZZLE, DEPTH, ORDER);
            }
        } else {
            draw_sprite_styled(
                sugarloaf,
                if m.from_left { &HOOD_SHOOT } else { &FEDORA_SHOOT },
                x,
                fallen_y,
                px,
                &style,
                !m.from_left,
                true,
            );
        }
    }

    // The main fella: shooting through the fight, then walking off
    // through the aftermath — last one standing.
    if elapsed < SHOOTOUT_END {
        draw_sprite_styled(
            sugarloaf,
            if shootout { &SHOOT_POSE } else { &FEDORA_SHOOT },
            member_x(main_slot),
            guy_y,
            px,
            &main_style,
            true,
            false,
        );
        if shootout && tick % 4 == 0 {
            let gun_y = guy_y + 6.5 * px;
            let flash_x = member_x(main_slot) + 2.0 * px - 4.0 * s;
            sugarloaf.rect(None, flash_x, gun_y - 1.0 * s, 4.0 * s, 4.0 * s, MUZZLE, DEPTH, ORDER);
        }
    } else {
        let u = ((elapsed - SHOOTOUT_END) / (total - SHOOTOUT_END)).clamp(0.0, 1.0);
        let start = member_x(main_slot);
        let x = start + (rect[0] + rect[2] + guy_w - start) * u;
        draw_sprite_styled(
            sugarloaf,
            walk_frame(elapsed, 7.0),
            x,
            guy_y,
            px,
            &main_style,
            false,
            false,
        );
    }

    // Tracer fire criss-crossing the corridor while both sides shoot.
    if shootout {
        let gun_y = guy_y + 6.5 * px;
        let left_x = rect[0] + rect[2] * 0.34;
        let right_x = rect[0] + rect[2] * 0.62;
        for i in 0..5u32 {
            // Rightward fire only while the left crew still has
            // shooters; return fire dries up as they drop.
            let phase = (elapsed * (2.2 + hash01(i) * 1.4) + hash01(i.wrapping_add(40)))
                % 1.0;
            if left_standing > 0 {
                let bx = left_x + (right_x - left_x) * phase;
                let by = gun_y + (hash01(i.wrapping_add(11)) - 0.5) * 10.0 * s;
                sugarloaf.rect(None, bx, by, 5.0 * s, 1.5 * s, TRACER, DEPTH, ORDER);
            }
            let bx = right_x - (right_x - left_x) * phase;
            let by = gun_y + (hash01(i.wrapping_add(77)) - 0.5) * 10.0 * s;
            sugarloaf.rect(None, bx, by, 5.0 * s, 1.5 * s, TRACER, DEPTH, ORDER);
        }
    }
}

/// Falls are keyed to absolute skit time; before the shootout starts
/// nobody is down.
fn shootout_progress(elapsed: f32) -> f32 {
    if elapsed < STANDOFF_END {
        0.0
    } else {
        elapsed
    }
}

/// Enters from the RIGHT edge, faces left, waters leftward, exits
/// left — the puddle stays behind, glistening.
fn render_piss(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    elapsed: f32,
    scale: f32,
    theme: &IdeTheme,
) {
    let s = scale.clamp(0.5, 3.0);
    let px = 3.0 * s;
    let guy_w = SPRITE_W as f32 * px;
    let guy_h = SPRITE_H as f32 * px;
    let floor_y = rect[1] + rect[3] - 36.0 * s;
    let guy_y = floor_y - guy_h;
    let stop_x = rect[0] + rect[2] * 0.58;
    let shirt = theme.f32(theme.accent);

    if elapsed < WALK_IN_END {
        let u = (elapsed / WALK_IN_END).clamp(0.0, 1.0);
        let start_x = rect[0] + rect[2] + guy_w;
        let x = start_x + (stop_x - start_x) * u;
        let bob = ((elapsed * 14.0).sin() * 1.2 * s).abs();
        draw_sprite(
            sugarloaf,
            walk_frame(elapsed, 7.0),
            x,
            guy_y - bob,
            px,
            shirt,
            true,
        );
        return;
    }

    if elapsed < STAND_END {
        draw_sprite(sugarloaf, &WALK_B, stop_x, guy_y, px, shirt, true);
        return;
    }

    if elapsed < ZIP_END {
        draw_sprite(sugarloaf, &PISS_POSE, stop_x, guy_y, px, shirt, true);
        let pissing = elapsed < PISS_END;
        let piss_t = ((elapsed - STAND_END) / (PISS_END - STAND_END)).clamp(0.0, 1.0);

        // Stream: dashed parabola from the fella's front (his left,
        // our screen-left) down to the landing spot.
        let x0 = stop_x + (SPRITE_W as f32 - 7.0) * px;
        let y0 = guy_y + 8.5 * px;
        let reach = 24.0 * s + 10.0 * s * (elapsed * 2.2).sin().abs();
        let drop = floor_y - y0;
        if pissing {
            let dash_phase = elapsed * 3.0;
            for i in 0..14 {
                let u = i as f32 / 13.0;
                if ((u * 7.0 + dash_phase) % 1.0) > 0.72 {
                    continue;
                }
                let dx = x0 - reach * u;
                let dy = y0 + drop * u * u;
                sugarloaf.rect(
                    None,
                    dx,
                    dy,
                    2.0 * s,
                    2.0 * s,
                    PISS_YELLOW,
                    DEPTH,
                    ORDER,
                );
            }
        }

        let puddle_w = (12.0 + 58.0 * piss_t) * s;
        let puddle_h = 4.0 * s;
        sugarloaf.rounded_rect(
            None,
            x0 - reach - puddle_w * 0.6,
            floor_y - puddle_h * 0.5,
            puddle_w,
            puddle_h,
            PISS_YELLOW,
            DEPTH,
            puddle_h * 0.5,
            ORDER,
        );
        return;
    }

    // Exit left, past the puddle.
    let total = total_seconds(AgentFxKind::Piss);
    let u = ((elapsed - ZIP_END) / (total - ZIP_END)).clamp(0.0, 1.0);
    let x = stop_x + (rect[0] - guy_w * 2.0 - stop_x) * u;
    let bob = ((elapsed * 16.0).sin() * 1.4 * s).abs();
    draw_sprite(
        sugarloaf,
        walk_frame(elapsed, 9.0),
        x,
        guy_y - bob,
        px,
        shirt,
        true,
    );

    let x0 = stop_x + (SPRITE_W as f32 - 7.0) * px;
    let reach = 24.0 * s;
    let puddle_w = 70.0 * s;
    let puddle_h = 4.0 * s;
    sugarloaf.rounded_rect(
        None,
        x0 - reach - puddle_w * 0.6,
        floor_y - puddle_h * 0.5,
        puddle_w,
        puddle_h,
        PISS_YELLOW,
        DEPTH,
        puddle_h * 0.5,
        ORDER,
    );
}

/// Storms in from the right, plants himself, and lets the grawlix fly
/// — cartoon cuss bubble, flailing arms, anger sparks — then storms
/// back off the way he came.
fn render_cuss(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    elapsed: f32,
    scale: f32,
    theme: &IdeTheme,
) {
    let s = scale.clamp(0.5, 3.0);
    let px = 3.0 * s;
    let guy_w = SPRITE_W as f32 * px;
    let guy_h = SPRITE_H as f32 * px;
    let floor_y = rect[1] + rect[3] - 36.0 * s;
    let guy_y = floor_y - guy_h;
    let stop_x = rect[0] + rect[2] * 0.55;
    let shirt = theme.f32(theme.accent);

    if elapsed < CUSS_WALK_IN_END {
        // Stomps in fast — he's already fuming.
        let u = (elapsed / CUSS_WALK_IN_END).clamp(0.0, 1.0);
        let start_x = rect[0] + rect[2] + guy_w;
        let x = start_x + (stop_x - start_x) * u;
        let bob = ((elapsed * 18.0).sin() * 1.6 * s).abs();
        draw_sprite(
            sugarloaf,
            walk_frame(elapsed, 10.0),
            x,
            guy_y - bob,
            px,
            shirt,
            true,
        );
        return;
    }

    if elapsed < RANT_END {
        let rant_t = elapsed - CUSS_WALK_IN_END;
        // Arms flail between fists-up and thrown-out ~5 times/sec,
        // with a little hop on the pose swap.
        let pose_tick = (rant_t * 5.0) as usize;
        let pose = if pose_tick % 2 == 0 { &RANT_A } else { &RANT_B };
        let hop = if pose_tick % 2 == 0 { 2.0 * s } else { 0.0 };
        draw_sprite(sugarloaf, pose, stop_x, guy_y - hop, px, shirt, true);

        // Grawlix speech bubble above-left of his head, symbols
        // reshuffling every beat like a cartoon strip.
        const GRAWLIX: [&str; 5] = ["#$@!", "%&#?!", "@#%!", "!$&@", "?!#$%"];
        let symbols = GRAWLIX[(rant_t * 4.0) as usize % GRAWLIX.len()];
        let font_size = 13.0 * s;
        let opts = DrawOpts {
            font_size,
            color: theme.u8(theme.fg),
            bold: true,
            ..DrawOpts::default()
        };
        let text_w = sugarloaf.text_mut().measure(symbols, &opts);
        let pad = 6.0 * s;
        let bubble_w = text_w + pad * 2.0;
        let bubble_h = font_size + pad * 1.4;
        let jitter = ((rant_t * 23.0).sin() * 1.5 * s, (rant_t * 31.0).cos() * 1.2 * s);
        let bubble_x = stop_x - bubble_w + 3.0 * px + jitter.0;
        let bubble_y = guy_y - bubble_h - 8.0 * s + jitter.1;
        sugarloaf.rounded_rect(
            None,
            bubble_x,
            bubble_y,
            bubble_w,
            bubble_h,
            theme.f32(theme.panel_bg()),
            DEPTH,
            6.0 * s,
            ORDER,
        );
        // Bubble tail: two shrinking squares stepping toward his head.
        for (i, side) in [3.0, 1.6].iter().enumerate() {
            sugarloaf.rect(
                None,
                bubble_x + bubble_w - (6.0 - i as f32 * 3.0) * s,
                bubble_y + bubble_h + i as f32 * 3.0 * s,
                side * s,
                side * s,
                theme.f32(theme.panel_bg()),
                DEPTH,
                ORDER,
            );
        }
        sugarloaf.text_mut().draw(
            bubble_x + pad,
            bubble_y + (bubble_h - font_size) * 0.45,
            symbols,
            &opts,
        );

        // Anger sparks popping around his head, cartoon-style.
        let spark_phase = (rant_t * 6.0) as usize;
        for i in 0..3 {
            if (spark_phase + i) % 3 == 0 {
                continue;
            }
            let angle = (spark_phase + i * 2) as f32 * 2.4;
            let sx = stop_x + 6.0 * px + angle.cos() * 9.0 * px * 0.6;
            let sy = guy_y - 2.0 * px + angle.sin() * 4.0 * px * 0.4;
            // A tiny "+" — two crossed 1px bars.
            sugarloaf.rect(None, sx - 2.0 * s, sy, 5.0 * s, 1.5 * s, ANGER_RED, DEPTH, ORDER);
            sugarloaf.rect(None, sx, sy - 2.0 * s, 1.5 * s, 5.0 * s, ANGER_RED, DEPTH, ORDER);
        }
        return;
    }

    // Storms back off to the right.
    let total = total_seconds(AgentFxKind::Cuss);
    let u = ((elapsed - RANT_END) / (total - RANT_END)).clamp(0.0, 1.0);
    let x = stop_x + (rect[0] + rect[2] + guy_w - stop_x) * u;
    let bob = ((elapsed * 18.0).sin() * 1.6 * s).abs();
    draw_sprite(
        sugarloaf,
        walk_frame(elapsed, 11.0),
        x,
        guy_y - bob,
        px,
        shirt,
        false,
    );
}
