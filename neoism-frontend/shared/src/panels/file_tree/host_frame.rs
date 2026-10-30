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
        let selected_path=self.entries.get(self.selected).and_then(|entry|entry.path.as_ref());
        self.selected=selected_path.and_then(|path|entries.iter().position(|entry|entry.path.as_ref()==Some(path))).unwrap_or_else(||self.selected.min(entries.len().saturating_sub(1)));
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

/// Original row avatar geometry and typography for another native painter.
pub fn paint_host_presence_row(
    painter:&mut (impl crate::primitives::paint::Paint+?Sized),
    peers:&[crate::editor::crdt::PresenceAvatarPeer],width:f32,scale:f32,
    theme:&crate::primitives::IdeTheme,
)->f32 {
    let height=super::ROW_HEIGHT*scale;
    let font=super::FONT_SIZE*scale;
    let avatar=(font+2.0).clamp(14.0,18.0);
    let opts=neoism_paint::text::DrawOpts{font_size:font*0.82,color:theme.u8(theme.dim),clip_rect:Some([0.0,0.0,width,height]),..Default::default()};
    let occupied=super::render::presence_cluster_width(painter,peers,avatar,&opts);
    super::render::draw_row_presence_cluster(painter,peers,width,0.0,height,avatar,scale,&opts,[0.0,0.0,width,height],theme,1.0,&[]);
    occupied
}
