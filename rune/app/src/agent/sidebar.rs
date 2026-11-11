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

/// The keyboard stops above the list (`select_new_chat` / `select_projects`): Up from the first
/// row reaches the directory row, then New chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvHeader {
    NewChat,
    Projects,
}

/// One keyboard stop in the panel: a project folder (dashboard only) or a conversation.
pub enum ConvRow<'a> {
    Project { directory: String, label: String, open: bool },
    Thread { thread: &'a ThreadSummary, nested: bool },
    /// A sub-agent of the conversation above it, while it works and for a moment after.
    /// `last`: no later sibling under the same parent. `through`: the levels above it whose
    /// own line still runs on past this row to a later sibling.
    Child { member: &'a super::family::Member, root: &'a str, nested: bool, last: bool, through: Vec<usize> },
}

/// The catalog directory for a workspace root (`workspace_catalog_scope`): the home directory
/// and the filesystem root are the all-projects dashboard (`None`).
pub fn catalog_scope(root: &str) -> Option<String> {
    let root = root.trim_end_matches('/');
    let home = crate::startup::home();
    let home = home.trim_end_matches('/');
    (!root.is_empty() && root != home).then(|| root.to_owned())
}

fn project_name(directory: &str) -> String {
    if directory.is_empty() { return "Other chats".into(); }
    let trimmed = directory.trim_end_matches('/');
    trimmed.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(directory).to_owned()
}

fn header_rect_key(header: ConvHeader) -> &'static str {
    match header { ConvHeader::NewChat => "conv-header:new-chat", ConvHeader::Projects => "conv-header:projects" }
}

impl App {
    /// The dashboard (`~` or `/`): every project's conversations, grouped by folder.
    pub fn conversations_dashboard(&self) -> bool { catalog_scope(&self.cwd).is_none() }

    /// The panel's rows in order. In a project: pinned, then recent. On the dashboard: pinned
    /// from every project, then a folder per project (`~` first, then most recent activity), each
    /// expanded to its chats unless collapsed (`session_rows`).
    pub fn conversation_rows<'a>(&'a self) -> Vec<ConvRow<'a>> {
        let (pinned, recent): (Vec<&ThreadSummary>, Vec<&ThreadSummary>) = self.threads.iter().partition(|t| t.pinned);
        // A conversation's sub-agents follow it, in the family's order.
        let with_children = |rows: &mut Vec<ConvRow<'a>>, thread: &'a ThreadSummary, nested: bool| {
            rows.push(ConvRow::Thread { thread, nested });
            let children = self.families.sidebar_children(&thread.session_id);
            // A level's line runs on while a later row sits at that level before the family
            // climbs back above it.
            let runs_on = |from: usize, level: usize| children[from + 1..].iter().take_while(|m| m.depth >= level).any(|m| m.depth == level);
            for (i, member) in children.iter().enumerate() {
                let through = (1..member.depth).filter(|level| runs_on(i, *level)).collect();
                rows.push(ConvRow::Child { member, root: &thread.session_id, nested, last: !runs_on(i, member.depth), through });
            }
        };
        if !self.conversations_dashboard() {
            let mut rows = Vec::new();
            for thread in pinned.into_iter().chain(recent) { with_children(&mut rows, thread, false); }
            return rows;
        }
        let home = crate::startup::home();
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
        let mut rows: Vec<ConvRow> = Vec::new();
        for thread in pinned { with_children(&mut rows, thread, false); }
        for (directory, mut entries) in groups {
            let open = !self.collapsed_projects.contains(&directory);
            let label = if !home.is_empty() && directory == home { "~".to_owned() }
                else if names.get(&project_name(&directory)).copied().unwrap_or(0) > 1 { directory.clone() }
                else { project_name(&directory) };
            rows.push(ConvRow::Project { directory, label, open });
            if open {
                entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                for thread in entries { with_children(&mut rows, thread, true); }
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

    /// Step through New chat, the directory row and the list (`select_next` / `select_prev`):
    /// the header stops sit above the first row; the list never wraps into them by itself.
    fn conversation_move(&mut self, delta: isize) {
        let count = self.conversation_rows().len();
        match self.conv_header {
            Some(ConvHeader::NewChat) if delta > 0 => { self.conv_header = Some(ConvHeader::Projects); return; }
            Some(ConvHeader::Projects) if delta > 0 => {
                if count == 0 { return; }
                self.conv_header = None;
                self.conv_selected = 0;
                self.conv_follow.request();
                return;
            }
            Some(ConvHeader::Projects) if delta < 0 => { self.conv_header = Some(ConvHeader::NewChat); return; }
            Some(_) => return,
            None if delta < 0 && (count == 0 || self.conv_selected == 0) => { self.conv_header = Some(ConvHeader::Projects); return; }
            None => {}
        }
        if count == 0 { return; }
        // A long jump up (Home, gg, Ctrl+U) stops at the first row rather than the header.
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
        match self.conv_header {
            Some(ConvHeader::NewChat) => {
                self.open_tab(TabKind::Agent { session: None });
                self.focus = crate::app::Focus::Content;
                return;
            }
            Some(ConvHeader::Projects) => return self.open_project_menu(),
            None => {}
        }
        enum Pick { Project(String), Thread(String), Child(String, String) }
        let pick = match self.conversation_rows().get(self.conv_selected) {
            Some(ConvRow::Project { directory, .. }) => Pick::Project(directory.clone()),
            Some(ConvRow::Thread { thread, .. }) => Pick::Thread(thread.session_id.clone()),
            Some(ConvRow::Child { member, root, .. }) => Pick::Child((*root).to_owned(), member.id.clone()),
            None => return,
        };
        match pick {
            Pick::Project(directory) => self.toggle_project(&directory),
            Pick::Thread(id) => self.open_conversation(&id),
            Pick::Child(root, child) => self.open_child(&root, &child),
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
        let r = match self.conv_header {
            // Header rows sit outside the scrolled list: their rects are window-space.
            Some(header) => *self.rects.get(header_rect_key(header))?,
            None => {
                let row = self.rects.get(&format!("conv:{}", self.conv_selected))?;
                [row[0], row[1] - self.conv_follow.scroll.get().0, row[2], row[3]]
            }
        };
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
        let keyboard = focused && app.conv_header.is_none() && index == app.conv_selected;
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
        // A title cut short slides to show the rest while the pointer is on its row.
        let (title_key, title_w) = (format!("thread:{}", t.session_id), crate::ui::marquee::natural(&t.title, app.ty.body()));
        let open = matches!(&app.active_tab().kind, TabKind::Agent { session: Some(s) } if *s == t.session_id);
        let keyboard = focused && app.conv_header.is_none() && index == app.conv_selected;
        // Chats under a dashboard folder sit one 18 pt step in.
        let item = col().key(format!("thread:{}", t.session_id)).w_full().pl(17.0 + if nested { 18.0 * app.ty.scale() } else { 0.0 }).pr(17.0).py(6.0).gap(3.0);
        let item = keyboard_row(index, item);
        // A conversation at work turns the loader in the row's trailing slot: the theme's ink
        // while it runs, red for background work, yellow while it waits on a permission.
        let activity = app.activity_of(t);
        let busy = match activity { "running" => Some(p.fg), "background" => Some(p.red), "permission" => Some(p.yellow), _ => None };
        item.fill(if keyboard { rgb(p.hover) } else if open { rgb(p.surface).alpha(0.6) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover))
            .child(row().w_full().center_y().gap(8.0)
                .child(crate::ui::neoism_mark(app, app.ty.small().size, &format!("thread-mark:{}", t.session_id)))
                .child(text("Neoism").font(app.ty.small()).color(rgb(p.dim)).grow(1.0))
                .child(text(ago(t.updated_at)).font(app.ty.small()).color(rgb(p.muted)).mr(if busy.is_some() { 20.0 } else { 46.0 }))
                .children(busy.map(|color| loader(app, color).mr(8.0))))
            .child(app.title(&title_key, t.title.clone(), app.ty.body(), rgb(p.fg), Some(240.0)))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut rune::Tx| a.title_hover(&title_key, title_w, inside))
            .on_press(move |s: &mut App| s.open_conversation(&id))
    };
    // A sub-agent under its conversation (`render_subagent_row`): one 24 pt step in per level,
    // joined to the row above by a line that turns in to its title; the last one ends the line.
    let child_row = |index: usize, m: &super::family::Member, root: &str, nested: bool, last: bool, through: &[usize]| {
        let s = app.ty.scale();
        let (root, id) = (root.to_owned(), m.id.clone());
        let viewed = matches!(&app.active_tab().kind, TabKind::Agent { session: Some(session) } if *session == m.id);
        let keyboard = focused && app.conv_header.is_none() && index == app.conv_selected;
        let base = 17.0 + if nested { 18.0 * s } else { 0.0 };
        let indent = base + 24.0 * s * m.depth as f32;
        // The connector: down from the row above to this row's elbow, then across to the
        // title; on down to the next sibling unless this is the last. A level above whose
        // line has further to go passes straight through.
        let (stroke, elbow, radius) = ((2.0 * s).round().max(2.0), 29.5 * s, 5.0 * s);
        let line_x = |depth: usize| base + 24.0 * s * depth as f32 - 19.0 * s;
        let ink = rgb(p.fg).alpha(0.55);
        let mut lines = vec![el().key("family-line").w(13.0 * s).h(elbow).absolute(line_x(m.depth), 0.0)
            .border_sides([0.0, 0.0, stroke, stroke], ink).corners([0.0, 0.0, 0.0, radius])];
        if !last { lines.push(el().key("family-line-on").w(stroke).pin(Some(line_x(m.depth)), Some(elbow - radius), None, Some(0.0)).fill(ink)); }
        for level in through {
            lines.push(el().key(format!("family-line-through:{level}")).w(stroke).pin(Some(line_x(*level)), Some(0.0), None, Some(0.0)).fill(ink));
        }
        let (label, dot) = match m.status.as_str() {
            "running" => ("working", None),
            "blocked" => ("needs approval", Some(p.yellow)),
            "failed" => ("stopped", Some(p.red)),
            _ => ("done", Some(p.green)),
        };
        let mark = match app.tools.icons.agent(m.agent.as_deref()) {
            Some(image) => el().key(format!("child-mark:{}", m.id)).w(app.ty.small().size).h(app.ty.small().size).pixels(image.pixels()),
            None => crate::ui::neoism_mark(app, app.ty.small().size, &format!("child-mark:{}", m.id)),
        };
        let (title_key, title_w) = (format!("child:{}", m.id), crate::ui::marquee::natural(&m.title, app.ty.body()));
        let status = match dot {
            None => loader(app, p.fg),
            Some(color) => el().w(10.0 * s).h(10.0 * s).center().child(el().w(7.0 * s).h(7.0 * s).radius(3.5 * s).fill(rgb(color))),
        };
        let item = col().key(format!("child:{}", m.id)).w_full().pl(indent).pr(17.0).py(6.0).gap(3.0);
        keyboard_row(index, item)
            .fill(if keyboard { rgb(p.hover) } else if viewed { rgb(p.surface).alpha(0.6) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover))
            .children(lines)
            .child(row().w_full().center_y().gap(8.0)
                .child(mark)
                .child(text(m.agent.clone().unwrap_or_else(|| "Sub-agent".into())).font(app.ty.small()).color(rgb(p.dim)).grow(1.0).truncate().shrink())
                .child(text(label).font(app.ty.small()).color(rgb(p.muted)).mr(20.0))
                .child(status.mr(8.0)))
            .child(app.title(&title_key, m.title.clone(), app.ty.body(), rgb(if viewed { p.accent } else { p.fg }), Some(240.0)))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut rune::Tx| a.title_hover(&title_key, title_w, inside))
            .on_press(move |a: &mut App| a.open_child(&root, &id))
    };
    // New chat and the directory row: keyboard stops with the list's highlight, and window
    // rects for the block cursor and the dropdown's anchor.
    let header_row = |header: ConvHeader| {
        let keyboard = focused && app.conv_header == Some(header);
        row().w_full().h(36.0).px(16.0).center_y().radius(6.0)
            .fill(if keyboard || (header == ConvHeader::Projects && app.project_menu.is_some()) { rgb(p.hover) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover))
            .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(header_rect_key(header).into(), r); })
    };
    let mirror = app.conv_follow.scroll.clone();
    let notch = 3.0 * crate::editor::view::line_height(app);
    let mut list = col().w_full();
    let rows = app.conversation_rows();
    let dashboard = app.conversations_dashboard();
    // Pinned conversations lead the list, each with its sub-agents.
    let mut pinned = 0;
    for (index, row) in rows.iter().enumerate() {
        match row {
            ConvRow::Thread { thread, .. } if thread.pinned => pinned = index + 1,
            ConvRow::Child { .. } if pinned == index => pinned = index + 1,
            _ => break,
        }
    }
    if pinned > 0 { list = list.child(heading("Pinned")); }
    for (index, entry) in rows.iter().enumerate() {
        // In a project the rest are "Recent"; the dashboard's folders head their own chats.
        if index == pinned && !dashboard { list = list.child(heading("Recent")); }
        list = list.child(match entry {
            ConvRow::Project { directory, label, open } => project_row(index, directory, label, *open),
            ConvRow::Thread { thread, nested } => row_for(index, thread, *nested),
            ConvRow::Child { member, root, nested, last, through } => child_row(index, member, root, *nested, *last, through),
        });
    }
    if pinned == rows.len() && !dashboard { list = list.child(heading("Recent")); }
    col().key("sidebar").h_full().fill(app.surface(p.bg)).border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child(row().key("sidebar-heading").w_full().h(38.5).px(16.0).center_y()
            .border_sides([0.0, 0.0, 1.5, 0.0], rgb(p.border))
            .child(text("Agent").font(app.ty.body()).color(rgb(p.fg))))
        .child(col().w_full().px(18.0).pt(12.0).gap(4.0)
            .child(header_row(ConvHeader::NewChat).key("new-chat").gap(16.0)
                .child(icon(app, glyph::EDIT, 0.9, rgb(p.fg)))
                .child(text("New chat").font(app.ty.body()).color(rgb(p.fg)))
                .on_press(|s: &mut App| { s.open_tab(TabKind::Agent { session: None }); s.focus = crate::app::Focus::Content; }))
            .child(header_row(ConvHeader::Projects).key("project-directory").gap(16.0)
                .child(icon(app, glyph::FOLDER, 0.9, rgb(p.folder)))
                .child(text(app.workspace_name.clone()).font(app.ty.body()).color(rgb(p.fg)).grow(1.0).truncate().shrink())
                .child(icon(app, glyph::CHEVRON_DOWN, 0.8, rgb(p.dim)))
                .on_press(|s: &mut App| {
                    s.focus = crate::app::Focus::Sidebar;
                    s.conv_header = Some(ConvHeader::Projects);
                    if s.project_menu.is_some() { s.close_project_menu(); } else { s.open_project_menu(); }
                }))
            .child(el().w_full().h(1.0).mt(6.0).fill(rgb(p.border))))
        .child(col().key(LIST_KEY).w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: false })
            .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1], pos.viewport[1])))
            .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert(LIST_KEY.into(), r); })
            // The shared wheel/trackpad feel (`ui/scroll.rs`).
            .on_wheel(move |a: &mut App, e: rune::WheelEvent, tx: &mut rune::Tx| {
                let mirror = a.conv_follow.scroll.clone();
                crate::ui::scroll::handle(&mut a.scrollers, LIST_KEY, &e, notch, std::rc::Rc::new(move || { let (o, m, _) = mirror.get(); (o, m) }), tx)
            })
            .child(list))
}

/// The four-square orbit a running conversation turns (`draw_status_spinner`): the same live
/// squares the tool cards ride (`tools::Spinner`), so every loader is in step and none commits.
fn loader(app: &App, color: u32) -> El {
    let s = app.ty.scale();
    let mut spinner = el().w(10.0 * s).h(10.0 * s);
    for (square, alpha) in app.tools.spinner.squares() {
        spinner = spinner.child(el().w(2.0 * s).h(2.0 * s).fill(rgb(color).alpha(alpha)).absolute(4.0 * s, 4.0 * s).live_offset(square));
    }
    spinner
}

impl App {
    /// Open a sub-agent in its conversation's own tab, in place of the conversation: the tab
    /// that shows the family, else a new one. The sub-agent already showing goes back to the
    /// conversation (`openThread` on the viewed child).
    pub fn open_child(&mut self, root: &str, child: &str) {
        let family = |kind: &TabKind| matches!(kind, TabKind::Agent { session: Some(id) } if self.families.root_of(id) == root || id == root);
        let tab = self.tabs.iter().find(|t| family(&t.kind)).map(|t| t.id);
        match tab {
            Some(id) => self.select_tab(id),
            None => self.open_conversation(root),
        }
        let viewing = matches!(&self.tabs[self.active].kind, TabKind::Agent { session: Some(id) } if id == child);
        self.view_session(if viewing { root } else { child });
        self.focus = crate::app::Focus::Content;
    }
}

#[cfg(test)]
mod family_tests {
    use super::*;
    use neoism_protocol::agent::AgentServerMessage as M;

    fn family(revision: u64, statuses: [&str; 4]) -> M {
        let branch = |id: &str, parent: &str, status: &str| serde_json::json!({"sessionId": id, "parentSessionId": parent, "status": status, "title": id, "agent": "explore"});
        M::CatalogFamily { session_id: "root".into(), snapshot: serde_json::json!({"rootSessionId": "root", "familyRevision": revision, "branches": [
            branch("a", "root", statuses[0]), branch("a1", "a", statuses[1]), branch("b", "root", statuses[2]), branch("c", "root", statuses[3]),
        ]}) }
    }

    #[test]
    fn sub_agents_at_work_are_listed_under_their_conversation_on_their_lines() {
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        app.load_fixture("other", "Other", "[]");
        app.apply_agent(family(1, ["outstanding"; 4]));
        let shape = |app: &App| app.conversation_rows().iter().map(|row| match row {
            ConvRow::Thread { thread, .. } => thread.session_id.clone(),
            ConvRow::Child { member, last, through, .. } => format!("{}:{}{}{:?}", member.id, member.depth, if *last { "L" } else { "-" }, through),
            ConvRow::Project { .. } => "project".into(),
        }).collect::<Vec<_>>();
        // `a1` is the last under `a`, while `a`'s own level runs on through it to `b` and `c`.
        assert_eq!(shape(&app), ["root", "a:1-[]", "a1:2L[1]", "b:1-[]", "c:1L[]", "other"]);
        // One finishes: it lingers; the catalog's word for the root drives its loader.
        app.apply_agent(family(2, ["outstanding", "outstanding", "completed", "outstanding"]));
        assert_eq!(shape(&app).len(), 6, "a sub-agent seen to finish stays a moment");
        assert!(app.families.next_expiry().is_some());
        app.apply_agent(M::CatalogActivity { session_id: "root".into(), activity: "running".into() });
        assert_eq!(app.activity_of(&app.threads[0]), "running");
        assert_eq!(app.activity_of(&app.threads[1]), "idle");
        assert!(app.sidebar_running());
    }

    #[test]
    fn a_sub_agent_that_stops_is_done_in_the_sidebar_and_on_its_task_card() {
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        app.apply_agent(family(1, ["outstanding"; 4]));
        // The card that launched `b`, still turning.
        let card: neoism_protocol::agent::HistoryMessage = serde_json::from_value(serde_json::json!({
            "id": "card-b", "role": "Assistant", "kind": "Tool", "tool": "task", "title": "Explore", "status": "running", "text": "", "detail": "task_id: b"})).unwrap();
        app.apply_agent(M::MessageUpdated { session_id: "root".into(), message: card, parent_message_id: None });
        assert!(app.families.working_now("b"));
        // While it works, its conversation shows the activity line though no turn of its own
        // was announced; an old child named only by a card never does.
        app.open_child("root", "b");
        assert_eq!(app.activity_state("b"), Some(crate::agent::activity::State::Generating));
        // Its session goes idle before any family snapshot says it finished.
        app.apply_agent(M::SessionIdle { session_id: "b".into() });
        assert!(!app.families.working_now("b"));
        assert_eq!(app.families.member("b").unwrap().status, "completed");
        assert_eq!(app.sessions["root"].messages[0].status, "completed", "its task card stops turning");
        assert_eq!(app.activity_state("b"), None);
        // Background tasks of the family show their own line, with or without a turn.
        app.apply_agent(M::BackgroundTasksUpdated { session_id: "root".into(), epoch: "jobs".into(), revision: 1,
            tasks: vec![neoism_protocol::agent::BackgroundJobRuntime { job_id: "job".into(), session_id: "root".into(), started_at: 1 }] });
        assert_eq!(app.activity_of(&app.threads[0]), "background");
        assert_eq!(app.activity_state("root"), Some(crate::agent::activity::State::Background));
        app.apply_agent(M::BackgroundTasksUpdated { session_id: "root".into(), epoch: "jobs".into(), revision: 2, tasks: vec![] });
        assert_eq!(app.background_count("root"), 0);
        // The catalog starting over forgets what it said was running.
        app.apply_agent(M::CatalogActivity { session_id: "root".into(), activity: "running".into() });
        app.apply_agent(M::CatalogReset { directory: None });
        assert_eq!(app.activity_of(&app.threads[0]), "idle");
        assert!(!app.families.working_now("a"));
    }

    /// The order a real background sub-agent's events arrive in (traced against the daemon):
    /// the turn that launched it ends first, it finishes later, the conversation's next
    /// execution no longer lists it, and every later update or history page still carries the
    /// launching card as `running`.
    #[test]
    fn runtime_outstanding_keeps_a_background_child_live_and_its_trace_visible() {
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        app.apply_agent(family(1, ["outstanding"; 4]));
        let runtime = |revision, status: &str| M::RuntimeSnapshot {
            session_id: "root".into(),
            snapshot: serde_json::from_value(serde_json::json!({
                "rootSessionId": "root", "familyRevision": revision,
                "branchesAuthoritative": true,
                "branches": [{"sessionId": "a", "parentSessionId": "root", "status": status}]
            })).unwrap(),
        };
        app.apply_agent(runtime(1, "outstanding"));
        app.apply_agent(M::SessionIdle { session_id: "root".into() });
        assert!(app.families.working_now("a"));
        assert_eq!(app.families.stopped("a"), None);
        app.open_child("root", "a");
        assert!(app.session_live("a"));
        assert_eq!(app.sessions["a"].live_from, Some(0));
        app.apply_agent(M::ContentDelta {
            session_id: "a".into(), message_id: "answer".into(), parent_message_id: None,
            kind: neoism_protocol::agent::ContentKind::Text, text: "Exploring the project".into(),
        });
        assert_eq!(app.sessions["a"].messages[0].text, "Exploring the project");
        app.apply_agent(runtime(2, "completed"));
        assert!(!app.session_live("a"));
        app.apply_agent(runtime(1, "outstanding"));
        assert!(!app.session_live("a"));
    }

    #[test]
    fn a_background_sub_agent_that_finished_stays_finished() {
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        let card = || -> neoism_protocol::agent::HistoryMessage { serde_json::from_value(serde_json::json!({
            "id": "card", "role": "Assistant", "kind": "Tool", "tool": "task", "title": "Task(@explore · Count files)", "status": "running",
            "text": "{\"background\":true}", "detail": "task_id: kid (use this to check or continue the subagent)"})).unwrap() };
        let snapshot = |revision: u64, status: Option<&str>| M::CatalogFamily { session_id: "root".into(), snapshot: serde_json::json!({
            "rootSessionId": "root", "familyRevision": revision, "executionId": if revision < 4 { "one" } else { "two" },
            "branches": status.map(|status| serde_json::json!({"sessionId": "kid", "parentSessionId": "root", "title": "Count files (@explore subagent)", "agent": "explore", "status": status})).into_iter().collect::<Vec<_>>()}) };
        let page = |app: &mut App| app.apply_agent(M::HistoryChunk { session_id: "root".into(), messages: vec![card()], next_cursor: None });
        let child_rows = |app: &App| app.conversation_rows().len() - 1;

        app.apply_agent(M::StreamingState { session_id: "root".into(), state: neoism_protocol::agent::StreamingState::Working, label: None });
        app.apply_agent(M::MessageUpdated { session_id: "root".into(), message: card(), parent_message_id: None });
        assert_eq!(app.sessions["root"].messages[0].status, "running", "launched, not named by the family yet");
        app.apply_agent(snapshot(2, Some("outstanding")));
        // The launching turn ends; its page is fetched again while the child works on.
        app.apply_agent(M::SessionIdle { session_id: "root".into() });
        page(&mut app);
        assert!(app.families.working_now("kid"));
        assert_eq!(app.sessions["root"].messages[0].status, "running");
        assert_eq!(child_rows(&app), 1);

        // The child goes idle, and a snapshot of the revision before still calls it at work.
        app.apply_agent(M::SessionIdle { session_id: "kid".into() });
        app.apply_agent(snapshot(2, Some("outstanding")));
        assert!(!app.families.working_now("kid"), "a repeated older snapshot does not start it again");
        app.apply_agent(snapshot(3, Some("completed")));
        assert_eq!(app.sessions["root"].messages[0].status, "completed");
        // The parent answers in a new execution that lists no children, and its page comes again.
        app.apply_agent(snapshot(4, None));
        app.apply_agent(M::StreamingState { session_id: "root".into(), state: neoism_protocol::agent::StreamingState::Working, label: None });
        app.apply_agent(M::MessageUpdated { session_id: "root".into(), message: card(), parent_message_id: None });
        app.apply_agent(M::SessionIdle { session_id: "root".into() });
        page(&mut app);
        assert!(!app.families.working_now("kid"));
        let kid = app.families.member("kid").unwrap();
        assert_eq!((kid.status.as_str(), kid.title.as_str(), kid.agent.as_deref()), ("completed", "Count files (@explore subagent)", Some("explore")));
        assert_eq!(app.sessions["root"].messages[0].status, "completed", "the card the server never settles");
        assert_eq!(app.activity_state("root"), None);
        // Run again by a later execution, it is at work again.
        app.apply_agent(snapshot(5, Some("outstanding")));
        assert!(app.families.working_now("kid"));
    }

    /// A conversation opened later: its old cards still say `running`, and nothing is.
    #[test]
    fn an_old_background_task_card_is_not_running() {
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        let card: neoism_protocol::agent::HistoryMessage = serde_json::from_value(serde_json::json!({
            "id": "card", "role": "Assistant", "kind": "Tool", "tool": "task", "title": "Task(@explore · Count files)", "status": "running", "text": "", "detail": "task_id: kid"})).unwrap();
        app.apply_agent(M::HistoryChunk { session_id: "root".into(), messages: vec![card], next_cursor: None });
        assert_eq!(app.sessions["root"].messages[0].status, "completed");
        assert_eq!(app.conversation_rows().len(), 1);
    }

    #[test]
    fn one_tab_shows_a_conversation_and_its_sub_agents() {
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        app.apply_agent(family(1, ["outstanding"; 4]));
        app.open_conversation("root");
        let tabs = app.tabs.len();
        app.open_child("root", "a");
        // Back to the conversation from the sidebar: the same tab, not a second one.
        app.open_conversation("root");
        assert_eq!(app.tabs.len(), tabs);
        assert_eq!(app.active_tab().kind, TabKind::Agent { session: Some("root".into()) });
        app.open_child("root", "c");
        app.open_child("root", "a1");
        assert_eq!(app.tabs.len(), tabs);
        assert_eq!(app.active_tab().kind, TabKind::Agent { session: Some("a1".into()) });
    }

    #[test]
    fn a_child_only_named_by_an_old_task_card_is_not_listed() {
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        let card: neoism_protocol::agent::HistoryMessage = serde_json::from_value(serde_json::json!({
            "id": "t", "role": "Assistant", "kind": "Tool", "tool": "task", "title": "Explore", "text": "", "detail": "task_id: old-child"})).unwrap();
        assert!(app.families.seed_from_history("root", &[card]));
        assert_eq!(app.families.roster("root").len(), 1, "it can still be opened from its card");
        assert_eq!(app.conversation_rows().len(), 1, "but it was never seen at work");
    }

    #[test]
    fn opening_a_sub_agent_reuses_its_conversations_tab_and_clicking_it_again_goes_back() {
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        app.apply_agent(family(1, ["outstanding"; 4]));
        app.open_conversation("root");
        let tabs = app.tabs.len();
        app.open_child("root", "b");
        assert_eq!(app.tabs.len(), tabs, "in place, in the family's tab");
        assert_eq!(app.active_tab().kind, TabKind::Agent { session: Some("b".into()) });
        // A sub-agent still at work shows its whole trace.
        assert_eq!(app.sessions["b"].live_from, Some(0));
        app.open_child("root", "b");
        assert_eq!(app.active_tab().kind, TabKind::Agent { session: Some("root".into()) });
        // One stream for the family, on the session it was first resumed on.
        assert_eq!(app.stream_owner.len(), 1);
    }
}
