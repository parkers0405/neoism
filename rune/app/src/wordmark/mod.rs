//! The NEOISM wordmark (`rune/README.md`, "NEOISM wordmark"), ported from the GPUiX host
//! (`gpuix/native/src/wordmark.rs`, `gpuix/src/ui/AnimatedWordmark.tsx`) and the shared geometry
//! (`shared/src/panels/agent_pane/view/wordmark.rs`): six letter slices of the original artwork,
//! each easing toward hovered at `1 - exp(-14 dt)`, scaled/lifted by `letter_rect` with the idle
//! shimmer, drawn with its glow copy, plus the fifteen click effects of `wordmark_animation`.
//!
//! Like the original (`animating: !reduced`), the wordmark draws every frame while it is on
//! screen; `tick` stops asking for frames as soon as no wordmark is visible.

mod animation;

use crate::app::App;
use crate::ui::theme::rgb;
use animation::WordmarkAnimation;
use rune::{El, Image, ImageCache, Shape, Tx, canvas, el};
use std::collections::HashMap;
use std::f32::consts::TAU;

const ARTWORK: &[u8] = include_bytes!("../../assets/splash/neoism-wordmark.png");
pub const LETTERS: usize = 6;
const HOVER_RATE: f32 = 14.0;
const HOVER_SCALE: f32 = 0.18;
const HOVER_LIFT: f32 = 0.18;
const SHIMMER_AMP: f32 = 0.025;
const SHIMMER_PERIOD: f32 = 3.4;
const BREATH_PERIOD: f32 = 4.6;

/// One on-screen wordmark (splash or agent): its hover values and click animation.
#[derive(Default)]
pub struct Host {
    hover: [f32; LETTERS],
    pointer: Option<usize>,
    animation: WordmarkAnimation,
    created: Option<f64>,
}

#[derive(Default)]
pub struct Wordmark {
    pub letters: Vec<Image>,
    /// Width / height of the whole artwork.
    pub aspect: f32,
    pub tint: u32,
    hosts: HashMap<String, Host>,
    /// The frame clock as of the last `tick`.
    clock: f64,
    last_tick: Option<f64>,
    reduced: bool,
}

impl Wordmark {
    pub fn load(&mut self, images: &ImageCache, tint: u32) {
        self.reduced = std::env::var("NEOISM_REDUCED_MOTION").as_deref() == Ok("1");
        self.tint = tint;
        let Some((width, height, mut pixels)) = decode(ARTWORK) else { return };
        let [tr, tg, tb] = [(tint >> 16) & 0xff, (tint >> 8) & 0xff, tint & 0xff];
        for px in pixels.chunks_exact_mut(4) {
            px[0] = (px[0] as u32 * tr / 255) as u8;
            px[1] = (px[1] as u32 * tg / 255) as u8;
            px[2] = (px[2] as u32 * tb / 255) as u8;
        }
        self.aspect = width as f32 / height as f32;
        self.letters = (0..LETTERS)
            .map(|i| {
                let x0 = i * width as usize / LETTERS;
                let x1 = (i + 1) * width as usize / LETTERS;
                let w = x1 - x0;
                let mut strip = Vec::with_capacity(w * height as usize * 4);
                for y in 0..height as usize {
                    let start = (y * width as usize + x0) * 4;
                    strip.extend_from_slice(&pixels[start..start + w * 4]);
                }
                images.rgba(w as u32, height, strip)
            })
            .collect();
    }

    fn host(&mut self, key: &str) -> &mut Host {
        let clock = self.clock;
        let host = self.hosts.entry(key.to_owned()).or_default();
        host.created.get_or_insert(clock);
        host
    }
}

impl Wordmark {
    /// Advance hover easing to frame time `now`. `visible` is the key of the wordmark on screen
    /// (`"splash-wordmark"` / `"agent-wordmark"`), or `None`. Returns true while it animates:
    /// the caller then rebuilds the view and asks for the next frame (`tx.wake_after(ZERO)`).
    /// Like the original (`animating: !reduced`), a visible wordmark animates every frame
    /// (shimmer, breathing); nothing runs while none is visible.
    pub fn tick(&mut self, visible: Option<&str>, now: f64) -> bool {
        let dt = self.last_tick.map(|last| (now - last).clamp(0.0, 0.10) as f32).unwrap_or(0.0);
        self.clock = now;
        let Some(key) = visible else {
            self.last_tick = None;
            return false;
        };
        self.last_tick = Some(now);
        let reduced = self.reduced;
        let host = self.host(key);
        let alpha = 1.0 - (-HOVER_RATE * dt).exp();
        for (i, hover) in host.hover.iter_mut().enumerate() {
            let target = if host.pointer == Some(i) { 1.0 } else { 0.0 };
            *hover = if reduced { 0.0 } else { *hover + (target - *hover) * alpha };
        }
        !reduced || host.animation.is_active(now)
    }
}

fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    buffer.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

/// `letter_rect` from the shared view: the letter's slice grown by hover and shimmer and
/// lifted by hover, about the slice's centre.
fn letter_rect(rect: [f32; 4], index: usize, hover: f32, seconds: f32) -> [f32; 4] {
    let [x, y, w, h] = rect;
    let letter_w = w / LETTERS as f32;
    let shimmer = (seconds / SHIMMER_PERIOD + index as f32 * 0.16) * TAU;
    let extra_scale = 1.0 + hover * HOVER_SCALE + shimmer.sin() * SHIMMER_AMP;
    let center_x = x + (index as f32 + 0.5) * letter_w;
    let center_y = y + h * 0.5 - hover * HOVER_LIFT * h;
    let (lw, lh) = (letter_w * extra_scale, h * extra_scale);
    [center_x - lw * 0.5, center_y - lh * 0.5, lw, lh]
}

/// The wordmark at `width` points wide. `key` is `splash-wordmark` (Home) or `agent-wordmark`.
pub fn view(app: &App, key: &str, width: f32) -> El {
    let mark = &app.wordmark;
    if mark.letters.is_empty() {
        return el().key(key).w(width).h(width / 6.2);
    }
    let height = width / mark.aspect.max(1.0);
    let splash = key.starts_with("splash");
    let clock = mark.clock;
    let host = mark.hosts.get(key);
    let seconds = host.and_then(|h| h.created).map(|c| (clock - c) as f32).unwrap_or(0.0);
    let frame = host.and_then(|h| h.animation.frame(clock));
    let show_original = frame.as_ref().is_none_or(|f| f.show_original());
    let mut root = el().key(key).w(width).h(height);

    let mut letters = Vec::with_capacity(LETTERS);
    for i in 0..LETTERS {
        let hover = if mark.reduced { 0.0 } else { host.map(|h| h.hover[i]).unwrap_or(0.0).clamp(0.0, 1.0) };
        let rect = if mark.reduced { [i as f32 * width / 6.0, 0.0, width / 6.0, height] } else { letter_rect([0.0, 0.0, width, height], i, hover, seconds) };
        letters.push((rect, hover));
    }

    if show_original {
        for (i, &([x, y, w, h], hover)) in letters.iter().enumerate() {
            let glow = if splash {
                let breath = ((seconds / BREATH_PERIOD * TAU).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
                1.0 + 0.05 * (0.05 + 0.13 * breath + hover * 0.4)
            } else {
                1.04 + hover * 0.05
            };
            let glow_rect = [x + w * (1.0 - glow) * 0.5, y + h * (1.0 - glow) * 0.5, w * glow, h * glow];
            let image = |[x, y, w, h]: [f32; 4], part: &str| {
                el().key(format!("{key}-{part}-{i}")).absolute(x, y).w(w).h(h).pixels(mark.letters[i].pixels())
            };
            // Same paint order as `wordmarkImageOps`: agent glow under the letter, splash over it.
            if splash {
                root = root.child(image([x, y, w, h], "letter")).child(image(glow_rect, "glow"));
            } else {
                root = root.child(image(glow_rect, "glow")).child(image([x, y, w, h], "letter"));
            }
        }
    }

    if let Some(frame) = frame {
        // The original's paint bounds: cells may fly well outside the artwork.
        let clip = [-width, -height * 4.0, width * 3.0, height * 9.0];
        let tint = rgb(mark.tint);
        let mut shape = Shape::new().view_box(clip[2], clip[3]);
        for (i, &(rect, _)) in letters.iter().enumerate() {
            frame.letter_quads(i, rect, |quad, alpha| {
                let points: Vec<[f32; 2]> = quad.iter().map(|p| [p[0] - clip[0], p[1] - clip[1]]).collect();
                shape = std::mem::take(&mut shape).fill(points, tint.alpha(alpha));
            });
        }
        root = root.child(canvas(shape).key(format!("{key}-effect")).absolute(clip[0], clip[1]).w(clip[2]).h(clip[3]));
    }

    // Hover and click targets are the idle slices, as the original hit-tests them; they sit
    // on top so moving letters never steal the pointer.
    let owned = key.to_owned();
    for i in 0..LETTERS {
        let enter_key = owned.clone();
        let press_key = owned.clone();
        root = root.child(
            el().key(format!("{key}-hit-{i}")).absolute(i as f32 * width / LETTERS as f32, 0.0).w(width / LETTERS as f32).h(height)
                .on_hover_change(move |s: &mut App, inside: bool, _: &mut Tx| {
                    let host = s.wordmark.host(&enter_key);
                    if inside { host.pointer = Some(i); } else if host.pointer == Some(i) { host.pointer = None; }
                })
                .on_press(move |s: &mut App| {
                    if s.wordmark.reduced { return; }
                    let clock = s.wordmark.clock;
                    s.wordmark.host(&press_key).animation.replay(clock);
                }),
        );
    }
    root
}
