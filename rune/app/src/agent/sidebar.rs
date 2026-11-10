//! `Alt+C`: the conversations sidebar (`reference/04-sidebar-conversations.png`).

use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::theme::{NERD, glyph, rgb};
use crate::ui::{icon, icons};
use neoism_protocol::agent::ThreadSummary;
use rune::tok::TextStyle;
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

/// One keyboard stop in the panel: a project folder (dashboard only) or a conversation.
pub enum ConvRow<'a> {
    Project { directory: String, label: String, open: bool },
    Thread { thread: &'a ThreadSummary, nested: bool },
}

/// The catalog directory for a workspace root (`workspace_catalog_scope`): the home directory
/// and the filesystem root are the all-projects dashboard (`None`).
pub fn catalog_scope(root: &str) -> Option<String> {
    let root = root.trim_end_matches('/');
    let home = std::env::var("HOME").unwrap_or_default();
    let home = home.trim_end_matches('/');
    (!root.is_empty() && root != home).then(|| root.to_owned())
}

fn project_name(directory: &str) -> String {
    if directory.is_empty() { return "Other chats".into(); }
    let trimmed = directory.trim_end_matches('/');
    trimmed.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(directory).to_owned()
}

impl App {
    /// The dashboard (`~` or `/`): every project's conversations, grouped by folder.
    pub fn conversations_dashboard(&self) -> bool { catalog_scope(&self.cwd).is_none() }

    /// The panel's rows in order. In a project: pinned, then recent. On the dashboard: pinned
    /// from every project, then a folder per project (`~` first, then most recent activity), each
    /// expanded to its chats unless collapsed (`session_rows`).
    pub fn conversation_rows(&self) -> Vec<ConvRow<'_>> {
        let (pinned, recent): (Vec<&ThreadSummary>, Vec<&ThreadSummary>) = self.threads.iter().partition(|t| t.pinned);
        if !self.conversations_dashboard() {
            return pinned.into_iter().chain(recent).map(|thread| ConvRow::Thread { thread, nested: false }).collect();
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let home = home.trim_end_matches('/').to_owned();
        let mut groups: Vec<(String, Vec<&ThreadSummary>)> = Vec::new();
        for thread in recent {
            let directory = thread.directory.clone().unwrap_or_default().trim_end_matches('/').to_owned();
            match groups.iter_mut().find(|(d, _)| *d == directory) {
                Some((_, entries)) => entries.push(thread),
                None => groups.push((directory, vec![thread])),
            }
        }
        let latest = |entries: &[&ThreadSummary]| entries.iter().map(|t| t.updated_at).max().unwrap_or(0);
        groups.sort_by(|(a, ea), (b, eb)| (*b == home).cmp(&(*a == home)).then(latest(eb).cmp(&latest(ea))).then(a.cmp(b)));
        let mut names: std::collections::HashMap<String, usize> = Default::default();
        for (directory, _) in &groups { *names.entry(project_name(directory)).or_default() += 1; }
        let mut rows: Vec<ConvRow> = pinned.into_iter().map(|thread| ConvRow::Thread { thread, nested: false }).collect();
        for (directory, mut entries) in groups {
            let open = !self.collapsed_projects.contains(&directory);
            let label = if !home.is_empty() && directory == home { "~".to_owned() }
                else if names.get(&project_name(&directory)).copied().unwrap_or(0) > 1 { directory.clone() }
                else { project_name(&directory) };
            rows.push(ConvRow::Project { directory, label, open });
            if open {
                entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                rows.extend(entries.into_iter().map(|thread| ConvRow::Thread { thread, nested: true }));
            }
        }
        rows
    }

    /// Conversations in panel order (the keyboard's threads).
    pub fn conversation_order(&self) -> Vec<&ThreadSummary> {
        self.conversation_rows().into_iter().filter_map(|r| match r { ConvRow::Thread { thread, .. } => Some(thread), _ => None }).collect()
    }

    /// A project folder on the dashboard opens or closes (`toggle_project_collapsed`).
    pub fn toggle_project(&mut self, directory: &str) {
        if !self.collapsed_projects.remove(directory) { self.collapsed_projects.insert(directory.to_owned()); }
    }

    fn conversation_move(&mut self, delta: isize) {
        let count = self.conversation_rows().len();
        if count == 0 { return; }
        self.conv_selected = (self.conv_selected as isize).saturating_add(delta).clamp(0, count as isize - 1) as usize;
        self.conv_follow.request();
    }

    /// Keys while the conversations panel has focus: walk, open, leave.
    pub fn conversation_key(&mut self, event: &rune::KeyEvent) -> bool {
        use rune::Key;
        // Ctrl+D / Ctrl+U move half a page (rows are two lines, measured from the first one).
        if let Some(letter @ ('d' | 'u')) = crate::keys::ctrl_letter(event) {
            let row_h = self.rects.get("conv:0").map_or(50.0, |r| r[3].max(1.0));
            let half = ((self.conv_follow.scroll.get().2 / row_h / 2.0).floor() as isize).max(1);
            self.conversation_move(if letter == 'd' { half } else { -half });
            return true;
        }
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
        enum Pick { Project(String), Thread(String) }
        let pick = match self.conversation_rows().get(self.conv_selected) {
            Some(ConvRow::Project { directory, .. }) => Pick::Project(directory.clone()),
            Some(ConvRow::Thread { thread, .. }) => Pick::Thread(thread.session_id.clone()),
            None => return,
        };
        match pick {
            Pick::Project(directory) => self.toggle_project(&directory),
            Pick::Thread(id) => self.open_conversation(&id),
        }
    }

    /// Keep the keyboard row three rows inside the list (rows are two lines, so the margin is
    /// measured in the row's own height).
    pub fn conversation_follow(&mut self, tx: &mut rune::Tx) {
        if !self.conv_follow.pending { return; }
        let (Some(row), Some(list)) = (self.rects.get(&format!("conv:{}", self.conv_selected)).copied(), self.rects.get(LIST_KEY).copied()) else {
            self.conv_follow.pending = false;
            return;
        };
        let top = row[1] - list[1];
        self.conv_follow.apply_span(LIST_KEY, top, row[3], SCROLL_OFF_ROWS * row[3], tx);
    }

    /// The block cursor on the keyboard row: a 0.6 em block in the row's left padding, one
    /// title line tall.
    pub fn conversation_cursor(&self) -> Option<[f32; 4]> {
        let row = self.rects.get(&format!("conv:{}", self.conv_selected))?;
        let r = [row[0], row[1] - self.conv_follow.scroll.get().0, row[2], row[3]];
        let font = self.ty.body().size;
        let (w, h) = ((font * 0.6).max(2.0), font * 1.3);
        Some([r[0] + (17.0 - w) / 2.0, r[1] + (r[3] - h) / 2.0, w, h])
    }

    /// `Alt+C`, the rail button and the splash "Agent panel" entry.
    pub fn toggle_conversations(&mut self) {
        // From the file or notes tree it switches to the agent panel; only the agent panel
        // itself toggles closed.
        self.sidebar = if self.sidebar == Some(crate::app::Sidebar::Conversations) { None } else { Some(crate::app::Sidebar::Conversations) };
        if self.sidebar.is_some() { self.refresh_threads(); }
    }
}

/// `Alt+C` / rail sidebar: New chat, scope, Pinned and Recent (`reference/04-sidebar-conversations.png`).
pub fn conversations(app: &App) -> El {
    let p = app.pal;
    let heading = |label: &'static str| text(label).font(app.ty.pixel(1.2)).color(rgb(p.fg)).ml(16.0).mt(22.0).mb(14.0);
    let focused = app.focus == crate::app::Focus::Sidebar;
    let keyboard_row = |index: usize, item: El| {
        // Each row's place in the scrolled content (window y plus the scroll at layout time), so
        // the cursor and follow stay exact while the list glides between layouts.
        item.on_layout(move |a: &mut App, r: [f32; 4]| {
            let offset = a.conv_follow.scroll.get().0;
            a.rects.insert(format!("conv:{index}"), [r[0], r[1] + offset, r[2], r[3]]);
        })
    };
    // A dashboard project folder (`render_project_row`): chevron in the 14 pt indent, the
    // tree's folder icon and colour, the name in the readable blue.
    let project_row = |index: usize, directory: &str, label: &str, open: bool| {
        let s = app.ty.scale();
        let directory_owned = directory.to_owned();
        let keyboard = focused && index == app.conv_selected;
        let chevron = TextStyle::new(8.0 * s, 400.0).family(NERD);
        let folder = TextStyle::new(13.0 * s, 400.0).family(NERD);
        keyboard_row(index, row().key(format!("project:{directory}")).w_full().h(30.0 * s).px(17.0).center_y()
            .fill(if keyboard { rgb(p.hover) } else { Color::TRANSPARENT }).hover(rgb(p.hover))
            .child(el().w(14.0 * s).center().child(text(if open { "\u{f078}" } else { "\u{f054}" }).font(chevron).color(rgb(p.muted))))
            .child(el().w(13.0 * s).center().child(text(if open { icons::FOLDER_OPEN } else { icons::FOLDER_CLOSED }).font(folder).color(rgb(p.folder))))
            .child(text(label.to_owned()).font(app.ty.body()).color(rgb(p.blue)).ml(8.0 * s).truncate().shrink())
            .on_press(move |a: &mut App| a.toggle_project(&directory_owned)))
    };
    let row_for = |index: usize, t: &ThreadSummary, nested: bool| {
        let id = t.session_id.clone();
        let open = matches!(&app.active_tab().kind, TabKind::Agent { session: Some(s) } if *s == t.session_id);
        let keyboard = focused && index == app.conv_selected;
        // Chats under a dashboard folder sit one 18 pt step in.
        let item = col().key(format!("thread:{}", t.session_id)).w_full().pl(17.0 + if nested { 18.0 * app.ty.scale() } else { 0.0 }).pr(17.0).py(6.0).gap(3.0);
        let item = keyboard_row(index, item);
        item.fill(if keyboard { rgb(p.hover) } else if open { rgb(p.surface).alpha(0.6) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover))
            .child(row().w_full().center_y().gap(8.0)
                .child(crate::ui::neoism_mark(app, app.ty.small().size, &format!("thread-mark:{}", t.session_id)))
                .child(text("Neoism").font(app.ty.small()).color(rgb(p.dim)).grow(1.0))
                .child(text(ago(t.updated_at)).font(app.ty.small()).color(rgb(p.muted)).mr(46.0)))
            .child(text(t.title.clone()).font(app.ty.body()).color(rgb(p.fg)).truncate().max_w(240.0))
            .on_press(move |s: &mut App| s.open_conversation(&id))
    };
    let mirror = app.conv_follow.scroll.clone();
    let mut list = col().w_full();
    let rows = app.conversation_rows();
    let dashboard = app.conversations_dashboard();
    let pinned = rows.iter().take_while(|r| matches!(r, ConvRow::Thread { thread, .. } if thread.pinned)).count();
    if pinned > 0 { list = list.child(heading("Pinned")); }
    for (index, entry) in rows.iter().enumerate() {
        // In a project the rest are "Recent"; the dashboard's folders head their own chats.
        if index == pinned && !dashboard { list = list.child(heading("Recent")); }
        list = list.child(match entry {
            ConvRow::Project { directory, label, open } => project_row(index, directory, label, *open),
            ConvRow::Thread { thread, nested } => row_for(index, thread, *nested),
        });
    }
    if pinned == rows.len() && !dashboard { list = list.child(heading("Recent")); }
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
