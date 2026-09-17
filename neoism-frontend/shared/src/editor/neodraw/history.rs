//! Undo/redo as full scene snapshots.
//!
//! Scenes are small (a handful of shapes), so snapshotting the whole
//! [`Scene`] on each edit is simpler and plenty fast versus a command
//! log. A checkpoint is taken *before* a mutating gesture; undo swaps
//! the live scene with the top snapshot and pushes the live one onto
//! the redo stack.

use super::pane::DrawPane;
use super::scene::Scene;

/// Cap on retained snapshots per direction.
const MAX_DEPTH: usize = 200;

#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Scene>,
    redo: Vec<Scene>,
}

impl History {
    /// Record a pre-edit snapshot, invalidating the redo stack.
    pub fn record(&mut self, scene: &Scene) {
        if self.undo.last() == Some(scene) {
            return; // no-op edit; don't stack duplicates
        }
        self.undo.push(scene.clone());
        if self.undo.len() > MAX_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Swap `current` for the most recent snapshot, returning it.
    fn undo(&mut self, current: &Scene) -> Option<Scene> {
        let prev = self.undo.pop()?;
        self.redo.push(current.clone());
        Some(prev)
    }

    fn redo(&mut self, current: &Scene) -> Option<Scene> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        Some(next)
    }
}

impl DrawPane {
    /// Snapshot the current scene before a mutating edit.
    pub fn checkpoint(&mut self) {
        self.history.record(&self.scene);
    }

    pub fn undo(&mut self) -> bool {
        // Capture the current scene first to avoid borrowing `self`
        // twice across the history call.
        let current = self.scene.clone();
        if let Some(prev) = self.history.undo(&current) {
            self.scene = prev;
            self.text_dims.clear();
            self.text_layouts.clear();
            self.selection.clear();
            self.editing_text = None;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        let current = self.scene.clone();
        if let Some(next) = self.history.redo(&current) {
            self.scene = next;
            self.text_dims.clear();
            self.text_layouts.clear();
            self.selection.clear();
            self.editing_text = None;
            self.dirty = true;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::neodraw::scene::{ShapeId, ShapeKind, Style};
    use crate::editor::neodraw::Shape;
    use std::path::PathBuf;

    fn rect(id: u64) -> Shape {
        Shape {
            id: ShapeId(id),
            kind: ShapeKind::Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                corner: 0.0,
            },
            style: Style::default(),
        }
    }

    #[test]
    fn undo_redo_round_trips() {
        let mut p = DrawPane::new(PathBuf::from("t.neodraw"));
        p.checkpoint();
        p.scene.shapes.push(rect(1));
        p.checkpoint();
        p.scene.shapes.push(rect(2));
        assert_eq!(p.scene.shapes.len(), 2);

        assert!(p.undo());
        assert_eq!(p.scene.shapes.len(), 1);
        assert!(p.undo());
        assert_eq!(p.scene.shapes.len(), 0);
        assert!(!p.undo());

        assert!(p.redo());
        assert_eq!(p.scene.shapes.len(), 1);
        assert!(p.redo());
        assert_eq!(p.scene.shapes.len(), 2);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut p = DrawPane::new(PathBuf::from("t.neodraw"));
        p.checkpoint();
        p.scene.shapes.push(rect(1));
        p.undo();
        assert!(p.history.can_redo());
        p.checkpoint();
        p.scene.shapes.push(rect(9));
        assert!(!p.history.can_redo(), "a fresh edit drops the redo stack");
    }
}
