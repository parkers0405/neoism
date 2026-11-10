//! Code editor tabs (`reference/18-editor-code.png`, motion `editor-scroll-cursor`): a file read
//! over the daemon's `Files` family, drawn with a breadcrumb, a line-number gutter, the current
//! line lit, syntax colour, and the window block cursor on the caret cell. Motions are vim's.

pub mod view;
mod wrap;

pub use view::view;

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;

pub struct Buffer {
    /// Workspace-relative path.
    pub path: String,
    pub lines: Vec<String>,
    pub loaded: bool,
    pub cursor: (usize, usize),
    /// Column the cursor wants on vertical moves (vim's `curswant`).
    pub want_col: usize,
    pub error: Option<String>,
}

impl Buffer {
    fn new(path: String, line: Option<u32>) -> Buffer {
        let row = line.map_or(0, |l| l.saturating_sub(1) as usize);
        Buffer { path, lines: Vec::new(), loaded: false, cursor: (row, 0), want_col: 0, error: None }
    }

    pub fn language(&self) -> Option<&str> {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        name.rsplit_once('.').map(|(_, ext)| ext)
    }

    fn clamp(&mut self) {
        let last = self.lines.len().saturating_sub(1);
        self.cursor.0 = self.cursor.0.min(last);
        let len = self.lines.get(self.cursor.0).map_or(0, |l| l.chars().count());
        self.cursor.1 = self.want_col.min(len.saturating_sub(1));
    }

    /// Vim normal-mode motion; returns whether the cursor moved.
    pub fn motion(&mut self, motion: Motion, page: usize) -> bool {
        let before = self.cursor;
        let last = self.lines.len().saturating_sub(1);
        let (row, col) = self.cursor;
        match motion {
            Motion::Left => { self.want_col = col.saturating_sub(1); }
            Motion::Right => { self.want_col = col + 1; }
            Motion::Up => self.cursor.0 = row.saturating_sub(1),
            Motion::Down => self.cursor.0 = (row + 1).min(last),
            Motion::HalfPageUp => self.cursor.0 = row.saturating_sub(page / 2),
            Motion::HalfPageDown => self.cursor.0 = (row + page / 2).min(last),
            Motion::Top => { self.cursor.0 = 0; self.want_col = 0; }
            Motion::Bottom => { self.cursor.0 = last; self.want_col = 0; }
            Motion::LineStart => self.want_col = 0,
            Motion::LineEnd => self.want_col = usize::MAX,
        }
        self.clamp();
        self.cursor != before
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    HalfPageUp,
    HalfPageDown,
    Top,
    Bottom,
    LineStart,
    LineEnd,
}

impl App {
    /// Open `path` (absolute, or relative to the workspace) in an editor tab, at `line`.
    pub fn open_file(&mut self, path: &str, line: Option<u32>) {
        let relative = std::path::Path::new(path).strip_prefix(&self.cwd).map(|p| p.display().to_string()).unwrap_or_else(|_| path.to_owned());
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Editor { path: relative.clone() }) {
            self.active = index;
            if let (Some(line), Some(buffer)) = (line, self.buffers.get_mut(&relative)) {
                buffer.cursor.0 = line.saturating_sub(1) as usize;
                buffer.clamp();
            }
            return;
        }
        self.buffers.insert(relative.clone(), Buffer::new(relative.clone(), line));
        self.open_tab(TabKind::Editor { path: relative.clone() });
        self.focus = crate::app::Focus::Content;
        // Notes live outside the workspace: their reads are scoped to the vault.
        let scope = self.notes.root.clone().filter(|root| relative.starts_with(root.as_str())).unwrap_or_else(|| self.cwd.clone());
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Files, serde_json::json!({"ReadFile": {"path": relative}}), &scope);
        }
    }

    pub(crate) fn apply_opened_file(&mut self, path: &str, bytes: &[u8]) {
        let Some(buffer) = self.buffers.get_mut(path) else { return };
        if bytes.iter().take(8192).any(|b| *b == 0) {
            buffer.error = Some("Binary file".into());
        } else {
            buffer.lines = String::from_utf8_lossy(bytes).split('\n').map(|l| l.trim_end_matches('\r').to_owned()).collect();
        }
        buffer.loaded = true;
        buffer.clamp();
    }

    /// The active editor tab's buffer.
    pub fn active_buffer(&self) -> Option<&Buffer> {
        match &self.active_tab().kind { TabKind::Editor { path } => self.buffers.get(path), _ => None }
    }

    pub fn editor_motion(&mut self, motion: Motion) -> bool {
        let page = self.editor_rows;
        let TabKind::Editor { path } = self.active_tab().kind.clone() else { return false };
        let Some(buffer) = self.buffers.get_mut(&path) else { return false };
        buffer.motion(motion, page)
    }
}

impl App {
    /// Scroll the editor so the cursor cell sits at least four lines inside the viewport, on the
    /// scroll spring. Uses laid-out rects, so wrapped lines count at their real height.
    /// Keep the cursor `scrolloff` rows inside the view: the target is computed from the
    /// cursor's row, and a held arrow keeps extending the spring's target rather than its
    /// current (mid-glide) offset, so the view never falls behind.
    pub fn editor_follow_cursor(&mut self, tx: &mut rune::Tx) {
        if !std::mem::take(&mut self.editor_follow) { return; }
        let Some(buffer) = self.active_buffer() else { return };
        let key = view::scroll_key(&buffer.path);
        let line_h = view::line_height(self);
        let row = view::cursor_visual_row(self, buffer) as f32;
        let (offset, max, viewport) = self.editor_scroll.get();
        let viewport = if viewport > 0.0 { viewport } else { self.rects.get("editor-view").map_or(0.0, |r| r[3]) };
        if viewport <= 0.0 { return; }
        let margin = (4.0 * line_h).min(((viewport - line_h) / 2.0).max(0.0));
        let base = self.wheel_target.filter(|t| (t - offset).abs() > 0.5).unwrap_or(offset);
        let (top, bottom) = (row * line_h, (row + 1.0) * line_h);
        let target = if top - margin < base { top - margin }
            else if bottom + margin > base + viewport { bottom + margin - viewport }
            else { return };
        // The content may have grown since the last layout reported `max`.
        let target = target.clamp(0.0, max.max(target));
        self.wheel_target = Some(target);
        tx.scroll_to_with(key, 0.0, target, view::SCROLL_SPRING);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer() -> Buffer {
        let mut b = Buffer::new("a.rs".into(), None);
        b.lines = vec!["fn main() {".into(), "".into(), "    println!();".into(), "}".into()];
        b.loaded = true;
        b
    }

    #[test]
    fn vertical_moves_keep_the_wanted_column() {
        let mut b = buffer();
        b.motion(Motion::Right, 20);
        b.motion(Motion::Right, 20);
        b.motion(Motion::Right, 20);
        assert_eq!(b.cursor, (0, 3));
        b.motion(Motion::Down, 20);
        assert_eq!(b.cursor, (1, 0), "an empty line holds column 0");
        b.motion(Motion::Down, 20);
        assert_eq!(b.cursor, (2, 3), "and the next line gets the column back");
        b.motion(Motion::Bottom, 20);
        assert_eq!(b.cursor, (3, 0));
        b.motion(Motion::LineEnd, 20);
        assert_eq!(b.cursor, (3, 0));
    }
}
