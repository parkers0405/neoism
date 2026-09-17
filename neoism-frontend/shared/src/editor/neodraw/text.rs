//! Text tool: place a text shape and type into it.
//!
//! Clicking with the Text tool either starts editing an existing text
//! shape under the cursor or creates a fresh one at the click. While
//! `editing_text` is set, printable keys append to that shape and the
//! caller routes them here instead of to tool shortcuts.

use super::pane::{DrawPane, Tool};
use super::scene::{Shape, ShapeId, ShapeKind, Vec2};
use unicode_segmentation::UnicodeSegmentation;

/// Default font size (world units) for a newly placed text shape.
const DEFAULT_TEXT_SIZE: f32 = 28.0;

impl DrawPane {
    pub fn editing(&self) -> bool {
        self.editing_text.is_some()
    }

    /// Scale the font size of the text being edited (or all selected text
    /// shapes) by `factor` — Ctrl +/-. Returns whether anything changed.
    pub fn change_text_size(&mut self, factor: f32) -> bool {
        if !factor.is_finite() || factor <= 0.0 {
            return false;
        }
        let targets: Vec<ShapeId> = if let Some(id) = self.editing_text {
            vec![id]
        } else {
            self.selection.clone()
        };
        if targets.is_empty() {
            return false;
        }
        self.checkpoint();
        let mut changed = false;
        for id in &targets {
            if let Some(shape) = self.scene.shapes.iter_mut().find(|s| s.id == *id) {
                if let ShapeKind::Text { size, .. } = &mut shape.kind {
                    let next = (*size * factor).clamp(6.0, 4000.0);
                    if (next - *size).abs() > f32::EPSILON {
                        *size = next;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// Double-click anywhere: edit the text under the cursor, or drop a
    /// new text box there. Works regardless of the active tool. Returns
    /// whether the click landed on the pane.
    pub fn double_click(&mut self, x: f32, y: f32) -> bool {
        if !self.window_in_bounds(x, y) {
            return false;
        }
        let Some(world) = self.window_to_world(x, y) else {
            return false;
        };
        // Generous tolerance so a near-miss still edits existing text.
        let tol = (8.0 / self.camera.zoom.max(0.01)).max(4.0);
        self.set_tool(Tool::Text);
        self.begin_text_at(world, tol);
        true
    }

    /// Begin text entry at `world`: edit an existing text shape there,
    /// or create a new empty one. Records an undo checkpoint.
    pub fn begin_text_at(&mut self, world: Vec2, hit_tol: f32) {
        if let Some(id) = self.text_shape_at(world, hit_tol) {
            if self.editing_text != Some(id) {
                self.checkpoint();
            }
            self.editing_text = Some(id);
            self.selection = vec![id];
            return;
        }
        self.checkpoint();
        let id = self.scene.next_id();
        let mut style = self.style_defaults.clone();
        style.seed = (id.0 as u32).wrapping_mul(2_654_435_761);
        self.scene.shapes.push(Shape {
            id,
            kind: ShapeKind::Text {
                x: world.x,
                y: world.y,
                content: String::new(),
                size: DEFAULT_TEXT_SIZE,
                width: None,
            },
            style,
        });
        self.selection = vec![id];
        self.editing_text = Some(id);
        self.dirty = true;
    }

    /// Append a string (typically one typed character) to the text shape
    /// being edited.
    pub fn insert_text(&mut self, s: &str) -> bool {
        let Some(id) = self.editing_text else {
            return false;
        };
        if let Some(content) = self.text_content_mut(id) {
            content.push_str(s);
            self.dirty = true;
            return true;
        }
        false
    }

    /// Delete the last character (or newline) of the edited text.
    pub fn text_backspace(&mut self) -> bool {
        let Some(id) = self.editing_text else {
            return false;
        };
        if let Some(content) = self.text_content_mut(id) {
            if let Some((start, _)) = content.grapheme_indices(true).next_back() {
                content.truncate(start);
                self.dirty = true;
                return true;
            }
        }
        false
    }

    pub fn text_newline(&mut self) -> bool {
        self.insert_text("\n")
    }

    /// Finish editing. An empty text shape is removed so a stray click
    /// doesn't leave an invisible artifact.
    pub fn commit_text(&mut self) -> bool {
        let Some(id) = self.editing_text.take() else {
            return false;
        };
        let empty = self
            .scene
            .shapes
            .iter()
            .find(|s| s.id == id)
            .map(|s| match &s.kind {
                ShapeKind::Text { content, .. } => content.trim().is_empty(),
                _ => false,
            })
            .unwrap_or(false);
        if empty {
            self.scene.shapes.retain(|s| s.id != id);
            self.selection.retain(|s| *s != id);
        }
        true
    }

    /// Topmost text shape hit by `world`, if any.
    fn text_shape_at(&self, world: Vec2, tol: f32) -> Option<ShapeId> {
        self.scene
            .shapes
            .iter()
            .rev()
            .find(|s| {
                matches!(s.kind, ShapeKind::Text { .. })
                    && self.shape_bounds(s).contains(world, tol)
            })
            .map(|s| s.id)
    }

    fn text_content_mut(&mut self, id: ShapeId) -> Option<&mut String> {
        self.scene
            .shapes
            .iter_mut()
            .find(|s| s.id == id)
            .and_then(|s| match &mut s.kind {
                ShapeKind::Text { content, .. } => Some(content),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::neodraw::Tool;
    use std::path::PathBuf;

    fn pane() -> DrawPane {
        let mut p = DrawPane::new(PathBuf::from("t.neodraw"));
        p.last_rect = Some([0.0, 0.0, 1000.0, 1000.0]);
        p.tool = Tool::Text;
        p
    }

    #[test]
    fn place_type_and_commit() {
        let mut p = pane();
        p.begin_text_at(Vec2::new(40.0, 40.0), 4.0);
        assert!(p.editing());
        p.insert_text("H");
        p.insert_text("i");
        let id = p.editing_text.unwrap();
        match &p.scene.shapes.iter().find(|s| s.id == id).unwrap().kind {
            ShapeKind::Text { content, .. } => assert_eq!(content, "Hi"),
            _ => panic!(),
        }
        p.text_backspace();
        assert!(p.commit_text());
        assert!(!p.editing());
        match &p.scene.shapes[0].kind {
            ShapeKind::Text { content, .. } => assert_eq!(content, "H"),
            _ => panic!(),
        }
    }

    #[test]
    fn ctrl_plus_minus_changes_font_size() {
        let mut p = pane();
        p.begin_text_at(Vec2::new(10.0, 10.0), 4.0);
        p.insert_text("hi");
        let id = p.editing_text.unwrap();
        let size0 = match &p.scene.shapes.iter().find(|s| s.id == id).unwrap().kind {
            ShapeKind::Text { size, .. } => *size,
            _ => panic!(),
        };
        assert!(p.change_text_size(1.5));
        let size1 = match &p.scene.shapes.iter().find(|s| s.id == id).unwrap().kind {
            ShapeKind::Text { size, .. } => *size,
            _ => panic!(),
        };
        assert!((size1 - size0 * 1.5).abs() < 0.01);
    }

    #[test]
    fn empty_text_is_discarded_on_commit() {
        let mut p = pane();
        p.begin_text_at(Vec2::new(10.0, 10.0), 4.0);
        assert_eq!(p.scene.shapes.len(), 1);
        p.commit_text();
        assert!(p.scene.shapes.is_empty(), "empty text removed");
    }

    #[test]
    fn reediting_existing_text_has_its_own_undo_checkpoint() {
        let mut p = pane();
        p.begin_text_at(Vec2::new(10.0, 10.0), 4.0);
        p.insert_text("before");
        p.commit_text();
        p.begin_text_at(Vec2::new(12.0, 14.0), 6.0);
        p.insert_text(" after");
        assert!(p.undo());
        match &p.scene.shapes[0].kind {
            ShapeKind::Text { content, .. } => assert_eq!(content, "before"),
            _ => panic!(),
        }
    }

    #[test]
    fn backspace_removes_a_complete_visible_character() {
        let mut p = pane();
        p.begin_text_at(Vec2::new(10.0, 10.0), 4.0);
        p.insert_text("a\u{301}\u{1f469}\u{200d}\u{1f4bb}");
        assert!(p.text_backspace());
        match &p.scene.shapes[0].kind {
            ShapeKind::Text { content, .. } => assert_eq!(content, "a\u{301}"),
            _ => panic!(),
        }
        assert!(p.text_backspace());
        assert!(!p.text_backspace());
    }

    #[test]
    fn clicking_existing_text_edits_it() {
        let mut p = pane();
        p.begin_text_at(Vec2::new(10.0, 10.0), 4.0);
        p.insert_text("abc");
        let first = p.editing_text.unwrap();
        p.commit_text();
        // Click roughly on the existing text.
        p.begin_text_at(Vec2::new(12.0, 14.0), 6.0);
        assert_eq!(p.editing_text, Some(first), "re-edits the same shape");
        assert_eq!(p.scene.shapes.len(), 1, "no new shape created");
    }
}
