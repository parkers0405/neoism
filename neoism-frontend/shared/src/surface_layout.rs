//! Pure layout for configurable, edge-docked Rust-owned surfaces.
//!
//! The registry describes geometry owned by the host. Lua may only patch where
//! that geometry is placed and whether it is visible; it never supplies draw or
//! input callbacks. All coordinates are logical pixels and every emitted edge
//! is snapped to the active device-pixel grid.

use std::collections::BTreeMap;
use std::fmt;

use neoism_lua::{DockEdge, SurfaceAlign, SurfaceLayoutPatch};

use crate::layout::Rect;

pub const CHROME_ACTIONS_SURFACE: &str = "chrome.actions";
pub const CHROME_MENU_ITEM: &str = "chrome.menu";
pub const CHROME_EXPLORER_ITEM: &str = "chrome.explorer";
pub const CHROME_NOTES_ITEM: &str = "chrome.notes";
pub const CHROME_NEW_AGENT_ITEM: &str = "chrome.new-agent";
pub const CHROME_SEARCH_ITEM: &str = "chrome.search";
pub const CHROME_PRESENCE_ITEM: &str = "chrome.presence";
pub const CHROME_AGENT_DETAILS_ITEM: &str = "chrome.agent-details";
pub const CHROME_AGENT_ITEM: &str = "chrome.agent";
pub const CHROME_SERVERS_ITEM: &str = "chrome.servers";

/// Rust-owned intrinsic dimensions for a surface item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceItemSize {
    pub width: f32,
    pub height: f32,
}

impl SurfaceItemSize {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// Host default for one reserving edge surface.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceDescriptor {
    pub visible: bool,
    pub dock: DockEdge,
    pub thickness: f32,
    pub order: i32,
}

/// Host default and intrinsic hit-box size for one Rust-owned item.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceItemDescriptor {
    pub visible: bool,
    pub surface: String,
    pub align: SurfaceAlign,
    pub order: i32,
    pub size: SurfaceItemSize,
}

/// The complete allow-list of surfaces and items understood by the renderer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceRegistry {
    pub surfaces: BTreeMap<String, SurfaceDescriptor>,
    pub items: BTreeMap<String, SurfaceItemDescriptor>,
}

impl SurfaceRegistry {
    /// Allow-listed built-in chrome defaults. Callers update each descriptor's
    /// Rust-owned intrinsic size before solving when its measured size differs.
    pub fn chrome_defaults(item_size: SurfaceItemSize) -> Self {
        let mut surfaces = BTreeMap::new();
        surfaces.insert(
            CHROME_ACTIONS_SURFACE.into(),
            SurfaceDescriptor {
                visible: true,
                dock: DockEdge::Top,
                thickness: 30.0,
                order: 0,
            },
        );

        let mut items = BTreeMap::new();
        for (id, order) in [
            (CHROME_MENU_ITEM, 0),
            (CHROME_EXPLORER_ITEM, 10),
            (CHROME_NOTES_ITEM, 20),
            (CHROME_NEW_AGENT_ITEM, 30),
            (CHROME_SEARCH_ITEM, 40),
        ] {
            items.insert(
                id.into(),
                SurfaceItemDescriptor {
                    visible: true,
                    surface: CHROME_ACTIONS_SURFACE.into(),
                    align: SurfaceAlign::Start,
                    order,
                    size: item_size,
                },
            );
        }
        for (id, order) in [
            (CHROME_PRESENCE_ITEM, 0),
            (CHROME_AGENT_DETAILS_ITEM, 10),
            (CHROME_AGENT_ITEM, 20),
            (CHROME_SERVERS_ITEM, 30),
        ] {
            items.insert(
                id.into(),
                SurfaceItemDescriptor {
                    visible: true,
                    surface: CHROME_ACTIONS_SURFACE.into(),
                    align: SurfaceAlign::End,
                    order,
                    size: item_size,
                },
            );
        }
        Self { surfaces, items }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSurface {
    pub visible: bool,
    pub dock: DockEdge,
    /// Configured logical thickness. `bounds` may be thinner when the viewport
    /// is smaller than the total requested reservation.
    pub thickness: f32,
    pub order: i32,
    pub bounds: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSurfaceItem {
    pub visible: bool,
    pub surface: String,
    pub align: SurfaceAlign,
    pub order: i32,
    pub size: SurfaceItemSize,
    /// `None` when either the item or its target surface is hidden.
    pub bounds: Option<Rect>,
}

/// One canonical result consumed by painting and hit testing.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSurfaceLayout {
    pub viewport: Rect,
    pub content: Rect,
    pub surfaces: BTreeMap<String, ResolvedSurface>,
    pub items: BTreeMap<String, ResolvedSurfaceItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceLayoutError {
    InvalidScale(f32),
    InvalidViewport(Rect),
    InvalidSurfaceThickness { surface: String, thickness: f32 },
    InvalidItemSize { item: String, size: SurfaceItemSize },
    UnknownSurface(String),
    UnknownItem(String),
    UnknownItemSurface { item: String, surface: String },
}

impl fmt::Display for SurfaceLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid surface layout: {self:?}")
    }
}

impl std::error::Error for SurfaceLayoutError {}

/// Resolve defaults plus a data-only customization patch.
pub fn resolve_surface_layout(
    viewport: Rect,
    scale: f32,
    registry: &SurfaceRegistry,
    patch: &SurfaceLayoutPatch,
) -> Result<ResolvedSurfaceLayout, SurfaceLayoutError> {
    validate_rect(viewport)
        .then_some(())
        .ok_or(SurfaceLayoutError::InvalidViewport(viewport))?;
    if !scale.is_finite() || scale <= 0.0 {
        return Err(SurfaceLayoutError::InvalidScale(scale));
    }
    for id in patch.surfaces.keys() {
        if !registry.surfaces.contains_key(id) {
            return Err(SurfaceLayoutError::UnknownSurface(id.clone()));
        }
    }
    for id in patch.items.keys() {
        if !registry.items.contains_key(id) {
            return Err(SurfaceLayoutError::UnknownItem(id.clone()));
        }
    }

    let mut surfaces = BTreeMap::new();
    for (id, descriptor) in &registry.surfaces {
        let custom = patch.surfaces.get(id);
        let thickness = custom
            .and_then(|value| value.thickness)
            .unwrap_or(descriptor.thickness);
        if !thickness.is_finite() || thickness < 0.0 {
            return Err(SurfaceLayoutError::InvalidSurfaceThickness {
                surface: id.clone(),
                thickness,
            });
        }
        surfaces.insert(
            id.clone(),
            ResolvedSurface {
                visible: custom
                    .and_then(|value| value.visible)
                    .unwrap_or(descriptor.visible),
                dock: custom
                    .and_then(|value| value.dock.as_ref())
                    .cloned()
                    .unwrap_or_else(|| descriptor.dock.clone()),
                thickness,
                order: custom
                    .and_then(|value| value.order)
                    .unwrap_or(descriptor.order),
                bounds: None,
            },
        );
    }

    let mut items = BTreeMap::new();
    for (id, descriptor) in &registry.items {
        if !descriptor.size.width.is_finite()
            || !descriptor.size.height.is_finite()
            || descriptor.size.width < 0.0
            || descriptor.size.height < 0.0
        {
            return Err(SurfaceLayoutError::InvalidItemSize {
                item: id.clone(),
                size: descriptor.size,
            });
        }
        let custom = patch.items.get(id);
        let surface = custom
            .and_then(|value| value.surface.as_ref())
            .cloned()
            .unwrap_or_else(|| descriptor.surface.clone());
        if !surfaces.contains_key(&surface) {
            return Err(SurfaceLayoutError::UnknownItemSurface {
                item: id.clone(),
                surface,
            });
        }
        items.insert(
            id.clone(),
            ResolvedSurfaceItem {
                visible: custom
                    .and_then(|value| value.visible)
                    .unwrap_or(descriptor.visible),
                surface,
                align: custom
                    .and_then(|value| value.align.as_ref())
                    .cloned()
                    .unwrap_or_else(|| descriptor.align.clone()),
                order: custom
                    .and_then(|value| value.order)
                    .unwrap_or(descriptor.order),
                size: descriptor.size,
                bounds: None,
            },
        );
    }

    let viewport = snap_rect(viewport, scale);
    let mut content = viewport;
    let mut surface_order: Vec<_> = surfaces
        .iter()
        .filter(|(_, surface)| surface.visible)
        .map(|(id, surface)| (edge_rank(&surface.dock), surface.order, id.clone()))
        .collect();
    surface_order.sort();

    for (_, _, id) in surface_order {
        let surface = surfaces.get_mut(&id).expect("surface id came from map");
        let requested = snap(surface.thickness, scale);
        let bounds = match surface.dock {
            DockEdge::Top => {
                let amount = requested.min(content.h);
                let rect = Rect::new(content.x, content.y, content.w, amount);
                content.y += amount;
                content.h -= amount;
                rect
            }
            DockEdge::Bottom => {
                let amount = requested.min(content.h);
                content.h -= amount;
                Rect::new(content.x, content.y + content.h, content.w, amount)
            }
            DockEdge::Left => {
                let amount = requested.min(content.w);
                let rect = Rect::new(content.x, content.y, amount, content.h);
                content.x += amount;
                content.w -= amount;
                rect
            }
            DockEdge::Right => {
                let amount = requested.min(content.w);
                content.w -= amount;
                Rect::new(content.x + content.w, content.y, amount, content.h)
            }
        };
        surface.bounds = Some(snap_rect(bounds, scale));
        content = snap_rect(content, scale);
    }

    for (surface_id, surface) in &surfaces {
        let Some(bounds) = surface.bounds else {
            continue;
        };
        let horizontal = matches!(surface.dock, DockEdge::Top | DockEdge::Bottom);
        let mut start_ids: Vec<_> = items
            .iter()
            .filter(|(_, item)| {
                item.visible
                    && item.surface == *surface_id
                    && item.align == SurfaceAlign::Start
            })
            .map(|(id, item)| (item.order, id.clone()))
            .collect();
        let mut end_ids: Vec<_> = items
            .iter()
            .filter(|(_, item)| {
                item.visible
                    && item.surface == *surface_id
                    && item.align == SurfaceAlign::End
            })
            .map(|(id, item)| (item.order, id.clone()))
            .collect();
        start_ids.sort();
        end_ids.sort();
        let (mut start, mut end) = if horizontal {
            (bounds.x, bounds.x + bounds.w)
        } else {
            (bounds.y, bounds.y + bounds.h)
        };
        // End items are allocated from the outer edge in reverse sort order so
        // their visible order still follows ascending `(order, id)`.
        for (from_end, id) in start_ids
            .into_iter()
            .map(|(_, id)| (false, id))
            .chain(end_ids.into_iter().rev().map(|(_, id)| (true, id)))
        {
            let item = items.get_mut(&id).expect("item id came from map");
            let extent = snap(
                if horizontal {
                    item.size.width
                } else {
                    item.size.height
                },
                scale,
            );
            let available = (end - start).max(0.0);
            let extent = extent.min(available);
            let along = if from_end {
                end -= extent;
                end
            } else {
                let value = start;
                start += extent;
                value
            };
            let rect = if horizontal {
                let cross = snap(item.size.height, scale).min(bounds.h);
                Rect::new(along, bounds.y + (bounds.h - cross) * 0.5, extent, cross)
            } else {
                let cross = snap(item.size.width, scale).min(bounds.w);
                Rect::new(bounds.x + (bounds.w - cross) * 0.5, along, cross, extent)
            };
            item.bounds = Some(snap_rect(rect, scale));
        }
    }

    Ok(ResolvedSurfaceLayout {
        viewport,
        content,
        surfaces,
        items,
    })
}

fn edge_rank(edge: &DockEdge) -> u8 {
    match edge {
        DockEdge::Top => 0,
        DockEdge::Bottom => 1,
        DockEdge::Left => 2,
        DockEdge::Right => 3,
    }
}

fn validate_rect(rect: Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.w.is_finite()
        && rect.h.is_finite()
        && rect.w >= 0.0
        && rect.h >= 0.0
        && (rect.x + rect.w).is_finite()
        && (rect.y + rect.h).is_finite()
}

fn snap(value: f32, scale: f32) -> f32 {
    (((value as f64) * (scale as f64)).round() / (scale as f64)) as f32
}

fn snap_rect(rect: Rect, scale: f32) -> Rect {
    let left = snap(rect.x, scale);
    let top = snap(rect.y, scale);
    let right = snap(rect.x + rect.w, scale);
    let bottom = snap(rect.y + rect.h, scale);
    Rect::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_lua::{SurfaceItemPatch, SurfacePatch};

    #[test]
    fn default_action_band_and_items_flow_across_the_top() {
        let registry = SurfaceRegistry::chrome_defaults(SurfaceItemSize::new(20.0, 20.0));
        let layout = resolve_surface_layout(
            Rect::new(0.0, 0.0, 800.0, 600.0),
            1.0,
            &registry,
            &SurfaceLayoutPatch::default(),
        )
        .unwrap();

        assert_eq!(layout.content, Rect::new(0.0, 30.0, 800.0, 570.0));
        assert_eq!(
            layout.surfaces[CHROME_ACTIONS_SURFACE].bounds,
            Some(Rect::new(0.0, 0.0, 800.0, 30.0))
        );
        assert_eq!(
            layout.items[CHROME_MENU_ITEM].bounds,
            Some(Rect::new(0.0, 5.0, 20.0, 20.0))
        );
        assert_eq!(
            layout.items[CHROME_SEARCH_ITEM].bounds,
            Some(Rect::new(80.0, 5.0, 20.0, 20.0))
        );
        assert_eq!(
            layout.items[CHROME_PRESENCE_ITEM].bounds,
            Some(Rect::new(720.0, 5.0, 20.0, 20.0))
        );
        assert_eq!(
            layout.items[CHROME_SERVERS_ITEM].bounds,
            Some(Rect::new(780.0, 5.0, 20.0, 20.0))
        );
    }

    #[test]
    fn one_surface_patch_moves_the_whole_action_band_to_a_44px_left_rail() {
        let registry = SurfaceRegistry::chrome_defaults(SurfaceItemSize::new(22.0, 22.0));
        let mut patch = SurfaceLayoutPatch::default();
        patch.surfaces.insert(
            CHROME_ACTIONS_SURFACE.into(),
            SurfacePatch {
                dock: Some(DockEdge::Left),
                thickness: Some(44.0),
                ..SurfacePatch::default()
            },
        );

        let layout = resolve_surface_layout(
            Rect::new(0.0, 0.0, 800.0, 600.0),
            1.0,
            &registry,
            &patch,
        )
        .unwrap();
        assert_eq!(layout.content, Rect::new(44.0, 0.0, 756.0, 600.0));
        assert_eq!(
            layout.surfaces[CHROME_ACTIONS_SURFACE].bounds,
            Some(Rect::new(0.0, 0.0, 44.0, 600.0))
        );
        assert_eq!(
            layout.items[CHROME_MENU_ITEM].bounds,
            Some(Rect::new(11.0, 0.0, 22.0, 22.0))
        );
        assert_eq!(
            layout.items[CHROME_SEARCH_ITEM].bounds,
            Some(Rect::new(11.0, 88.0, 22.0, 22.0))
        );
        assert_eq!(
            layout.items[CHROME_PRESENCE_ITEM].bounds,
            Some(Rect::new(11.0, 512.0, 22.0, 22.0))
        );
        assert_eq!(
            layout.items[CHROME_SERVERS_ITEM].bounds,
            Some(Rect::new(11.0, 578.0, 22.0, 22.0))
        );
    }

    #[test]
    fn item_visibility_order_align_and_relocation_are_resolved() {
        let mut registry =
            SurfaceRegistry::chrome_defaults(SurfaceItemSize::new(10.0, 10.0));
        registry.surfaces.insert(
            "plugin.rail".into(),
            SurfaceDescriptor {
                visible: true,
                dock: DockEdge::Right,
                thickness: 20.0,
                order: 5,
            },
        );
        let mut patch = SurfaceLayoutPatch::default();
        patch.items.insert(
            CHROME_EXPLORER_ITEM.into(),
            SurfaceItemPatch {
                visible: Some(false),
                ..SurfaceItemPatch::default()
            },
        );
        patch.items.insert(
            CHROME_SEARCH_ITEM.into(),
            SurfaceItemPatch {
                order: Some(-10),
                ..SurfaceItemPatch::default()
            },
        );
        patch.items.insert(
            CHROME_SERVERS_ITEM.into(),
            SurfaceItemPatch {
                surface: Some("plugin.rail".into()),
                align: Some(SurfaceAlign::Start),
                order: Some(7),
                ..SurfaceItemPatch::default()
            },
        );

        let layout = resolve_surface_layout(
            Rect::new(0.0, 0.0, 200.0, 100.0),
            1.0,
            &registry,
            &patch,
        )
        .unwrap();
        assert!(!layout.items[CHROME_EXPLORER_ITEM].visible);
        assert_eq!(layout.items[CHROME_EXPLORER_ITEM].bounds, None);
        assert_eq!(layout.items[CHROME_SEARCH_ITEM].order, -10);
        assert_eq!(layout.items[CHROME_SEARCH_ITEM].bounds.unwrap().x, 0.0);
        assert_eq!(layout.items[CHROME_SERVERS_ITEM].surface, "plugin.rail");
        assert_eq!(layout.items[CHROME_SERVERS_ITEM].align, SurfaceAlign::Start);
        assert_eq!(layout.items[CHROME_SERVERS_ITEM].order, 7);
        assert_eq!(
            layout.items[CHROME_SERVERS_ITEM].bounds,
            Some(Rect::new(185.0, 30.0, 10.0, 10.0))
        );
    }

    #[test]
    fn snaps_edges_and_rejects_non_finite_geometry() {
        let registry = SurfaceRegistry::chrome_defaults(SurfaceItemSize::new(20.0, 20.0));
        let layout = resolve_surface_layout(
            Rect::new(0.2, 0.2, 100.2, 100.2),
            2.0,
            &registry,
            &SurfaceLayoutPatch::default(),
        )
        .unwrap();
        assert_eq!(layout.viewport, Rect::new(0.0, 0.0, 100.5, 100.5));
        assert_eq!(layout.content, Rect::new(0.0, 30.0, 100.5, 70.5));

        assert!(matches!(
            resolve_surface_layout(
                Rect::new(0.0, 0.0, f32::NAN, 10.0),
                1.0,
                &registry,
                &SurfaceLayoutPatch::default()
            ),
            Err(SurfaceLayoutError::InvalidViewport(_))
        ));
    }
}
