//! Dragging a file, note or folder into another folder (`shared/src/panels/file_tree/drag.rs`,
//! Finder's spring-loaded folders). A press on a row arms a drag; 5 pt of travel makes it
//! live: the row dims in place, a copy of it follows the pointer, and the folder it would land
//! in is tinted and shivers (2.4 pt at about 4 Hz, easing in over 120 ms). A closed folder
//! hovered for 450 ms springs open. Releasing moves the entry; a press that never went live is
//! the click it always was.
//!
//! Where a release lands:
//! - on a folder row: into that folder;
//! - on a file row: into that file's folder (beside it);
//! - on the tree's blank space: into the root;
//! - back on itself, on its own folder, inside itself, or outside the sidebar: nowhere. It
//!   stays in the folder it is in (an entry is never thrown to the root by a missed drop).

use super::{Row, Tree, join};
use crate::app::{App, Sidebar};
use crate::ui::theme::{NERD, rgb};
use rune::tok::TextStyle;
use rune::{Drag, El, Phase, Tx, el, row, text};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Travel before a press becomes a drag (`DRAG_ACTIVATION_PX`).
const ACTIVATION: f32 = 5.0;
/// Hover before a closed folder opens under the drag (`SPRING_OPEN_DWELL`).
const SPRING_OPEN: Duration = Duration::from_millis(450);

pub struct TreeDrag {
    notes: bool,
    source: Row,
    point: [f32; 2],
    live: bool,
    /// The folder a release would move the entry into ("" is the root).
    target: Option<String>,
    /// When `target` became the target: the spring-open clock and the shiver's phase.
    since: Instant,
    sprang: HashSet<String>,
    /// The target folder row's sideways shiver.
    shiver: rune::Live<[f32; 2]>,
}

fn parent(path: &str) -> &str { path.rsplit_once('/').map_or("", |(dir, _)| dir) }

/// Whether `dir` is `path` or inside it.
fn within(dir: &str, path: &str) -> bool { dir == path || dir.starts_with(&format!("{path}/")) }

/// The folder `source` would move into when released over `hovered` (a row, or the tree's
/// blank space) inside the panel; `None` when the release would change nothing or is illegal.
fn drop_target(source: &Row, hovered: Option<&Row>) -> Option<String> {
    let home = parent(&source.path);
    let dir = match hovered {
        Some(row) if row.path == source.path => return None,
        Some(row) if row.entry.is_dir => row.path.as_str(),
        Some(row) => parent(&row.path),
        None => "",
    };
    // Already there, or a folder into itself or its own subtree.
    if dir == home || (source.entry.is_dir && within(dir, &source.path)) { return None; }
    Some(dir.to_owned())
}

impl App {
    /// The row under a window point, and whether the point is inside the tree panel at all.
    fn tree_row_at(&self, tree: &Tree, point: [f32; 2]) -> (Option<Row>, bool) {
        let Some([x, y, w, h]) = self.rects.get(super::view::SCROLL_KEY).copied() else { return (None, false) };
        let header = super::view::HEADER_H * self.ty.scale();
        let inside = point[0] >= x && point[0] < x + w && point[1] >= y - header && point[1] < y + h;
        if !inside || point[1] < y { return (None, inside); }
        let row_h = super::view::ROW_H * self.ty.scale();
        let index = ((point[1] - y + self.tree_follow.scroll.get().0) / row_h).floor().max(0.0) as usize;
        (tree.rows().get(index).cloned(), true)
    }

    /// A tree row's drag gesture, from press to release.
    pub fn tree_drag(&mut self, index: usize, d: &Drag, _: &mut Tx) {
        if d.phase == Phase::Down {
            self.tree_press_suppressed = false;
            let Some(sidebar) = self.sidebar.filter(|s| matches!(s, Sidebar::Files | Sidebar::Notes)) else { return };
            let notes = sidebar == Sidebar::Notes;
            let Some(source) = (if notes { &self.notes } else { &self.tree }).rows().get(index).cloned() else { return };
            self.tree_dnd = Some(TreeDrag { notes, source, point: d.window_point(), live: false, target: None, since: Instant::now(),
                sprang: HashSet::new(), shiver: rune::Live::new([0.0, 0.0]) });
            return;
        }
        let Some(mut drag) = self.tree_dnd.take() else { return };
        drag.point = d.window_point();
        if !drag.live && d.dx.hypot(d.dy) >= ACTIVATION { drag.live = true; }
        if drag.live {
            let tree = if drag.notes { &self.notes } else { &self.tree };
            let (hovered, inside) = self.tree_row_at(tree, drag.point);
            let target = if inside && !d.cancelled() { drop_target(&drag.source, hovered.as_ref()) } else { None };
            if target != drag.target { drag.target = target; drag.since = Instant::now(); drag.shiver.set([0.0, 0.0]); }
        }
        if d.phase != Phase::Up { self.tree_dnd = Some(drag); return; }
        // A drag that went live is not also a click on the row it started on.
        self.tree_press_suppressed = drag.live;
        if let (true, false, Some(dir)) = (drag.live, d.cancelled(), drag.target) {
            let to = join(&dir, &drag.source.entry.name);
            self.tree_move_path(drag.notes, &drag.source.path, &to);
        }
    }

    /// Per frame while a drag is live: the target folder shivers, and one hovered long enough
    /// springs open. Returns (a drag is live, the views must re-run).
    pub(crate) fn tree_drag_tick(&mut self) -> (bool, bool) {
        let Some(drag) = self.tree_dnd.as_mut().filter(|d| d.live) else { return (false, false) };
        let Some(dir) = drag.target.clone() else { return (true, false) };
        let t = drag.since.elapsed().as_secs_f32();
        let scale = self.ty.scale();
        drag.shiver.set([(t * 26.0).sin() * 2.4 * scale * (t / 0.12).clamp(0.0, 1.0), 0.0]);
        if dir.is_empty() || drag.since.elapsed() < SPRING_OPEN || !drag.sprang.insert(dir.clone()) { return (true, false); }
        let sidebar = if drag.notes { Sidebar::Notes } else { Sidebar::Files };
        let tree = self.tree_of(sidebar);
        if !tree.open.insert(dir.clone()) { return (true, false); }
        self.list_dir(sidebar, &dir);
        (true, true)
    }

    /// How a row is drawn while something is dragged: (it is the one lifted out, it is the
    /// folder the drop would land in and its shiver).
    pub(crate) fn tree_drag_row(&self, notes: bool, path: &str) -> (bool, Option<&rune::Live<[f32; 2]>>) {
        match self.tree_dnd.as_ref().filter(|d| d.live && d.notes == notes) {
            Some(drag) => (drag.source.path == path, (drag.target.as_deref() == Some(path) && !path.is_empty()).then_some(&drag.shiver)),
            None => (false, None),
        }
    }

    /// Whether a live drag would land in the tree's root (the whole panel says so).
    pub(crate) fn tree_drag_root(&self, notes: bool) -> bool {
        self.tree_dnd.as_ref().is_some_and(|d| d.live && d.notes == notes && d.target.as_deref() == Some(""))
    }
}

/// The lifted row, following the pointer over everything (`tree-drag-preview`).
pub fn ghost(app: &App) -> El {
    let Some(drag) = app.tree_dnd.as_ref().filter(|d| d.live) else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let name = &drag.source.entry.name;
    let (glyph, color) = if drag.source.entry.is_dir { (crate::ui::icons::FOLDER_CLOSED, p.folder) } else { crate::ui::icons::for_file(name) };
    let row_h = super::view::ROW_H * s;
    let width = (60.0 + name.chars().count() as f32 * 8.0).min(280.0) * s;
    row().key("tree-drag-preview").w(width).h(row_h).px(10.0 * s).gap(8.0 * s).center_y().radius(6.0 * s)
        .fill(rgb(p.surface)).border(s.max(1.0), rgb(p.border))
        .absolute(drag.point[0] + 12.0 * s, drag.point[1] - row_h / 2.0)
        .child(text(glyph).font(TextStyle::new(13.0 * s, 400.0).family(NERD)).color(rgb(color)))
        .child(text(name.clone()).font(app.ty.sized(13.0)).color(rgb(p.fg)).truncate().shrink())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Entry;

    fn item(path: &str, is_dir: bool) -> Row {
        Row { path: path.into(), depth: path.matches('/').count(), entry: Entry { name: path.rsplit('/').next().unwrap().into(), is_dir } }
    }

    #[test]
    fn a_drop_lands_where_it_is_aimed_and_a_miss_moves_nothing() {
        let note = item("docs/plan.md", false);
        // Onto a folder: into it. Onto a file: beside it. Onto blank space: the root.
        assert_eq!(drop_target(&note, Some(&item("archive", true))), Some("archive".into()));
        assert_eq!(drop_target(&note, Some(&item("archive/old.md", false))), Some("archive".into()));
        assert_eq!(drop_target(&note, None), Some(String::new()));
        // Back on itself, on its own folder or on a sibling: it stays where it is, never the root.
        assert_eq!(drop_target(&note, Some(&note)), None);
        assert_eq!(drop_target(&note, Some(&item("docs", true))), None);
        assert_eq!(drop_target(&note, Some(&item("docs/other.md", false))), None);
        // Something already at the root dropped on blank space or a root file: nothing to do.
        let top = item("readme.md", false);
        assert_eq!(drop_target(&top, None), None);
        assert_eq!(drop_target(&top, Some(&item("license", false))), None);
        // A folder never goes into itself or its own subtree.
        let folder = item("docs", true);
        assert_eq!(drop_target(&folder, Some(&item("docs/deep", true))), None);
        assert_eq!(drop_target(&folder, Some(&item("docs/deep/x.md", false))), None);
        assert_eq!(drop_target(&folder, Some(&item("docs-old", true))), Some("docs-old".into()));
    }
}
