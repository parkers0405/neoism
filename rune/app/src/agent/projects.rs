//! The conversations panel's project dropdown (GPUiX `openProjectPicker` over the shared
//! `ConversationNavigator` in Projects mode). Anchored 6 pt under the panel's directory row, as
//! wide as the row within 320–420 pt: a 40 pt "Find a project…" field over a 1 pt rule, then
//! up to six 32 pt rows. Rows (`conversation_project_rows`): the `~` dashboard, the current
//! root, the window's other workspace roots and every project that has chats (most recent
//! first), then "Add workspace…" on the dashboard. The current one carries a 4 pt dot; the
//! highlight and the list glide on the navigator's 0.12 s springs. Choosing a row re-roots the
//! workspace (the dashboard is the home directory).

use crate::app::App;
use crate::ui::theme::rgb;
use rune::{Color, El, Key, KeyEvent, Spring, TextEdit, Tx, col, el, row, text};

const ROW_H: f32 = 32.0;
const INPUT_H: f32 = 40.0;
const INPUT_FONT: f32 = 14.0;
const INPUT_PAD: f32 = 14.0;
const ROW_FONT: f32 = 13.0;
const PADDING: f32 = 4.0;
const SEPARATOR: f32 = 1.0;
const MAX_ROWS: usize = 6;
/// `file_tree::FRAME_STROKE`.
const FRAME_STROKE: f32 = 2.25;
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
    /// The project rows (`openProjectPicker`): `~`, then the current root, the window's other
    /// roots and the projects chats live in (newest first), then "Add workspace…" on `~`.
    pub fn project_rows(&self) -> Vec<ProjectRow> {
        let home = trimmed(&std::env::var("HOME").unwrap_or_default());
        let current = trimmed(&self.cwd);
        let mut rows = vec![ProjectRow {
            value: ALL_PROJECTS.into(), title: "~".into(),
            description: if home.is_empty() { "Home directory unavailable".into() } else { home.clone() },
            footer: if home.is_empty() { "Unavailable".into() } else { "Dashboard".into() },
            current: !home.is_empty() && current == home, disabled: home.is_empty(),
        }];
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
        let paths = std::iter::once(current.clone()).chain(self.window_roots()).chain(discovered.into_iter().map(|(_, d)| d));
        for path in paths {
            if path.is_empty() || path == "/" || !seen.insert(path.clone()) { continue; }
            let title = path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(&path).to_owned();
            rows.push(ProjectRow { value: format!("project:{path}"), title, description: path.clone(), footer: String::new(), current: path == current, disabled: false });
        }
        if self.conversations_dashboard() {
            rows.push(ProjectRow { value: ADD_WORKSPACE.into(), title: "Add workspace…".into(), description: "Choose a project folder".into(), footer: String::new(), current: false, disabled: false });
        }
        rows
    }

    pub fn open_project_menu(&mut self) {
        let mut query = TextEdit::new("");
        query.focused = true;
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
            if let Ok(home) = std::env::var("HOME") { self.set_workspace_root(&home); }
        } else if value == ADD_WORKSPACE {
            self.open_picker(crate::palette::Mode::Directories);
        } else if let Some(path) = value.strip_prefix("project:") {
            self.set_workspace_root(path);
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

    /// The selected row's settled rect, for the window cursor.
    pub fn project_menu_row_rect(&self) -> Option<[f32; 4]> {
        let menu = self.project_menu.as_ref()?;
        let list = self.rects.get(LIST_KEY)?;
        let row_h = ROW_H * self.ty.scale();
        let slot = menu.selected.checked_sub(menu.top)?;
        Some([list[0], list[1] + slot as f32 * row_h, list[2], row_h])
    }

    pub fn project_menu_clip(&self) -> Option<[f32; 4]> { self.rects.get(LIST_KEY).copied() }
}

/// The dropdown over the window, under the directory row.
pub fn view(app: &App) -> El {
    let Some(menu) = &app.project_menu else { return el() };
    let Some(anchor) = app.rects.get("conv-header:projects").copied() else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let stroke = (FRAME_STROKE * s).max(2.0);
    let inset = stroke + PADDING * s;
    let rows = menu.visible();
    let visible = rows.len().clamp(1, MAX_ROWS);
    let margin = 8.0 * s;
    let w = anchor[2].clamp(320.0 * s, 420.0 * s).min((app.window[0] - 2.0 * margin).max(0.0));
    let header = inset + (INPUT_H + SEPARATOR + PADDING) * s;
    let h = header + inset + ROW_H * s * visible as f32;
    let x = anchor[0].clamp(margin, (app.window[0] - margin - w).max(margin));
    let y = (anchor[1] + anchor[3] + 6.0 * s).clamp(margin, (app.window[1] - margin - h).max(margin));
    let body_w = w - 2.0 * inset;
    let row_h = ROW_H * s;

    let input = rune::text_input(&menu.query, app.ty.sized(INPUT_FONT))
        .key("project-menu-query")
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder("Find a project…", rgb(p.muted))
        .color(rgb(p.fg)).w_full()
        .on_edit(|a: &mut App| &mut a.project_menu.as_mut().expect("project menu open").query)
        .on_input_change(|a: &mut App, _: &mut Tx| { if let Some(menu) = a.project_menu.as_mut() { menu.reset_selection(); } })
        .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| a.project_menu_key(e));

    // The list: content slides to whole-row tops and the highlight glides, both on the
    // navigator's 0.12 s spring; a new query mounts both at rest.
    let reset = menu.query.text.clone();
    let mut content = col().key(format!("project-menu-content:{reset}")).w(body_w).h(rows.len().max(1) as f32 * row_h)
        .absolute(0.0, 0.0).offset(0.0, -(menu.top as f32) * row_h).offset_motion(MOTION);
    if rows.is_empty() {
        content = content.child(text("No matching projects").key("empty").font(app.ty.sized(ROW_FONT)).color(rgb(p.muted)).absolute(14.0 * s, 9.5 * s));
    } else {
        if rows.get(menu.selected).is_some_and(|r| !r.disabled) {
            content = content.child(el().key("highlight").w(body_w).h(row_h).radius(3.0 * s).fill(rgb(p.hover))
                .absolute(0.0, 0.0).offset(0.0, menu.selected as f32 * row_h).offset_motion(MOTION));
        }
        for (index, item) in rows.iter().enumerate() {
            let ink = if item.disabled { rgb(p.muted) } else { rgb(p.fg) };
            let font = app.ty.sized(ROW_FONT);
            let has_secondary = !item.description.is_empty() || !item.footer.is_empty();
            let available = body_w - 28.0 * s;
            let mut line = row().key(format!("project-row:{}", item.value)).w(body_w).h(row_h).absolute(0.0, index as f32 * row_h)
                .pl(14.0 * s).pr(14.0 * s).center_y().instant_changes()
                // The current project's dot sits in the row's left padding.
                .child(el().key("dot").w(4.0 * s).h(4.0 * s).radius(2.0 * s).absolute(5.0 * s, (row_h - 4.0 * s) / 2.0)
                    .fill(if item.current { ink } else { Color::TRANSPARENT }))
                // The title keeps its own width up to its budget (36% beside a path), so a long
                // path truncates instead of squeezing the name away (monospace: 0.6 em a char).
                .child({
                    let budget = if has_secondary { available * 0.36 } else { available };
                    let natural = item.title.chars().count() as f32 * font.size * 0.6;
                    text(item.title.clone()).key("title").font(font).color(ink).truncate().max_w(budget).min_w(natural.min(budget))
                });
            if !item.footer.is_empty() {
                line = line.child(text(item.footer.clone()).key("footer").font(font).color(rgb(p.muted)).ml(10.0 * s).truncate().shrink());
            }
            if !item.description.is_empty() {
                line = line.child(text(item.description.clone()).key("description").font(font).color(rgb(p.muted)).ml(10.0 * s).truncate().shrink());
            }
            let value = item.value.clone();
            let disabled = item.disabled;
            content = content.child(line
                .on_pointer_move(move |a: &mut App, _| {
                    if let Some(menu) = a.project_menu.as_mut().filter(|m| m.selected != index && !disabled) { menu.selected = index; }
                })
                .on_press(move |a: &mut App| if !disabled { a.activate_project(&value) }));
        }
    }
    let list = el().key(LIST_KEY).w(body_w).h(visible as f32 * row_h).clip_rounded(0.0)
        .on_layout(|a: &mut App, r: [f32; 4]| { a.rects.insert(LIST_KEY.into(), r); })
        .on_wheel(|a: &mut App, e: rune::WheelEvent, _: &mut Tx| {
            let Some(menu) = a.project_menu.as_mut() else { return false };
            let count = menu.visible().len();
            let steps = if e.dy > 0.0 { -1 } else if e.dy < 0.0 { 1 } else { 0 };
            menu.top = (menu.top as isize + steps).clamp(0, count.saturating_sub(MAX_ROWS) as isize) as usize;
            true
        })
        .child(content);

    // `draw_modal_frame_top`: the surface-coloured stroke round the top corners, the panel fill.
    let radius = 8.0 * s;
    let card = col().key("project-menu").w(w).h(h).absolute(x, y)
        .fill(rgb(p.surface)).corners([radius, radius, 0.0, 0.0]).padding(stroke)
        .on_press(|_: &mut App| {})
        .child(col().w_full().grow(1.0).fill(rgb(p.bg)).corners([(radius - stroke).max(0.0), (radius - stroke).max(0.0), 0.0, 0.0])
            .padding(PADDING * s)
            .child(row().w_full().h(INPUT_H * s).px(INPUT_PAD * s).center_y().child(input))
            .child(el().w_full().h(SEPARATOR * s).fill(rgb(p.border)))
            .child(el().h(PADDING * s))
            .child(list));
    el().key("project-menu-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press(|a: &mut App| a.close_project_menu())
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;

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
