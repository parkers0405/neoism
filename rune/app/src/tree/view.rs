//! Tree rows (`file_tree/render.rs`), in original points × scale: 26 pt rows, 12 pt padding,
//! 14 pt indent per depth holding the chevron, 13 pt icon, 8 pt gap, 13 pt label. Folders draw in
//! the theme folder colour; file labels are dim until selected.

use crate::app::{App, Focus};
use crate::ui::icons;
use crate::ui::theme::{NERD, rgb};
use rune::tok::TextStyle;
use rune::{Color, El, ScrollSpec, col, el, row, text};

const ROW_H: f32 = 26.0;
const PAD_X: f32 = 12.0;
const INDENT: f32 = 14.0;
const FONT: f32 = 13.0;
const ICON_GAP: f32 = 8.0;
const HEADER_H: f32 = 36.0;

pub fn view(app: &App) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let rows = app.tree.rows();
    let focused = app.focus == Focus::Sidebar;
    let mut list = col().w_full();
    for (index, item) in rows.iter().enumerate() {
        let selected = index == app.tree.selected;
        let open = item.entry.is_dir && app.tree.open.contains(&item.path);
        let (glyph, icon_color) = if item.entry.is_dir {
            (if open { icons::FOLDER_OPEN } else { icons::FOLDER_CLOSED }, p.folder)
        } else {
            icons::for_file(&item.entry.name)
        };
        let label_color = if item.entry.is_dir || selected { p.fg } else { p.dim };
        let chevron = item.entry.is_dir.then(|| if open { "\u{f078}" } else { "\u{f054}" });
        let icon_font = TextStyle::new(FONT * s, 400.0).family(NERD);
        let chevron_font = TextStyle::new((INDENT - 6.0) * s, 400.0).family(NERD);
        let mut line = row().key(format!("tree:{}", item.path)).w_full().h(ROW_H * s)
            .pl(PAD_X * s + item.depth as f32 * INDENT * s).pr(PAD_X * s).center_y()
            .fill(if selected && focused { rgb(p.hover) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover).alpha(0.5))
            .child(el().w(INDENT * s).center().child_opt(chevron.map(|c| text(c).font(chevron_font).color(rgb(p.muted)))))
            .child(el().w(FONT * s).center().child(text(glyph).font(icon_font).color(rgb(icon_color))))
            .child(text(item.entry.name.clone()).font(app.ty.sized(FONT)).color(rgb(label_color)).ml(ICON_GAP * s).truncate().shrink())
            .on_press(move |a: &mut App| { a.focus = Focus::Sidebar; a.tree_activate(Some(index)); });
        if selected {
            line = line.on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("tree-row".into(), r); });
        }
        list = list.child(line);
    }
    col().key("file-tree").w_full().h_full().fill(rgb(p.bg))
        .border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child(row().w_full().h(HEADER_H * s).px(16.0 * s).center_y()
            .child(text("File Tree").font(app.ty.sized_bold(14.0)).color(rgb(p.fg))))
        .child(col().key("file-tree-rows").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: false }).child(list))
}

/// The keyboard cursor on the selected row (`file_tree::keyboard_cursor_rect`): a 0.6 em block in
/// the row's left padding, 6 pt shorter than the row.
pub fn cursor_rect(app: &App, row: [f32; 4]) -> [f32; 4] {
    let s = app.ty.scale();
    let font = FONT * s;
    let w = (font * 0.6).max(2.0);
    let h = (row[3] - 6.0 * s).max(font).min(row[3]);
    [row[0] + (PAD_X * s - w).max(0.0), row[1] + (row[3] - h) / 2.0, w, h]
}

trait ChildOpt {
    fn child_opt(self, child: Option<El>) -> Self;
}

impl ChildOpt for El {
    fn child_opt(self, child: Option<El>) -> El {
        match child { Some(c) => self.child(c), None => self }
    }
}
