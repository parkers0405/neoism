//! `Alt+C`: the conversations sidebar (`reference/04-sidebar-conversations.png`).

use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::icon;
use crate::ui::theme::{glyph, rgb};
use neoism_protocol::agent::ThreadSummary;
use rune::{Color, El, ScrollSpec, col, el, row, text};

fn ago(updated_ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    if updated_ms == 0 || updated_ms > now { return String::new(); }
    let s = (now - updated_ms) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        86400..=604799 => format!("{}d ago", s / 86400),
        _ => format!("{}w ago", s / 604800),
    }
}

const LIST_KEY: &str = "conversation-list";
/// Rows kept between the keyboard row and the list's edge (`side_panel::SCROLL_OFF_ROWS`).
const SCROLL_OFF_ROWS: f32 = 3.0;

impl App {
    /// Conversations in panel order: pinned, then recent.
    pub fn conversation_order(&self) -> Vec<&ThreadSummary> {
        let (pinned, recent): (Vec<&ThreadSummary>, Vec<&ThreadSummary>) = self.threads.iter().partition(|t| t.pinned);
        pinned.into_iter().chain(recent).collect()
    }

    fn conversation_move(&mut self, delta: isize) {
        let count = self.conversation_order().len();
        if count == 0 { return; }
        self.conv_selected = (self.conv_selected as isize).saturating_add(delta).clamp(0, count as isize - 1) as usize;
        self.conv_follow.request();
    }

    /// Keys while the conversations panel has focus: walk, open, leave.
    pub fn conversation_key(&mut self, event: &rune::KeyEvent) -> bool {
        use rune::Key;
        if event.mods.ctrl || event.mods.alt { return false; }
        let pending_g = std::mem::take(&mut self.pending_g);
        match &event.key {
            Key::Up => self.conversation_move(-1),
            Key::Down => self.conversation_move(1),
            Key::Home => self.conversation_move(isize::MIN / 2),
            Key::End => self.conversation_move(isize::MAX / 2),
            Key::Enter | Key::Right => self.open_selected_conversation(),
            Key::Escape => self.focus = crate::app::Focus::Content,
            Key::Text(t) => match t.as_str() {
                "k" => self.conversation_move(-1),
                "j" => self.conversation_move(1),
                "l" | "o" => self.open_selected_conversation(),
                "G" => self.conversation_move(isize::MAX / 2),
                "g" if pending_g => self.conversation_move(isize::MIN / 2),
                "g" => self.pending_g = true,
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    fn open_selected_conversation(&mut self) {
        let Some(id) = self.conversation_order().get(self.conv_selected).map(|t| t.session_id.clone()) else { return };
        self.open_conversation(&id);
    }

    /// Keep the keyboard row three rows inside the list (rows are two lines, so the margin is
    /// measured in the row's own height).
    pub fn conversation_follow(&mut self, tx: &mut rune::Tx) {
        if !self.conv_follow.pending { return; }
        let (Some(row), Some(list)) = (self.rects.get(&format!("conv:{}", self.conv_selected)).copied(), self.rects.get(LIST_KEY).copied()) else {
            self.conv_follow.pending = false;
            return;
        };
        let top = row[1] - list[1] + self.conv_follow.scroll.get().0;
        self.conv_follow.apply_span(LIST_KEY, top, row[3], SCROLL_OFF_ROWS * row[3], tx);
    }

    /// The block cursor on the keyboard row: a 0.6 em block in the row's left padding, one
    /// title line tall.
    pub fn conversation_cursor(&self) -> Option<[f32; 4]> {
        let r = self.rects.get("conv-row")?;
        let font = self.ty.body().size;
        let (w, h) = ((font * 0.6).max(2.0), font * 1.3);
        Some([r[0] + (17.0 - w) / 2.0, r[1] + (r[3] - h) / 2.0, w, h])
    }

    /// `Alt+C`, the rail button and the splash "Agent panel" entry.
    pub fn toggle_conversations(&mut self) {
        self.sidebar = if self.sidebar.is_some() { None } else { Some(crate::app::Sidebar::Conversations) };
        if self.sidebar.is_some() { self.refresh_threads(); }
    }
}

/// `Alt+C` / rail sidebar: New chat, scope, Pinned and Recent (`reference/04-sidebar-conversations.png`).
pub fn conversations(app: &App) -> El {
    let p = app.pal;
    let heading = |label: &'static str| text(label).font(app.ty.pixel(1.2)).color(rgb(p.fg)).ml(16.0).mt(22.0).mb(14.0);
    let focused = app.focus == crate::app::Focus::Sidebar;
    let row_for = |index: usize, t: &ThreadSummary| {
        let id = t.session_id.clone();
        let open = matches!(&app.active_tab().kind, TabKind::Agent { session: Some(s) } if *s == t.session_id);
        let keyboard = focused && index == app.conv_selected;
        let mut item = col().key(format!("thread:{}", t.session_id)).w_full().px(17.0).py(6.0).gap(3.0)
            .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(format!("conv:{index}"), r); });
        if keyboard { item = item.on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert("conv-row".into(), r); }); }
        item.fill(if keyboard { rgb(p.hover) } else if open { rgb(p.surface).alpha(0.6) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover))
            .child(row().w_full().center_y().gap(8.0)
                .child(icon(app, "\u{f0a1e}", 0.7, rgb(p.dim)))
                .child(text("Neoism").font(app.ty.small()).color(rgb(p.dim)).grow(1.0))
                .child(text(ago(t.updated_at)).font(app.ty.small()).color(rgb(p.muted)).mr(46.0)))
            .child(text(t.title.clone()).font(app.ty.body()).color(rgb(p.fg)).truncate().max_w(240.0))
            .on_press(move |s: &mut App| s.open_conversation(&id))
    };
    let mirror = app.conv_follow.scroll.clone();
    let mut list = col().w_full();
    let order = app.conversation_order();
    let pinned = order.iter().take_while(|t| t.pinned).count();
    if pinned > 0 { list = list.child(heading("Pinned")); }
    for (index, t) in order.iter().enumerate() {
        if index == pinned { list = list.child(heading("Recent")); }
        list = list.child(row_for(index, t));
    }
    if pinned == order.len() { list = list.child(heading("Recent")); }
    col().key("sidebar").h_full().fill(app.surface(p.bg)).border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child(row().key("sidebar-heading").w_full().h(38.5).px(16.0).center_y()
            .border_sides([0.0, 0.0, 1.5, 0.0], rgb(p.border))
            .child(text("Agent").font(app.ty.body()).color(rgb(p.fg))))
        .child(col().w_full().px(18.0).pt(12.0).gap(4.0)
            .child(row().key("new-chat").w_full().h(36.0).px(16.0).gap(16.0).center_y().radius(6.0).hover(rgb(p.hover))
                .child(icon(app, glyph::EDIT, 0.9, rgb(p.fg)))
                .child(text("New chat").font(app.ty.body()).color(rgb(p.fg)))
                .on_press(|s: &mut App| { s.open_tab(TabKind::Agent { session: None }); }))
            .child(row().w_full().h(36.0).px(16.0).gap(16.0).center_y()
                .child(icon(app, glyph::FOLDER, 0.9, rgb(p.folder)))
                .child(text(app.workspace_name.clone()).font(app.ty.body()).color(rgb(p.fg)).grow(1.0))
                .child(icon(app, glyph::CHEVRON_DOWN, 0.8, rgb(p.dim))))
            .child(el().w_full().h(1.0).mt(6.0).fill(rgb(p.border))))
        .child(col().key(LIST_KEY).w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: false })
            .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
            .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert(LIST_KEY.into(), r); })
            .child(list))
}
