//! Pointer + keyboard gesture orchestration for the interactive editor.
//!
//! The desktop/web shell converts raw events into window-logical
//! coordinates and a few intents (press/drag/release, tool keys); these
//! methods turn those into scene edits using the primitives in
//! `edit.rs` / `create.rs`. Coordinate mapping goes through the pane's
//! captured `last_rect` so callers never deal with the camera directly.

use super::edit::Handle;
use super::geometry::Bounds;
use super::pane::{DrawPane, Tool};
use super::scene::{Shape, ShapeId, ShapeKind, Vec2};

/// Click tolerance in screen pixels when hit-testing shapes/handles.
const PICK_TOLERANCE_PX: f32 = 6.0;

/// An in-progress select-tool manipulation.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum DrawGesture {
    #[default]
    Idle,
    /// Panning the viewport; tracks the last window-space position.
    Pan { last: Vec2 },
    /// Rubber-band selection rectangle (world coords).
    Marquee {
        start: Vec2,
        current: Vec2,
        initial: Vec<ShapeId>,
        additive: bool,
        text_click: bool,
    },
    /// Dragging (or about to click) a graph node; tracks the press point
    /// so a release without movement counts as a click (open note).
    GraphNode { idx: usize, start: Vec2 },
    /// Eraser sweep — shapes swept over are dimmed, deleted on release.
    Erase,
    /// Translating the selection; tracks the last world position.
    Move { last: Vec2 },
    /// Resizing via a handle; holds the start bounds and a snapshot of
    /// the affected shapes so each move re-applies an absolute scale.
    Resize {
        handle: Handle,
        start_bounds: Bounds,
        grab_offset: Vec2,
        snapshot: Vec<Shape>,
    },
}

impl DrawPane {
    /// Pointer-down at a window-logical position. Returns whether the
    /// event was consumed (i.e. it landed on this pane).
    pub fn begin_pointer(&mut self, x: f32, y: f32, additive: bool) -> bool {
        let Some(rect) = self.last_rect else {
            return false;
        };
        if !self.window_in_bounds(x, y) {
            return false;
        }

        // Graph view: grab a node, or pan the canvas.
        if self.graph.is_some() {
            // Clicking the filename label opens that note (like a link).
            if let Some(idx) = self.graph_label_at(x, y) {
                let path = self
                    .graph
                    .as_ref()
                    .and_then(|g| g.nodes.get(idx))
                    .map(|n| n.path.clone())
                    .filter(|p| !p.is_empty());
                if path.is_some() {
                    self.graph_open_request = path;
                }
                return true;
            }
            let Some(world) = self.window_to_world(x, y) else {
                return false;
            };
            let hit = self.graph.as_ref().and_then(|g| g.node_at(world));
            if let Some(idx) = hit {
                if let Some(g) = self.graph.as_mut() {
                    g.begin_drag(idx);
                }
                self.gesture = DrawGesture::GraphNode { idx, start: world };
            } else {
                self.gesture = DrawGesture::Pan {
                    last: Vec2::new(x, y),
                };
            }
            return true;
        }
        // Toolbar clicks take precedence over the canvas beneath them —
        // and the whole bar swallows the click, not just the buttons.
        if super::toolbar::point_on_toolbar(rect, x, y) {
            if self.editing() {
                self.commit_text();
            }
            if let Some(item) = super::toolbar::toolbar_hit(rect, x, y) {
                self.apply_toolbar_item(item);
            }
            return true;
        }
        // Hand tool pans the viewport (window-space drag).
        if self.tool == Tool::Hand {
            self.gesture = DrawGesture::Pan {
                last: Vec2::new(x, y),
            };
            return true;
        }

        let Some(world) = self.window_to_world(x, y) else {
            return false;
        };
        let tol = PICK_TOLERANCE_PX / self.camera.zoom.max(0.01);

        if self.tool == Tool::Eraser {
            self.erasing.clear();
            self.mark_erase_at(world, tol);
            self.gesture = DrawGesture::Erase;
            return true;
        }

        if matches!(self.tool, Tool::Select | Tool::Text) && self.begin_resize_at(x, y) {
            return true;
        }
        let text_click = self.tool == Tool::Text;
        // Any other click ends an in-progress text edit.
        if self.editing() {
            self.commit_text();
        }

        if self.is_creation_tool() {
            self.begin_draft(world);
            return true;
        }

        let hit = self.pick_top(world, tol);
        let initial = self.selection.clone();
        self.select_at(world, tol, additive);
        if hit.is_some_and(|id| self.selection.contains(&id)) {
            self.checkpoint();
            self.gesture = DrawGesture::Move { last: world };
        } else if hit.is_some() {
            self.gesture = DrawGesture::Idle;
        } else {
            // Empty space always selects, even between already-selected objects.
            self.gesture = DrawGesture::Marquee {
                start: world,
                current: world,
                initial,
                additive,
                text_click,
            };
        }
        true
    }

    fn begin_resize_at(&mut self, x: f32, y: f32) -> bool {
        let Some(rect) = self.last_rect else {
            return false;
        };
        let Some(start_bounds) = self.selection_bounds() else {
            return false;
        };
        let camera = self.placed_camera(rect);
        let Some(handle) = self.hit_handle(
            Vec2::new(x, y),
            &camera,
            super::render::HANDLE_HALF_PX + 3.0,
        ) else {
            return false;
        };
        let snapshot = self
            .scene
            .shapes
            .iter()
            .filter(|shape| self.selection.contains(&shape.id))
            .map(|shape| {
                let mut snapshot = shape.clone();
                if let ShapeKind::Text { width, .. } = &mut snapshot.kind {
                    if width.is_none() {
                        *width = Some(self.shape_bounds(shape).width());
                    }
                }
                snapshot
            })
            .collect();
        self.checkpoint();
        let pointer = camera.screen_to_world(Vec2::new(x, y));
        let anchor = handle.world_pos(start_bounds);
        self.gesture = DrawGesture::Resize {
            handle,
            start_bounds,
            grab_offset: Vec2::new(pointer.x - anchor.x, pointer.y - anchor.y),
            snapshot,
        };
        true
    }

    /// Pointer drag to a window-logical position. Returns whether the
    /// scene changed (so the caller can request a redraw).
    pub fn drag_pointer(&mut self, x: f32, y: f32) -> bool {
        // Panning works in window space, before any world mapping.
        if let DrawGesture::Pan { last } = self.gesture {
            self.pan_by(x - last.x, y - last.y);
            self.gesture = DrawGesture::Pan {
                last: Vec2::new(x, y),
            };
            return true;
        }

        let Some(world) = self.window_to_world(x, y) else {
            return false;
        };

        if self.draft.is_some() {
            self.update_draft(world);
            return true;
        }

        match self.gesture.clone() {
            DrawGesture::Pan { .. } => false,
            DrawGesture::GraphNode { .. } => {
                if let Some(g) = self.graph.as_mut() {
                    g.drag_to(world);
                }
                true
            }
            DrawGesture::Erase => {
                let tol = PICK_TOLERANCE_PX / self.camera.zoom.max(0.01);
                self.mark_erase_at(world, tol);
                true
            }
            DrawGesture::Marquee {
                start,
                initial,
                additive,
                text_click,
                ..
            } => {
                let selecting = (world.x - start.x).hypot(world.y - start.y)
                    * self.camera.zoom
                    >= 3.0;
                if selecting {
                    if text_click {
                        self.tool = Tool::Select;
                    }
                    self.select_in_rect(start, world);
                    if additive {
                        for id in &initial {
                            if !self.selection.contains(id) {
                                self.selection.push(*id);
                            }
                        }
                    }
                } else {
                    self.selection = if additive {
                        initial.clone()
                    } else {
                        Vec::new()
                    };
                }
                self.gesture = DrawGesture::Marquee {
                    start,
                    current: world,
                    initial,
                    additive,
                    text_click: text_click && !selecting,
                };
                true
            }
            DrawGesture::Move { last } => {
                self.translate_selection(Vec2::new(world.x - last.x, world.y - last.y));
                self.gesture = DrawGesture::Move { last: world };
                true
            }
            DrawGesture::Resize {
                handle,
                start_bounds,
                grab_offset,
                snapshot,
            } => {
                self.restore_shapes(&snapshot);
                self.resize_selection(
                    handle,
                    start_bounds,
                    Vec2::new(world.x - grab_offset.x, world.y - grab_offset.y),
                );
                true
            }
            DrawGesture::Idle => false,
        }
    }

    /// Pointer-up. Commits a draft or ends a move/resize. Returns
    /// whether anything was finalized.
    pub fn end_pointer(&mut self) -> bool {
        if self.draft.is_some() {
            self.commit_draft();
            return true;
        }
        if let DrawGesture::Marquee {
            start,
            current,
            initial,
            additive,
            text_click,
        } = self.gesture.clone()
        {
            self.gesture = DrawGesture::Idle;
            let dragged = (current.x - start.x).hypot(current.y - start.y)
                * self.camera.zoom
                >= 3.0;
            if dragged {
                self.select_in_rect(start, current);
                if additive {
                    for id in initial {
                        if !self.selection.contains(&id) {
                            self.selection.push(id);
                        }
                    }
                }
                if text_click {
                    self.tool = Tool::Select;
                }
            } else if text_click && !additive {
                self.begin_text_at(start, PICK_TOLERANCE_PX / self.camera.zoom.max(0.01));
            } else {
                self.selection = if additive { initial } else { Vec::new() };
            }
            return true;
        }
        if matches!(self.gesture, DrawGesture::Erase) {
            self.commit_erase();
            self.gesture = DrawGesture::Idle;
            return true;
        }
        if let DrawGesture::GraphNode { idx, start } = self.gesture {
            // A release without real movement is a click → open the note.
            let moved = self
                .graph
                .as_ref()
                .and_then(|g| g.nodes.get(idx))
                .map(|n| {
                    let dx = n.pos.x - start.x;
                    let dy = n.pos.y - start.y;
                    dx * dx + dy * dy > 25.0
                })
                .unwrap_or(false);
            if let Some(g) = self.graph.as_mut() {
                g.end_drag();
            }
            if !moved {
                let path = self
                    .graph
                    .as_ref()
                    .and_then(|g| g.nodes.get(idx))
                    .map(|n| n.path.clone())
                    .filter(|p| !p.is_empty());
                if path.is_some() {
                    self.graph_open_request = path;
                }
            }
            self.gesture = DrawGesture::Idle;
            return true;
        }
        let active = !matches!(self.gesture, DrawGesture::Idle);
        self.gesture = DrawGesture::Idle;
        active
    }

    /// Add any shapes under `world` to the pending-erase set.
    fn mark_erase_at(&mut self, world: Vec2, tol: f32) {
        for s in &self.scene.shapes {
            // Don't erase text — it's the note's body in "Draw on Note"
            // (delete text via select + Delete instead).
            if matches!(s.kind, ShapeKind::Text { .. }) {
                continue;
            }
            if s.hit_select(world, tol) {
                self.erasing.insert(s.id);
            }
        }
    }

    /// Delete the swept shapes (one undo step for the whole stroke).
    fn commit_erase(&mut self) {
        if self.erasing.is_empty() {
            return;
        }
        self.checkpoint();
        let gone = std::mem::take(&mut self.erasing);
        self.scene.shapes.retain(|s| !gone.contains(&s.id));
        self.selection.retain(|id| !gone.contains(id));
        self.dirty = true;
    }

    /// Node whose filename label (screen-space) contains `(x, y)`.
    pub fn graph_label_at(&self, x: f32, y: f32) -> Option<usize> {
        self.graph_label_rects
            .iter()
            .find(|(r, _)| x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3])
            .map(|(_, i)| *i)
    }

    /// Node under the cursor — label first, then the disc.
    fn graph_pick(&self, x: f32, y: f32) -> Option<usize> {
        if let Some(i) = self.graph_label_at(x, y) {
            return Some(i);
        }
        let world = self.window_to_world(x, y)?;
        self.graph.as_ref()?.node_at(world)
    }

    /// Update the hovered graph node from a window-space position.
    /// Returns whether the hover changed (so the host can redraw).
    pub fn set_graph_hover(&mut self, x: f32, y: f32) -> bool {
        if self.graph.is_none() {
            return false;
        }
        let next = if self.window_in_bounds(x, y) {
            self.graph_pick(x, y)
        } else {
            None
        };
        if next != self.graph_hover {
            self.graph_hover = next;
            true
        } else {
            false
        }
    }

    /// Whether a pointer gesture (draft or move/resize) is in progress.
    pub fn pointer_active(&self) -> bool {
        self.draft.is_some() || !matches!(self.gesture, DrawGesture::Idle)
    }

    /// Restore the given shapes' geometry/style by id (used to re-base a
    /// resize each move so the scale doesn't compound).
    fn restore_shapes(&mut self, snapshot: &[Shape]) {
        for snap in snapshot {
            if let Some(existing) = self.scene.shapes.iter_mut().find(|s| s.id == snap.id)
            {
                *existing = snap.clone();
            }
        }
    }

    /// Switch the active tool, abandoning any in-progress gesture.
    pub fn set_tool(&mut self, tool: Tool) {
        self.cancel_draft();
        if self.editing() {
            self.commit_text();
        }
        self.erasing.clear();
        self.gesture = DrawGesture::Idle;
        self.tool = tool;
    }

    /// Cancel the current gesture / clear selection (Escape).
    pub fn cancel(&mut self) -> bool {
        if self.editing() {
            self.commit_text();
            return true;
        }
        if self.draft.is_some() {
            self.cancel_draft();
            return true;
        }
        if let DrawGesture::Marquee { initial, .. } = &self.gesture {
            self.selection = initial.clone();
            self.gesture = DrawGesture::Idle;
            return true;
        }
        if !matches!(self.gesture, DrawGesture::Idle) {
            self.gesture = DrawGesture::Idle;
            self.erasing.clear();
            return true;
        }
        if self.has_selection() {
            self.clear_selection();
            return true;
        }
        false
    }

    /// Delete the selected shapes. Returns whether anything was removed.
    pub fn delete_selection(&mut self) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.checkpoint();
        let before = self.scene.shapes.len();
        self.scene
            .shapes
            .retain(|s| !self.selection.contains(&s.id));
        self.selection.clear();
        let removed = self.scene.shapes.len() != before;
        self.dirty |= removed;
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::neodraw::scene::{Color, Scene, ShapeId, ShapeKind, Style};
    use std::path::PathBuf;

    fn rect(id: u64, x: f32, y: f32, w: f32, h: f32) -> Shape {
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
                fill: Some(Color::WHITE),
                ..Style::default()
            },
        }
    }

    /// Pane with an identity placed-camera: last_rect at origin, zoom 1,
    /// so window coords equal world coords.
    fn pane_with(shapes: Vec<Shape>) -> DrawPane {
        let mut p = DrawPane::new(PathBuf::from("t.neodraw"));
        p.scene = Scene { version: 1, shapes };
        p.last_rect = Some([0.0, 0.0, 2000.0, 2000.0]);
        p
    }

    #[test]
    fn click_then_drag_moves_shape() {
        let mut p = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0)]);
        assert!(p.begin_pointer(50.0, 50.0, false));
        assert_eq!(p.selection, vec![ShapeId(1)]);
        assert!(p.drag_pointer(70.0, 60.0));
        assert!(p.end_pointer());
        assert_eq!(
            p.scene.shapes[0].bounds().xywh(),
            [20.0, 10.0, 100.0, 100.0]
        );
    }

    #[test]
    fn drag_on_empty_canvas_marquee_selects() {
        let mut p = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0)]);
        // Press in empty space, drag a box that encloses the shape.
        p.begin_pointer(500.0, 500.0, false);
        assert!(p.selection.is_empty());
        assert!(p.drag_pointer(-10.0, -10.0)); // marquee updating
                                               // The shape must not have moved.
        assert_eq!(p.scene.shapes[0].bounds().xywh(), [0.0, 0.0, 100.0, 100.0]);
        p.end_pointer();
        assert_eq!(
            p.selection,
            vec![ShapeId(1)],
            "marquee selects enclosed shape"
        );
    }

    #[test]
    fn empty_space_inside_selection_starts_a_new_marquee() {
        let mut p = pane_with(vec![
            rect(1, 0.0, 0.0, 50.0, 50.0),
            rect(2, 200.0, 0.0, 50.0, 50.0),
        ]);
        p.selection = vec![ShapeId(1), ShapeId(2)];
        // Click empty space between the two shapes but inside their box.
        assert!(p.begin_pointer(120.0, 25.0, false));
        assert!(matches!(p.gesture, DrawGesture::Marquee { .. }));
        p.drag_pointer(260.0, 100.0);
        assert_eq!(p.selection, vec![ShapeId(2)]);
        p.end_pointer();
        assert_eq!(p.scene.shapes[0].bounds().xywh()[0], 0.0);
        assert_eq!(p.scene.shapes[1].bounds().xywh()[0], 200.0);
    }

    #[test]
    fn shift_marquee_keeps_original_selection_and_updates_live() {
        let mut p = pane_with(vec![
            rect(1, 0.0, 0.0, 50.0, 50.0),
            rect(2, 200.0, 0.0, 50.0, 50.0),
        ]);
        p.selection = vec![ShapeId(1)];
        p.begin_pointer(300.0, 200.0, true);
        p.drag_pointer(150.0, -10.0);
        assert!(p.selection.contains(&ShapeId(1)) && p.selection.contains(&ShapeId(2)));
        p.drag_pointer(280.0, 180.0);
        assert_eq!(p.selection, vec![ShapeId(1)]);
        p.cancel();
        assert_eq!(p.selection, vec![ShapeId(1)]);
    }

    #[test]
    fn text_tool_drag_selects_without_creating_a_stray_text_shape() {
        let mut p = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0)]);
        p.tool = Tool::Text;
        p.begin_pointer(500.0, 500.0, false);
        p.drag_pointer(-10.0, -10.0);
        p.end_pointer();
        assert_eq!(p.scene.shapes.len(), 1);
        assert_eq!(p.selection, vec![ShapeId(1)]);
        assert_eq!(p.tool, Tool::Select);
        p.tool = Tool::Text;
        p.begin_pointer(500.0, 500.0, false);
        p.end_pointer();
        assert!(p.editing());
        assert_eq!(p.scene.shapes.len(), 2);
    }

    #[test]
    fn marquee_misses_shape_outside_box() {
        let mut p = pane_with(vec![rect(1, 0.0, 0.0, 50.0, 50.0)]);
        p.begin_pointer(500.0, 500.0, false);
        p.drag_pointer(400.0, 400.0); // box far from the shape
        p.end_pointer();
        assert!(p.selection.is_empty());
    }

    #[test]
    fn resize_via_handle_does_not_compound() {
        let mut p = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0)]);
        p.selection = vec![ShapeId(1)];
        // Press on the bottom-right handle (world == window here).
        assert!(p.begin_pointer(100.0, 100.0, false));
        assert!(matches!(p.gesture, DrawGesture::Resize { .. }));
        // Two drags; result must reflect the final pointer, not the sum.
        p.drag_pointer(150.0, 150.0);
        p.drag_pointer(200.0, 200.0);
        p.end_pointer();
        assert_eq!(p.scene.shapes[0].bounds().xywh(), [0.0, 0.0, 200.0, 200.0]);
    }

    #[test]
    fn creation_tool_drag_creates_shape() {
        let mut p = pane_with(vec![]);
        p.tool = Tool::Rect;
        p.begin_pointer(10.0, 10.0, false);
        p.drag_pointer(60.0, 50.0);
        p.end_pointer();
        assert_eq!(p.scene.shapes.len(), 1);
        assert_eq!(p.scene.shapes[0].bounds().xywh(), [10.0, 10.0, 50.0, 40.0]);
        assert_eq!(p.tool, Tool::Rect, "stays on the tool for repeat drawing");
    }

    #[test]
    fn eraser_dims_then_deletes_swept_shapes() {
        let mut p = pane_with(vec![
            rect(1, 0.0, 0.0, 50.0, 50.0),
            rect(2, 200.0, 0.0, 50.0, 50.0),
        ]);
        p.tool = Tool::Eraser;
        p.begin_pointer(25.0, 25.0, false); // over shape 1
        assert!(p.erasing.contains(&ShapeId(1)));
        assert_eq!(p.scene.shapes.len(), 2, "deletion deferred to release");
        p.drag_pointer(225.0, 25.0); // sweep over shape 2
        assert!(p.erasing.contains(&ShapeId(2)));
        p.end_pointer();
        assert!(p.scene.shapes.is_empty(), "swept shapes removed on release");
        assert!(p.erasing.is_empty());
    }

    #[test]
    fn delete_and_cancel() {
        let mut p = pane_with(vec![rect(1, 0.0, 0.0, 10.0, 10.0)]);
        p.selection = vec![ShapeId(1)];
        assert!(p.cancel()); // clears selection
        assert!(p.selection.is_empty());
        p.selection = vec![ShapeId(1)];
        assert!(p.delete_selection());
        assert!(p.scene.shapes.is_empty());
    }

    #[test]
    fn pointer_outside_pane_is_not_consumed() {
        let mut p = pane_with(vec![rect(1, 0.0, 0.0, 100.0, 100.0)]);
        p.last_rect = Some([100.0, 100.0, 200.0, 200.0]);
        assert!(!p.begin_pointer(10.0, 10.0, false));
    }
}
