//! `DrawPane` — the editable state behind a `.neodraw` tab.
//!
//! Pure model + viewport state, mirroring [`MarkdownPane`] so the same
//! pane drives both the native winit shell and the web wasm shell.
//! Rendering (sugarloaf) and input handling land in later phases; this
//! is the document + camera + tool skeleton they hang off of.
//!
//! [`MarkdownPane`]: crate::editor::markdown::MarkdownPane

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::geometry::Bounds;
use super::scene::{Scene, Shape, ShapeId, ShapeKind, Style, Vec2};

/// Maps world space (logical pixels in the document) to screen space.
///
/// `screen = world * zoom + pan`, so `pan` is the screen-space
/// position of the world origin and `zoom` is a uniform scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub pan: Vec2,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera {
    pub fn world_to_screen(&self, p: Vec2) -> Vec2 {
        Vec2::new(p.x * self.zoom + self.pan.x, p.y * self.zoom + self.pan.y)
    }

    pub fn screen_to_world(&self, p: Vec2) -> Vec2 {
        Vec2::new(
            (p.x - self.pan.x) / self.zoom,
            (p.y - self.pan.y) / self.zoom,
        )
    }
}

/// The active editing tool. `Select` is the resting state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Hand,
    Pen,
    Highlighter,
    Eraser,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Text,
}

/// Editable state behind one `.neodraw` tab.
#[derive(Debug, Clone)]
pub struct DrawPane {
    pub path: PathBuf,
    pub title: String,
    pub scene: Scene,
    pub camera: Camera,
    pub tool: Tool,
    /// Currently selected shapes (paint/handle order follows scene order).
    pub selection: Vec<ShapeId>,
    /// Style applied to newly created shapes (driven by the palette).
    pub style_defaults: Style,
    /// In-progress shape-creation gesture, if any (see `create.rs`).
    pub draft: Option<super::create::Draft>,
    /// In-progress select-tool manipulation (move/resize). See `input.rs`.
    pub gesture: super::input::DrawGesture,
    /// The pane's window-logical rect captured at the last `render_pane`,
    /// used to map pointer events into world space.
    pub last_rect: Option<[f32; 4]>,
    /// The text shape currently being typed into, if any (Text tool).
    pub editing_text: Option<super::scene::ShapeId>,
    /// When set, the next render fits the scene to the pane (used on
    /// open so a drawing authored at any scale lands fully in view).
    pub fit_pending: bool,
    /// Undo/redo history of scene snapshots (see `history.rs`).
    pub history: super::history::History,
    /// Shapes captured by copy, pasted with fresh ids.
    pub clipboard: Vec<super::scene::Shape>,
    /// Real measured world-space `(width, height)` of each text shape,
    /// filled by the renderer. Used for accurate selection frames /
    /// hit-testing in place of the rough estimate in `geometry.rs`.
    pub text_dims: HashMap<ShapeId, Vec2>,
    pub(super) text_layouts: HashMap<ShapeId, super::text_layout::TextLayout>,
    /// Shapes the eraser drag has swept over (drawn translucent, deleted
    /// on release).
    pub erasing: std::collections::HashSet<ShapeId>,
    /// True after Space is pressed (not editing) — arms the `Space x`
    /// leader chord to close the tab.
    pub space_armed: bool,
    /// When set, this pane shows a live animated note-graph instead of a
    /// static scene (the Obsidian-style view). See `graph_sim.rs`.
    pub graph: Option<super::graph_sim::GraphSim>,
    /// Set when a graph node is clicked (not dragged); the host reads it
    /// to open that note, then clears it.
    pub graph_open_request: Option<String>,
    /// Centre the graph in the pane on the next render.
    pub graph_needs_center: bool,
    /// Node index currently hovered (disc or label) — drawn highlighted /
    /// the label underlined + blue, like a link.
    pub graph_hover: Option<usize>,
    /// Screen-space label rects captured at render time `([x,y,w,h], node)`
    /// so pointer/hover code can hit-test the filename text.
    pub graph_label_rects: Vec<([f32; 4], usize)>,
    /// Unsaved edits since the last load/save.
    pub dirty: bool,
    /// Parse error from the last `set_source`, surfaced in the UI.
    pub error: Option<String>,
}

impl DrawPane {
    /// An empty pane for a (possibly not-yet-existing) path.
    pub fn new(path: PathBuf) -> Self {
        let title = title_from_path(&path);
        Self {
            path,
            title,
            scene: Scene::empty(),
            camera: Camera::default(),
            tool: Tool::default(),
            selection: Vec::new(),
            style_defaults: Style::default(),
            draft: None,
            gesture: super::input::DrawGesture::Idle,
            last_rect: None,
            editing_text: None,
            fit_pending: false,
            history: super::history::History::default(),
            clipboard: Vec::new(),
            text_dims: HashMap::new(),
            text_layouts: HashMap::new(),
            erasing: std::collections::HashSet::new(),
            space_armed: false,
            graph: None,
            graph_open_request: None,
            graph_needs_center: false,
            graph_hover: None,
            graph_label_rects: Vec::new(),
            dirty: false,
            error: None,
        }
    }

    /// Build a pane from a `.neodraw` file's JSON. A parse failure
    /// yields an empty scene with `error` set rather than panicking.
    pub fn from_source(path: PathBuf, json: &str) -> Self {
        let mut pane = Self::new(path);
        pane.set_source(json);
        pane
    }

    /// Replace the document from JSON, preserving the camera/tool so a
    /// live external edit doesn't yank the viewport around.
    pub fn set_source(&mut self, json: &str) {
        match Scene::from_json(json) {
            Ok(scene) => {
                self.scene = scene;
                self.text_dims.clear();
                self.text_layouts.clear();
                self.error = None;
                self.selection
                    .retain(|id| self.scene.shapes.iter().any(|s| s.id == *id));
            }
            Err(err) => {
                self.error = Some(err);
            }
        }
        self.dirty = false;
        // Fit the (possibly large) scene into view on the next render.
        self.fit_pending = true;
    }

    /// The on-disk JSON for the current scene.
    pub fn to_source(&self) -> String {
        self.scene.to_json()
    }

    /// Open a `.neodraw` file from disk. A missing file yields an empty
    /// scene (a fresh canvas); a malformed file keeps `error` set.
    pub fn load(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(json) => Self::from_source(path, &json),
            Err(_) => Self::new(path),
        }
    }

    /// Persist the current scene to its path as pretty JSON.
    pub fn save(&mut self) -> std::io::Result<()> {
        match std::fs::write(&self.path, self.to_source()) {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
                Ok(())
            }
            Err(err) => {
                self.error = Some(err.to_string());
                Err(err)
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Bounds for a shape, preferring the renderer-measured text size so
    /// selection frames hug the real glyphs (no trailing slack, no
    /// clipped last letter).
    pub fn shape_bounds(&self, shape: &Shape) -> Bounds {
        if let ShapeKind::Text { x, y, .. } = &shape.kind {
            if let Some(dim) = self.text_dims.get(&shape.id) {
                return Bounds::new(Vec2::new(*x, *y), Vec2::new(x + dim.x, y + dim.y));
            }
        }
        shape.bounds()
    }

    /// The document camera shifted so world-origin lands at the pane's
    /// top-left — i.e. the transform actually used to paint into `rect`.
    pub fn placed_camera(&self, rect: [f32; 4]) -> Camera {
        Camera {
            pan: Vec2::new(self.camera.pan.x + rect[0], self.camera.pan.y + rect[1]),
            zoom: self.camera.zoom,
        }
    }

    /// Map a window-logical pointer position to world space using the
    /// last rendered rect. `None` if the pane hasn't rendered yet.
    pub fn window_to_world(&self, x: f32, y: f32) -> Option<Vec2> {
        let rect = self.last_rect?;
        Some(self.placed_camera(rect).screen_to_world(Vec2::new(x, y)))
    }

    /// Whether a window-logical point falls within the pane's rect.
    pub fn window_in_bounds(&self, x: f32, y: f32) -> bool {
        match self.last_rect {
            Some(r) => x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3],
            None => false,
        }
    }

    /// Pan the viewport by a window-space delta (Hand tool / wheel).
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.camera.pan.x += dx;
        self.camera.pan.y += dy;
    }

    /// Zoom about a window-logical point, keeping the world point under
    /// the cursor fixed. Clamped to a sane range.
    pub fn zoom_at(&mut self, x: f32, y: f32, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 || !x.is_finite() || !y.is_finite() {
            return;
        }
        let Some(rect) = self.last_rect else {
            return;
        };
        let world = self.placed_camera(rect).screen_to_world(Vec2::new(x, y));
        let new_zoom = (self.camera.zoom * factor).clamp(0.05, 8.0);
        self.camera.zoom = new_zoom;
        // Re-solve pan so `world` still maps under (x, y).
        self.camera.pan.x = x - rect[0] - world.x * new_zoom;
        self.camera.pan.y = y - rect[1] - world.y * new_zoom;
    }

    /// Center and scale the scene to fit within `rect` (window-logical).
    pub fn fit_to_view(&mut self, rect: [f32; 4]) {
        let Some(b) = self
            .scene
            .shapes
            .iter()
            .map(|shape| self.shape_bounds(shape))
            .reduce(Bounds::union)
        else {
            self.camera = Camera::default();
            return;
        };
        let pad = 48.0;
        let avail_w = (rect[2] - 2.0 * pad).max(1.0);
        let avail_h = (rect[3] - 2.0 * pad).max(1.0);
        let fit_w = avail_w / b.width().max(1.0);
        let fit_h = avail_h / b.height().max(1.0);
        let c = b.center();
        if b.height() > b.width() * 1.5 {
            // Tall documents (note "page views"): fit to WIDTH and anchor to
            // the top so they read like a scrollable page, not zoomed out.
            let zoom = fit_w.clamp(0.05, 1.5);
            self.camera.zoom = zoom;
            self.camera.pan.x = rect[2] * 0.5 - c.x * zoom;
            self.camera.pan.y = pad - (c.y - b.height() * 0.5) * zoom;
        } else {
            let zoom = fit_w.min(fit_h).clamp(0.05, 1.5);
            self.camera.zoom = zoom;
            self.camera.pan.x = rect[2] * 0.5 - c.x * zoom;
            self.camera.pan.y = rect[3] * 0.5 - c.y * zoom;
        }
    }
}

/// Whether `path` names a `.neodraw` document.
pub fn is_neodraw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("neodraw"))
        .unwrap_or(false)
}

/// Derive a tab title from a path: file stem, or a generic fallback.
fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Untitled".to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn zoom_keeps_the_pointer_anchor_and_supports_the_fit_minimum() {
        let mut pane = super::DrawPane::new(std::path::PathBuf::from("zoom.neodraw"));
        pane.last_rect = Some([80.0, 50.0, 800.0, 600.0]);
        pane.camera.zoom = 0.05;
        let before = pane.window_to_world(320.0, 240.0).unwrap();
        pane.zoom_at(320.0, 240.0, 1.2);
        assert!((pane.camera.zoom - 0.06).abs() < 0.0001);
        let after = pane.window_to_world(320.0, 240.0).unwrap();
        assert!((before.x - after.x).abs() < 0.001);
        assert!((before.y - after.y).abs() < 0.001);
        let camera = pane.camera;
        pane.zoom_at(320.0, 240.0, f32::NAN);
        assert_eq!(pane.camera, camera);
    }

    use super::*;

    #[test]
    fn camera_round_trips_a_point() {
        let cam = Camera {
            pan: Vec2::new(120.0, -40.0),
            zoom: 2.0,
        };
        let world = Vec2::new(10.0, 15.0);
        let screen = cam.world_to_screen(world);
        let back = cam.screen_to_world(screen);
        assert!((back.x - world.x).abs() < 1e-4);
        assert!((back.y - world.y).abs() < 1e-4);
    }

    #[test]
    fn title_derives_from_stem() {
        let pane = DrawPane::new(PathBuf::from("/notes/handoff.neodraw"));
        assert_eq!(pane.title, "handoff");
    }

    #[test]
    fn bad_json_sets_error_keeps_empty_scene() {
        let mut pane = DrawPane::new(PathBuf::from("x.neodraw"));
        pane.set_source("{ not json");
        assert!(pane.error.is_some());
        assert!(pane.scene.shapes.is_empty());
    }

    #[test]
    fn fit_centers_scene_in_rect() {
        use super::super::scene::{Scene, Shape, ShapeId, ShapeKind, Style};
        let mut pane = DrawPane::new(PathBuf::from("t.neodraw"));
        pane.scene = Scene {
            version: 1,
            shapes: vec![Shape {
                id: ShapeId(1),
                kind: ShapeKind::Rect {
                    x: 1000.0,
                    y: 1000.0,
                    w: 200.0,
                    h: 100.0,
                    corner: 0.0,
                },
                style: Style::default(),
            }],
        };
        let rect = [0.0, 0.0, 800.0, 600.0];
        pane.fit_to_view(rect);
        // Scene center should map to the rect center.
        let cam = pane.placed_camera(rect);
        let world_c = Vec2::new(1100.0, 1050.0);
        let screen_c = cam.world_to_screen(world_c);
        assert!((screen_c.x - 400.0).abs() < 0.5);
        assert!((screen_c.y - 300.0).abs() < 0.5);
    }

    #[test]
    fn zoom_at_keeps_cursor_point_fixed() {
        use super::super::scene::{Scene, Shape, ShapeId, ShapeKind, Style};
        let mut pane = DrawPane::new(PathBuf::from("t.neodraw"));
        pane.scene = Scene {
            version: 1,
            shapes: vec![Shape {
                id: ShapeId(1),
                kind: ShapeKind::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 50.0,
                    h: 50.0,
                    corner: 0.0,
                },
                style: Style::default(),
            }],
        };
        let rect = [10.0, 20.0, 800.0, 600.0];
        pane.last_rect = Some(rect);
        let (cx, cy) = (300.0, 250.0);
        let before = pane.placed_camera(rect).screen_to_world(Vec2::new(cx, cy));
        pane.zoom_at(cx, cy, 1.5);
        let after = pane.placed_camera(rect).screen_to_world(Vec2::new(cx, cy));
        assert!((before.x - after.x).abs() < 0.01);
        assert!((before.y - after.y).abs() < 0.01);
        assert!((pane.camera.zoom - 1.5).abs() < 0.001);
    }

    #[test]
    fn from_source_loads_scene() {
        let json = r#"{ "version": 1, "shapes": [
            { "id": 1, "type": "rect", "x": 0, "y": 0, "w": 10, "h": 10 }
        ] }"#;
        let pane = DrawPane::from_source(PathBuf::from("a.neodraw"), json);
        assert!(pane.error.is_none());
        assert_eq!(pane.scene.shapes.len(), 1);
        assert!(!pane.dirty);
    }
}
