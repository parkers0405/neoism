//! Data-only frame for native hosts that paint without Sugarloaf.
use super::{FileTree, TreeEntry};

pub struct FileTreeHostFrame {
    pub top: usize,
    pub offset: f32,
    pub selected: usize,
    pub cursor_offset: f32,
    pub animating: bool,
}

impl FileTree {
    pub fn host_reveal_selection(&mut self, height: f32) {
        self.clamp_scroll(self.visible_rows_for_panel_height(height));
    }

    /// Host relayouts and background listings must not reveal an offscreen
    /// selection. Only explicit keyboard/select actions move to that row.
    pub fn host_set_entries(&mut self, entries: Vec<TreeEntry>) {
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        if self.reveal_flash.as_ref().is_some_and(|flash| flash.index >= entries.len()) {
            self.reveal_flash = None;
        }
        self.entries = entries;
        self.clamp_scroll_bounds(self.last_panel_height_rows);
    }

    pub fn host_frame(&mut self, height: f32) -> FileTreeHostFrame {
        let rows = self.visible_rows_for_panel_height(height);
        self.last_panel_height_rows = rows;
        self.clamp_scroll_bounds(rows);
        let offset = self.tick_scroll();
        let cursor_offset = self.tick_cursor();
        FileTreeHostFrame {
            top: self.scroll_top,
            offset,
            selected: self.selected,
            cursor_offset,
            animating: self.scroll.position != 0.0 || self.cursor_spring.position != 0.0,
        }
    }
}
