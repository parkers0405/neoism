//! Window shortcuts and the chrome focus ladder (`rune/README.md`, "Keyboard focus and the cursor";
//! source of truth: `neoism-frontend/chrome-navigation.ts`).

use crate::app::{App, Focus};
use crate::tabs::TabKind;
use rune::{Key, KeyEvent, Tx};

pub fn on_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if shortcut(app, event, tx) { return true; }
    if app.settings.is_some() { return app.settings_key(event); }
    // A full-screen program or running command in the Home pane owns the keyboard.
    if app.terminal_key(event) { return true; }
    if app.focus == Focus::Content && matches!(app.active_tab().kind, TabKind::Editor { .. }) && editor_key(app, event, tx) {
        return true;
    }
    if app.focus == Focus::Sidebar && app.active_tree().is_some() && tree_key(app, event) {
        return true;
    }
    let m = event.mods;
    match (&event.key, app.focus) {
        (Key::Enter, Focus::Buffers(index)) => {
            if index < app.tabs.len() { app.active = index; } else { app.open_tab(TabKind::Home); }
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
pub fn shortcut(app: &mut App, event: &KeyEvent, _tx: &mut Tx) -> bool {
    // An open menu owns its navigation keys before anything else.
    if app.menu.is_some() && app.menu_key(event) { return true; }
    let m = event.mods;
    let alt_only = m.alt && !m.ctrl && !m.shift && !m.logo;
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
    if alt_only {
        if let Key::Text(t) = &event.key {
            match t.to_lowercase().as_str() {
                "a" => { app.open_tab(TabKind::Agent { session: None }); return true; }
                "t" => { app.open_tab(TabKind::Home); return true; }
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

/// Vim normal-mode motions in an editor tab; the view follows the cursor on the scroll spring.
fn editor_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    use crate::editor::Motion;
    let m = event.mods;
    let pending_g = std::mem::take(&mut app.pending_g);
    let motion = match &event.key {
        Key::Left => Motion::Left,
        Key::Right => Motion::Right,
        Key::Up => Motion::Up,
        Key::Down => Motion::Down,
        Key::Home => Motion::LineStart,
        Key::End => Motion::LineEnd,
        Key::Text(t) if m.ctrl && t == "d" => Motion::HalfPageDown,
        Key::Text(t) if m.ctrl && t == "u" => Motion::HalfPageUp,
        Key::Text(t) if !m.ctrl && !m.alt => match t.as_str() {
            "h" => Motion::Left,
            "l" => Motion::Right,
            "k" => Motion::Up,
            "j" => Motion::Down,
            "0" => Motion::LineStart,
            "$" => Motion::LineEnd,
            "G" => Motion::Bottom,
            "g" if pending_g => Motion::Top,
            "g" => { app.pending_g = true; return true; }
            _ => return false,
        },
        _ => return false,
    };
    // The view follows once the moved cursor cell has been laid out (lines wrap).
    if app.editor_motion(motion) { app.editor_follow = true; }
    let _ = tx;
    true
}

/// Tree navigation while the file tree has focus.
fn tree_key(app: &mut App, event: &KeyEvent) -> bool {
    if event.mods.ctrl || event.mods.alt { return false; }
    match &event.key {
        Key::Up => app.tree_move(-1),
        Key::Down => app.tree_move(1),
        Key::Right | Key::Enter => app.tree_activate(None),
        Key::Left => app.tree_collapse(),
        Key::Escape => app.focus = Focus::Content,
        Key::Text(t) => match t.as_str() {
            "k" => app.tree_move(-1),
            "j" => app.tree_move(1),
            "l" | "o" => app.tree_activate(None),
            "h" => app.tree_collapse(),
            _ => return false,
        },
        _ => return false,
    }
    true
}

fn focus_up(app: &mut App) {
    app.focus = match app.focus {
        Focus::Workspaces(i) => Focus::Workspaces(i),
        Focus::Buffers(_) => Focus::Workspaces(0),
        Focus::Content | Focus::Sidebar => Focus::Buffers(app.active),
    };
}

fn focus_down(app: &mut App) {
    app.focus = match app.focus {
        Focus::Workspaces(_) => Focus::Buffers(app.active),
        _ => Focus::Content,
    };
}

/// Left/Right on a tab row steps with wrapping through the `+` slot; `Alt+Left` from the first
/// tab enters the sidebar; in content `Alt+Left/Right` moves between sidebar and content.
fn focus_step(app: &mut App, previous: bool) {
    app.focus = match app.focus {
        Focus::Buffers(0) if previous && app.sidebar.is_some() => Focus::Sidebar,
        Focus::Buffers(i) => {
            let count = app.tabs.len() + 1;
            Focus::Buffers(if previous { (i + count - 1) % count } else { (i + 1) % count })
        }
        Focus::Workspaces(i) => Focus::Workspaces(if i == 0 { 1 } else { 0 }),
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
        let row = if self.finder.is_some() { self.rects.get("finder-row").map(|r| (*r, 14.0, 13.0)) }
            else if self.picker.is_some() { self.rects.get("picker-row").map(|r| (*r, 10.0, 13.0)) }
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
            Focus::Workspaces(i) => self.rects.get(&format!("workspace-tab:{i}")).map(|r| tab_bar(*r, 14.0, scale, i == 1)),
            Focus::Content if matches!(self.active_tab().kind, TabKind::Editor { .. }) => self.rects.get("editor-cursor").copied(),
            Focus::Sidebar if self.active_tree().is_some() => self.rects.get("tree-row").map(|r| crate::tree::view::cursor_rect(self, *r)),
            Focus::Sidebar | Focus::Content => self.cursor.caret_block(),
        }
    }

    /// Keyboard focus in text fields follows the app's focus zone: the active tab's composer
    /// holds it only while content has focus and no picker, finder or menu is open (the
    /// original moves the one cursor out of the field rather than leaving it typing).
    pub fn sync_field_focus(&mut self) -> bool {
        let overlay = self.picker.is_some() || self.finder.is_some();
        let want = self.focus == Focus::Content && !overlay && !matches!(self.active_tab().kind, TabKind::Editor { .. });
        let active = self.active;
        let mut changed = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let focused = want && index == active;
            if tab.draft.focused != focused { tab.draft.focused = focused; changed = true; }
        }
        changed
    }
}
