//! Runtime "look" style slots — the Mash Up Pack surface beyond
//! theme/shader: scrollbars, markdown decorations, icon overrides.
//!
//! Every field is an *override*: `None`/default means "keep this
//! site's existing style", so an empty `LookStyle` renders the app
//! exactly as before. The desktop host merges the active pack's slots
//! with the user's `[look.*]` config (config wins) and publishes the
//! result here; draw sites read through the accessor fns. Mirrors the
//! `ACTIVE_IDE_THEME` cell pattern — wasm never publishes, so web
//! keeps the defaults.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Scrollbar restyle knobs. Sites have divergent defaults (square
/// gray 6px widget bars vs rounded themed markdown bars), so every
/// field is optional and resolved per site via the `*_or` helpers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollbarStyle {
    /// Thumb thickness in logical px.
    pub width: Option<f32>,
    /// Corner rounding as a fraction of width: 0.0 square … 0.5 pill.
    pub radius_factor: Option<f32>,
    /// Minimum thumb length in logical px.
    pub min_thumb: Option<f32>,
    /// Packed `0xRRGGBB`, like `IdeTheme` slots.
    pub thumb: Option<u32>,
    pub thumb_drag: Option<u32>,
    pub track: Option<u32>,
}

impl ScrollbarStyle {
    pub fn width_or(&self, site_default: f32) -> f32 {
        self.width.unwrap_or(site_default)
    }

    pub fn radius_factor_or(&self, site_default: f32) -> f32 {
        self.radius_factor.unwrap_or(site_default)
    }

    pub fn min_thumb_or(&self, site_default: f32) -> f32 {
        self.min_thumb.unwrap_or(site_default)
    }

    /// Resolved radius for a thumb of the given width.
    pub fn radius(&self, width: f32, site_default_factor: f32) -> f32 {
        (self.radius_factor_or(site_default_factor)).clamp(0.0, 0.5) * width
    }

    pub fn thumb_or(&self, site_default: [f32; 4]) -> [f32; 4] {
        self.thumb
            .map(|c| rgb_f32(c, site_default[3]))
            .unwrap_or(site_default)
    }

    pub fn thumb_drag_or(&self, site_default: [f32; 4]) -> [f32; 4] {
        self.thumb_drag
            .map(|c| rgb_f32(c, site_default[3]))
            .unwrap_or(site_default)
    }

    /// Track color; sites that don't draw a track today draw one only
    /// when the style sets it.
    pub fn track_or(&self, site_default: Option<[f32; 4]>) -> Option<[f32; 4]> {
        self.track
            .map(|c| rgb_f32(c, site_default.map_or(1.0, |d| d[3])))
            .or(site_default)
    }
}

/// Packed `0xRRGGBB` → `[r, g, b, alpha]` floats.
pub fn rgb_f32(color: u32, alpha: f32) -> [f32; 4] {
    [
        ((color >> 16) & 0xff) as f32 / 255.0,
        ((color >> 8) & 0xff) as f32 / 255.0,
        (color & 0xff) as f32 / 255.0,
        alpha,
    ]
}

/// Task-list checkbox rendering in the markdown surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckboxLook {
    /// Thin muted square + green ✓ (today's style).
    #[default]
    Modern,
    /// Chunky filled box + bold X — Windows-3.1 ROADMAP.TXT energy.
    Retro95,
}

impl CheckboxLook {
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "retro95" | "retro-95" | "retro_95" => CheckboxLook::Retro95,
            _ => CheckboxLook::Modern,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkdownStyle {
    pub checkbox: CheckboxLook,
    /// Font family for the markdown surface only (terminal keeps its
    /// own). Resolved against the font library by the host.
    pub font_family: Option<String>,
}

/// One semantic icon override (`"folder"`, `"status.branch"`, …).
#[derive(Clone, Copy, Debug, Default)]
pub struct IconOverride {
    pub glyph: Option<&'static str>,
    pub color: Option<[u8; 4]>,
}

#[derive(Clone, Debug, Default)]
pub struct LookStyle {
    pub scrollbar: ScrollbarStyle,
    pub markdown: MarkdownStyle,
    /// Per-letter tint cycle for the NEOISM wordmarks; empty = tint
    /// everything with the theme's `fg`.
    pub wordmark_colors: Vec<u32>,
    pub icons: HashMap<String, IconOverride>,
}

static ACTIVE_LOOK: RwLock<Option<Arc<LookStyle>>> = RwLock::new(None);

fn default_look() -> &'static Arc<LookStyle> {
    static DEFAULT: OnceLock<Arc<LookStyle>> = OnceLock::new();
    DEFAULT.get_or_init(|| Arc::new(LookStyle::default()))
}

pub fn set_active_look(look: LookStyle) {
    if let Ok(mut cell) = ACTIVE_LOOK.write() {
        *cell = Some(Arc::new(look));
    }
}

pub fn active_look() -> Arc<LookStyle> {
    ACTIVE_LOOK
        .read()
        .ok()
        .and_then(|cell| cell.clone())
        .unwrap_or_else(|| default_look().clone())
}

/// Hot-path accessor: `ScrollbarStyle` is `Copy`.
pub fn scrollbar_style() -> ScrollbarStyle {
    active_look().scrollbar
}

pub fn markdown_checkbox_look() -> CheckboxLook {
    active_look().markdown.checkbox
}

pub fn markdown_font_family() -> Option<String> {
    active_look().markdown.font_family.clone()
}

pub fn icon_override(key: &str) -> Option<IconOverride> {
    active_look().icons.get(key).copied()
}

/// The wordmark letter-tint cycle: the pack's colors, or `[fallback]`
/// (the theme ink) when unset.
pub fn wordmark_colors_or(fallback: u32) -> Vec<u32> {
    let colors = active_look().wordmark_colors.clone();
    if colors.is_empty() {
        vec![fallback]
    } else {
        colors
    }
}

/// Tint an RGBA wordmark bitmap in place, one color per vertical
/// letter strip, cycling `colors` across `strips` equal columns. The
/// source art is a white glyph, so a channel multiply recolors it.
/// Both NEOISM wordmarks (splash + agent home) draw their letters as
/// `source_rect` strips of one texture, which is why baking the cycle
/// into the pixels needs no draw-side changes.
pub fn tint_wordmark_pixels(
    pixels: &mut [u8],
    width: usize,
    strips: usize,
    colors: &[u32],
) {
    if width == 0 || strips == 0 || colors.is_empty() {
        return;
    }
    let channels: Vec<[u32; 3]> = colors
        .iter()
        .map(|c| [(c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff])
        .collect();
    for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
        let x = i % width;
        let strip = (x * strips / width).min(strips - 1);
        let [tr, tg, tb] = channels[strip % channels.len()];
        px[0] = (px[0] as u32 * tr / 255) as u8;
        px[1] = (px[1] as u32 * tg / 255) as u8;
        px[2] = (px[2] as u32 * tb / 255) as u8;
    }
}

/// The overridden glyph for `key`, or `default` — the one-liner every
/// glyph table wants.
pub fn themed_glyph(key: &str, default: &'static str) -> &'static str {
    icon_override(key)
        .and_then(|over| over.glyph)
        .unwrap_or(default)
}

/// Intern an override glyph so icon tables can keep returning
/// `&'static str`. Bounded by the number of distinct glyph strings
/// ever configured; re-publishing the same glyph reuses its slot.
pub fn intern_glyph(glyph: &str) -> &'static str {
    static INTERNED: RwLock<Vec<&'static str>> = RwLock::new(Vec::new());
    if let Ok(interned) = INTERNED.read() {
        if let Some(hit) = interned.iter().find(|s| **s == glyph) {
            return hit;
        }
    }
    let leaked: &'static str = Box::leak(glyph.to_string().into_boxed_str());
    if let Ok(mut interned) = INTERNED.write() {
        // A racing writer may have added it between the read and the
        // write lock; prefer the existing slot to bound the leak.
        if let Some(hit) = interned.iter().find(|s| **s == glyph) {
            return hit;
        }
        interned.push(leaked);
    }
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_fall_back_per_site() {
        let style = ScrollbarStyle::default();
        assert_eq!(style.width_or(6.0), 6.0);
        assert_eq!(style.radius(6.0, 0.5), 3.0);
        assert_eq!(style.thumb_or([0.6, 0.6, 0.6, 0.5]), [0.6, 0.6, 0.6, 0.5]);
        assert_eq!(style.track_or(None), None);

        let style = ScrollbarStyle {
            width: Some(10.0),
            radius_factor: Some(0.0),
            thumb: Some(0x808080),
            track: Some(0xc0c0c0),
            ..Default::default()
        };
        assert_eq!(style.width_or(6.0), 10.0);
        assert_eq!(style.radius(10.0, 0.5), 0.0);
        // Override keeps the site's alpha so fades still work.
        assert_eq!(
            style.thumb_or([0.6, 0.6, 0.6, 0.5]),
            [
                128.0 / 255.0,
                128.0 / 255.0,
                128.0 / 255.0,
                0.5
            ]
        );
        assert!(style.track_or(None).is_some());
    }

    #[test]
    fn checkbox_names() {
        assert_eq!(CheckboxLook::from_name("retro95"), CheckboxLook::Retro95);
        assert_eq!(CheckboxLook::from_name("Retro-95"), CheckboxLook::Retro95);
        assert_eq!(CheckboxLook::from_name("modern"), CheckboxLook::Modern);
        assert_eq!(CheckboxLook::from_name("whatever"), CheckboxLook::Modern);
    }

    #[test]
    fn glyph_interning_reuses_slots() {
        let a = intern_glyph("\u{f07b}");
        let b = intern_glyph("\u{f07b}");
        assert_eq!(a.as_ptr(), b.as_ptr());
    }
}
