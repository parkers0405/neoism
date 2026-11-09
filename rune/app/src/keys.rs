//! Window shortcuts and the chrome focus ladder (`rune/README.md`, "Keyboard focus and the cursor";
//! source of truth: `neoism-frontend/chrome-navigation.ts`).

use crate::app::{App, Focus};
use crate::tabs::TabKind;
use rune::{Key, KeyEvent, Tx};

pub fn on_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if shortcut(app, event, tx) { return true; }
    // A full-screen program or running command in the Home pane owns the keyboard.
    if app.terminal_key(event) { return true; }
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
    let m = event.mods;
    let alt_only = m.alt && !m.ctrl && !m.shift && !m.logo;
    if let Key::Escape = event.key {
        if app.overlay.take().is_some() { return true; }
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
                "t" | "d" => { app.open_tab(TabKind::Home); return true; }
                "c" => {
                    app.toggle_conversations();
                    return true;
                }
                "p" => { app.open_picker(crate::palette::Mode::Commands); return true; }
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
