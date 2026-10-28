//! Data-only frame for native hosts that paint without Sugarloaf.
use super::FileTree;

pub struct FileTreeHostFrame {
    pub top: usize,
    pub offset: f32,
    pub selected: usize,
    pub cursor_offset: f32,
    pub animating: bool,
}

impl FileTree {
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
