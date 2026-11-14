//! Animated GIF, WebP and PNG (APNG): every frame composited onto the canvas and given a mip
//! chain on the cache worker, with the time each one ends. A paint picks the frame for its
//! clock (`Motion::at`) and wakes for the next only while the image is drawn, so an animation
//! off screen or in a hidden window costs nothing.
use std::io::Cursor;

use image::{AnimationDecoder, ImageDecoder};

use super::{Decoded, ImageError, Mip, mip_chain};

/// Decoded pixels the frames of one animation may take, mips included; a larger one shows its
/// first frame still.
const BUDGET: usize = 256 << 20;

/// The frames after the first (which is [`Decoded::mips`]) and when each frame ends.
pub(crate) struct Motion {
    /// Frames 1.. as mip chains.
    pub frames: Vec<Vec<Mip>>,
    /// Seconds from the start of a play to the end of each frame, frame 0's first; the last is
    /// the length of a play.
    pub ends: Vec<f64>,
    /// How many times it plays; `None` loops forever.
    pub plays: Option<u32>,
}

impl Motion {
    /// The frame `t` seconds after the animation started, and when (on the same clock) the
    /// next one shows; `None` once its last play has ended and it rests on its last frame.
    pub(crate) fn at(&self, t: f64) -> (usize, Option<f64>) {
        let last = self.ends.len() - 1;
        let period = self.ends[last];
        let play = (t.max(0.0) / period).floor();
        if self.plays.is_some_and(|n| play >= f64::from(n)) { return (last, None); }
        let within = t.max(0.0) - play * period;
        let i = self.ends.partition_point(|&end| end <= within).min(last);
        (i, Some(play * period + self.ends[i]))
    }

    /// Frame `i`'s mips, given frame 0's.
    pub(crate) fn frame<'a>(&'a self, first: &'a [Mip], i: usize) -> &'a [Mip] {
        if i == 0 { first } else { &self.frames[i - 1] }
    }
}

/// Which animated format `bytes` sniff as. A web build hands GIF and WebP to the browser,
/// which decodes their first frame.
#[derive(Clone, Copy)]
enum Kind {
    #[cfg(not(target_arch = "wasm32"))]
    Gif,
    #[cfg(not(target_arch = "wasm32"))]
    WebP,
    Png,
}

fn sniff(bytes: &[u8]) -> Option<Kind> {
    #[cfg(not(target_arch = "wasm32"))]
    if bytes.starts_with(b"GIF8") { return Some(Kind::Gif); }
    #[cfg(not(target_arch = "wasm32"))]
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" { return Some(Kind::WebP); }
    bytes.starts_with(b"\x89PNG\r\n\x1a\n").then_some(Kind::Png)
}

/// An animation of more than one frame, or `None` for a still (or a format that isn't
/// animated), which decodes as any image does.
pub(super) fn decode(bytes: &[u8]) -> Option<Result<Decoded, ImageError>> {
    let kind = sniff(bytes)?;
    let error = |e: image::ImageError| ImageError(e.to_string());
    let cursor = Cursor::new(bytes);
    // Each format's own loop count: a GIF's says how often it repeats after its first play, a
    // WebP's and an APNG's how often it plays (as browsers count them).
    let (size, frames, plays) = match kind {
        #[cfg(not(target_arch = "wasm32"))]
        Kind::Gif => {
            let decoder = image::codecs::gif::GifDecoder::new(cursor).ok()?;
            let plays = finite(decoder.loop_count()).map(|n| n.saturating_add(1));
            (decoder.dimensions(), decoder.into_frames(), plays)
        }
        #[cfg(not(target_arch = "wasm32"))]
        Kind::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(cursor).ok()?;
            if !decoder.has_animation() { return None; }
            // Browsers composite on transparency, whatever background the file names.
            decoder.set_background_color(image::Rgba([0, 0, 0, 0])).ok()?;
            let plays = finite(decoder.loop_count());
            (decoder.dimensions(), decoder.into_frames(), plays)
        }
        Kind::Png => {
            let decoder = image::codecs::png::PngDecoder::new(cursor).ok()?;
            if !decoder.is_apng().ok()? { return None; }
            let size = decoder.dimensions();
            let decoder = decoder.apng().ok()?;
            let plays = finite(decoder.loop_count());
            (size, decoder.into_frames(), plays)
        }
    };
    let (w, h) = size;
    if w == 0 || h == 0 || w > 8192 || h > 8192 { return Some(Err(ImageError("animation is too large".into()))); }
    let per_frame = w as usize * h as usize * 16 / 3;
    let mut chains = Vec::new();
    let mut ends = Vec::new();
    let mut elapsed = 0.0;
    for frame in frames {
        let frame = match frame {
            Ok(frame) => frame,
            // A truncated file plays the frames it has, as browsers do.
            Err(_) if !chains.is_empty() => break,
            Err(e) => return Some(Err(error(e))),
        };
        if !chains.is_empty() && (chains.len() + 1) * per_frame > BUDGET { return Some(still(chains)); }
        let (numer, denom) = frame.delay().numer_denom_ms();
        let ms = f64::from(numer) / f64::from(denom.max(1));
        // Browsers show a frame of 10 ms or less for 100 ms: such files mean "as fast as you
        // can", which would otherwise spin.
        elapsed += if ms <= 10.0 { 0.1 } else { ms / 1000.0 };
        let buffer = frame.into_buffer();
        match mip_chain(buffer.width(), buffer.height(), buffer.as_raw()) {
            Ok(decoded) => chains.push(decoded.mips),
            Err(e) => return Some(Err(e)),
        }
        ends.push(elapsed);
    }
    if chains.len() < 2 { return (!chains.is_empty()).then(|| still(chains)); }
    let mut chains = chains.into_iter();
    let first = chains.next().expect("two frames or more");
    Some(Ok(Decoded { mips: first, motion: Some(Box::new(Motion { frames: chains.collect(), ends, plays })) }))
}

/// The first frame alone, for an animation too large to hold.
fn still(chains: Vec<Vec<Mip>>) -> Result<Decoded, ImageError> {
    let first = chains.into_iter().next().ok_or_else(|| ImageError("animation has no frames".into()))?;
    Ok(Decoded { mips: first, motion: None })
}

fn finite(count: image::metadata::LoopCount) -> Option<u32> {
    match count {
        image::metadata::LoopCount::Infinite => None,
        image::metadata::LoopCount::Finite(n) => Some(n.get()),
    }
}
