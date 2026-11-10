//! Window shortcuts and the chrome focus ladder (`rune/README.md`, "Keyboard focus and the cursor";
//! source of truth: `neoism-frontend/chrome-navigation.ts`).

use crate::app::{App, Focus};
use crate::panes::{Axis, Direction};
use crate::tabs::TabKind;
use rune::{Key, KeyEvent, Tx};

pub fn on_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if app.modal.is_some() { return crate::ui::modal::keys(app, event, tx); }
    if shortcut(app, event, tx) { return true; }
    if app.settings.is_some() { return app.settings_key(event); }
    // A full-screen program or running command in the Home pane owns the keyboard.
    if app.terminal_key(event) { return true; }
    if app.focus == Focus::Content && matches!(app.active_tab().kind, TabKind::Editor { .. }) && app.editor_key(event, tx) {
        return true;
    }
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
                "d" => { app.open_picker(crate::palette::Mode::Directories); return true; }
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
    let page = app.tree_page_rows();
    match &event.key {
        _ if ctrl_letter(event) == Some('d') => app.tree_move(page / 2),
        _ if ctrl_letter(event) == Some('u') => app.tree_move(-page / 2),
        _ if event.mods.ctrl => return false,
        Key::Up => app.tree_move(-1),
        Key::Down => app.tree_move(1),
        Key::PageDown => app.tree_move(page),
        Key::PageUp => app.tree_move(-page),
        Key::Home => app.tree_move(isize::MIN / 2),
        Key::End => app.tree_move(isize::MAX / 2),
        Key::Right | Key::Enter => app.tree_activate(None),
        Key::Left => app.tree_collapse(),
        Key::Escape => app.focus = Focus::Content,
        Key::Text(t) => match t.as_str() {
            "k" => app.tree_move(-1),
            "j" => app.tree_move(1),
            "l" | "o" => app.tree_activate(None),
            "h" => app.tree_collapse(),
            "G" => app.tree_move(isize::MAX / 2),
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
        let row = if let Some(finder) = &self.finder { self.finder_row_rect(finder).map(|r| (r, 14.0, 13.0)) }
            else if let Some(picker) = &self.picker { self.palette_row_rect(picker).map(|r| (r, 10.0, 13.0)) }
            else { None };
        if let Some((r, pad_x, font)) = row {
            let s = self.ty.scale();
            let (font, row_h) = (font * s, r[3]);
            let w = (font * 0.6).max(2.0);
            let h = (row_h - 5.0 * s).max(font).min(row_h);
            return Some([(r[0] + pad_x * s - w - 2.0 * s).max(r[0]), r[1] + (row_h - h) / 2.0, w, h]);
        }
        match self.focus {
            Focus::Buffers(i) => self.rects.get(&format!("pane-tab:{i}")).map(|r| tab_bar(*r, 16.0, scale, i == self.tabs.len())),
            Focus::Workspaces(i) => self.rects.get(&format!("workspace-tab:{i}")).map(|r| tab_bar(*r, 14.0, scale, i == self.workspaces.len())),
            Focus::Content if matches!(self.active_tab().kind, TabKind::Editor { .. }) => self.active_buffer().and_then(|b| crate::editor::view::cursor_rect(self, b)),
            Focus::Sidebar if self.active_tree().is_some() => self.rects.get("tree-row").map(|r| crate::tree::view::cursor_rect(self, *r)),
            Focus::Sidebar if self.sidebar == Some(crate::app::Sidebar::Conversations) && !self.threads.is_empty() => self.conversation_cursor(),
            Focus::Sidebar | Focus::Content => self.cursor.caret_block(),
        }
    }

    /// The scroll view the cursor's target lives in: the cursor is clipped to it, so a row or
    /// cell scrolled out of view never paints over the tab bar, breadcrumb or status line.
    pub fn cursor_clip(&self) -> Option<[f32; 4]> {
        if self.menu.is_some() || self.settings.is_some() { return None; }
        if self.finder.is_some() { return self.rects.get("finder-results").copied(); }
        if self.picker.is_some() { return self.rects.get("picker-results").copied(); }
        match self.focus {
            Focus::Buffers(_) => self.rects.get(&format!("pane-tab-strip:{}", self.layout.focus)).copied(),
            Focus::Workspaces(_) => self.rects.get("workspace-tabs").copied(),
            Focus::Content if matches!(self.active_tab().kind, TabKind::Editor { .. }) => self.active_buffer().and_then(|b| b.view_rect),
            Focus::Sidebar if self.active_tree().is_some() => self.rects.get("file-tree-rows").copied(),
            Focus::Sidebar if self.sidebar == Some(crate::app::Sidebar::Conversations) => self.rects.get("conversation-list").copied(),
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
        // An inline composer picker (`/model`, `/cd` …) types into its own query row.
        let inline_picker = self.composer.picker.as_ref().is_some_and(|p| p.kind != crate::agent::slash::Kind::Slash);
        // A block's output filter types into its own field; a click back into the composer
        // (focused since the last sync) takes the keyboard back from it.
        let active_id = self.tabs.get(self.active).map(|t| t.id);
        if self.block_filter_focused() && self.field_tab.is_none() && self.tabs.get(self.active).is_some_and(|t| t.draft.focused) {
            if let Some(filter) = active_id.and_then(|id| self.terminals.get_mut(&id)).and_then(|t| t.filter.as_mut()) { filter.1.focused = false; }
        }
        let overlay = self.picker.is_some() || self.finder.is_some() || inline_picker || self.modal.is_some() || self.menu.is_some() || self.settings.is_some()
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
