//! Tree rows (`file_tree/render.rs`), in original points × scale: 26 pt rows, 12 pt padding,
//! 14 pt indent per depth holding the chevron, 13 pt icon, 8 pt gap, 13 pt label. Folders draw in
//! the theme folder colour; file labels are dim until selected.

use crate::app::{App, Focus};
use crate::ui::icons;
use crate::ui::theme::{NERD, rgb};
use rune::tok::TextStyle;
use crate::ui::follow::Reveal;
use rune::{Color, El, ScrollSpec, col, el, row, text};

pub(crate) const ROW_H: f32 = 26.0;
const PAD_X: f32 = 12.0;
const INDENT: f32 = 14.0;
const FONT: f32 = 13.0;
const ICON_GAP: f32 = 8.0;
pub(crate) const HEADER_H: f32 = 36.0;
pub(crate) const SCROLL_KEY: &str = "file-tree-rows";
/// The Notes header's buttons (Material Design: file-plus, folder-plus).
const NEW_NOTE: &str = "\u{f0752}";
const NEW_FOLDER: &str = "\u{f0257}";
/// Rows kept between the selection and the view's edge (`file_tree::SCROLL_OFF_ROWS`).
const SCROLLOFF: i64 = 4;

impl App {
    /// Keep the selected row in view as the keyboard walks the tree (held arrows included).
    pub fn tree_follow_selection(&mut self, tx: &mut rune::Tx) {
        let Some(tree) = self.active_tree() else { return };
        let (selected, count) = (tree.selected, tree.rows().len());
        let row_h = ROW_H * self.ty.scale();
        self.tree_follow.apply(SCROLL_KEY, selected, row_h, count as f32 * row_h, Reveal::Margin(SCROLLOFF), tx);
    }
}

pub fn view(app: &App) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let Some(tree) = app.active_tree() else { return el() };
    let rows = tree.rows();
    let focused = app.focus == Focus::Sidebar;
    let notes = app.sidebar == Some(crate::app::Sidebar::Notes);
    let mut list = col().w_full();
    for (index, item) in rows.iter().enumerate() {
        // While something is dragged: the lifted row dims, the folder it would land in is
        // tinted and shivers (`tree/drag.rs`).
        let (lifted, landing) = app.tree_drag_row(notes, &item.path);
        let selected = index == tree.selected;
        let open = item.entry.is_dir && tree.open.contains(&item.path);
        let (glyph, mut icon_color) = if item.entry.is_dir {
            (if open { icons::FOLDER_OPEN } else { icons::FOLDER_CLOSED }, p.folder)
        } else {
            icons::for_file(&item.entry.name)
        };
        let git = tree.git_statuses.get(&item.path).map(|status| super::git::decoration(*status, p));
        let label_color = git.map_or_else(|| if item.entry.is_dir || selected { p.fg } else { p.dim }, |(_, color)| color);
        if item.entry.is_dir && let Some((_, color)) = git { icon_color = color; }
        let chevron = item.entry.is_dir.then(|| if open { "\u{f078}" } else { "\u{f054}" });
        let icon_font = TextStyle::new(FONT * s, 400.0).family(NERD);
        let chevron_font = TextStyle::new((INDENT - 6.0) * s, 400.0).family(NERD);
        let mut line = row().key(format!("tree:{}", item.path)).w_full().h(ROW_H * s)
            .pl(PAD_X * s + item.depth as f32 * INDENT * s).pr(PAD_X * s).center_y()
            .fill(if landing.is_some() { rgb(p.accent).alpha(0.22) } else if selected && focused { rgb(p.hover) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover).alpha(0.5))
            .opacity(if lifted { 0.35 } else { 1.0 })
            .child(el().w(INDENT * s).center().child_opt(chevron.map(|c| text(c).font(chevron_font).color(rgb(p.muted)))))
            .child(el().w(FONT * s).center().child(text(glyph).font(icon_font).color(rgb(icon_color))))
            .child(text(item.entry.name.clone()).font(app.ty.sized(FONT)).color(rgb(label_color)).ml(ICON_GAP * s).truncate().grow(1.0).shrink())
            // The people with their cursor in this file, at the row's right end.
            .child_opt((!item.entry.is_dir).then(|| {
                let path = tree.open_path(&item.path);
                let host = if crate::startup::absolute(&path) { path } else { format!("{}/{path}", app.cwd.trim_end_matches('/')) };
                crate::collab::view::tree_row(app, &host)
            }).flatten())
            .child_opt(git.map(|(marker, color)| row().pl(8.0 * s)
                .child(text(marker).font(app.ty.sized_bold(FONT * 0.9)).color(rgb(color)))))
            .on_press(move |a: &mut App| {
                // The release that ends a drag is not a click on the row it started on.
                if std::mem::take(&mut a.tree_press_suppressed) { return; }
                a.focus = Focus::Sidebar;
                a.tree_activate(Some(index));
            })
            .on_drag(move |a: &mut App, d: &mut rune::Drag, tx: &mut rune::Tx| a.tree_drag(index, d, tx))
            .on_secondary_press(move |a: &mut App, at: rune::PointerPress, _: &mut rune::Tx| a.open_tree_context(Some(index), [at.x, at.y]));
        if let Some(shiver) = landing { line = line.live_offset(shiver); }
        if selected {
            line = line.on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("tree-row".into(), r); });
        }
        list = list.child(line);
    }
    let mirror = app.tree_follow.scroll.clone();
    let notch = 3.0 * crate::editor::view::line_height(app);
    col().key("file-tree").w_full().h_full().fill(app.surface(p.bg))
        .border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child({
            let notes = app.sidebar == Some(crate::app::Sidebar::Notes);
            let mut header = row().w_full().h(HEADER_H * s).pl(16.0 * s).pr(8.0 * s).center_y()
                .child(text(if notes { "Notes" } else { "File Tree" }).font(app.ty.sized_bold(14.0)).color(rgb(p.fg)));
            // Notes are made here: a new note or a new folder beside the selection.
            if notes && app.notes.root.is_some() {
                let button = |key: &'static str, glyph: &'static str, folder: bool| el().key(key).w(26.0 * s).h(26.0 * s).radius(6.0 * s).center()
                    .cursor(rune::Cursor::Pointer).hover(rgb(p.fg).alpha(0.085))
                    .child(text(glyph).font(TextStyle::new(15.0 * s, 400.0).family(NERD)).color(rgb(p.dim)))
                    .on_press(move |a: &mut App| a.notes_create(folder));
                header = header.child(row().grow(1.0).justify_end().gap(2.0 * s)
                    .child(button("notes-new-note", NEW_NOTE, false))
                    .child(button("notes-new-folder", NEW_FOLDER, true)));
            }
            header
        })
        .child(col().key(SCROLL_KEY).w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: false })
            .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
            .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert(SCROLL_KEY.into(), r); })
            // The shared wheel/trackpad feel (`ui/scroll.rs`).
            .on_wheel(move |a: &mut App, e: rune::WheelEvent, tx: &mut rune::Tx| {
                let mirror = a.tree_follow.scroll.clone();
                crate::ui::scroll::handle(&mut a.scrollers, SCROLL_KEY, &e, notch, std::rc::Rc::new(move || { let (o, m, _) = mirror.get(); (o, m) }), tx)
            })
            // Right-click on the blank space under the rows: the tree's own menu.
            .on_secondary_press(|a: &mut App, at: rune::PointerPress, _: &mut rune::Tx| a.open_tree_context(None, [at.x, at.y]))
            .child(list)
            // A drop here lands in the root: the whole panel says so (`tree-root-drop`).
            .child_opt(app.tree_drag_root(notes).then(|| el().key("tree-root-drop").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .fill(rgb(p.accent).alpha(0.14)).border(s.max(1.0), rgb(p.accent)))))
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
