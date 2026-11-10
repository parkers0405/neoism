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
use rune::{El, Image, ImageCache, Live, Shape, Tx, canvas, el};
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
pub struct Host {
    hover: [f32; LETTERS],
    pointer: Option<usize>,
    animation: WordmarkAnimation,
    created: Option<f64>,
    /// The idle shimmer, breath and hover lift run on live bindings: each letter's lift, its
    /// scale, and its glow's scale (the view binds them once; frames never re-run views).
    lift: [Live<[f32; 2]>; LETTERS],
    scale: [Live<f32>; LETTERS],
    glow: [Live<f32>; LETTERS],
    /// The drawn height, for the lift (set by the view).
    height: std::cell::Cell<f32>,
    splash: bool,
}

impl Default for Host {
    fn default() -> Host {
        Host {
            hover: [0.0; LETTERS], pointer: None, animation: WordmarkAnimation::default(), created: None,
            lift: std::array::from_fn(|_| Live::new([0.0, 0.0])),
            scale: std::array::from_fn(|_| Live::new(1.0)),
            glow: std::array::from_fn(|_| Live::new(1.0)),
            height: std::cell::Cell::new(0.0),
            splash: false,
        }
    }
}

impl Host {
    /// Write this frame's shimmer, breath and hover into the bindings.
    fn drive(&self, seconds: f32) {
        let h = self.height.get();
        for i in 0..LETTERS {
            let hover = self.hover[i].clamp(0.0, 1.0);
            let shimmer = (seconds / SHIMMER_PERIOD + i as f32 * 0.16) * TAU;
            let scale = 1.0 + hover * HOVER_SCALE + shimmer.sin() * SHIMMER_AMP;
            let glow = if self.splash {
                let breath = ((seconds / BREATH_PERIOD * TAU).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
                1.0 + 0.05 * (0.05 + 0.13 * breath + hover * 0.4)
            } else {
                1.04 + hover * 0.05
            };
            self.lift[i].set([0.0, -hover * HOVER_LIFT * h]);
            self.scale[i].set(scale);
            self.glow[i].set(scale * glow);
        }
    }
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
        let host = self.hosts.entry(key.to_owned()).or_insert_with(|| Host { splash: key.starts_with("splash"), ..Host::default() });
        host.created.get_or_insert(clock);
        host
    }
}

impl Wordmark {
    /// Advance the hosts actually drawn in visible panes, including unfocused splits.
    /// Returns (moving, rebuild): `moving` while anything animates (keep frames coming; the
    /// idle shimmer runs on live bindings), `rebuild` only when a view must re-run: a wordmark
    /// that just appeared (to bind), or a click effect drawn as a canvas.
    pub fn tick(&mut self, visible: &[String], now: f64) -> (bool, bool) {
        let dt = self.last_tick.map(|last| (now - last).clamp(0.0, 0.10) as f32).unwrap_or(0.0);
        self.clock = now;
        for (key, host) in &mut self.hosts {
            if !visible.contains(key) { host.pointer = None; host.hover = [0.0; LETTERS]; }
        }
        if visible.is_empty() { self.last_tick = None; return (false, false); }
        self.last_tick = Some(now);
        let reduced = self.reduced;
        let alpha = 1.0 - (-HOVER_RATE * dt).exp();
        let mut rebuild = false;
        let mut effect = false;
        for key in visible {
            let fresh = !self.hosts.contains_key(key);
            let host = self.host(key);
            rebuild |= fresh;
            for (i, hover) in host.hover.iter_mut().enumerate() {
                let target = if host.pointer == Some(i) { 1.0 } else { 0.0 };
                *hover = if reduced { 0.0 } else { *hover + (target - *hover) * alpha };
            }
            if !reduced { host.drive(host.created.map_or(0.0, |c| (now - c) as f32)); }
            effect |= host.animation.is_active(now);
        }
        (!reduced || effect, rebuild || effect)
    }
}

pub fn host_key(kind: &str, tab: u64) -> String { format!("{kind}-wordmark:{tab}") }

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

/// The wordmark at `width` points wide, keyed per tab through `host_key`.
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

    if let Some(host) = host { host.height.set(height); }
    let slice = |i: usize| [i as f32 * width / LETTERS as f32, 0.0, width / LETTERS as f32, height];
    // Click effects place their cells from the eased letter rects (static once reduced).
    let letters: Vec<([f32; 4], f32)> = (0..LETTERS).map(|i| {
        let hover = if mark.reduced { 0.0 } else { host.map(|h| h.hover[i]).unwrap_or(0.0).clamp(0.0, 1.0) };
        (if mark.reduced { slice(i) } else { letter_rect([0.0, 0.0, width, height], i, hover, seconds) }, hover)
    }).collect();

    if show_original {
        for i in 0..LETTERS {
            let [x, y, w, h] = slice(i);
            // Each part sits on its idle slice; the shimmer, breath and hover ride its bindings.
            let image = |part: &str, scale: Option<&Live<f32>>| {
                let mut part = el().key(format!("{key}-{part}-{i}")).absolute(x, y).w(w).h(h).pixels(mark.letters[i].pixels());
                if let (Some(host), false) = (host, mark.reduced) {
                    part = part.live_offset(&host.lift[i]);
                    if let Some(scale) = scale { part = part.live_scale(scale); }
                }
                part
            };
            let letter = image("letter", host.map(|h| &h.scale[i]));
            let glow = image("glow", host.map(|h| &h.glow[i]));
            // Same paint order as `wordmarkImageOps`: agent glow under the letter, splash over it.
            root = if splash { root.child(letter).child(glow) } else { root.child(glow).child(letter) };
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splash_and_agent_in_separate_panes_both_ease_hover() {
        let mut app = App::new();
        let splash = host_key("splash", app.active_tab().id);
        app.split(crate::panes::Axis::Horizontal);
        app.open_tab(crate::tabs::TabKind::Agent { session: None });
        let agent = host_key("agent", app.active_tab().id);
        let visible = app.visible_wordmarks();
        assert!(visible.contains(&splash) && visible.contains(&agent));
        app.wordmark.host(&splash).pointer = Some(0);
        app.wordmark.host(&agent).pointer = Some(3);
        app.wordmark.tick(&visible, 0.0);
        for frame in 1..=60 { app.wordmark.tick(&visible, frame as f64 / 120.0); }
        assert!(app.wordmark.hosts[&splash].hover[0] > 0.99);
        assert!(app.wordmark.hosts[&agent].hover[3] > 0.99);
        assert_eq!(app.wordmark.hosts[&agent].hover[0], 0.0);
        app.wordmark.host(&splash).pointer = None;
        for frame in 61..=120 { app.wordmark.tick(&visible, frame as f64 / 120.0); }
        assert!(app.wordmark.hosts[&splash].hover[0] < 0.01);
        assert!(app.wordmark.hosts[&agent].hover[3] > 0.99);
    }

    #[test]
    fn hidden_wordmarks_stop_ticking_and_clear_stale_pointer_state() {
        let mut mark = Wordmark::default();
        let key = host_key("agent", 2);
        mark.host(&key).pointer = Some(1);
        mark.tick(&[key.clone()], 0.0);
        mark.tick(&[key.clone()], 0.1);
        assert_eq!(mark.tick(&[], 0.2), (false, false));
        assert!(mark.hosts[&key].pointer.is_none());
        assert_eq!(mark.hosts[&key].hover, [0.0; LETTERS]);
    }
}
