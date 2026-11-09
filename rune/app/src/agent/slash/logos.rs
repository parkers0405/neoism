//! Provider marks (`ui/ProviderLogo.tsx`): a 512 x 64 atlas of 64 px alpha masks, tinted with
//! the original colour policy: Anthropic's clay, OpenRouter/OpenAI/xAI in white on dark themes
//! (black on light), everything else in the theme's blue.

use crate::ui::theme::Palette;
use rune::{Image, ImageCache};
use std::cell::RefCell;
use std::collections::HashMap;

const ATLAS: &[u8] = include_bytes!("../../../assets/provider-logos.png");
const TILE: u32 = 64;

#[derive(Default)]
pub struct Logos {
    atlas: RefCell<Option<(u32, Vec<u8>)>>,
    tinted: RefCell<HashMap<(usize, u32), Image>>,
}

fn is_dark(bg: u32) -> bool {
    let (r, g, b) = ((bg >> 16) & 0xff, (bg >> 8) & 0xff, bg & 0xff);
    (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) < 128.0
}

/// `providerLogoColor`.
pub fn color(index: usize, pal: &Palette) -> u32 {
    match index {
        1 => 0xd97757,
        0 | 2 | 7 => if is_dark(pal.bg) { 0xffffff } else { 0x000000 },
        _ => pal.blue,
    }
}

impl Logos {
    /// The tinted mark for atlas tile `index`, made once per colour.
    pub fn image(&self, images: &ImageCache, index: usize, rgb: u32) -> Option<Image> {
        if let Some(image) = self.tinted.borrow().get(&(index, rgb)) { return Some(image.clone()); }
        let mut atlas = self.atlas.borrow_mut();
        if atlas.is_none() { *atlas = decode(ATLAS); }
        let (width, rgba) = atlas.as_ref()?;
        if (index as u32 + 1) * TILE > *width { return None; }
        let (r, g, b) = ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
        let mut tile = Vec::with_capacity((TILE * TILE * 4) as usize);
        for y in 0..TILE {
            for x in 0..TILE {
                let at = ((y * width + index as u32 * TILE + x) * 4) as usize;
                tile.extend_from_slice(&[r, g, b, rgba[at + 3]]);
            }
        }
        let image = images.rgba(TILE, TILE, tile);
        self.tinted.borrow_mut().insert((index, rgb), image.clone());
        Some(image)
    }
}

fn decode(bytes: &[u8]) -> Option<(u32, Vec<u8>)> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    buffer.truncate(info.buffer_size());
    (info.color_type == png::ColorType::Rgba).then_some((info.width, buffer))
}
