//! The pixel fellas and their props (`view/fx.rs`): 12 x 16 art, drawn facing right.
//! Legend: `h` hair, `k` skin, `s` shirt, `p` pants, `b` boots, `f` hat/hood/halo, `g` gun.

#![allow(dead_code)]

/// Straight sRGB with alpha, as the original paint colours.
pub type Rgba = [f32; 4];

// 12-wide × 16-tall art pixels. Drawn facing RIGHT; `flip` mirrors.
// legend: h hair · k skin · s shirt · p pants · b boots
pub const WALK_A: [&str; 16] = [
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
pub const WALK_B: [&str; 16] = [
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
pub const PISS_POSE: [&str; 16] = [
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
pub const RANT_A: [&str; 16] = [
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
pub const RANT_B: [&str; 16] = [
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
pub const SHOOT_POSE: [&str; 16] = [
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
pub const HOOD_SHOOT: [&str; 16] = [
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
pub const FEDORA_SHOOT: [&str; 16] = [
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
pub const JESUS: [&str; 16] = [
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
pub const KNEEL_UP: [&str; 16] = [
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
pub const KNEEL_BOW: [&str; 16] = [
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

pub const SPRITE_W: usize = 12;
pub const SPRITE_H: usize = 16;

pub const HAIR: Rgba = [0.29, 0.18, 0.11, 1.0];
pub const SKIN: Rgba = [0.91, 0.71, 0.55, 1.0];
pub const PANTS: Rgba = [0.17, 0.29, 0.55, 1.0];
pub const BOOTS: Rgba = [0.15, 0.13, 0.11, 1.0];
pub const PISS_YELLOW: Rgba = [0.95, 0.82, 0.14, 0.9];
pub const ANGER_RED: Rgba = [0.88, 0.22, 0.16, 0.95];
pub const GUN: Rgba = [0.16, 0.16, 0.18, 1.0];
pub const MUZZLE: Rgba = [1.0, 0.92, 0.45, 0.95];
pub const TRACER: Rgba = [1.0, 0.95, 0.6, 0.85];
pub const GOLD: Rgba = [0.87, 0.70, 0.22, 1.0];
pub const GOLD_DARK: Rgba = [0.62, 0.48, 0.13, 1.0];
pub const ROBE_WHITE: Rgba = [0.96, 0.95, 0.90, 1.0];

