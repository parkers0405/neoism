//! Mac Finder-style spring-loaded drag-and-drop for the file tree.
//!
//! A press on a real (non-virtual) row ARMS a drag; once the cursor
//! travels past [`DRAG_ACTIVATION_PX`] it goes `live` and a ghost of the
//! dragged item follows the cursor. Hovering a closed folder for
//! [`SPRING_OPEN_DWELL`] springs it open (Finder's "spring-loaded
//! folders"), and the hovered drop target wiggles. Releasing over a
//! valid folder MOVES the item into it; releasing elsewhere cancels; a
//! release that never went live is just a click (the caller opens the
//! file / toggles the folder).
//!
//! The pure state + policy live here in the shared panel; the host
//! (desktop `screen/bridges/file_tree/mouse.rs`) drives begin/update/end
//! from its pointer chains and commits the move via `fs::rename` (local)
//! or `FilesClientMessage::Rename` (joined workspace). Rendering of the
//! ghost + wiggle lives in `render.rs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use web_time::Instant;

use super::scan::normalize_path;
use super::state::FileTree;
use super::types::NodeKind;

/// Pixels the cursor must travel from the press point before an armed
/// press becomes a live drag. Mirrors the Island tab drag threshold so
/// a plain click never smears into a drag.
pub(super) const DRAG_ACTIVATION_PX: f32 = 5.0;

/// How long the cursor must dwell over a closed folder before it springs
/// open under the drag.
pub(super) const SPRING_OPEN_DWELL: Duration = Duration::from_millis(450);

/// Live drag state: what is being dragged, where the ghost is, and which
/// folder (if any) is the current drop target.
pub struct FileDragState {
    #[allow(dead_code)]
    pub(super) source_index: usize,
    /// Normalized absolute path of the dragged item.
    pub(super) source_path: PathBuf,
    /// Label painted on the cursor-following ghost.
    pub(super) source_label: String,
    /// True once the cursor has moved past the activation threshold. The
    /// ghost only paints, and a release only moves, when this is set.
    pub(super) live: bool,
    pub(super) start_x: f32,
    pub(super) start_y: f32,
    pub(super) current_x: f32,
    pub(super) current_y: f32,
    /// Normalized path of the folder currently under the cursor that is a
    /// legal drop target, if any (drives the highlight + wiggle).
    pub(super) hovered_dir: Option<PathBuf>,
    /// When the current `hovered_dir` was first entered — the dwell clock
    /// for spring-open and the phase origin for the wiggle.
    pub(super) hovered_since: Option<Instant>,
    /// Folders already auto-sprung this drag, so the dwell fires once per
    /// folder instead of every frame past the threshold.
    pub(super) sprang: HashSet<PathBuf>,
}

/// What a release resolved to.
pub enum FileDropOutcome {
    /// The press never became a drag — the caller treats it as a click
    /// (open the file / toggle the folder).
    Click,
    /// Move `source` into directory `dest_dir`.
    Move { source: PathBuf, dest_dir: PathBuf },
    /// A live drag released over no valid target — do nothing.
    Cancel,
}

impl FileTree {
    /// Arm a potential drag from the row at `index`. Returns `true` if the
    /// row is draggable (a real file/dir path, not a virtual/workspace
    /// row); the caller then DEFERS activation to release. Returns `false`
    /// for virtual or path-less rows, which the caller should activate
    /// immediately as before.
    pub fn begin_file_drag(&mut self, index: usize, mouse_x: f32, mouse_y: f32) -> bool {
        let Some(entry) = self.entries.get(index) else {
            return false;
        };
        if entry.is_virtual() {
            return false;
        }
        let Some(path) = entry.path.clone() else {
            return false;
        };
        self.file_drag = Some(FileDragState {
            source_index: index,
            source_path: normalize_path(&path),
            source_label: entry.label.clone(),
            live: false,
            start_x: mouse_x,
            start_y: mouse_y,
            current_x: mouse_x,
            current_y: mouse_y,
            hovered_dir: None,
            hovered_since: None,
            sprang: HashSet::new(),
        });
        true
    }

    /// Drive an armed/live drag: move the ghost, flip `live` past the
    /// activation threshold, resolve the hovered drop-target folder, and
    /// return the path of a closed folder that has been dwelled on long
    /// enough to spring open (returned once per folder). `hovered_row` is
    /// the row currently under the cursor (host hit-test).
    pub fn update_file_drag(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        hovered_row: Option<usize>,
    ) -> Option<PathBuf> {
        // Resolve the drop target against `&self.entries` BEFORE taking a
        // mutable borrow of `self.file_drag`.
        let source_path = self.file_drag.as_ref()?.source_path.clone();
        let target = self.drag_drop_target(hovered_row, &source_path);

        let drag = self.file_drag.as_mut()?;
        drag.current_x = mouse_x;
        drag.current_y = mouse_y;
        if !drag.live {
            let dx = mouse_x - drag.start_x;
            let dy = mouse_y - drag.start_y;
            if (dx * dx + dy * dy).sqrt() < DRAG_ACTIVATION_PX {
                return None;
            }
            drag.live = true;
        }

        // Re-arm the dwell clock whenever the hovered folder changes.
        let target_path = target.as_ref().map(|(path, _)| path.clone());
        if drag.hovered_dir != target_path {
            drag.hovered_dir = target_path;
            drag.hovered_since = drag.hovered_dir.as_ref().map(|_| Instant::now());
        }

        if let (Some((dir, closed)), Some(since)) = (target, drag.hovered_since) {
            if closed
                && since.elapsed() >= SPRING_OPEN_DWELL
                && drag.sprang.insert(dir.clone())
            {
                return Some(dir);
            }
        }
        None
    }

    /// Finish a drag. Returns the outcome and clears the drag state.
    pub fn end_file_drag(&mut self) -> FileDropOutcome {
        let Some(drag) = self.file_drag.take() else {
            return FileDropOutcome::Cancel;
        };
        if !drag.live {
            return FileDropOutcome::Click;
        }
        match drag.hovered_dir {
            Some(dest_dir) if dest_dir != drag.source_path => FileDropOutcome::Move {
                source: drag.source_path,
                dest_dir,
            },
            _ => FileDropOutcome::Cancel,
        }
    }

    /// Cancel any in-flight drag without acting (e.g. on Escape or focus
    /// loss). No-op when nothing is being dragged.
    pub fn cancel_file_drag(&mut self) {
        self.file_drag = None;
    }

    pub fn file_drag(&self) -> Option<&FileDragState> {
        self.file_drag.as_ref()
    }

    /// True only once a drag has crossed the activation threshold — the
    /// window in which the ghost paints and the cursor shows "grabbing".
    pub fn is_file_dragging(&self) -> bool {
        self.file_drag.as_ref().is_some_and(|drag| drag.live)
    }

    /// The hovered folder resolved to `(normalized_path, is_closed)` when
    /// the row under the cursor is a LEGAL drop target for `source_path`,
    /// else `None`. A legal target is a directory that is not the source,
    /// not inside the source's own subtree (can't drop a folder into
    /// itself), and not the source's current parent (a no-op move).
    fn drag_drop_target(
        &self,
        hovered_row: Option<usize>,
        source_path: &Path,
    ) -> Option<(PathBuf, bool)> {
        let entry = self.entries.get(hovered_row?)?;
        if entry.is_virtual() {
            return None;
        }
        let open = match entry.kind {
            NodeKind::Dir { open } => open,
            NodeKind::File => return None,
        };
        let dir = normalize_path(entry.path.as_ref()?);
        if dir.as_path() == source_path {
            return None;
        }
        // Inside the dragged folder's own subtree.
        if dir.starts_with(source_path) {
            return None;
        }
        // Already living directly in this folder — moving there is a no-op.
        if source_path.parent() == Some(dir.as_path()) {
            return None;
        }
        Some((dir, !open))
    }

    /// Index of the row being dragged, resolved from its path so it
    /// survives row re-indexing. `None` until the drag is live. The
    /// renderer dims this row in place (it's been lifted out) and paints
    /// the real row content following the cursor.
    pub(super) fn drag_source_index(&self) -> Option<usize> {
        let source = self.file_drag.as_ref().filter(|drag| drag.live)?.source_path.as_path();
        self.entries.iter().position(|entry| {
            entry.path.as_deref().map(normalize_path).as_deref() == Some(source)
        })
    }

    /// Index of the folder row the drag is currently hovering, resolved
    /// from its path so it survives the row re-indexing a spring-open
    /// causes. Used by the renderer to draw the highlight + wiggle.
    pub(super) fn drag_hovered_index(&self) -> Option<usize> {
        let dir = self.file_drag.as_ref()?.hovered_dir.as_ref()?;
        self.entries.iter().position(|entry| {
            entry.path.as_deref().map(normalize_path).as_deref() == Some(dir.as_path())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::file_tree::types::{GitStatus, TreeEntry, VirtualEntryKind};

    fn entry(
        label: &str,
        kind: NodeKind,
        path: &str,
        depth: u8,
        virtual_kind: Option<VirtualEntryKind>,
    ) -> TreeEntry {
        TreeEntry {
            label: label.into(),
            depth,
            kind,
            path: Some(PathBuf::from(path)),
            git_status: GitStatus::None,
            virtual_kind,
        }
    }

    /// src/ (open) > a.rs, docs/ (closed), assets/ (open) > img/ (closed).
    fn sample() -> FileTree {
        let mut tree = FileTree::empty();
        tree.set_entries(vec![
            entry("src", NodeKind::Dir { open: true }, "/w/src", 0, None),
            entry("a.rs", NodeKind::File, "/w/src/a.rs", 1, None),
            entry("docs", NodeKind::Dir { open: false }, "/w/docs", 0, None),
            entry("assets", NodeKind::Dir { open: true }, "/w/assets", 0, None),
            entry("img", NodeKind::Dir { open: false }, "/w/assets/img", 1, None),
        ]);
        tree
    }

    #[test]
    fn only_real_rows_are_draggable() {
        let mut tree = sample();
        assert!(tree.begin_file_drag(1, 0.0, 0.0), "file row is draggable");
        tree.cancel_file_drag();
        assert!(tree.begin_file_drag(0, 0.0, 0.0), "dir row is draggable");
        tree.cancel_file_drag();

        tree.set_entries(vec![entry(
            "Neoism",
            NodeKind::Dir { open: false },
            "/w/nz",
            0,
            Some(VirtualEntryKind::NeoismWorkspace),
        )]);
        assert!(!tree.begin_file_drag(0, 0.0, 0.0), "virtual row is not draggable");
        assert!(tree.file_drag().is_none());
    }

    #[test]
    fn press_becomes_live_only_past_threshold() {
        let mut tree = sample();
        tree.begin_file_drag(1, 100.0, 100.0);
        assert!(!tree.is_file_dragging());
        // A tiny nudge stays a click.
        tree.update_file_drag(101.0, 101.0, Some(2));
        assert!(!tree.is_file_dragging());
        assert!(matches!(tree.end_file_drag(), FileDropOutcome::Click));
        // Past the threshold it goes live.
        tree.begin_file_drag(1, 100.0, 100.0);
        tree.update_file_drag(100.0, 120.0, Some(2));
        assert!(tree.is_file_dragging());
    }

    #[test]
    fn dropping_a_file_into_a_folder_moves_it() {
        let mut tree = sample();
        tree.begin_file_drag(1, 100.0, 100.0); // /w/src/a.rs
        tree.update_file_drag(100.0, 130.0, Some(2)); // over /w/docs
        match tree.end_file_drag() {
            FileDropOutcome::Move { source, dest_dir } => {
                assert_eq!(source, PathBuf::from("/w/src/a.rs"));
                assert_eq!(dest_dir, PathBuf::from("/w/docs"));
            }
            _ => panic!("expected a move"),
        }
    }

    #[test]
    fn dropping_into_the_current_parent_is_a_noop() {
        let mut tree = sample();
        tree.begin_file_drag(1, 100.0, 100.0); // /w/src/a.rs, already in /w/src
        tree.update_file_drag(100.0, 130.0, Some(0)); // over /w/src
        assert!(matches!(tree.end_file_drag(), FileDropOutcome::Cancel));
    }

    #[test]
    fn cannot_drop_a_folder_into_its_own_subtree() {
        let mut tree = sample();
        tree.begin_file_drag(3, 100.0, 100.0); // /w/assets
        tree.update_file_drag(100.0, 130.0, Some(4)); // over /w/assets/img
        assert!(matches!(tree.end_file_drag(), FileDropOutcome::Cancel));
    }

    #[test]
    fn releasing_over_empty_space_cancels() {
        let mut tree = sample();
        tree.begin_file_drag(1, 100.0, 100.0);
        tree.update_file_drag(100.0, 130.0, None);
        assert!(matches!(tree.end_file_drag(), FileDropOutcome::Cancel));
    }
}
