//! `Alt+C`: the conversations sidebar (`reference/04-sidebar-conversations.png`).

use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::theme::{NERD, glyph, rgb};
use neoism_protocol::agent::ThreadSummary;
use rune::tok::TextStyle;
use rune::{Color, El, ScrollSpec, col, el, row, text};

/// The room between the conversations list's rows; a sub-agent's connector runs across it.
const ROW_GAP: f32 = 2.0;

pub(crate) fn ago(updated_ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    if updated_ms == 0 || updated_ms > now { return String::new(); }
    let s = (now - updated_ms) / 1000;
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        86400..=604799 => format!("{}d", s / 86400),
        _ => format!("{}w", s / 604800),
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
    /// The folder button: add a project.
    AddProject,
}

/// One keyboard stop in the panel: a project folder (dashboard only) or a conversation.
pub enum ConvRow<'a> {
    Project { directory: String, label: String, open: bool },
    Thread { thread: &'a ThreadSummary, nested: bool },
    /// A sub-agent of the conversation above it, while it works and for a moment after.
    /// `last`: no later sibling under the same parent. `through`: the levels above it whose
    /// own line still runs on past this row to a later sibling.
    Child { member: &'a super::family::Member, root: &'a str, nested: bool, last: bool, through: Vec<usize> },
    /// "Load more" under a project's chats: its next five.
    More { directory: String },
}

/// The catalog directory for a workspace root (`workspace_catalog_scope`): the home directory
/// and the filesystem root are the all-projects dashboard (`None`).
pub fn catalog_scope(root: &str) -> Option<String> {
    let root = root.trim_end_matches('/');
    let home = crate::startup::home();
    let home = home.trim_end_matches('/');
    (!root.is_empty() && root != home).then(|| root.to_owned())
}

pub(crate) fn project_name(directory: &str) -> String {
    if directory.is_empty() { return "Other chats".into(); }
    let trimmed = directory.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or(directory).to_owned()
}

fn header_rect_key(header: ConvHeader) -> &'static str {
    match header { ConvHeader::NewChat => "conv-header:new-chat", ConvHeader::Projects => "conv-header:projects", ConvHeader::AddProject => "conv-header:add-project" }
}

impl App {
    /// The dashboard (`~`, `/`, or the filter on "All projects"): every project's conversations,
    /// grouped by folder.
    pub fn conversations_dashboard(&self) -> bool { self.conv_all || catalog_scope(&self.cwd).is_none() }

    /// The panel's rows in order. In a project: pinned, then recent. On the dashboard: pinned
    /// from every project, then a folder per project (`~` first, then most recent activity), each
    /// expanded to its chats unless collapsed (`session_rows`).
    pub fn conversation_rows<'a>(&'a self) -> Vec<ConvRow<'a>> {
        // The search field keeps the chats whose title or project contains it.
        let needle = self.conv_query.text.trim().to_lowercase();
        let matches = |t: &&ThreadSummary| needle.is_empty() || t.title.to_lowercase().contains(&needle)
            || t.directory.as_deref().is_some_and(|d| project_name(d).to_lowercase().contains(&needle));
        let (pinned, recent): (Vec<&ThreadSummary>, Vec<&ThreadSummary>) = self.threads.iter().filter(matches).partition(|t| t.pinned);
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
            rows.push(ConvRow::Project { directory: directory.clone(), label, open });
            if open {
                entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                for thread in entries { with_children(&mut rows, thread, true); }
                if needle.is_empty() && self.catalog.more_in(&directory) { rows.push(ConvRow::More { directory }); }
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
            // Up from the first row goes to the search field.
            None if delta < 0 && (count == 0 || self.conv_selected == 0) => { self.conv_query.focused = true; return; }
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
        if self.conv_query.focused { return self.conversation_search_key(event); }
        // The header row reads left to right: search, filter, new chat.
        if let Some(header) = self.conv_header {
            if event.mods.ctrl || event.mods.alt { return false; }
            let text = match &event.key { Key::Text(t) => t.as_str(), _ => "" };
            match (&event.key, text, header) {
                (Key::Left, ..) | (_, "h", _) => match header {
                    ConvHeader::Projects => { self.conv_header = None; self.conv_query.focused = true; }
                    ConvHeader::AddProject => self.conv_header = Some(ConvHeader::Projects),
                    ConvHeader::NewChat => self.conv_header = Some(ConvHeader::AddProject),
                },
                (Key::Right | Key::Tab, ..) | (_, "l", _) => self.conv_header = Some(if header == ConvHeader::Projects { ConvHeader::AddProject } else { ConvHeader::NewChat }),
                (Key::Down, ..) | (_, "j", _) => {
                    self.conv_header = None;
                    self.conv_selected = 0;
                    self.conv_follow.request();
                }
                (Key::Enter, ..) => self.open_selected_conversation(),
                (Key::Escape, ..) => self.focus = crate::app::Focus::Content,
                (Key::Up, ..) | (_, "k", _) => {}
                _ => return false,
            }
            return true;
        }
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
                "/" => { self.conv_header = None; self.conv_query.focused = true; }
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    /// Keys while the search field has the keyboard: Down or Enter step into the results,
    /// Escape clears the query, then leaves the field. Everything else is typed.
    pub fn conversation_search_key(&mut self, event: &rune::KeyEvent) -> bool {
        use rune::Key;
        match &event.key {
            Key::Escape if !self.conv_query.text.is_empty() => { self.conv_query = rune::TextEdit::new(""); self.conv_query.focused = true; self.conv_selected = 0; }
            Key::Escape => self.conv_query.focused = false,
            Key::Down => { self.conv_query.focused = false; self.conv_header = None; self.conv_selected = 0; self.conv_follow.request(); }
            // Along the header: Tab, or Right once there is nothing to move through.
            Key::Tab => { self.conv_query.focused = false; self.conv_header = Some(ConvHeader::Projects); }
            Key::Right if self.conv_query.text.is_empty() => { self.conv_query.focused = false; self.conv_header = Some(ConvHeader::Projects); }
            Key::Enter => {
                self.conv_query.focused = false;
                self.conv_header = None;
                let first = self.conversation_rows().iter().position(|r| matches!(r, ConvRow::Thread { .. }));
                if let Some(first) = first { self.conv_selected = first; self.open_selected_conversation(); }
            }
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
            Some(ConvHeader::AddProject) => return self.open_project_add(),
            None => {}
        }
        enum Pick { Project(String), Thread(String), Child(String, String), More(String) }
        let pick = match self.conversation_rows().get(self.conv_selected) {
            Some(ConvRow::Project { directory, .. }) => Pick::Project(directory.clone()),
            Some(ConvRow::Thread { thread, .. }) => Pick::Thread(thread.session_id.clone()),
            Some(ConvRow::Child { member, root, .. }) => Pick::Child((*root).to_owned(), member.id.clone()),
            Some(ConvRow::More { directory }) => Pick::More(directory.clone()),
            None => return,
        };
        match pick {
            Pick::Project(directory) => self.toggle_project(&directory),
            Pick::Thread(id) => self.open_conversation(&id),
            Pick::Child(root, child) => self.open_child(&root, &child),
            Pick::More(directory) => self.load_more_chats(&directory),
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

    /// The window cursor in the panel: a block in the keyboard card's left padding, one title
    /// line tall; a block just left of a header button. The search field shows its
    /// own caret.
    pub fn conversation_cursor(&self) -> Option<[f32; 4]> {
        if self.conv_query.focused { return None; }
        let s = self.ty.scale();
        if let Some(header) = self.conv_header {
            // Header buttons sit outside the scrolled list: their rects are window-space.
            let r = *self.rects.get(header_rect_key(header))?;
            let (w, h) = ((14.0 * s * 0.6).max(2.0), 14.0 * s * 1.3);
            return Some([r[0] - w + 1.0 * s, r[1] + (r[3] - h) / 2.0, w, h]);
        }
        let row = self.rects.get(&format!("conv:{}", self.conv_selected))?;
        let r = [row[0], row[1] - self.conv_follow.scroll.get().0, row[2], row[3]];
        let font = 14.0 * s;
        let (w, h) = ((font * 0.6).max(2.0), font * 1.3);
        Some([r[0] + ((12.0 * s - w) / 2.0).max(0.0), r[1] + (r[3] - h) / 2.0, w, h])
    }

    /// `Alt+C`, the rail button and the splash "Agent panel" entry.
    pub fn toggle_conversations(&mut self) {
        if self.mobile {
            if self.sidebar == Some(crate::app::Sidebar::Conversations) { crate::mobile::close_history(self); }
            else { crate::mobile::open_history(self); }
            return;
        }
        // From the file or notes tree it switches to the agent panel; only the agent panel
        // itself toggles closed.
        self.sidebar = if self.sidebar == Some(crate::app::Sidebar::Conversations) { None } else { Some(crate::app::Sidebar::Conversations) };
        if self.sidebar.is_some() { self.refresh_threads(); }
    }
}

/// A square header button: one line icon, the hover surface behind it.
/// `size` is the glyph's own: the three marks fill their em differently, so each is set to
/// look the same size as its neighbours.
fn header_button(app: &App, header: ConvHeader, icon: &'static str, size: f32, active: bool) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    let keyboard = app.focus == crate::app::Focus::Sidebar && app.conv_header == Some(header) && !app.conv_query.focused;
    el().w(26.0 * s).h(26.0 * s).ml(7.0 * s).radius(7.0 * s).center()
        .fill(if active || keyboard { rgb(p.hover) } else { Color::TRANSPARENT }).hover(rgb(p.hover))
        .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(header_rect_key(header).into(), r); })
        .child(text(icon).font(app.ty.icon_sized(size)).color(rgb(if active || keyboard { p.fg } else { p.dim })))
}

/// The framework a conversation runs in: an external agent's own mark (Claude Code, Codex),
/// Neoism's for everything Neoism runs itself, whichever model it uses.
fn framework_mark(app: &App, t: &ThreadSummary, size: f32) -> El {
    t.external_provider.as_deref().and_then(|p| super::slash::provider_mark_for(app, p, size))
        .unwrap_or_else(|| crate::ui::neoism_mark(app, size, &format!("thread-mark:{}", t.session_id)))
}

/// `Alt+C` / rail sidebar: a search field with the project filter and New chat beside it, over
/// the conversations as cards (project and age on top, the title and its provider below).
pub fn conversations(app: &App) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let meta = app.ty.chat_sized(12.0);
    let title_font = TextStyle::new(14.0 * s, 600.0);
    let focused = app.focus == crate::app::Focus::Sidebar && !app.conv_query.focused;
    let keyboard_row = |index: usize, item: El| {
        // Each row's place in the scrolled content (window y plus the scroll at layout time), so
        // the follow stays exact while the list glides between layouts.
        item.on_layout(move |a: &mut App, r: [f32; 4]| {
            let offset = a.conv_follow.scroll.get().0;
            a.rects.insert(format!("conv:{index}"), [r[0], r[1] + offset, r[2], r[3]]);
        })
    };
    // A dashboard project folder: chevron, folder and name on one quiet line.
    let project_row = |index: usize, directory: &str, label: &str, open: bool| {
        let directory_owned = directory.to_owned();
        let keyboard = focused && app.conv_header.is_none() && index == app.conv_selected;
        keyboard_row(index, row().key(format!("project:{directory}")).w_full().h(30.0 * s).px(12.0 * s).mt(6.0 * s).center_y().gap(8.0 * s).radius(8.0 * s)
            .fill(if keyboard { rgb(p.hover) } else { Color::TRANSPARENT }).hover(rgb(p.hover))
            .child(text(if open { crate::ui::icons::FOLDER_OPEN } else { crate::ui::icons::FOLDER_CLOSED }).font(app.ty.icon_sized(13.0)).color(rgb(p.folder)))
            .child(text(label.to_owned()).font(TextStyle::new(12.5 * s, 500.0)).color(rgb(p.dim)).truncate().shrink().grow(1.0))
            .child(text(if open { "\u{f078}" } else { "\u{f054}" }).font(TextStyle::new(8.0 * s, 400.0).family(NERD)).color(rgb(p.muted)))
            .on_press(move |a: &mut App| a.toggle_project(&directory_owned)))
    };
    let row_for = |index: usize, t: &ThreadSummary| {
        let id = t.session_id.clone();
        // A title cut short slides to show the rest while the pointer is on its row.
        let (title_key, title_w) = (format!("thread:{}", t.session_id), crate::ui::marquee::natural(&t.title, title_font) * 0.86);
        let open = matches!(&app.active_tab().kind, TabKind::Agent { session: Some(s) }
            if *s == t.session_id || (app.mobile && app.families.root_of(s) == t.session_id));
        let keyboard = focused && app.conv_header.is_none() && index == app.conv_selected;
        let directory = t.directory.as_deref().unwrap_or_default().trim_end_matches('/');
        let home = crate::startup::home();
        let project = if directory.is_empty() { None } else if directory == home.trim_end_matches('/') { Some("~".to_owned()) } else { Some(project_name(directory)) };
        let item = col().key(format!("thread:{}", t.session_id)).w_full()
            .px(12.0 * s).py(9.0 * s).gap(5.0 * s).radius(9.0 * s);
        let item = keyboard_row(index, item);
        // A conversation at work turns the loader beside its age: the theme's ink while it
        // runs, red for background work, yellow while it waits on a permission.
        let status = if app.mobile { Status::for_conversation(app, &t.session_id, Some(app.activity_of(t))) }
            else { Status::for_session(app, &t.session_id, app.activity_of(t)) };
        item.fill(if open { rgb(p.surface).alpha(0.45) } else if keyboard { rgb(p.hover) } else { Color::TRANSPARENT })
            .hover(if open { rgb(p.surface).alpha(0.45) } else { rgb(p.hover).alpha(0.7) })
            .child(row().w_full().center_y().gap(6.0 * s)
                .child(text(project.unwrap_or_else(|| "No project".into())).font(meta).color(rgb(p.muted)).truncate().shrink().grow(1.0))
                .children(t.pinned.then(|| text(glyph::PIN).font(app.ty.icon_sized(12.0)).color(rgb(p.muted))))
                // At work (or waiting on you) the mark takes the age's place.
                .child(if status == Status::Idle { text(ago(t.updated_at)).font(meta).color(rgb(p.muted)) } else { status_mark(app, status) }))
            .child(row().w_full().center_y().gap(8.0 * s)
                .child(app.title(&title_key, t.title.clone(), title_font, rgb(p.fg), None).grow(1.0))
                .child(framework_mark(app, t, 14.0 * s)))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut rune::Tx| a.title_hover(&title_key, title_w, inside))
            .on_press({ let id = id.clone(); move |s: &mut App| s.open_conversation(&id) })
            // Right-click: pin, rename or delete the chat (`ui/context.rs`).
            .on_secondary_press(move |a: &mut App, at: rune::PointerPress, _: &mut rune::Tx| a.open_chat_context(&id, [at.x, at.y]))
    };
    // A sub-agent under its conversation (`render_subagent_row`): one 24 pt step in per level,
    // joined to the row above by a line that turns in to its title; the last one ends the line.
    let child_row = |index: usize, m: &super::family::Member, root: &str, last: bool, through: &[usize]| {
        let (root, id) = (root.to_owned(), m.id.clone());
        let viewed = matches!(&app.active_tab().kind, TabKind::Agent { session: Some(session) } if *session == m.id);
        let keyboard = focused && app.conv_header.is_none() && index == app.conv_selected;
        let base = 12.0 * s;
        let indent = base + 24.0 * s * m.depth as f32;
        // The connector: down from the row above to this row's elbow, then across to the
        // title; on down to the next sibling unless this is the last. A level above whose
        // line has further to go passes straight through.
        let (stroke, elbow, radius) = ((2.0 * s).round().max(2.0), 27.0 * s, 6.0 * s);
        let line_x = |depth: usize| base + 24.0 * s * depth as f32 - 17.0 * s;
        // One solid line: an opaque ink (translucent pieces darken where they overlap), and
        // each piece runs on through the list's gap between rows to meet the next.
        let (fg, bg) = (rgb(p.fg), rgb(p.bg));
        let ink = Color { r: bg.r + (fg.r - bg.r) * 0.5, g: bg.g + (fg.g - bg.g) * 0.5, b: bg.b + (fg.b - bg.b) * 0.5, a: 1.0 };
        let mut lines = vec![el().key("family-line").w(11.0 * s).h(elbow + ROW_GAP * s).absolute(line_x(m.depth), -ROW_GAP * s)
            .border_sides([0.0, 0.0, stroke, stroke], ink).corners([0.0, 0.0, 0.0, radius])];
        if !last { lines.push(el().key("family-line-on").w(stroke).pin(Some(line_x(m.depth)), Some(elbow - radius), None, Some(-ROW_GAP * s)).fill(ink)); }
        for level in through {
            lines.push(el().key(format!("family-line-through:{level}")).w(stroke).pin(Some(line_x(*level)), Some(-ROW_GAP * s), None, Some(-ROW_GAP * s)).fill(ink));
        }
        let (label, dot) = match m.status.as_str() {
            "running" => ("working", None),
            "blocked" => ("needs approval", Some(p.yellow)),
            "failed" => ("stopped", Some(p.red)),
            _ => ("done", Some(p.green)),
        };
        let agent = m.agent.as_deref().unwrap_or("Sub-agent");
        let mark = super::slash::harness_mark(app, agent, 13.0 * s).unwrap_or_else(|| match app.tools.icons.agent(m.agent.as_deref()) {
            Some(image) => el().key(format!("child-mark:{}", m.id)).w(13.0 * s).h(13.0 * s).pixels(image.pixels()),
            None => crate::ui::neoism_mark(app, 13.0 * s, &format!("child-mark:{}", m.id)),
        });
        let (title_key, title_w) = (format!("child:{}", m.id), crate::ui::marquee::natural(&m.title, title_font) * 0.86);
        let status = match dot {
            None => loader(app, p.fg),
            Some(color) => el().w(10.0 * s).h(10.0 * s).center().child(el().w(6.0 * s).h(6.0 * s).radius(3.0 * s).fill(rgb(color))),
        };
        let item = col().key(format!("child:{}", m.id)).w_full().pl(indent).pr(12.0 * s).py(8.0 * s).gap(4.0 * s).radius(9.0 * s);
        keyboard_row(index, item)
            .fill(if viewed { rgb(p.surface).alpha(0.45) } else if keyboard { rgb(p.hover) } else { Color::TRANSPARENT })
            .hover(if viewed { rgb(p.surface).alpha(0.45) } else { rgb(p.hover).alpha(0.7) })
            .children(lines)
            .child(row().w_full().center_y().gap(6.0 * s)
                .child(text(super::slash::harness_name(agent).unwrap_or(agent).to_owned()).font(meta).color(rgb(p.muted)).grow(1.0).truncate().shrink())
                .child(text(label).font(meta).color(rgb(p.muted)))
                .child(status))
            .child(row().w_full().center_y().gap(8.0 * s)
                .child(app.title(&title_key, m.title.clone(), title_font, rgb(if viewed { p.accent } else { p.fg }), None).grow(1.0))
                .child(mark))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut rune::Tx| a.title_hover(&title_key, title_w, inside))
            .on_press(move |a: &mut App| a.open_child(&root, &id))
    };
    // "Load more": a quiet line under a project's chats.
    let more_row = |index: usize, key: String, label: String, directory: String| {
        let keyboard = focused && app.conv_header.is_none() && index == app.conv_selected;
        keyboard_row(index, row().key(key).w_full().h(28.0 * s).px(12.0 * s).gap(6.0 * s).center_y().radius(8.0 * s)
            .fill(if keyboard { rgb(p.hover) } else { Color::TRANSPARENT }).hover(rgb(p.hover).alpha(0.7))
            .child(text(label).font(TextStyle::new(12.0 * s, 500.0)).color(rgb(p.dim)))
            .child(text("\u{f078}").font(TextStyle::new(7.0 * s, 400.0).family(NERD)).color(rgb(p.muted)))
            .on_press(move |a: &mut App| a.load_more_chats(&directory)))
    };
    let mirror = app.conv_follow.scroll.clone();
    let notch = 3.0 * crate::editor::view::line_height(app);
    let mut list = col().w_full().px(8.0 * s).pt(4.0 * s).pb(12.0 * s).gap(ROW_GAP * s);
    let rows = app.conversation_rows();
        // Pinned conversations lead the list, marked by a pin on their card rather than a heading.
    for (index, entry) in rows.iter().enumerate() {
        list = list.child(match entry {
            ConvRow::Project { directory, label, open } => project_row(index, directory, label, *open),
            ConvRow::Thread { thread, .. } => row_for(index, thread),
            ConvRow::Child { member, root, last, through, .. } => child_row(index, member, root, *last, through),
            ConvRow::More { directory } => more_row(index, format!("more:{directory}"), if app.catalog.loading(directory) { "Loading\u{2026}".into() } else { "Load more".into() }, directory.clone()),
        });
    }
    if rows.is_empty() {
        let note = if app.conv_query.text.trim().is_empty() { "No conversations yet" } else { "No matching conversations" };
        list = list.child(text(note).font(app.ty.chat_sized(13.0)).color(rgb(p.muted)).ml(12.0 * s).mt(14.0 * s));
    }
    let search = rune::text_input(&app.conv_query, app.ty.chat_sized(14.0)).key("conversation-search")
        .caret(rgb(p.fg), crate::ui::cursor::selection(app))
        .placeholder("Search", rgb(p.muted)).color(rgb(p.fg)).grow(1.0).shrink()
        .on_edit(|a: &mut App| &mut a.conv_query)
        .on_input_change(|a: &mut App, _: &mut rune::Tx| { a.conv_selected = 0; })
        .on_input_key(|a: &mut App, e: &rune::KeyEvent, tx: &mut rune::Tx| crate::keys::shortcut(a, e, tx) || a.conversation_search_key(e));
    let header = row().key("sidebar-heading").w_full().h(if app.mobile { 48.0 } else { 46.0 * s }).pl(16.0 * s).pr(10.0 * s).center_y().gap(2.0 * s)
        .child(text(glyph::SEARCH_LINE).font(app.ty.icon_sized(13.0)).color(rgb(p.muted)).mr(8.0 * s))
        .child(search);
    let header = header
        .child(header_button(app, ConvHeader::Projects, glyph::FILTER, 15.0, app.project_menu.is_some()).key("project-directory")
            .on_press(|s: &mut App| {
                s.conv_query.focused = false;
                if s.project_menu.is_some() { s.close_project_menu(); } else { s.open_project_menu(); }
            }))
        .child(header_button(app, ConvHeader::AddProject, glyph::FOLDER_PLUS_LINE, 14.5, app.project_add.is_some()).key("add-project")
            .on_press(|s: &mut App| { s.conv_query.focused = false; s.open_project_add(); }));
    let header = header
        .child(header_button(app, ConvHeader::NewChat, glyph::COMPOSE, 12.5, false).key("new-chat")
            .on_press(|s: &mut App| {
                s.conv_query.focused = false;
                if s.mobile { crate::mobile::new_chat(s); }
                else { s.open_tab(TabKind::Agent { session: None }); s.focus = crate::app::Focus::Content; }
            }));
    let sidebar = col().key("sidebar");
    let sidebar = if app.mobile { sidebar.min_h(0.0) } else { sidebar.h_full().fill(rgb(p.bg)).border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border)) };
    sidebar
        .child(header)
        .children(app.mobile.then(|| crate::mobile::device_trigger(app)))
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

/// What a conversation is doing, as its card's mark says it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status { Idle, Running, Background, Compacting, NeedsYou, Finished }

impl Status {
    fn for_session(app: &App, id: &str, activity: &str) -> Self {
        match activity {
            _ if app.sessions.get(id).is_some_and(|s| s.compacting) => Self::Compacting,
            "running" => Self::Running,
            "background" => Self::Background,
            "permission" => Self::NeedsYou,
            _ if app.finished.contains(id) => Self::Finished,
            _ => Self::Idle,
        }
    }

    pub(crate) fn for_tab(app: &App, kind: &TabKind) -> Self {
        let TabKind::Agent { session: Some(id) } = kind else { return Self::Idle };
        let activity = app.thread_activity.get(id).map(String::as_str)
            .or_else(|| app.threads.iter().find(|t| t.session_id == *id).map(|t| app.activity_of(t)));
        Self::for_conversation(app, id, activity)
    }

    fn for_conversation(app: &App, id: &str, activity: Option<&str>) -> Self {
        let activity = match activity {
            // A just-started turn or a sub-agent may only have a local session status.
            None | Some("idle") if app.session_live(id) => "running",
            None | Some("idle") if app.background_count(id) > 0 => "background",
            Some(activity) => activity,
            None => "idle",
        };
        Self::for_session(app, id, activity)
    }

    pub(crate) fn animated(self) -> bool {
        matches!(self, Self::Running | Self::Background | Self::Compacting)
    }
}

/// Four squares in a card's top right corner, in place of its age, while the conversation is
/// not idle. Waiting on you they sit still on a yellow diamond; finished and not yet opened,
/// on a white one. At work they move, each kind its own way: the
/// ink orbit while a reply runs, a red chase round the diamond for background work, a green
/// draw-in and out while it compacts. All ride the shared live squares (`tools::Spinner`).
pub(crate) fn status_mark(app: &App, status: Status) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    let (size, dot) = (11.0 * s, 2.5 * s);
    let centre = (size - dot) / 2.0;
    let mut mark = el().w(size).h(size);
    let still = |mut mark: El, color: Color| {
        for [x, y] in super::tools::diamond(3.5 * s) { mark = mark.child(el().w(dot).h(dot).radius(0.5 * s).fill(color).absolute(centre + x, centre + y)); }
        mark
    };
    let live = |mut mark: El, color: u32, squares: &mut dyn Iterator<Item = (&rune::Live<[f32; 2]>, f32)>| {
        for (square, alpha) in squares { mark = mark.child(el().w(dot).h(dot).radius(0.5 * s).fill(rgb(color).alpha(alpha)).absolute(centre, centre).live_offset(square)); }
        mark
    };
    mark = match status {
        Status::Idle => mark,
        Status::NeedsYou => still(mark, rgb(p.yellow)),
        Status::Finished => still(mark, rgb(p.fg)),
        Status::Running => live(mark, p.fg, &mut app.tools.spinner.squares()),
        Status::Background => live(mark, p.red, &mut app.tools.spinner.chase()),
        Status::Compacting => live(mark, p.green, &mut app.tools.spinner.squeeze()),
    };
    mark
}

/// The four-square orbit a running conversation turns (`draw_status_spinner`): the same live
/// squares the tool cards ride (`tools::Spinner`), so every loader is in step and none commits.
pub(crate) fn loader(app: &App, color: u32) -> El {
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
        crate::mobile::select_chat_from_history(self);
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

    /// How many `kind` messages for `session` went to the agent.
    fn sent(app: &App, kind: &str, session: &str) -> usize {
        app.agent_sent.borrow().iter().filter(|m| m.contains(kind) && m.contains(&format!("\"{session}\""))).count()
    }

    /// The daemon binds a stream to a session on `SwitchThread` and on `ResumeStream`, and a
    /// conversation's stream already carries its sub-agents: a second one doubled every word
    /// of a sub-agent's reply while it streamed.
    #[test]
    fn a_sub_agent_of_a_followed_conversation_gets_no_stream_of_its_own() {
        let mut app = App::new();
        app.open_conversation("root");
        assert_eq!((sent(&app, "SwitchThread", "root"), sent(&app, "ResumeStream", "root")), (1, 1));
        app.apply_agent(family(1, ["outstanding"; 4]));
        // Opened while it works, before its history was fetched ahead.
        app.sessions.remove("b");
        app.open_child("root", "b");
        assert_eq!(app.active_tab().kind, TabKind::Agent { session: Some("b".into()) });
        assert_eq!(sent(&app, "SwitchThread", "b") + sent(&app, "ResumeStream", "b"), 0);
        assert!(sent(&app, "GetHistory", "b") >= 1, "its history is still asked for");
        assert_eq!(sent(&app, "StopStream", "b"), 0);
    }

    #[test]
    fn a_sub_agent_followed_first_gives_way_to_its_conversation() {
        let mut app = App::new();
        // A restored tab on a sub-agent: nothing says yet whose it is.
        app.open_conversation("b");
        assert_eq!((sent(&app, "SwitchThread", "b"), sent(&app, "ResumeStream", "b")), (1, 1));
        app.apply_agent(family(1, ["outstanding"; 4]));
        assert_eq!(app.stream_owner.get("root").map(String::as_str), Some("b"), "the family's only stream so far");
        // Another sub-agent of the same conversation is not on `b`'s stream.
        app.open_conversation("c");
        assert_eq!(sent(&app, "ResumeStream", "c"), 1);
        // The conversation itself: its stream carries them all, theirs stop.
        app.open_conversation("root");
        assert_eq!(sent(&app, "ResumeStream", "root"), 1);
        assert_eq!((sent(&app, "StopStream", "b"), sent(&app, "StopStream", "c")), (1, 1));
        assert_eq!(app.stream_owner.get("root").map(String::as_str), Some("root"));
        assert!(app.stream_extra.is_empty());
    }

    #[test]
    fn completed_follow_up_tool_does_not_stop_the_running_child() {
        let mut app = App::new();
        app.load_fixture("root", "Parent", "[]");
        app.apply_agent(M::CatalogFamily { session_id: "root".into(), snapshot: serde_json::json!({
            "rootSessionId":"root", "familyRevision":1,
            "branches":[{"sessionId":"child", "parentSessionId":"root", "status":"outstanding"}]
        }) });
        app.open_child("root", "child");
        for status in ["completed", "error"] {
            app.apply_agent(M::MessageUpdated { session_id: "root".into(), parent_message_id: None,
                message: serde_json::from_value(serde_json::json!({
                    "id":"follow-up", "role":"Assistant", "kind":"Tool", "tool":"task",
                    "status":status, "text":"Follow-up queued", "detail":"task_id: child"
                })).unwrap() });
            assert!(app.families.working_now("child"));
            assert!(app.families.sidebar_children("root").iter().any(|m| m.id == "child"));
            assert!(app.session_live("child"));
        }
        app.apply_agent(M::SessionIdle { session_id: "child".into() });
        assert!(app.families.working_now("child"), "a turn boundary does not finish the task");
        app.apply_agent(M::CatalogFamily { session_id: "root".into(), snapshot: serde_json::json!({
            "rootSessionId":"root", "familyRevision":2,
            "branches":[{"sessionId":"child", "parentSessionId":"root", "status":"completed"}]
        }) });
        assert!(!app.families.working_now("child"));
    }

    #[test]
    fn session_completion_refreshes_sidebar_and_catalog_reset_drops_stale_row_flags() {
        let mut app = App::new();
        app.load_fixture("root", "Finished conversation", "[]");
        app.threads[0].catalog_activity = Some("running".into());
        app.threads[0].busy = true;
        app.apply_agent(M::CatalogActivity { session_id: "root".into(), activity: "running".into() });
        app.agent_sent.borrow_mut().clear();
        app.apply_agent(M::SessionIdle { session_id: "root".into() });
        app.apply_agent(M::StreamingState { session_id: "root".into(), state: neoism_protocol::agent::StreamingState::Working, label: None });
        assert_eq!(app.activity_state("root"), None, "a late verb cannot restart Crafting while catalog reconciliation is in flight");
        assert!(app.agent_sent.borrow().iter().any(|m| m.contains("ListThreads")), "completion must reconcile the independent sidebar catalog");
        let mut threads = app.threads.clone();
        threads[0].catalog_activity = Some("idle".into());
        app.apply_agent(M::ThreadList { threads, requested_cursor: None, next_cursor: None });
        assert!(!app.sidebar_running());

        app.threads[0].catalog_activity = Some("running".into());
        app.threads[0].busy = true;
        app.apply_agent(M::CatalogActivity { session_id: "root".into(), activity: "running".into() });
        app.apply_agent(M::CatalogReset { directory: None });
        assert!(!app.sidebar_running(), "reset must not fall back to a stale thread row");
        app.apply_agent(M::CatalogActivity { session_id: "root".into(), activity: "running".into() });
        assert!(app.sidebar_running(), "fresh catalog activity must still start the loader");
    }

    #[test]
    fn catalog_activity_wins_over_stale_background_counts_per_conversation() {
        let mut app = App::new();
        app.load_fixture("release", "Neoism Release", "[]");
        app.load_fixture("diagnosing", "Diagnosing frames", "[]");
        app.load_fixture("optimistic", "Show optimistic message", "[]");
        let jobs = |revision, tasks| M::BackgroundTasksUpdated {
            session_id: "release".into(), epoch: "jobs".into(), revision, tasks,
        };
        let job = neoism_protocol::agent::BackgroundJobRuntime {
            job_id: "job".into(), session_id: "release".into(), started_at: 1,
        };
        app.apply_agent(jobs(1, vec![job.clone()]));
        assert_eq!(app.activity_of(&app.threads[0]), "background", "legacy fallback");

        // A catalog refresh sees completion before the session stream catches up.
        let mut threads = app.threads.clone();
        threads[0].catalog_activity = Some("idle".into());
        threads[1].catalog_activity = Some("idle".into());
        threads[2].catalog_activity = Some("running".into());
        app.apply_agent(M::ThreadList { threads, requested_cursor: None, next_cursor: None });
        let activities = |app: &App| app.threads.iter().map(|t| app.activity_of(t).to_owned()).collect::<Vec<_>>();
        assert_eq!(activities(&app), ["idle", "idle", "running"]);
        assert_eq!(app.activity_state("release"), None, "stale background counts cannot keep the conversation activity alive either");
        assert!(!app.session_live("diagnosing"));
        assert!(app.session_live("optimistic"));

        // Foreground work takes precedence even when a background job really is running.
        for activity in ["running", "permission", "background", "idle"] {
            app.apply_agent(M::CatalogActivity { session_id: "release".into(), activity: activity.into() });
            assert_eq!(app.activity_of(&app.threads[0]), activity);
        }
        app.apply_agent(jobs(2, vec![]));
        app.apply_agent(jobs(1, vec![job]));
        assert_eq!(app.background_count("release"), 0, "an older job snapshot cannot revive completed work");
        assert_eq!(activities(&app), ["idle", "idle", "running"]);
    }

    #[test]
    fn refreshed_catalog_stops_a_completed_chat_despite_stale_tool_states() {
        let mut app = App::new();
        app.load_fixture("root", "Stash Changes and Update", "[]");
        app.apply_agent(M::CatalogActivity { session_id: "root".into(), activity: "running".into() });
        assert!(app.sidebar_running());
        assert!(app.session_live("root"));

        // The idle event was missed, but the next catalog refresh reports completion.
        let mut threads = app.threads.clone();
        threads[0].catalog_activity = Some("idle".into());
        threads[0].busy = true; // The legacy flag must not beat catalog evidence.
        app.apply_agent(M::ThreadList { threads, requested_cursor: None, next_cursor: None });
        assert_eq!(app.activity_of(&app.threads[0]), "idle");
        assert!(!app.sidebar_running());
        assert!(!app.session_live("root"));

        // A late tool update and its derived streaming verb cannot restart the turn.
        let tool = serde_json::from_value(serde_json::json!({
            "id": "tool", "role": "Assistant", "kind": "Tool", "tool": "bash", "status": "running"
        })).unwrap();
        app.apply_agent(M::MessageUpdated { session_id: "root".into(), message: tool, parent_message_id: None });
        app.apply_agent(M::StreamingState { session_id: "root".into(), state: neoism_protocol::agent::StreamingState::Working, label: None });
        assert_eq!(app.activity_state("root"), None);
        assert!(!app.sidebar_running());
        assert!(!app.session_live("root"));

        // Genuine new work still starts normally, including its timeline activity.
        app.apply_agent(M::CatalogActivity { session_id: "root".into(), activity: "running".into() });
        assert!(app.sidebar_running());
        assert!(app.session_live("root"));
        assert!(app.activity_state("root").is_some());
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
            ConvRow::More { .. } => "more".into(),
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
        // A turn ending is not the child ending; the next lifecycle snapshot settles it.
        app.apply_agent(M::SessionIdle { session_id: "b".into() });
        assert!(app.families.working_now("b"));
        app.apply_agent(family(2, ["outstanding", "outstanding", "completed", "outstanding"]));
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

    #[test]
    fn an_external_background_task_stays_outside_the_work_fold_until_the_child_stops() {
        use crate::agent::timeline::RowItem;
        let mut app = App::new();
        app.load_fixture("root", "Root", r#"[{"id":"prompt","kind":"User","role":"User","text":"Survey"}]"#);
        let message = serde_json::from_value(serde_json::json!({
            "id": "launch", "role": "Assistant", "kind": "Tool", "tool": "task", "title": "Claude survey",
            "agent_kind": "claude", "status": "completed", "detail": "task_id: b\nstatus: running"
        })).unwrap();
        app.apply_agent(M::MessageUpdated { session_id: "root".into(), message, parent_message_id: None });
        let visible = |app: &App| {
            let session = &app.sessions["root"];
            session.row_items((session.structure, session.messages.len(), usize::MAX, false, true)).contains(&RowItem::Message(1))
        };
        assert!(!visible(&app), "a launch receipt alone does not prove the child is running");
        app.apply_agent(family(1, ["outstanding"; 4]));
        assert!(visible(&app), "the child stays visible even after its parent goes idle");
        app.apply_agent(family(2, ["outstanding", "outstanding", "completed", "outstanding"]));
        assert!(!visible(&app), "the finished card folds back into Work");
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
    fn child_lifecycle_orders_both_streams_and_reopens_for_follow_up() {
        use neoism_protocol::agent::{StreamingState, SubagentStatus};
        let mut app = App::new();
        app.load_fixture("root", "Root", "[]");
        app.apply_agent(family(2, ["outstanding"; 4]));
        app.open_child("root", "a");
        app.apply_agent(M::StreamingState { session_id: "a".into(), state: StreamingState::Working, label: None });
        let runtime = |revision, status| M::RuntimeSnapshot {
            session_id: "root".into(), snapshot: serde_json::from_value(serde_json::json!({
                "rootSessionId":"root", "familyRevision":revision, "branchesAuthoritative":true,
                "branches":[{"sessionId":"a", "parentSessionId":"root", "status":status}]
            })).unwrap(),
        };
        let lifecycle = |revision, status| M::SubagentUpdate {
            session_id: "a".into(), status, title: None, agent: None, current_tool: None, started_at: None,
            parent_session_id: Some("root".into()), root_session_id: Some("root".into()),
            execution_id: None, family_revision: Some(revision),
        };
        app.apply_agent(runtime(1, "completed"));
        assert!(app.session_live("a"), "an older runtime response cannot finish a newer catalog child");
        app.apply_agent(lifecycle(3, SubagentStatus::Completed));
        assert!(!app.session_live("a"), "lifecycle completion clears the child's own streaming flag");
        assert_eq!(app.activity_state("a"), None);
        assert!(app.families.working_now("b"), "a single-child lifecycle event must not finish siblings");

        app.apply_agent(family(4, ["outstanding"; 4]));
        assert!(app.session_live("a"), "new follow-up work must leave the completed state");
        app.apply_agent(lifecycle(3, SubagentStatus::Completed));
        app.apply_agent(runtime(3, "completed"));
        assert!(app.session_live("a"), "late completion of the previous turn must not finish its follow-up");
        app.apply_agent(lifecycle(5, SubagentStatus::Completed));
        assert!(!app.session_live("a"));
        app.apply_agent(lifecycle(6, SubagentStatus::Running));
        assert!(app.session_live("a"), "a versioned follow-up event may also reopen the child");
        assert!(app.families.working_now("b"));
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

        // The child goes idle before its lifecycle completes.
        app.apply_agent(M::SessionIdle { session_id: "kid".into() });
        app.apply_agent(snapshot(2, Some("outstanding")));
        assert!(app.families.working_now("kid"));
        app.apply_agent(snapshot(3, Some("completed")));
        app.apply_agent(snapshot(2, Some("outstanding")));
        assert!(!app.families.working_now("kid"), "a repeated older snapshot does not start it again");
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
