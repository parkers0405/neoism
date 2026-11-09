//! The NEOISM wordmark (`rune/README.md`, "NEOISM wordmark"): the original artwork split into six
//! letters, each lifting and growing under the pointer, with a click effect.

use crate::state::App;
use rune::{El, Image, ImageCache, Spring, Tx, el, row};

const ARTWORK: &[u8] = include_bytes!("../assets/splash/neoism-wordmark.png");
pub const LETTERS: usize = 6;
const HOVER_SCALE: f32 = 0.18;
const HOVER_LIFT: f32 = 0.18;
/// `1 - exp(-14 dt)` settles in about 0.22 s; a critically damped spring of that length matches.
const HOVER: Spring = Spring::new(0.22, 0.0);

#[derive(Default)]
pub struct Wordmark {
    pub letters: Vec<Image>,
    /// Width / height of the whole artwork.
    pub aspect: f32,
    pub hovered: Option<usize>,
    /// Bumped by a click; the view replays the build-in effect when it changes.
    pub plays: u32,
}

impl Wordmark {
    pub fn load(&mut self, images: &ImageCache, tint: u32) {
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

/// The wordmark at `width` points wide. `key` separates the splash and agent instances.
pub fn view(app: &App, key: &str, width: f32) -> El {
    let mark = &app.wordmark;
    if mark.letters.is_empty() {
        return el().key(key).w(width).h(width / 6.2);
    }
    let height = width / mark.aspect.max(1.0);
    let letter_w = width / LETTERS as f32;
    let mut letters = row().key(key).w(width).h(height);
    for (i, image) in mark.letters.iter().enumerate() {
        let hovered = mark.hovered == Some(i);
        letters = letters.child(
            el().key(format!("{key}-{i}")).w(letter_w).h(height)
                .pixels(image.pixels())
                .scale(if hovered { 1.0 + HOVER_SCALE } else { 1.0 }).scale_motion(HOVER)
                .offset(0.0, if hovered { -HOVER_LIFT * height } else { 0.0 }).offset_motion(HOVER)
                .on_hover_change(move |s: &mut App, inside: bool, _: &mut Tx| {
                    if inside { s.wordmark.hovered = Some(i); } else if s.wordmark.hovered == Some(i) { s.wordmark.hovered = None; }
                })
                .on_press(|s: &mut App| s.wordmark.plays += 1),
        );
    }
    letters
}
