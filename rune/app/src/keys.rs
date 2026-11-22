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
    if app.prompt.is_some() { return app.context_prompt_key(event); }
    if app.context.is_some() { return app.context_key(event); }
    if app.form.is_some() { return app.form_key(event, tx); }
    if app.project_add.is_some() && app.browser.is_none() { app.project_add_key(event); return true; }
    if app.browser.is_some() { return app.browser_key(event, tx) || shortcut(app, event, tx); }
    if app.composer.picker.as_ref().is_some_and(|p| matches!(p.kind, crate::agent::slash::Kind::Usage | crate::agent::slash::Kind::Tasks | crate::agent::slash::Kind::Goals | crate::agent::slash::Kind::Context)) {
        return app.picker_key(event, tx);
    }
    // A picker above the composer owns its list keys even when its query field has lost the
    // keyboard (a click on the card, or a reply that rebuilt the picker): without this the
    // arrows did nothing until the field was clicked again.
    if app.focus == Focus::Content && matches!(event.key, Key::Up | Key::Down | Key::Enter | Key::Escape)
        && !event.mods.ctrl && !event.mods.alt && !event.mods.logo
        && app.composer.picker.as_ref().is_some_and(|p| p.kind != crate::agent::slash::Kind::Slash && p.mention.is_none())
    {
        return app.picker_key(event, tx);
    }
    if app.focus == Focus::Content && app.prompt_key(event) { return true; }
    if shortcut(app, event, tx) { return true; }
    if app.diag_popup.is_some() && app.diag_popup_key(event) { return true; }
    if app.settings.is_some() { return app.settings_key(event); }
    if app.git_panel_key(event, tx) { return true; }
    // A full-screen program or running command in the Home pane owns the keyboard.
    if app.terminal_key(event) { return true; }
    if app.focus == Focus::Content && matches!(app.active_tab().kind, TabKind::Editor { .. }) && app.editor_key(event, tx) {
        return true;
    }
    if app.project_menu.is_some() && app.project_menu_key(event) { return true; }
    if app.focus == Focus::Content && app.timeline_jump_key(event, tx) { return true; }
    if app.focus == Focus::Sidebar && app.sidebar == Some(crate::app::Sidebar::Conversations) && app.conversation_key(event) {
        return true;
    }
    if app.focus == Focus::Sidebar && app.active_tree().is_some() && tree_key(app, event) {
        return true;
    }
    let m = event.mods;
    match (&event.key, app.focus) {
        (Key::Enter, Focus::Content) if !m.ctrl && !m.alt && !m.shift && !m.logo
            && app.focused_field_mut().is_none() && app.picker.is_none() && app.finder.is_none()
            && app.composer.picker.is_none() && app.menu.is_none() => app.replay_empty_wordmark(),
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

/// Window shortcuts, shared with text fields through `on_input_key`. App panel commands
/// use Alt on every platform. Option+arrows retain the app-wide focus ladder, and Option+Shift
/// arrows reorder the active tab unless the workspace strip has focus.
pub fn shortcut(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if app.modal.is_some() { return crate::ui::modal::keys(app, event, tx); }
    if app.capture_binding(event) { return true; }
    // Super+V / Super+C, as the desktop binds them (fields keep their own Ctrl chords).
    let m = event.mods;
    if m.logo && !m.ctrl && !m.alt && !m.shift {
        let letter = match &event.key { Key::Text(t) => t.to_lowercase(), Key::Command(c) => c.to_ascii_lowercase().to_string(), _ => String::new() };
        match letter.as_str() {
            "v" => { app.paste_chord(tx); return true; }
            "c" => { if app.copy_chord(tx) || !cfg!(target_os = "macos") { return true; } }
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
    // Keep the app-wide zoom aliases even when the saved font bindings use Super.
    // A typed '+' includes Shift on most layouts; it is still the zoom-in chord.
    if (m.ctrl || m.cmd || m.logo) && !m.alt {
        let key = match &event.key {
            Key::Command(c) => c.to_string(),
            Key::Text(text) => text.clone(),
            _ => String::new(),
        };
        let action = match key.as_str() {
            "=" | "+" => Some(crate::palette::commands::Action::IncreaseFontSize),
            "-" => Some(crate::palette::commands::Action::DecreaseFontSize),
            "0" => Some(crate::palette::commands::Action::ResetFontSize),
            _ => None,
        };
        if let Some(action) = action { app.run_action(action, tx); return true; }
    }
    let m = event.mods;
    let mac = cfg!(target_os = "macos");
    let alt_only = m.alt && !m.ctrl && !m.shift && !m.logo;
    // Keep the original app-wide navigation on every platform. The macOS chord is
    // an additional alias, not a replacement (independent of cursor material).
    let focus_chord = alt_only || (mac && m.logo && m.ctrl && !m.alt && !m.shift);
    let alt_move = m.alt && m.shift && !m.logo;
    let move_chord = alt_move || (mac && m.logo && m.ctrl && m.shift && !m.alt);
    if move_chord && matches!(event.key, Key::Left | Key::Right) {
        app.move_chrome_tab(event.key == Key::Right, (alt_move && m.ctrl) || matches!(app.focus, Focus::Workspaces(_)), tx);
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
    let workspace_chord = alt_only || (mac && m.logo && !m.ctrl && !m.alt && !m.shift);
    if workspace_chord {
        // Option+digits produce symbols on macOS; workspace slots follow the number row.
        use rune::PhysicalKeyCode as Code;
        let physical = match tx.physical_key() {
            Some(Code::Digit1) => Some(1), Some(Code::Digit2) => Some(2), Some(Code::Digit3) => Some(3),
            Some(Code::Digit4) => Some(4), Some(Code::Digit5) => Some(5), Some(Code::Digit6) => Some(6),
            Some(Code::Digit7) => Some(7), Some(Code::Digit8) => Some(8), Some(Code::Digit9) => Some(9),
            _ => None,
        };
        let text = match &event.key { Key::Text(t) => t.clone(), Key::Command(c) => c.to_string(), _ => String::new() };
        if let Some(n) = physical.or_else(|| text.parse::<usize>().ok()) { if (1..=9).contains(&n) { app.select_workspace(n - 1); return true; } }
    }
    if alt_only && !mac {
        if let Key::Text(t) = &event.key {
            // Preserve the extra terminal alias on other platforms. App letter chords
            // otherwise belong to the configured catalogue, not Option on macOS.
            if t.eq_ignore_ascii_case("t") {
                app.open_tab(TabKind::Terminal);
                return true;
            }
        }
    }
    if focus_chord {
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
    if event.mods.alt || event.mods.logo { return false; }
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
            "e" | "d" | "a" if app.sidebar == Some(crate::app::Sidebar::Notes) => {
                use crate::ui::context::Command;
                app.tree_prompt(match t.as_str() { "e" => Command::Rename, "d" => Command::Delete, _ => Command::NewFile });
            }
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
        Focus::Content | Focus::Sidebar | Focus::Details => Focus::Buffers(app.active),
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
/// (`Buffers(tabs.len())`); focus-left from the first tab enters the sidebar; in content
/// focus-left/right moves between panes, and past the leftmost pane into the sidebar.
/// Focus chords use Alt+arrows everywhere, with Ctrl+Cmd+arrows also on macOS.
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
        Focus::Content if !previous && app.git_panel.open => Focus::Details,
        Focus::Details if previous => Focus::Content,
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
    /// The scrolled surface the cursor sits in and where that is scrolled to (`Cursor::carry`):
    /// a file with the keyboard in it.
    pub fn cursor_riding(&self) -> Option<(String, f32)> {
        if self.attach.preview.is_some() || self.menu.is_some() || self.settings.is_some() || self.browser.is_some()
            || self.finder.is_some() || self.project_menu.is_some() || self.picker.is_some() { return None; }
        if self.focus != Focus::Content || !matches!(self.active_tab().kind, TabKind::Editor { .. }) { return None; }
        let buffer = self.active_buffer().filter(|b| b.prompt.is_none())?;
        Some((buffer.path.clone(), buffer.scroll.get().0))
    }

    pub fn cursor_target(&self) -> Option<[f32; 4]> {
        // A full-window picture preview has no cursor.
        if self.attach.preview.is_some() { return None; }
        use crate::ui::cursor::tab_bar;
        let scale = 1.0;
        if self.menu.is_some() { return self.rects.get("menu-row").map(|r| crate::ui::menu::cursor_rect(self, *r)); }
        // Settings shows its own focus (the row's tint, the fields' carets).
        if self.settings.is_some() { return None; }
        if self.browser.is_some() { return self.rects.get("browser-row").filter(|_| self.browser.as_ref().is_some_and(|b| b.selected.is_some())).map(|r| crate::tree::view::cursor_rect(self, *r)); }
        // Search and commands own the insertion caret. Result navigation only changes their
        // highlight; it must never steal the cursor from the editable query (including IME).
        if self.finder.is_some() || self.picker.as_ref().is_some_and(|p| matches!(p.mode,
            crate::palette::Mode::Commands | crate::palette::Mode::Systems | crate::palette::Mode::Servers)) {
            let clip = self.cursor_clip()?;
            let line = self.cursor.caret_line.get()?;
            // During initial focus transfer Rune can still report the underlying editor's
            // caret. Wait for the query's measured caret rather than drawing that stale one.
            if line[0] < clip[0] - 1.0 || line[0] > clip[0] + clip[2] + 1.0
                || line[1] < clip[1] - 1.0 || line[1] + line[3] > clip[1] + clip[3] + 1.0 { return None; }
            return Some(crate::ui::cursor::block_for_caret(line, self.ty.mono_sized(14.0).size));
        }
        // The project filter and the add-project card draw their own caret and highlight.
        if self.project_menu.is_some() || self.project_add.is_some() { return None; }
        let row = if let Some(picker) = &self.picker { self.palette_row_rect(picker).map(|r| (r, 10.0, 13.0)) }
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
            Focus::Details | Focus::Sidebar | Focus::Content => self.cursor.caret_block(),
        }
    }

    /// The scroll view the cursor's target lives in: the cursor is clipped to it, so a row or
    /// cell scrolled out of view never paints over the tab bar, breadcrumb or status line.
    pub fn cursor_clip(&self) -> Option<[f32; 4]> {
        if self.menu.is_some() { return self.rects.get("app-menu").copied(); }
        if let Some(settings) = &self.settings { return self.rects.get(&crate::settings::view::scroll_key(settings)).copied(); }
        if self.finder.is_some() { return self.rects.get("finder-query-clip").copied(); }
        if self.browser.is_some() { return self.rects.get(crate::browser::view::LIST_KEY).copied(); }
        if let Some(picker) = &self.picker {
            return self.rects.get(if matches!(picker.mode, crate::palette::Mode::Commands | crate::palette::Mode::Systems | crate::palette::Mode::Servers) {
                "picker-query-clip"
            } else { "picker-results" }).copied();
        }
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
        // A click into the conversations search takes the keyboard to the sidebar; once the
        // keyboard is elsewhere the field lets go.
        let conversations = self.sidebar == Some(crate::app::Sidebar::Conversations);
        if conversations && self.conv_query.focused && !self.conv_query_seen && own.is_none() && clicked.is_none() {
            self.focus = Focus::Sidebar;
            self.conv_header = None;
        } else if self.focus != Focus::Sidebar || !conversations {
            self.conv_query.focused = false;
        }
        self.conv_query_seen = self.conv_query.focused;
        if self.focus != Focus::Details || !self.git_panel.open {
            self.git_panel.message.focused = false;
        }
        // An inline composer picker (`/model`, `/cd` …) types into its own query row.
        let inline_picker = !self.has_prompt() && self.composer.picker.as_ref().is_some_and(|p| p.kind != crate::agent::slash::Kind::Slash);
        // A block's output filter types into its own field; a click back into the composer
        // (focused since the last sync) takes the keyboard back from it.
        let active_id = self.tabs.get(self.active).map(|t| t.id);
        if self.block_filter_focused() && self.field_tab.is_none() && self.tabs.get(self.active).is_some_and(|t| t.draft.focused) {
            if let Some(filter) = active_id.and_then(|id| self.terminals.get_mut(&id)).and_then(|t| t.filter.as_mut()) { filter.1.focused = false; }
        }
        // An inline picker's query keeps the keyboard while it is open and nothing over it is.
        let covered = self.project_add.is_some() || self.project_menu.is_some() || self.picker.is_some() || self.finder.is_some()
            || self.browser.is_some() || self.form.is_some() || self.modal.is_some() || self.menu.is_some() || self.settings.is_some();
        let mut refocused = false;
        if inline_picker && !covered && self.focus == Focus::Content && let Some(picker) = self.composer.picker.as_mut().filter(|p| p.mention.is_none() && !p.query.focused) {
            picker.query.focused = true;
            refocused = true;
        }
        let overlay = self.project_add.is_some() || self.project_menu.is_some() || self.picker.is_some() || self.finder.is_some() || self.browser.is_some() || self.form.is_some() || inline_picker || self.modal.is_some() || self.menu.is_some() || self.settings.is_some()
            || self.block_filter_focused();
        let prompt_changed = self.sync_prompt_focus(self.focus == Focus::Content && !overlay);
        let want = self.focus == Focus::Content && !overlay && !self.question_prompt() && !matches!(self.active_tab().kind, TabKind::Editor { .. } | TabKind::Database { .. } | TabKind::GitDiff);
        let active = self.active;
        let mut changed = prompt_changed | refocused;
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
    fn command_zoom_accepts_shifted_plus_and_preserves_markdown_insert_state() {
        for primary in [Mods { logo: true, ..Default::default() }, Mods { cmd: true, ..Default::default() }, Mods { ctrl: true, ..Default::default() }] {
            let mut app = App::new();
            app.keybindings = serde_json::json!([
                {"action":"increasefontsize", "with":"super", "key":"="},
                {"action":"decreasefontsize", "with":"super", "key":"-"}
            ]);
            app.ty.size = 14.0;
            app.open_file("zoom.md", None);
            app.apply_opened_file("zoom.md", b"# Zoom heading\neditable text\n");
            app.buffers.get_mut("zoom.md").unwrap().doc.mode = crate::editor::Mode::Insert;
            app.buffers.get_mut("zoom.md").unwrap().doc.cursor_line = 1;
            let mut runtime = rune::Runtime::new(app, crate::ui::chrome::root,
                (800.0, 500.0), rune::Env::new(true, 1.0));
            runtime.on_key = Some(crate::on_key);
            for _ in 0..3 { runtime.frame(1.0 / 60.0); }
            let height = |rt: &rune::Runtime<App>| rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Zoom heading")).unwrap().rect[3];
            let before = height(&runtime);
            let plus = KeyEvent { key: Key::Command('+'), mods: Mods { shift: true, ..primary } };
            runtime.key(plus);
            for _ in 0..3 { runtime.frame(1.0 / 60.0); }
            assert_eq!(runtime.state.ty.size, 15.0);
            assert!(height(&runtime) > before, "the rendered Markdown must reflow, not just the chrome");
            let minus = KeyEvent { key: Key::Command('-'), mods: primary };
            runtime.key(minus.clone());
            runtime.frame(1.0 / 60.0);
            assert_eq!(runtime.state.ty.size, 14.0, "after frame: {minus:?}");
            let buffer = runtime.state.active_buffer().unwrap();
            assert_eq!(buffer.doc.mode, crate::editor::Mode::Insert);
            assert_eq!(buffer.doc.cursor_line, 1);
            assert_eq!(buffer.doc.text_for_disk(), "# Zoom heading\neditable text\n");
            assert!(!buffer.dirty());
        }
    }

    #[test]
    fn zoom_aliases_do_not_take_plain_text_or_override_explicit_bindings() {
        let mut app = App::new();
        app.ty.size = 14.0;
        app.keybindings = serde_json::json!([
            {"action":"togglefiletree", "with":"control", "key":"-"}
        ]);
        for mods in [Mods::default(), Mods { alt: true, logo: true, ..Default::default() }] {
            assert!(!shortcut(&mut app, &KeyEvent { key: Key::Text("+".into()), mods }, &mut Tx::default()));
            assert_eq!(app.ty.size, 14.0);
        }
        assert!(shortcut(&mut app, &ctrl(Key::Command('-')), &mut Tx::default()));
        assert_eq!(app.ty.size, 14.0);
        assert_eq!(app.sidebar, Some(crate::app::Sidebar::Files));
    }

    #[test]
    fn cursor_clip_follows_tree_menu_and_settings_viewports() {
        let mut app = App::new();
        app.sidebar = Some(crate::app::Sidebar::Files);
        app.focus = Focus::Sidebar;
        let tree_clip = [55.0, 66.0, 300.0, 400.0];
        app.rects.insert("file-tree-rows".into(), tree_clip);
        // A selected row can scroll above the header; its caret must keep the list's clip.
        app.rects.insert("tree-row".into(), [55.0, 20.0, 300.0, 26.0]);
        assert_eq!(app.cursor_clip(), Some(tree_clip));
        app.toggle_menu();
        let menu_clip = [55.0, 30.0, 196.0, 138.0];
        app.rects.insert("app-menu".into(), menu_clip);
        assert_eq!(app.cursor_clip(), Some(menu_clip));
        app.menu = None;
        app.open_settings();
        let settings_clip = [240.0, 94.0, 600.0, 400.0];
        let key = crate::settings::view::scroll_key(app.settings.as_ref().unwrap());
        app.rects.insert(key, settings_clip);
        assert_eq!(app.cursor_clip(), Some(settings_clip));
    }

    #[test]
    fn alt_arrows_move_app_wide_cursor_with_glass_on_and_off() {
        use crate::ui::tab_drag::Source;
        for glass in [false, true] {
            let mut app = App::new();
            app.keybindings = serde_json::json!([]);
            app.cursor.liquid_glass = glass;
            app.workspaces.push(crate::workspace::Entry { id: "test".into(), title: "Test".into(), server: None, state: None });
            app.sidebar = Some(crate::app::Sidebar::Files);
            app.apply_dir_listing("", &serde_json::json!([{"name":"file.rs", "is_dir":false}]));
            app.rects.insert(Source::Buffer(app.active_tab().id).rect_key(), [320.0, 40.0, 160.0, 32.0]);
            app.rects.insert(Source::Workspace(app.workspaces[app.workspace_active].id.clone()).rect_key(), [55.0, 0.0, 160.0, 32.0]);
            app.rects.insert(format!("pane-new:{}", app.layout.focus), [480.0, 40.0, 32.0, 32.0]);
            app.rects.insert("tree-row".into(), [55.0, 100.0, 240.0, 26.0]);
            app.focus = Focus::Content;
            app.sync_field_focus();
            app.cursor.caret_line.set(Some([500.0, 400.0, 1.0, 22.0]));
            let active = app.active;
            let workspace = app.workspace_active;
            let mut previous = None;
            for (key, expected) in [
                (Key::Up, Focus::Buffers(active)),
                (Key::Up, Focus::Workspaces(workspace)),
                (Key::Down, Focus::Buffers(active)),
                (Key::Right, Focus::Buffers(app.tabs.len())),
                (Key::Left, Focus::Buffers(active)),
                (Key::Left, Focus::Sidebar),
                (Key::Right, Focus::Content),
                (Key::Left, Focus::Sidebar),
            ] {
                let event = KeyEvent { key, mods: Mods { alt: true, ..Default::default() } };
                assert!(on_key(&mut app, &event, &mut Tx::default()), "glass={glass}, {event:?}");
                app.sync_field_focus();
                assert_eq!(app.focus, expected, "glass={glass}");
                let target = app.cursor_target().expect("every focus zone has an app-wide target");
                assert_ne!(previous, Some(target), "focus change must move the optical/fallback destination");
                previous = Some(target);
                app.cursor.tick(Some(target), std::time::Instant::now());
                assert!(app.cursor.placed());
                #[cfg(target_os = "macos")]
                assert!(crate::ui::chrome::glass_cursor_allowed(&app));
            }
        }
    }

    #[test]
    fn ctrl_chords_read_as_letters_however_they_arrive() {
        assert_eq!(ctrl_letter(&ctrl(Key::Command('d'))), Some('d'));
        assert_eq!(ctrl_letter(&ctrl(Key::Text("U".into()))), Some('u'));
        assert_eq!(ctrl_letter(&ctrl(Key::Text("\u{4}".into()))), Some('d'));
        assert_eq!(ctrl_letter(&KeyEvent { key: Key::Command('d'), mods: Mods::default() }), None);
    }

    #[test]
    fn alt_numbers_switch_workspaces_without_typing_into_the_composer() {
        let mut app = App::new();
        app.new_workspace();
        for n in [1, 2, 1] {
            let event = KeyEvent { key: Key::Text(n.to_string().into()), mods: Mods { alt: true, ..Default::default() } };
            assert!(shortcut(&mut app, &event, &mut Tx::default()));
            assert_eq!(app.workspace_active, n - 1);
            assert!(app.active_tab().draft.text.is_empty());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_focus_alias_is_preserved() {
        let mut app = App::new();
        app.keybindings = serde_json::json!([]);
        app.focus = Focus::Content;
        let alias = KeyEvent { key: Key::Up, mods: Mods { logo: true, ctrl: true, cmd: true, ..Default::default() } };
        assert!(shortcut(&mut app, &alias, &mut Tx::default()));
        assert_eq!(app.focus, Focus::Buffers(app.active));
    }

    #[test]
    fn alt_shift_arrows_reorder_active_buffer_tabs_from_content_sidebar_and_strip() {
        let mut app = App::new();
        app.keybindings = serde_json::json!([]);
        let home = app.active_tab().id;
        app.open_file("shortcut.rs", None);
        let editor = app.active_tab().id;
        for focus in [Focus::Content, Focus::Sidebar, Focus::Buffers(app.active)] {
            app.focus = focus;
            for (key, order) in [(Key::Left, vec![editor, home]), (Key::Right, vec![home, editor])] {
                let event = KeyEvent { key, mods: Mods { alt: true, shift: true, ..Default::default() } };
                assert!(on_key(&mut app, &event, &mut Tx::default()));
                assert_eq!(app.layout.focused().tabs, order);
                assert_eq!(app.active_tab().id, editor);
                assert_eq!(app.focus, focus);
            }
        }
    }

    #[test]
    fn alt_shift_recovers_when_a_workspace_switch_interrupts_a_tab_drag() {
        use rune::{Env, Runtime};
        let mut app = App::new();
        app.window = [1000.0, 700.0];
        let home = app.active_tab().id;
        app.open_file("drag.rs", None);
        let editor = app.active_tab().id;
        app.new_workspace();
        app.select_workspace(0);
        let mut rt = Runtime::new(app, crate::ui::chrome::root, (1000.0, 700.0), Env::new(true, 1.0));
        rt.on_key = Some(on_key);
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        let (x, y) = rt.find(&format!("pane-tab:{editor}")).unwrap();
        rt.pointer_down(x, y);
        rt.frame(1.0 / 120.0);
        let move_left = || KeyEvent { key: Key::Left, mods: Mods { alt: true, shift: true, ..Default::default() } };
        rt.key(move_left());
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.layout.focused().tabs, [home, editor], "a held drag still owns tab movement");

        rt.update(|app, _| app.select_workspace(1));
        rt.frame(1.0 / 120.0);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 120.0);
        rt.update(|app, _| app.select_workspace(0));
        rt.frame(1.0 / 120.0);
        rt.key(move_left());
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.layout.focused().tabs, [editor, home], "the shortcut must recover without another drag");
    }

    #[test]
    fn alt_shift_reorders_tabs_through_focused_agent_and_code_controls() {
        use crate::agent::slash::{Kind, Picker};
        use rune::{Env, Runtime};
        for surface in ["home", "agent", "question", "usage", "tasks", "goals", "code", "insert"] {
            let mut app = App::new();
            app.window = [1000.0, 700.0];
            app.keybindings = serde_json::json!([]);
            let home = app.active_tab().id;
            match surface {
                "home" => { app.open_tab(TabKind::Home); }
                "code" | "insert" => {
                    app.open_file("reorder.rs", None);
                    app.apply_opened_file("reorder.rs", b"let value = 1;\n");
                    if surface == "insert" { app.buffers.get_mut("reorder.rs").unwrap().doc.mode = crate::editor::Mode::Insert; }
                }
                _ => {
                    app.load_fixture("reorder", "Reorder", "[]");
                    if surface == "question" {
                        app.apply_agent(neoism_protocol::agent::AgentServerMessage::QuestionAsked {
                            session_id: "reorder".into(), request: serde_json::json!({"id":"question", "questions":[{"question":"Which environment?"}]}),
                        });
                    } else if let Some(kind) = match surface { "usage" => Some(Kind::Usage), "tasks" => Some(Kind::Tasks), "goals" => Some(Kind::Goals), _ => None } {
                        app.composer.picker = Some(Picker::new(kind, "Details", Vec::new()));
                    }
                }
            }
            let active = app.active_tab().id;
            app.sync_field_focus();
            let mut rt = Runtime::new(app, crate::ui::chrome::root, (1000.0, 700.0), Env::new(true, 1.0));
            rt.on_key = Some(on_key);
            for _ in 0..30 { rt.frame(1.0 / 120.0); }
            if surface == "question" {
                let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("agent-prompt-answer") && !e.exiting).unwrap().rect;
                let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
                rt.pointer_move(x, y); rt.pointer_down(x, y); rt.pointer_up(x, y);
                for _ in 0..30 { rt.frame(1.0 / 120.0); }
            }
            for (key, order) in [(Key::Left, vec![active, home]), (Key::Right, vec![home, active])] {
                rt.key(KeyEvent { key, mods: Mods { alt: true, shift: true, ..Default::default() } });
                for _ in 0..30 { rt.frame(1.0 / 120.0); }
                assert_eq!(rt.state.layout.focused().tabs, order, "reorder blocked by {surface}");
                assert_eq!(rt.state.active_tab().id, active, "reorder changed selection in {surface}");
            }
        }
    }

    #[test]
    fn alt_shift_arrows_reorder_workspaces_and_ctrl_forces_the_workspace_strip() {
        let mut app = App::new();
        app.keybindings = serde_json::json!([]);
        let first = app.workspace_id.clone();
        app.new_workspace();
        let second = app.workspace_id.clone();
        app.focus = Focus::Workspaces(app.workspace_active);
        for (key, order) in [(Key::Left, vec![second.clone(), first.clone()]), (Key::Right, vec![first.clone(), second.clone()])] {
            let event = KeyEvent { key, mods: Mods { alt: true, shift: true, ..Default::default() } };
            assert!(on_key(&mut app, &event, &mut Tx::default()));
            assert_eq!(app.workspaces.iter().map(|w| w.id.clone()).collect::<Vec<_>>(), order);
            assert_eq!(app.workspace_id, second);
            assert_eq!(app.focus, Focus::Workspaces(app.workspace_active));
        }
        app.focus = Focus::Content;
        let event = KeyEvent { key: Key::Left, mods: Mods { alt: true, shift: true, ctrl: true, cmd: true, ..Default::default() } };
        assert!(on_key(&mut app, &event, &mut Tx::default()));
        assert_eq!(app.workspaces.iter().map(|w| w.id.clone()).collect::<Vec<_>>(), vec![second.clone(), first]);
        assert_eq!(app.workspace_id, second);
        assert_eq!(app.focus, Focus::Content);
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
