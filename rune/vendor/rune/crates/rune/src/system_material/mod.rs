//! System backdrop materials (`El::system_material`): the platform blurs what is behind the
//! window under an element's rounded rect. On macOS a native view per material sits under the
//! transparent Metal layer and moves in the same Core Animation transaction as the drawable,
//! so the material, Rune's shadow and the content on top present together. Everywhere else,
//! in opaque windows and in headless renders, the element's fill (or a neutral tint) stands in.

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(test)]
mod tests;

use crate::Color;
use rune_motion::NodeId;

/// A backdrop material the system composites behind a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SystemMaterial {
    /// Liquid Glass (`NSGlassEffectView`, regular style) on macOS 26 and later; the popover
    /// material on earlier macOS.
    Glass,
    /// The popover vibrancy material (`NSVisualEffectView`, `.popover`, behind window).
    Popover,
}

/// Native material views per window. Elements past this many draw their fallback instead.
pub const MAX_NATIVE_MATERIALS: usize = 4;

/// One material element as built this frame, in physical pixels like the instances.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MaterialDraw {
    pub id: NodeId,
    pub material: SystemMaterial,
    /// x, y, width, height.
    pub rect: [f32; 4],
    /// Corner radius, already clamped to half the shorter side.
    pub radius: f32,
    /// Continuous corners (`El::continuous_corners`).
    pub continuous: bool,
    /// The element's effective opacity, ancestors included.
    pub opacity: f32,
}

impl MaterialDraw {
    /// The native view's frame (x, y, width, height from the top left) and corner radius, in points.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn points(&self, scale: f32) -> ([f64; 4], f64) {
        let s = f64::from(scale.max(1e-3));
        (self.rect.map(|v| f64::from(v) / s), f64::from(self.radius) / s)
    }
}

/// Which existing view each draw takes: the view that showed the same element, else a free view
/// of the same material, else none (the caller makes one). `views` holds each view's material
/// and the element it showed last; `draws` each draw's material and element.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn assign(views: &[(SystemMaterial, Option<NodeId>)], draws: &[(SystemMaterial, NodeId)]) -> Vec<Option<usize>> {
    let mut taken = vec![false; views.len()];
    let mut out = vec![None; draws.len()];
    for (slot, &(material, id)) in out.iter_mut().zip(draws) {
        if let Some(v) = (0..views.len()).find(|&v| !taken[v] && views[v] == (material, Some(id))) {
            taken[v] = true;
            *slot = Some(v);
        }
    }
    for (slot, &(material, _)) in out.iter_mut().zip(draws) {
        if slot.is_some() { continue; }
        if let Some(v) = (0..views.len()).find(|&v| !taken[v] && views[v].0 == material) {
            taken[v] = true;
            *slot = Some(v);
        }
    }
    out
}

/// The Metal layer's `presentsWithTransaction`, which native materials, a moving window frame
/// and a live resize all need. One owner, so none turns it off under another.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PresentFlag {
    moving: bool,
    materials: bool,
    live: bool,
    written: bool,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl PresentFlag {
    /// Whether the layer presents with transactions.
    pub(crate) fn written(&self) -> bool {
        self.written
    }

    /// A window frame animation started or stopped. The value to write, when it changed.
    pub(crate) fn moving(&mut self, on: bool) -> Option<bool> {
        self.moving = on;
        self.write()
    }

    /// Native materials synced with the drawable switched on or off. The value to write, when it changed.
    pub(crate) fn materials(&mut self, on: bool) -> Option<bool> {
        self.materials = on;
        self.write()
    }

    /// The user started or stopped dragging the window's edge. The value to write, when it changed.
    pub(crate) fn live(&mut self, on: bool) -> Option<bool> {
        self.live = on;
        self.write()
    }

    fn write(&mut self) -> Option<bool> {
        let on = self.moving || self.materials || self.live;
        (on != self.written).then(|| { self.written = on; on })
    }
}

/// One corner radius for a rect: native materials take a single radius, so this uses the
/// largest corner, clamped to half the shorter side as Rune's own rounded rects are.
pub(crate) fn clamp_radius(radii: [f32; 4], width: f32, height: f32) -> f32 {
    radii.into_iter().fold(0.0f32, f32::max).min(width.min(height) * 0.5).max(0.0)
}

/// What a material element paints in Rune's own surface when no native view backs it: its
/// fill, or a neutral tint close to the material's average look when it has none.
pub(crate) fn fallback_fill(fill: Color, dark: bool) -> Color {
    if fill.a > 0.0 { return fill; }
    if dark { Color { r: 0.16, g: 0.16, b: 0.17, a: 0.94 } } else { Color { r: 0.96, g: 0.96, b: 0.97, a: 0.94 } }
}

/// How the shell hosts materials, from `RUNE_SYSTEM_MATERIAL`: unset for native views synced
/// with the drawable; `legacy` forces the pre-26 `NSVisualEffectView` path; `unsynced` moves
/// native views outside the drawable's transaction (to compare); `off` draws the fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Synced,
    Legacy,
    Unsynced,
    Off,
}

impl Mode {
    pub(crate) fn parse(value: Option<&str>) -> Mode {
        match value.map(str::trim) {
            Some("legacy") => Mode::Legacy,
            Some("unsynced") => Mode::Unsynced,
            Some("off" | "0") => Mode::Off,
            _ => Mode::Synced,
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn from_env() -> Mode {
        Mode::parse(std::env::var("RUNE_SYSTEM_MATERIAL").ok().as_deref())
    }
}
