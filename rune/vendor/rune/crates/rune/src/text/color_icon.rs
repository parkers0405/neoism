//! Icons that paint their own colours (`fill="#…"`, gradients), such as file-type badges. They
//! rasterize in colour with resvg and draw as images; a monochrome icon stays an alpha mask in
//! the glyph atlas, tinted by its element's colour.

use std::collections::HashMap;

use super::{Icon, TextSystem};
use crate::color::Color;
use crate::pixels::Pixels;

/// CPU bytes of colour icon images kept before images unused last frame are dropped.
const BUDGET: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct ColorIcons {
    /// What each icon, by address, paints with.
    kinds: HashMap<usize, Kind>,
    /// Each colour icon's image by address, device-pixel side and `currentColor` (`#rrggbb`, or
    /// empty for an icon without it); None when it can't rasterize.
    images: HashMap<(usize, u32, String), Entry>,
    bytes: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Mono,
    Color,
    /// Its own colours, and `currentColor` parts that take the element's colour.
    ColorAndCurrent,
}

struct Entry {
    pixels: Option<Pixels>,
    used: bool,
}

impl ColorIcons {
    pub(super) fn begin_frame(&mut self) {
        if self.bytes > BUDGET {
            self.images.retain(|_, e| e.used);
            self.bytes = self.images.values().filter_map(|e| e.pixels.as_ref()).map(bytes_of).sum();
        }
        self.images.values_mut().for_each(|e| e.used = false);
    }
}

impl TextSystem {
    /// The colour image of `icon` at `px` device pixels square when its body names its own
    /// colours, its `currentColor` parts in `color`; None for a monochrome icon, which draws
    /// from the alpha atlas ([`Self::icon_quad`]). Without Rune's `svg` feature every icon draws
    /// monochrome.
    ///
    /// With `one` (`El::icon_one_color`) the image is the icon's silhouette in `color`: its own
    /// colours drawn as their coverage only, as an SVG mask filled with `currentColor` draws it.
    pub fn color_icon(&mut self, icon: &'static Icon, px: u32, color: Color, one: bool) -> Option<&Pixels> {
        let at = icon as *const Icon as usize;
        let ColorIcons { kinds, images, bytes } = &mut self.color_icons;
        let hex = || color.hex()[..7].to_owned();
        let current = match *kinds.entry(at).or_insert_with(|| kind(icon)) {
            Kind::Mono => return None,
            _ if one => format!("one{}", hex()),
            Kind::Color => String::new(),
            Kind::ColorAndCurrent => hex(),
        };
        let entry = images.entry((at, px, current)).or_insert_with_key(|(_, _, current)| {
            let pixels = match current.strip_prefix("one") {
                Some(hex) => rasterize(icon, px, hex, Some(color)),
                None => rasterize(icon, px, current, None),
            };
            *bytes += pixels.as_ref().map_or(0, bytes_of);
            Entry { pixels, used: false }
        });
        entry.used = true;
        entry.pixels.as_ref()
    }
}

fn kind(icon: &Icon) -> Kind {
    let b = icon.body;
    if !["fill=\"#", "stroke=\"#", "stop-color=\"#"].iter().any(|p| b.contains(p)) {
        return Kind::Mono;
    }
    if b.contains("currentColor") { Kind::ColorAndCurrent } else { Kind::Color }
}


fn bytes_of(p: &Pixels) -> usize {
    let (w, h) = p.size();
    w as usize * h as usize * 4
}

#[cfg(feature = "svg")]
fn rasterize(icon: &Icon, px: u32, current: &str, one: Option<Color>) -> Option<Pixels> {
    let v = icon.view;
    let body = if current.is_empty() { icon.body.to_owned() } else { icon.body.replace("currentColor", current) };
    let doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 {v} {v}">{body}</svg>"#
    );
    let (w, h, mut rgba) = crate::image::svg_rgba(doc.as_bytes(), px).ok()?;
    // One colour: the drawing's coverage in it; its alpha comes from the instance.
    if let Some(c) = one {
        let rgb = [c.r, c.g, c.b].map(|v| (crate::color::srgb_encode(v) * 255.0).round() as u8);
        rgba.chunks_exact_mut(4).for_each(|p| p[..3].copy_from_slice(&rgb));
    }
    let pixels = Pixels::new();
    pixels.set(w, h, rgba);
    Some(pixels)
}

#[cfg(not(feature = "svg"))]
fn rasterize(_: &Icon, _: u32, _: &str, _: Option<Color>) -> Option<Pixels> {
    None
}
