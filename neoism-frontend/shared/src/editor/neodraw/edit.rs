//! Selection and direct-manipulation edits on a [`DrawPane`].
//!
//! These are pure, screen-agnostic operations (world coordinates in,
//! scene mutated). The interaction layer converts pointer events into
//! world coordinates via the [`Camera`](super::Camera) and drives a
//! gesture by snapshotting the scene at gesture-start and re-applying
//! an *absolute* transform on each move (so drags don't compound).

use super::geometry::Bounds;
use super::pane::{Camera, DrawPane};
use super::scene::{ShapeId, Vec2};

/// A selection-box manipulation handle. Eight for resize, one to rotate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Rotate,
}

impl Handle {
    /// The eight resize handles, in no particular order.
    pub const RESIZE: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Default shape handles. Auto-height text uses Left/Right width handles.
    pub const CORNERS: [Handle; 4] = [
        Handle::TopLeft,
        Handle::TopRight,
        Handle::BottomRight,
        Handle::BottomLeft,
    ];

    /// World position of this handle on the given selection bounds.
    pub fn world_pos(self, b: Bounds) -> Vec2 {
        let c = b.center();
        match self {
            Handle::TopLeft => Vec2::new(b.min.x, b.min.y),
            Handle::Top => Vec2::new(c.x, b.min.y),
            Handle::TopRight => Vec2::new(b.max.x, b.min.y),
            Handle::Right => Vec2::new(b.max.x, c.y),
            Handle::BottomRight => Vec2::new(b.max.x, b.max.y),
            Handle::Bottom => Vec2::new(c.x, b.max.y),
            Handle::BottomLeft => Vec2::new(b.min.x, b.max.y),
            Handle::Left => Vec2::new(b.min.x, c.y),
            // The rotate handle floats above the top edge.
            Handle::Rotate => Vec2::new(c.x, b.min.y - ROTATE_OFFSET),
        }
    }

    /// The opposite point that stays fixed while this handle drags,
    /// plus which axes the drag scales.
    fn anchor(self, b: Bounds) -> (Vec2, bool, bool) {
        let c = b.center();
        match self {
            Handle::TopLeft => (Vec2::new(b.max.x, b.max.y), true, true),
            Handle::TopRight => (Vec2::new(b.min.x, b.max.y), true, true),
            Handle::BottomRight => (Vec2::new(b.min.x, b.min.y), true, true),
            Handle::BottomLeft => (Vec2::new(b.max.x, b.min.y), true, true),
            Handle::Top => (Vec2::new(c.x, b.max.y), false, true),
            Handle::Bottom => (Vec2::new(c.x, b.min.y), false, true),
            Handle::Left => (Vec2::new(b.max.x, c.y), true, false),
            Handle::Right => (Vec2::new(b.min.x, c.y), true, false),
            Handle::Rotate => (c, false, false),
        }
    }
}

/// World units the rotate handle sits above the selection's top edge.
const ROTATE_OFFSET: f32 = 28.0;

impl DrawPane {
    /// Topmost shape (last in z-order) hit by a world-space point.
    pub fn pick_top(&self, world: Vec2, tol: f32) -> Option<ShapeId> {
        self.scene
            .shapes
            .iter()
            .rev()
            .find(|s| {
                if matches!(s.kind, super::scene::ShapeKind::Text { .. }) {
                    self.shape_bounds(s).contains(world, tol)
                } else {
                    s.hit_select(world, tol)
                }
            })
            .map(|s| s.id)
    }

    /// Click selection. `additive` toggles membership (shift-click).
    /// Returns whether a shape was hit.
    pub fn select_at(&mut self, world: Vec2, tol: f32, additive: bool) -> bool {
        match self.pick_top(world, tol) {
            Some(id) => {
                if additive {
                    if let Some(pos) = self.selection.iter().position(|s| *s == id) {
                        self.selection.remove(pos);
                    } else {
                        self.selection.push(id);
                    }
                } else if !self.selection.contains(&id) {
                    self.selection = vec![id];
                }
                true
            }
            None => {
                if !additive {
                    self.selection.clear();
                }
                false
            }
        }
    }

    pub fn select_all(&mut self) {
        self.selection = self.scene.shapes.iter().map(|s| s.id).collect();
    }

    /// Select every shape whose bounds intersect the world-space rect
    /// `a`–`b` (the marquee). Replaces the current selection.
    pub fn select_in_rect(&mut self, a: Vec2, b: Vec2) {
        let (min_x, max_x) = (a.x.min(b.x), a.x.max(b.x));
        let (min_y, max_y) = (a.y.min(b.y), a.y.max(b.y));
        self.selection = self
            .scene
            .shapes
            .iter()
            .filter(|s| {
                let bd = self.shape_bounds(s);
                bd.min.x <= max_x
                    && bd.max.x >= min_x
                    && bd.min.y <= max_y
                    && bd.max.y >= min_y
            })
            .map(|s| s.id)
            .collect();
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub fn has_selection(&self) -> bool {
        !self.selection.is_empty()
    }

    /// Copy the selected shapes into the pane clipboard.
    pub fn copy_selection(&mut self) -> bool {
        let copied: Vec<_> = self
            .scene
            .shapes
            .iter()
            .filter(|s| self.selection.contains(&s.id))
            .cloned()
            .collect();
        if copied.is_empty() {
            return false;
        }
        self.clipboard = copied;
        true
    }

    /// Paste clipboard shapes with fresh ids, offset so they don't sit
    /// exactly atop the originals, then select them.
    pub fn paste(&mut self) -> bool {
        if self.clipboard.is_empty() {
            return false;
        }
        self.checkpoint();
        const OFFSET: Vec2 = Vec2 { x: 16.0, y: 16.0 };
        let mut next = self.scene.next_id().0;
        let mut new_ids = Vec::new();
        let clones: Vec<_> = self.clipboard.clone();
        for mut shape in clones {
            shape.id = ShapeId(next);
            next += 1;
            shape.kind.translate(OFFSET);
            new_ids.push(shape.id);
            self.scene.shapes.push(shape);
        }
        self.selection = new_ids;
        self.dirty = true;
        true
    }

    /// Clone the selection in place (copy + paste in one step, Ctrl+D).
    pub fn duplicate_selection(&mut self) -> bool {
        self.copy_selection() && self.paste()
    }

    /// Combined world-space bounds of the current selection.
    pub fn selection_bounds(&self) -> Option<Bounds> {
        let mut acc: Option<Bounds> = None;
        for s in &self.scene.shapes {
            if self.selection.contains(&s.id) {
                let b = self.shape_bounds(s);
                acc = Some(match acc {
                    Some(a) => a.union(b),
                    None => b,
                });
            }
        }
        acc
    }

    /// Move every selected shape by `delta` (world space).
    pub fn translate_selection(&mut self, delta: Vec2) {
        if delta.x == 0.0 && delta.y == 0.0 {
            return;
        }
        for s in &mut self.scene.shapes {
            if self.selection.contains(&s.id) {
                s.kind.translate(delta);
            }
        }
        self.dirty = true;
    }

    /// Scale every selected shape about `anchor`.
    pub fn scale_selection(&mut self, anchor: Vec2, sx: f32, sy: f32) {
        if !sx.is_finite() || !sy.is_finite() {
            return;
        }
        for s in &mut self.scene.shapes {
            if self.selection.contains(&s.id) {
                if let super::scene::ShapeKind::Text { width, .. } = &mut s.kind {
                    if width.is_none() {
                        *width = self.text_dims.get(&s.id).map(|size| size.x);
                    }
                }
                s.kind.scale(anchor, sx, sy);
            }
        }
        self.dirty = true;
    }

    /// Resize the selection by dragging `handle`. `start_bounds` is the
    /// selection's bounds at gesture start; `pointer` is the current
    /// pointer in world space. Apply this to a gesture-start *snapshot*
    /// of the scene each move so it doesn't compound.
    pub fn resize_selection(
        &mut self,
        handle: Handle,
        start_bounds: Bounds,
        pointer: Vec2,
    ) {
        let (anchor, active_x, active_y) = handle.anchor(start_bounds);
        if self.selection.len() == 1 {
            let id = self.selection[0];
            if let Some(shape) = self.scene.shapes.iter_mut().find(|shape| shape.id == id)
            {
                if let super::scene::ShapeKind::Text { x, y, width, .. } = &mut shape.kind
                {
                    if active_x {
                        let left_handle = matches!(
                            handle,
                            Handle::Left | Handle::TopLeft | Handle::BottomLeft
                        );
                        let new_width = if left_handle {
                            start_bounds.max.x - pointer.x
                        } else {
                            pointer.x - start_bounds.min.x
                        }
                        .max(16.0);
                        *width = Some(new_width);
                        *x = if left_handle {
                            start_bounds.max.x - new_width
                        } else {
                            start_bounds.min.x
                        };
                    }
                    *y = start_bounds.min.y;
                    self.dirty = true;
                    return;
                }
            }
        }
        let start = handle.world_pos(start_bounds);
        let factor = |on: bool, p: f32, h: f32, a: f32| -> f32 {
            if !on {
                return 1.0;
            }
            let denom = h - a;
            if denom.abs() < f32::EPSILON {
                1.0
            } else {
                (p - a) / denom
            }
        };
        let sx = factor(active_x, pointer.x, start.x, anchor.x);
        let sy = factor(active_y, pointer.y, start.y, anchor.y);
        self.scale_selection(anchor, sx, sy);
    }

    pub(super) fn selection_handle_positions(
        &self,
        camera: &Camera,
    ) -> Vec<(Handle, Vec2)> {
        let Some(bounds) = self.selection_bounds() else {
            return Vec::new();
        };
        let text_only = self.selection.len() == 1
            && self.scene.shapes.iter().any(|shape| {
                self.selection.contains(&shape.id)
                    && matches!(shape.kind, super::scene::ShapeKind::Text { .. })
            });
        let handles: &[Handle] = if text_only {
            &[Handle::Left, Handle::Right]
        } else {
            &Handle::CORNERS
        };
        handles
            .iter()
            .copied()
            .map(|handle| {
                let world = handle.world_pos(bounds);
                let mut point = camera.world_to_screen(world);
                point.x += if world.x == bounds.min.x {
                    -2.0
                } else if world.x == bounds.max.x {
                    2.0
                } else {
                    0.0
                };
                point.y += if world.y == bounds.min.y {
                    -2.0
                } else if world.y == bounds.max.y {
                    2.0
                } else {
                    0.0
                };
                (handle, point)
            })
            .collect()
    }

    /// Hit-test the selection handles in *screen* space. `half_px` is
    /// half the handle's clickable square size in screen pixels.
    pub fn hit_handle(
        &self,
        screen: Vec2,
        camera: &Camera,
        half_px: f32,
    ) -> Option<Handle> {
        self.selection_handle_positions(camera)
            .into_iter()
            .find_map(|(handle, point)| {
                ((screen.x - point.x).abs() <= half_px
                    && (screen.y - point.y).abs() <= half_px)
                    .then_some(handle)
            })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_resize_changes_wrap_width_not_font_size_and_uses_matching_handles() {
        let mut p = super::DrawPane::new(std::path::PathBuf::from("resize.neodraw"));
        p.last_rect = Some([0.0, 0.0, 1000.0, 1000.0]);
        p.begin_text_at(super::Vec2::new(10.0, 20.0), 0.0);
        p.insert_text("A text box whose width can be changed");
        let id = p.editing_text.unwrap();
        p.commit_text();
        p.text_dims.insert(id, super::Vec2::new(240.0, 35.0));
        p.set_tool(crate::editor::neodraw::Tool::Text);
        let handles = p.selection_handle_positions(&p.camera);
        assert_eq!(handles.len(), 2);
        let right = handles
            .iter()
            .find(|(h, _)| *h == super::Handle::Right)
            .unwrap()
            .1;
        assert_eq!(
            p.hit_handle(right, &p.camera, 4.5),
            Some(super::Handle::Right)
        );
        assert!(p.begin_pointer(right.x, right.y, false));
        assert!(matches!(
            p.gesture,
            crate::editor::neodraw::DrawGesture::Resize { .. }
        ));
        p.drag_pointer(182.0, right.y);
        p.drag_pointer(132.0, right.y);
        p.end_pointer();
        match p.scene.shapes[0].kind {
            crate::editor::neodraw::ShapeKind::Text {
                x, y, size, width, ..
            } => {
                assert_eq!((x, y, size, width), (10.0, 20.0, 28.0, Some(120.0)));
            }
            _ => panic!(),
        }
        assert!(p.undo());
        assert!(matches!(
            p.scene.shapes[0].kind,
            crate::editor::neodraw::ShapeKind::Text { width: None, .. }
        ));
    }

    use super::*;
    use crate::editor::neodraw::scene::{Color, Scene, Shape, ShapeId, ShapeKind, Style};
    use std::path::PathBuf;

    fn rect(id: u64, x: f32, y: f32, w: f32, h: f32, fill: bool) -> Shape {
        Shape {
            id: ShapeId(id),
            kind: ShapeKind::Rect {
                x,
                y,
                w,
                h,
                corner: 0.0,
            },
            style: Style {
                fill: fill.then_some(Color::WHITE),
                ..Style::default()
            },
        }
    }

    fn pane_with(shapes: Vec<Shape>) -> DrawPane {
        let mut pane = DrawPane::new(PathBuf::from("t.neodraw"));
        pane.scene = Scene { version: 1, shapes };
        pane
    }

    #[test]
    fn pick_returns_topmost() {
        let pane = pane_with(vec![
            rect(1, 0.0, 0.0, 100.0, 100.0, true),
            rect(2, 0.0, 0.0, 100.0, 100.0, true),
        ]);
        // Both overlap; the later (id 2) is on top.
        assert_eq!(pane.pick_top(Vec2::new(50.0, 50.0), 2.0), Some(ShapeId(2)));
    }

    #[test]
    fn click_selects_and_empty_click_clears() {
        let mut pane = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0, true)]);
        assert!(pane.select_at(Vec2::new(50.0, 50.0), 2.0, false));
        assert_eq!(pane.selection, vec![ShapeId(1)]);
        assert!(!pane.select_at(Vec2::new(500.0, 500.0), 2.0, false));
        assert!(pane.selection.is_empty());
    }

    #[test]
    fn shift_click_toggles() {
        let mut pane = pane_with(vec![
            rect(1, 0.0, 0.0, 50.0, 50.0, true),
            rect(2, 100.0, 0.0, 50.0, 50.0, true),
        ]);
        pane.select_at(Vec2::new(25.0, 25.0), 2.0, false);
        pane.select_at(Vec2::new(125.0, 25.0), 2.0, true);
        assert_eq!(pane.selection, vec![ShapeId(1), ShapeId(2)]);
        pane.select_at(Vec2::new(25.0, 25.0), 2.0, true);
        assert_eq!(pane.selection, vec![ShapeId(2)]);
    }

    #[test]
    fn translate_moves_only_selected() {
        let mut pane = pane_with(vec![
            rect(1, 0.0, 0.0, 10.0, 10.0, true),
            rect(2, 100.0, 0.0, 10.0, 10.0, true),
        ]);
        pane.selection = vec![ShapeId(1)];
        pane.translate_selection(Vec2::new(5.0, 5.0));
        assert_eq!(pane.scene.shapes[0].bounds().xywh(), [5.0, 5.0, 10.0, 10.0]);
        assert_eq!(
            pane.scene.shapes[1].bounds().xywh(),
            [100.0, 0.0, 10.0, 10.0]
        );
        assert!(pane.dirty);
    }

    #[test]
    fn resize_bottom_right_doubles_from_top_left_anchor() {
        let mut pane = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0, true)]);
        pane.selection = vec![ShapeId(1)];
        let b = pane.selection_bounds().unwrap();
        // Drag the bottom-right handle from (100,100) out to (200,200).
        pane.resize_selection(Handle::BottomRight, b, Vec2::new(200.0, 200.0));
        assert_eq!(
            pane.scene.shapes[0].bounds().xywh(),
            [0.0, 0.0, 200.0, 200.0]
        );
    }

    #[test]
    fn resize_right_edge_only_scales_x() {
        let mut pane = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0, true)]);
        pane.selection = vec![ShapeId(1)];
        let b = pane.selection_bounds().unwrap();
        pane.resize_selection(Handle::Right, b, Vec2::new(150.0, 999.0));
        let [x, y, w, h] = pane.scene.shapes[0].bounds().xywh();
        assert_eq!([x, y, h], [0.0, 0.0, 100.0]);
        assert_eq!(w, 150.0);
    }

    #[test]
    fn duplicate_offsets_and_selects_copy() {
        let mut pane = pane_with(vec![rect(1, 0.0, 0.0, 40.0, 40.0, true)]);
        pane.selection = vec![ShapeId(1)];
        assert!(pane.duplicate_selection());
        assert_eq!(pane.scene.shapes.len(), 2);
        // The new shape is selected and offset from the original.
        assert_eq!(pane.selection.len(), 1);
        let new_id = pane.selection[0];
        assert_ne!(new_id, ShapeId(1));
        let new_bounds = pane
            .scene
            .shapes
            .iter()
            .find(|s| s.id == new_id)
            .unwrap()
            .bounds()
            .xywh();
        assert_eq!(new_bounds, [16.0, 16.0, 40.0, 40.0]);
    }

    #[test]
    fn hit_handle_finds_corner_in_screen_space() {
        let mut pane = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0, true)]);
        pane.selection = vec![ShapeId(1)];
        // Identity camera: world == screen. Bottom-right corner at (100,100).
        let hit = pane.hit_handle(Vec2::new(101.0, 99.0), &Camera::default(), 5.0);
        assert_eq!(hit, Some(Handle::BottomRight));
        assert_eq!(
            pane.hit_handle(Vec2::new(50.0, 50.0), &Camera::default(), 5.0),
            None
        );
    }
}
