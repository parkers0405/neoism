//! The conversations panel's project filter. A rounded popover anchored 6 pt under the
//! panel's filter button, 300 pt wide: a "Search projects…" field over an accent rule, then
//! up to six 46 pt rows, each a name over its path. Rows (`conversation_project_rows`): the `~` dashboard, the current
//! root, the window's other workspace roots and every project that has chats (most recent
//! first), then "Add workspace…" on the dashboard. The current one carries a check badge; the
//! highlight and the list glide on the navigator's 0.12 s springs. Choosing a row re-roots the
//! workspace (the dashboard is the home directory).

use crate::app::App;
use crate::ui::theme::{glyph, rgb};
use rune::{Color, El, Key, KeyEvent, Spring, TextEdit, Tx, col, el, row, text};

const ROW_H: f32 = 46.0;
const INPUT_H: f32 = 44.0;
const INPUT_FONT: f32 = 14.0;
const ROW_FONT: f32 = 14.0;
const PADDING: f32 = 6.0;
const SEPARATOR: f32 = 1.5;
const MAX_ROWS: usize = 6;
const WIDTH: f32 = 300.0;
const RADIUS: f32 = 12.0;
/// `LIST_SCROLL_ANIMATION_SECONDS` (0.12 s): a critically damped spring, ω = 4 / 0.12.
const MOTION: Spring = Spring::new(0.188, 0.0);
const LIST_KEY: &str = "project-menu-rows";
pub const ALL_PROJECTS: &str = "all-projects";
pub const ADD_WORKSPACE: &str = "native:add-conversation-workspace";

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectRow {
    pub value: String,
    pub title: String,
    pub description: String,
    pub footer: String,
    pub current: bool,
    pub disabled: bool,
}

pub struct ProjectMenu {
    rows: Vec<ProjectRow>,
    pub query: TextEdit,
    pub selected: usize,
    pub top: usize,
}

impl ProjectMenu {
    /// The rows the query leaves (title, path or footer contains it).
    pub fn visible(&self) -> Vec<&ProjectRow> {
        let needle = self.query.text.trim().to_lowercase();
        self.rows.iter().filter(|r| needle.is_empty()
            || [&r.title, &r.description, &r.footer].iter().any(|s| s.to_lowercase().contains(&needle))).collect()
    }

    /// The current row, else the first enabled one (`refilter`).
    fn reset_selection(&mut self) {
        let rows = self.visible();
        self.selected = rows.iter().position(|r| r.current && !r.disabled).or_else(|| rows.iter().position(|r| !r.disabled)).unwrap_or(0);
        self.top = 0;
        self.reveal();
    }

    /// Step to the next enabled row, wrapping (`step`).
    fn step(&mut self, forward: bool) {
        let rows = self.visible();
        let n = rows.len();
        if n == 0 { return; }
        for delta in 1..=n {
            let i = if forward { (self.selected + delta) % n } else { (self.selected + n - delta) % n };
            if !rows[i].disabled { self.selected = i; break; }
        }
        self.reveal();
    }

    /// Keep the selection inside the six-row window (`reveal`).
    fn reveal(&mut self) {
        let visible = self.visible().len().min(MAX_ROWS).max(1);
        if self.selected < self.top { self.top = self.selected; }
        else if self.selected >= self.top + visible { self.top = self.selected + 1 - visible; }
    }

    fn chosen(&self) -> Option<String> {
        self.visible().get(self.selected).filter(|r| !r.disabled).map(|r| r.value.clone())
    }
}

fn trimmed(path: &str) -> String { path.trim_end_matches('/').to_owned() }

impl App {
    pub(crate) fn new_chat_directory(&self) -> Option<String> {
        if self.paired_chat_device() && (self.cwd == "/" || !crate::startup::absolute(&self.cwd)) { None }
        else { Some(self.cwd.clone()) }
    }

    /// The project rows (`openProjectPicker`): `~`, then the current root, the window's other
    /// roots and the projects chats live in (newest first), then "Add workspace…" on `~`.
    pub fn project_rows(&self) -> Vec<ProjectRow> {
        let home = trimmed(&crate::startup::home());
        let current = trimmed(&self.cwd);
        let mut rows = vec![ProjectRow {
            value: ALL_PROJECTS.into(), title: "All projects".into(),
            description: String::new(),
            footer: if home.is_empty() { "Unavailable".into() } else { String::new() },
            current: self.conv_all || (!home.is_empty() && current == home), disabled: false,
        }];
        if self.paired_chat_device() && self.sidebar.is_none() {
            rows[0] = ProjectRow { value: "project:~".into(), title: "Home".into(), description: "Host's default folder".into(),
                footer: String::new(), current: self.new_chat_directory().is_none(), disabled: false };
        }
        let mut discovered: Vec<(u64, String)> = Vec::new();
        for thread in &self.threads {
            let Some(directory) = thread.directory.as_deref().map(trimmed) else { continue };
            match discovered.iter_mut().find(|(_, d)| *d == directory) {
                Some((at, _)) => *at = (*at).max(thread.updated_at),
                None => discovered.push((thread.updated_at, directory)),
            }
        }
        discovered.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut seen = std::collections::HashSet::from([home.clone()]);
        let paths = std::iter::once(current.clone()).chain(self.window_roots()).chain(discovered.into_iter().map(|(_, d)| d))
            // Projects the catalog knows of whose chats are not loaded yet.
            .chain(self.catalog.directories().iter().cloned());
        for path in paths {
            if path.is_empty() || path == "/" || (self.paired_chat_device() && !crate::startup::absolute(&path)) || !seen.insert(path.clone()) { continue; }
            let title = super::sidebar::project_name(&path);
            rows.push(ProjectRow { value: format!("project:{path}"), title, description: path.clone(), footer: String::new(), current: !self.conv_all && path == current, disabled: false });
        }
        if self.conversations_dashboard() && !self.paired_chat_device() {
            rows.push(ProjectRow { value: ADD_WORKSPACE.into(), title: "Add workspace…".into(), description: "Choose a project folder".into(), footer: String::new(), current: false, disabled: false });
        }
        rows
    }

    pub fn open_project_menu(&mut self) {
        let mut query = TextEdit::new("");
        query.focused = !cfg!(target_os = "ios");
        let mut menu = ProjectMenu { rows: self.project_rows(), query, selected: 0, top: 0 };
        menu.reset_selection();
        self.project_menu = Some(menu);
    }

    pub fn close_project_menu(&mut self) { self.project_menu = None; }

    /// Choose a row (`activateProject`): `~` re-roots at home, a project at its path; "Add
    /// workspace…" opens the directory picker (Rune has no native folder dialog yet).
    pub fn activate_project(&mut self, value: &str) {
        self.project_menu = None;
        if value == ALL_PROJECTS {
            // The filter only: the workspace stays in its own directory.
            self.conv_all = true;
        } else if value == ADD_WORKSPACE {
            self.open_file_browser(crate::browser::Mode::ChooseDirectory);
        } else if let Some(path) = value.strip_prefix("project:") {
            if self.paired_chat_device() {
                self.resolve_root(path);
                self.workspace_name = if path == "~" { "Home".into() } else { super::sidebar::project_name(path) };
                self.slash_connected();
            } else { self.change_directory(path); }
            self.conv_selected = 0;
            self.conv_header = None;
            return;
        }
        self.conv_selected = 0;
        self.conv_header = None;
        self.refresh_threads();
    }

    /// Keys while the dropdown is open; every key stops here.
    pub fn project_menu_key(&mut self, event: &KeyEvent) -> bool {
        let Some(menu) = self.project_menu.as_mut() else { return false };
        match &event.key {
            Key::Escape => self.close_project_menu(),
            Key::Up => menu.step(false),
            Key::Down | Key::Tab => menu.step(true),
            Key::Text(t) if event.mods.ctrl && t == "p" => menu.step(false),
            Key::Text(t) if event.mods.ctrl && t == "n" => menu.step(true),
            Key::Enter => {
                if let Some(value) = menu.chosen() { self.activate_project(&value); }
            }
            _ => return false,
        }
        true
    }

    pub fn project_menu_clip(&self) -> Option<[f32; 4]> { self.rects.get(LIST_KEY).copied() }
}

/// The dropdown over the window, under the filter button: a rounded popover with a search
/// field over an accent rule, then the projects, the current one badged with a check.
pub fn view(app: &App) -> El {
    let Some(menu) = &app.project_menu else { return el() };
    let anchor_key = if app.paired_chat_device() && app.sidebar.is_none() { "mobile-composer-folder" } else { "conv-header:projects" };
    let Some(anchor) = app.rects.get(anchor_key).copied() else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let pad = PADDING * s;
    let rows = menu.visible();
    let visible = rows.len().clamp(1, MAX_ROWS);
    let margin = 8.0 * s;
    let w = (WIDTH * s).min((app.window[0] - 2.0 * margin).max(0.0));
    let row_h = ROW_H * s;
    let h = 2.0 + INPUT_H * s + SEPARATOR * s + 2.0 * pad + row_h * visible as f32;
    let x = anchor[0].clamp(margin, (app.window[0] - margin - w).max(margin));
    let y = (anchor[1] + anchor[3] + 6.0 * s).clamp(margin, (app.window[1] - margin - h).max(margin));
    let body_w = w - 2.0 - 2.0 * pad;
    // The window's own background, set off by its border and shadow.
    let panel = rgb(p.bg);

    let input = rune::text_input(&menu.query, app.ty.chat_sized(INPUT_FONT))
        .key("project-menu-query")
        .caret(rgb(p.fg), crate::ui::cursor::selection(app))
        .placeholder("Search projects…", rgb(p.muted))
        .color(rgb(p.fg)).grow(1.0).shrink()
        .on_edit(|a: &mut App| &mut a.project_menu.as_mut().expect("project menu open").query)
        .on_input_change(|a: &mut App, _: &mut Tx| { if let Some(menu) = a.project_menu.as_mut() { menu.reset_selection(); } })
        .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| a.project_menu_key(e));

    // The list: content slides to whole-row tops and the highlight glides, both on the
    // navigator's 0.12 s spring; a new query mounts both at rest.
    let reset = menu.query.text.clone();
    let content_h = rows.len().max(1) as f32 * row_h;
    let mut content = col().key(format!("project-menu-content:{reset}")).w(body_w).h(content_h).min_h(content_h);
    if !app.mobile {
        content = content.absolute(0.0, 0.0).offset(0.0, -(menu.top as f32) * row_h).offset_motion(MOTION);
    }
    if rows.is_empty() {
        content = content.child(text("No matching projects").key("empty").font(app.ty.chat_sized(ROW_FONT)).color(rgb(p.muted)).absolute(10.0 * s, 9.0 * s));
    } else {
        if rows.get(menu.selected).is_some_and(|r| !r.disabled) {
            content = content.child(el().key("highlight").w(body_w).h(row_h).radius(7.0 * s).fill(rgb(p.fg).alpha(0.07))
                .absolute(0.0, 0.0).offset(0.0, menu.selected as f32 * row_h).offset_motion(MOTION));
        }
        for (index, item) in rows.iter().enumerate() {
            let ink = if item.disabled { rgb(p.muted) } else { rgb(p.fg) };
            let icon = match item.value.as_str() {
                ADD_WORKSPACE => glyph::FOLDER_PLUS_LINE,
                _ => glyph::FOLDER_LINE,
            };
            // The current project's badge: a check on the accent, ringed in the panel's colour.
            let mut mark = el().key("icon").w(18.0 * s).h(18.0 * s).center()
                .child(match item.value.as_str() {
                    ADD_WORKSPACE => text(icon).font(app.ty.icon_sized(16.0)).color(rgb(p.dim)),
                    _ => text(crate::ui::icons::FOLDER_CLOSED).font(app.ty.icon_sized(14.0)).color(rgb(p.folder)),
                });
            if item.current {
                mark = mark.child(el().key("check").w(13.0 * s).h(13.0 * s).radius(6.5 * s).fill(rgb(p.blue)).border(2.0 * s, panel)
                    .absolute(9.0 * s, 8.0 * s).center()
                    .child(text(glyph::CHECK).font(app.ty.icon_sized(7.0)).color(rgb(p.bg))));
            }
            // The name, with its path on a second line under it.
            let secondary = if item.description.is_empty() { &item.footer } else { &item.description };
            let mut words = col().grow(1.0).shrink().min_w(0.0).gap(1.0 * s)
                .child(text(item.title.clone()).key("title").font(app.ty.sans(ROW_FONT, 500.0)).color(ink).truncate());
            if !secondary.is_empty() {
                words = words.child(text(secondary.clone()).key("description").font(app.ty.chat_sized(11.5)).color(rgb(p.muted)).truncate());
            }
            let line = row().key(format!("project-row:{}", item.value)).w(body_w).h(row_h).absolute(0.0, index as f32 * row_h)
                .px(10.0 * s).gap(10.0 * s).center_y().instant_changes()
                .child(mark)
                .child(words);
            let value = item.value.clone();
            let disabled = item.disabled;
            content = content.child(line
                .on_pointer_move(move |a: &mut App, _| {
                    if let Some(menu) = a.project_menu.as_mut().filter(|m| m.selected != index && !disabled) { menu.selected = index; }
                })
                .on_press(move |a: &mut App| if !disabled { a.activate_project(&value) }));
        }
    }
    let list = col().key(LIST_KEY).w(body_w).h(visible as f32 * row_h).clip_rounded(7.0 * s)
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert(LIST_KEY.into(), r); });
    let list = if app.mobile {
        list.child(col().key(format!("project-scroll:{reset}")).w_full().h(visible as f32 * row_h)
            .scroll(rune::ScrollSpec { stick_bottom: false }).blur_after_touch_scroll().child(content))
    } else {
        list.on_wheel(|a: &mut App, e: rune::WheelEvent, _: &mut Tx| {
            let Some(menu) = a.project_menu.as_mut() else { return false };
            let count = menu.visible().len();
            let steps = if e.dy > 0.0 { -1 } else if e.dy < 0.0 { 1 } else { 0 };
            menu.top = (menu.top as isize + steps).clamp(0, count.saturating_sub(MAX_ROWS) as isize) as usize;
            true
        })
        .child(content)
    };

    let card = col().key("project-menu").w(w).h(h).absolute(x, y)
        .fill(panel).radius(RADIUS * s).border(1.0, rgb(p.fg).alpha(0.1))
        .shadow(8.0 * s, 28.0 * s, Color::BLACK.alpha(0.45))
        .on_press(|_: &mut App| {})
        .child(row().w_full().h(INPUT_H * s).px(14.0 * s).gap(9.0 * s).center_y()
            .child(text(glyph::SEARCH_LINE).font(app.ty.icon_sized(14.0)).color(rgb(p.muted)))
            .child(input))
        .child(el().h(SEPARATOR * s).mx(pad + 4.0 * s).fill(rgb(p.blue).alpha(0.75)))
        .child(el().w_full().padding(pad).child(list));
    el().key("project-menu-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press(|a: &mut App| a.close_project_menu())
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swipe_started_on_a_project_row_scrolls_the_dropdown() {
        let mut app = App::new_surface(String::new(), true);
        app.window = [390.0, 844.0];
        app.rects.insert("conv-header:projects".into(), [134.0, 110.0, 40.0, 40.0]);
        app.project_menu = Some(ProjectMenu {
            rows: std::iter::once(row(ALL_PROJECTS, false, false))
                .chain((1..16).map(|i| row(&format!("project-{i}"), false, false))).collect(),
            query: TextEdit::new(""), selected: 0, top: 0,
        });
        let mut rt = rune::Runtime::new(app, |a, _| view(a), (390.0, 844.0), rune::Env::default());
        rt.state.conv_all = false;
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let [x, y, w, _] = rt.state.project_menu_clip().unwrap();
        let [qx, qy, qw, qh] = rt.elements().iter().find(|e| e.key.as_deref() == Some("project-menu-query")).unwrap().rect;
        rt.touch(0, rune::TouchPhase::Started, qx + qw / 2.0, qy + qh / 2.0, 0.5);
        rt.touch(0, rune::TouchPhase::Ended, qx + qw / 2.0, qy + qh / 2.0, 0.55);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.project_menu.as_ref().unwrap().query.focused);
        let x = x + w / 2.0;
        let first = y + ROW_H * rt.state.ty.scale() / 2.0;
        rt.touch(1, rune::TouchPhase::Started, x, first + 100.0, 1.0);
        rt.touch(1, rune::TouchPhase::Moved, x, first, 1.1);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.project_menu.as_ref().unwrap().query.focused, "focus stays while the finger is down");
        rt.touch(1, rune::TouchPhase::Ended, x, first, 1.3);
        rt.frame(0.1);
        assert!(rt.state.project_menu.is_some(), "a swipe is not a project selection");
        assert!(!rt.state.project_menu.as_ref().unwrap().query.focused, "lifting the finger dismisses the keyboard");
        rt.touch(2, rune::TouchPhase::Started, x, first, 1.4);
        rt.touch(2, rune::TouchPhase::Ended, x, first, 1.45);
        rt.frame(0.1);
        assert!(rt.state.project_menu.is_none());
        assert!(!rt.state.conv_all, "the first visible row is no longer All projects");
    }

    fn row(value: &str, current: bool, disabled: bool) -> ProjectRow {
        ProjectRow { value: value.into(), title: value.into(), description: String::new(), footer: String::new(), current, disabled }
    }

    #[test]
    fn opens_on_the_current_row_and_skips_disabled_ones() {
        let mut menu = ProjectMenu { rows: vec![row("~", false, true), row("a", false, false), row("b", true, false)], query: TextEdit::new(""), selected: 0, top: 0 };
        menu.reset_selection();
        assert_eq!(menu.selected, 2);
        menu.step(true);
        assert_eq!(menu.selected, 1, "wraps past the disabled dashboard");
        menu.query.replace_text("zz");
        menu.reset_selection();
        assert!(menu.visible().is_empty());
    }

    #[test]
    fn rows_list_the_dashboard_current_root_and_chat_projects_once() {
        let mut app = App::new();
        app.cwd = "/work/a".into();
        app.threads = [("x", "/work/b", 5), ("y", "/work/a/", 9), ("z", "/work/c", 7)].iter().map(|(id, dir, at)| {
            serde_json::from_value(serde_json::json!({"session_id": id, "title": id, "directory": dir, "updated_at": at})).unwrap()
        }).collect();
        let rows = app.project_rows();
        let values: Vec<&str> = rows.iter().map(|r| r.value.as_str()).collect();
        assert_eq!(values[0], ALL_PROJECTS);
        assert_eq!(values[1], "project:/work/a");
        assert!(rows[1].current);
        let b = values.iter().position(|v| *v == "project:/work/b").unwrap();
        let c = values.iter().position(|v| *v == "project:/work/c").unwrap();
        assert!(c < b, "newest chat project first");
        assert_eq!(values.iter().filter(|v| **v == "project:/work/a").count(), 1);
    }
}
