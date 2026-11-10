//! Window shortcuts and the chrome focus ladder (`rune/README.md`, "Keyboard focus and the cursor";
//! source of truth: `neoism-frontend/chrome-navigation.ts`).

use crate::app::{App, Focus};
use crate::panes::{Axis, Direction};
use crate::tabs::TabKind;
use rune::{Key, KeyEvent, Tx};

pub fn on_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    // The image preview is modal over everything.
    if app.image_preview_key(event) { return true; }
    if app.modal.is_some() { return crate::ui::modal::keys(app, event, tx); }
    if app.browser.is_some() { return app.browser_key(event, tx) || shortcut(app, event, tx); }
    if shortcut(app, event, tx) { return true; }
    if app.settings.is_some() { return app.settings_key(event); }
    // A full-screen program or running command in the Home pane owns the keyboard.
    if app.terminal_key(event) { return true; }
    if app.focus == Focus::Content && matches!(app.active_tab().kind, TabKind::Editor { .. }) && app.editor_key(event, tx) {
        return true;
    }
    if app.project_menu.is_some() && app.project_menu_key(event) { return true; }
    if app.focus == Focus::Sidebar && app.sidebar == Some(crate::app::Sidebar::Conversations) && app.conversation_key(event) {
        return true;
    }
    if app.focus == Focus::Sidebar && app.active_tree().is_some() && tree_key(app, event) {
        return true;
    }
    let m = event.mods;
    match (&event.key, app.focus) {
        (Key::Enter, Focus::Workspaces(index)) => {
            if index == app.workspaces.len() { app.new_workspace(); } else { app.select_workspace(index); }
            true
        }
        (Key::Enter, Focus::Buffers(index)) => {
            match app.tabs.get(index).map(|t| t.id) {
                Some(tab) => app.select_tab(tab),
                None => { app.open_tab(TabKind::Home); }
            }
            true
        }
        (Key::Left, Focus::Buffers(_) | Focus::Workspaces(_)) if !m.alt => { focus_step(app, true); true }
        (Key::Right, Focus::Buffers(_) | Focus::Workspaces(_)) if !m.alt => { focus_step(app, false); true }
        (Key::Down, Focus::Buffers(_) | Focus::Workspaces(_)) => { focus_down(app); true }
        _ => false,
    }
}

/// Window shortcuts. Text fields route their keys here first (`on_input_key`), so Alt chords
/// never type into the composer.
pub fn shortcut(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if app.modal.is_some() { return crate::ui::modal::keys(app, event, tx); }
    if app.capture_binding(event) { return true; }
    // Super+V / Super+C, as the desktop binds them (fields keep their own Ctrl chords).
    let m = event.mods;
    if m.logo && !m.ctrl && !m.alt {
        let letter = match &event.key { Key::Text(t) => t.to_lowercase(), Key::Command(c) => c.to_ascii_lowercase().to_string(), _ => String::new() };
        match letter.as_str() {
            "v" => { app.paste_chord(tx); return true; }
            "c" => { app.copy_chord(tx); return true; }
            _ => {}
        }
    }
    // Ctrl+Insert / Shift+Insert / Shift+Delete: the terminal-style chords, and what a Hyprland
    // Super+C / Super+V bind sends as `send_shortcut` (the compositor keeps the Super itself).
    match (&event.key, m.ctrl, m.shift, m.alt || m.logo) {
        (Key::Insert, true, false, false) => { app.copy_chord(tx); return true; }
        (Key::Insert, false, true, false) => { app.paste_chord(tx); return true; }
        (Key::Delete, false, true, false) if app.focused_field_mut().is_some() => { app.cut_chord(tx); return true; }
        _ => {}
    }
    // An open menu owns its navigation keys before anything else.
    if app.menu.is_some() && app.menu_key(event) { return true; }
    if app.bound_shortcut(event, tx) { return true; }
    let m = event.mods;
    let alt_only = m.alt && !m.ctrl && !m.shift && !m.logo;
    if m.alt && m.shift && !m.logo && matches!(event.key, Key::Left | Key::Right) {
        app.move_chrome_tab(event.key == Key::Right, m.ctrl || matches!(app.focus, Focus::Workspaces(_)));
        tx.animated();
        return true;
    }
    if let Key::Escape = event.key {
        if app.composer.picker.is_some() {
            if !app.picker_back() { app.close_prompt_picker(); }
            return true;
        }
        match app.focus {
            Focus::Workspaces(_) => { app.focus = Focus::Buffers(app.active); return true; }
            Focus::Buffers(_) => { app.focus = Focus::Content; return true; }
            _ => {}
        }
    }
    if m.ctrl && !m.alt && !m.logo {
        let split = match &event.key {
            Key::Tab => { app.step_tab(!m.shift); return true; }
            Key::Left if m.shift => { app.step_tab(false); return true; }
            Key::Right if m.shift => { app.step_tab(true); return true; }
            // Ctrl chords arrive as `Command` keys.
            Key::Command(c) if m.shift => match c.to_ascii_lowercase() {
                'r' => Some(Axis::Horizontal),
                'd' => Some(Axis::Vertical),
                '[' | '{' => { app.focus_split(false); return true; }
                ']' | '}' => { app.focus_split(true); return true; }
                _ => None,
            },
            _ => None,
        };
        if let Some(axis) = split { app.split(axis); return true; }
    }
    if m.ctrl && m.alt && !m.shift && !m.logo {
        if let Some(direction) = arrow(&event.key) { app.nudge_split(direction); return true; }
    }
    if alt_only {
        if let Key::Text(t) = &event.key {
            if let Ok(n) = t.parse::<usize>() { if (1..=9).contains(&n) { app.select_workspace(n - 1); return true; } }
            match t.to_lowercase().as_str() {
                "a" => { app.open_tab(TabKind::Agent { session: None }); return true; }
                "t" => { app.open_tab(TabKind::Terminal); return true; }
                "d" => { app.open_file_browser(crate::browser::Mode::ChooseDirectory); return true; }
                "c" => {
                    app.toggle_conversations();
                    return true;
                }
                "p" => { app.open_picker(crate::palette::Mode::Commands); return true; }
                "s" => { app.open_finder(crate::finder::Mode::Files); return true; }
                "e" => { app.toggle_file_tree(); return true; }
                "n" => { app.toggle_tree(crate::app::Sidebar::Notes); return true; }
                _ => {}
            }
        }
        match event.key {
            Key::Up => { focus_up(app); return true; }
            Key::Down => { focus_down(app); return true; }
            Key::Left => { focus_step(app, true); return true; }
            Key::Right => { focus_step(app, false); return true; }
            _ => {}
        }
    }
    false
}

/// The letter of a Ctrl chord. Rune delivers Ctrl chords as `Command` keys; some layouts send
/// the letter as text, or its control code (`\u{4}` for Ctrl+D).
pub fn ctrl_letter(event: &KeyEvent) -> Option<char> {
    if !event.mods.ctrl || event.mods.alt { return None; }
    let c = match &event.key {
        Key::Command(c) => *c,
        Key::Text(t) => t.chars().next()?,
        _ => return None,
    };
    Some(if (c as u32) < 0x20 { (c as u8 + b'a' - 1) as char } else { c.to_ascii_lowercase() })
}

/// Tree navigation while the file tree has focus.
fn tree_key(app: &mut App, event: &KeyEvent) -> bool {
    if event.mods.alt { return false; }
    let pending_g = std::mem::take(&mut app.pending_g);
    let count = app.tree_count.take();
    let page = app.tree_page_rows();
    // A count prefixes the next motion (`fuzzy` tree's `push_count_digit`); a leading 0 isn't one.
    if let Key::Text(t) = &event.key && let Some(d) = t.chars().next().and_then(|c| c.to_digit(10)).filter(|_| t.chars().count() == 1) && !event.mods.ctrl {
        if count.is_some() || d != 0 {
            app.tree_count = Some(count.unwrap_or(0).saturating_mul(10).saturating_add(d as usize));
            return true;
        }
    }
    let times = count.unwrap_or(1) as isize;
    match &event.key {
        _ if ctrl_letter(event) == Some('d') => app.tree_move(page / 2),
        _ if ctrl_letter(event) == Some('u') => app.tree_move(-page / 2),
        _ if event.mods.ctrl => return false,
        Key::Up => app.tree_move(-times),
        Key::Down => app.tree_move(times),
        Key::PageDown => app.tree_move(page),
        Key::PageUp => app.tree_move(-page),
        Key::Home => app.tree_move(isize::MIN / 2),
        Key::End => app.tree_move(isize::MAX / 2),
        Key::Right | Key::Enter => app.tree_activate(None),
        Key::Left => app.tree_collapse(),
        Key::Escape => app.focus = Focus::Content,
        Key::Text(t) => match t.as_str() {
            "k" => app.tree_move(-times),
            "j" => app.tree_move(times),
            "l" | "o" => app.tree_activate(None),
            "h" => app.tree_collapse(),
            "G" => match count { Some(row) => app.tree_go_to_row(row), None => app.tree_move(isize::MAX / 2) },
            "g" if pending_g => app.tree_move(isize::MIN / 2),
            "g" => app.pending_g = true,
            _ => return false,
        },
        _ => return false,
    }
    true
}

fn arrow(key: &Key) -> Option<Direction> {
    match key {
        Key::Left => Some(Direction::Left),
        Key::Right => Some(Direction::Right),
        Key::Up => Some(Direction::Up),
        Key::Down => Some(Direction::Down),
        _ => None,
    }
}

/// Content climbs to the pane above first, then to the focused pane's tab row.
fn focus_up(app: &mut App) {
    let focus = app.focus;
    app.focus = match focus {
        Focus::Workspaces(i) => Focus::Workspaces(i),
        Focus::Buffers(_) => Focus::Workspaces(app.workspace_active),
        Focus::Content if app.focus_split_toward(Direction::Up) => Focus::Content,
        Focus::Content | Focus::Sidebar => Focus::Buffers(app.active),
    };
}

fn focus_down(app: &mut App) {
    app.focus = match app.focus {
        Focus::Workspaces(_) => Focus::Buffers(app.active),
        Focus::Content => { app.focus_split_toward(Direction::Down); Focus::Content }
        _ => Focus::Content,
    };
}

/// Left/Right on a tab row steps with wrapping through the focused pane's tabs and its `+` slot
/// (`Buffers(tabs.len())`); `Alt+Left` from the first tab enters the sidebar; in content
/// `Alt+Left/Right` moves between panes, and past the leftmost pane into the sidebar.
fn focus_step(app: &mut App, previous: bool) {
    let plus = app.tabs.len();
    let mut slots = app.pane_tab_indices();
    slots.push(plus);
    let focus = app.focus;
    app.focus = match focus {
        Focus::Buffers(i) if previous && slots.first() == Some(&i) && app.sidebar.is_some() => Focus::Sidebar,
        Focus::Buffers(i) => {
            let at = slots.iter().position(|s| *s == i).unwrap_or(0);
            let count = slots.len();
            Focus::Buffers(slots[if previous { (at + count - 1) % count } else { (at + 1) % count }])
        }
        Focus::Workspaces(i) => Focus::Workspaces(if previous { (i + app.workspaces.len()) % (app.workspaces.len() + 1) } else { (i + 1) % (app.workspaces.len() + 1) }),
        Focus::Content if app.focus_split_toward(if previous { Direction::Left } else { Direction::Right }) => Focus::Content,
        Focus::Content if previous && app.sidebar.is_some() => Focus::Sidebar,
        Focus::Sidebar if !previous => Focus::Content,
        other => other,
    };
}

impl App {
    /// Step the open palette's or finder's list slide (`Residual`). Returns whether it moves.
    pub fn step_list_residual(&mut self, clock: f64) -> bool {
        let s = self.ty.scale();
        let (reset, top, row_h) = if let Some(f) = &self.finder {
            (format!("finder:{:?}:{}", f.mode, f.query.text), f.top, crate::finder::view::ROW_H * s)
        } else if let Some(p) = &self.picker {
            (format!("picker:{:?}:{}", p.mode, p.query.text), p.top, crate::palette::view::row_height(p.mode) * s)
        } else {
            self.list_residual = Default::default();
            return false;
        };
        self.list_residual.step(&reset, top, row_h, clock)
    }

    /// Where the window cursor belongs: the focus bar on a tab, or the focused field's caret.
    pub fn cursor_target(&self) -> Option<[f32; 4]> {
        use crate::ui::cursor::tab_bar;
        let scale = 1.0;
        // An open finder or palette parks the cursor on its selected row; the query field keeps
        // its own line caret (`selected_cursor_rect`: a 0.6 em block left of the row's text).
        if self.menu.is_some() { return self.rects.get("menu-row").map(|r| crate::ui::menu::cursor_rect(self, *r)); }
        if self.settings.is_some() {
            let settings = self.settings.as_ref()?;
            let index = *settings.visible().get(settings.focused)?;
            return self.rects.get(&format!("settings-control:{index}")).map(|r| crate::settings::view::cursor_rect(self, *r));
        }
        if self.browser.is_some() { return self.rects.get("browser-row").filter(|_| self.browser.as_ref().is_some_and(|b| b.selected.is_some())).map(|r| crate::tree::view::cursor_rect(self, *r)); }
        let row = if let Some(finder) = &self.finder { self.finder_row_rect(finder).map(|r| (r, 14.0, 13.0)) }
            else if self.project_menu.is_some() { self.project_menu_row_rect().map(|r| (r, 14.0, 13.0)) }
            else if let Some(picker) = &self.picker { self.palette_row_rect(picker).map(|r| (r, 10.0, 13.0)) }
            else { None };
        if let Some((r, pad_x, font)) = row {
            let s = self.ty.scale();
            let (font, row_h) = (font * s, r[3]);
            let w = (font * 0.6).max(2.0);
            let h = (row_h - 5.0 * s).max(font).min(row_h);
            // Where the row is drawn: a window that just moved still slides it back into place.
            let slide = if self.finder.is_some() || self.picker.is_some() { self.list_residual.value() } else { 0.0 };
            return Some([(r[0] + pad_x * s - w - 2.0 * s).max(r[0]), r[1] + slide + (row_h - h) / 2.0, w, h]);
        }
        match self.focus {
            // The focused tab's rect by its id; the `+` slot past the last tab by its pane.
            Focus::Buffers(i) => {
                use crate::ui::tab_drag::Source;
                let key = self.tabs.get(i).map_or_else(|| format!("pane-new:{}", self.layout.focus), |t| Source::Buffer(t.id).rect_key());
                self.rects.get(&key).map(|r| tab_bar(*r, 16.0, scale, i >= self.tabs.len()))
            }
            Focus::Workspaces(i) => {
                use crate::ui::tab_drag::Source;
                let key = self.workspaces.get(i).map_or_else(|| "workspace-new".to_owned(), |w| Source::Workspace(w.id.clone()).rect_key());
                self.rects.get(&key).map(|r| tab_bar(*r, 14.0, scale, i >= self.workspaces.len()))
            }
            Focus::Content if matches!(self.active_tab().kind, TabKind::Editor { .. }) => self.active_buffer().and_then(|b| crate::editor::view::cursor_rect(self, b)),
            Focus::Sidebar if self.active_tree().is_some() => self.rects.get("tree-row").map(|r| crate::tree::view::cursor_rect(self, *r)),
            Focus::Sidebar if self.sidebar == Some(crate::app::Sidebar::Conversations) && (self.conv_header.is_some() || !self.threads.is_empty()) => self.conversation_cursor(),
            Focus::Sidebar | Focus::Content => self.cursor.caret_block(),
        }
    }

    /// The scroll view the cursor's target lives in: the cursor is clipped to it, so a row or
    /// cell scrolled out of view never paints over the tab bar, breadcrumb or status line.
    pub fn cursor_clip(&self) -> Option<[f32; 4]> {
        if self.menu.is_some() || self.settings.is_some() { return None; }
        if self.finder.is_some() { return self.rects.get("finder-results").copied(); }
        if self.browser.is_some() { return self.rects.get(crate::browser::view::LIST_KEY).copied(); }
        if self.picker.is_some() { return self.rects.get("picker-results").copied(); }
        if self.project_menu.is_some() { return self.project_menu_clip(); }
        match self.focus {
            Focus::Buffers(_) => self.rects.get(&format!("pane-tab-strip:{}", self.layout.focus)).copied(),
            Focus::Workspaces(_) => self.rects.get("workspace-tabs").copied(),
            Focus::Content if matches!(self.active_tab().kind, TabKind::Editor { .. }) => self.active_buffer().and_then(|b| b.view_rect),
            Focus::Sidebar if self.active_tree().is_some() => self.rects.get("file-tree-rows").copied(),
            // The header stops sit above the scrolled list: unclipped there.
            Focus::Sidebar if self.sidebar == Some(crate::app::Sidebar::Conversations) && self.conv_header.is_none() => self.rects.get("conversation-list").copied(),
            _ => None,
        }
    }

    /// Keyboard focus in text fields follows the app's focus zone: the active tab's composer
    /// holds it only while content has focus and no picker, finder or menu is open (the
    /// original moves the one cursor out of the field rather than leaving it typing).
    pub fn sync_field_focus(&mut self) -> bool {
        // A click into another pane's composer focused its field: that pane takes focus. (A
        // field left focused from the last sync is just a tab `active` moved away from.)
        let clicked = self.tabs.iter().enumerate()
            .find(|(i, t)| *i != self.active && t.draft.focused && self.field_tab != Some(t.id))
            .map(|(_, t)| t.id);
        if let Some(tab) = clicked {
            self.select_tab(tab);
            self.focus = Focus::Content;
        }
        // A click into this tab's own composer while the keyboard was elsewhere (the sidebar,
        // a tab row, the tree): the composer takes it, with the window cursor.
        // (Not on the first sync: a composer starts out focused before any sync has run.)
        let own = self.tabs.get(self.active).filter(|t| t.draft.focused && self.field_tab != Some(t.id)).map(|t| t.id);
        if self.field_synced && own.is_some() && self.focus != Focus::Content && self.picker.is_none() && self.finder.is_none() {
            self.focus = Focus::Content;
        }
        self.field_synced = true;
        // An inline composer picker (`/model`, `/cd` …) types into its own query row.
        let inline_picker = self.composer.picker.as_ref().is_some_and(|p| p.kind != crate::agent::slash::Kind::Slash);
        // A block's output filter types into its own field; a click back into the composer
        // (focused since the last sync) takes the keyboard back from it.
        let active_id = self.tabs.get(self.active).map(|t| t.id);
        if self.block_filter_focused() && self.field_tab.is_none() && self.tabs.get(self.active).is_some_and(|t| t.draft.focused) {
            if let Some(filter) = active_id.and_then(|id| self.terminals.get_mut(&id)).and_then(|t| t.filter.as_mut()) { filter.1.focused = false; }
        }
        let overlay = self.picker.is_some() || self.finder.is_some() || self.browser.is_some() || inline_picker || self.modal.is_some() || self.menu.is_some() || self.settings.is_some()
            || self.block_filter_focused();
        let want = self.focus == Focus::Content && !overlay && !matches!(self.active_tab().kind, TabKind::Editor { .. } | TabKind::Database { .. });
        let active = self.active;
        let mut changed = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let focused = want && index == active;
            if tab.draft.focused != focused { tab.draft.focused = focused; changed = true; }
        }
        self.field_tab = want.then(|| self.tabs.get(active).map(|t| t.id)).flatten();
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Mods;

    fn ctrl(key: Key) -> KeyEvent { KeyEvent { key, mods: Mods { ctrl: true, cmd: true, ..Mods::default() } } }

    #[test]
    fn ctrl_chords_read_as_letters_however_they_arrive() {
        assert_eq!(ctrl_letter(&ctrl(Key::Command('d'))), Some('d'));
        assert_eq!(ctrl_letter(&ctrl(Key::Text("U".into()))), Some('u'));
        assert_eq!(ctrl_letter(&ctrl(Key::Text("\u{4}".into()))), Some('d'));
        assert_eq!(ctrl_letter(&KeyEvent { key: Key::Command('d'), mods: Mods::default() }), None);
    }

    #[test]
    fn ctrl_d_and_u_page_the_tree() {
        let mut app = App::new();
        let entries: Vec<_> = (0..80).map(|i| serde_json::json!({"name": format!("f{i:02}"), "is_dir": false})).collect();
        app.apply_dir_listing("", &serde_json::Value::Array(entries));
        app.sidebar = Some(crate::app::Sidebar::Files);
        app.tree_follow.scroll.set((0.0, 0.0, 26.0 * 21.0 * app.ty.scale()));
        assert!(tree_key(&mut app, &ctrl(Key::Command('d'))));
        let half = app.active_tree().unwrap().selected;
        assert!(half >= 5, "Ctrl+D moved half a page, got {half}");
        assert!(tree_key(&mut app, &ctrl(Key::Command('u'))));
        assert_eq!(app.active_tree().unwrap().selected, 0);
    }
}
