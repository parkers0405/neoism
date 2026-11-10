//! Home: the welcome splash, then the Warp-style block terminal over a daemon PTY
//! (`reference/01..03`, motion `home-terminal-scroll`). Full-screen programs and REPLs get
//! Ghostty's grid; keys then go straight to the PTY through Ghostty's encoders.

mod actions;
mod blocks;
mod composer;
mod grid;
mod input;
mod shell_events;
mod splash;
mod terminal;
mod view;

pub use blocks::Block;
pub use terminal::{Terminal, program_title};
pub use view::{block_menu_overlay, view};

use composer::MenuKind;
use input::TermKey;
use splash::MenuAction;

use crate::app::{App, Connection};
use crate::daemon::Family;
use crate::ghostty::{MouseAction, MouseButton, MouseInput};
use crate::tabs::{Tab, TabKind};
use neoism_protocol::pty::{ClientMessage as PtyClient, Presentation, ServerMessage as PtyServer};
use rune::{KeyEvent, Tx};
use serde_json::{Value, json};
use std::time::Duration;

/// Home view state that is not terminal state: the measured splash area, hovered button, and
/// whether the wordmark is on screen (it scrolls away above the blocks).
/// Kept per tab: split panes each lay out their own splash.
#[derive(Default)]
pub struct SplashState {
    pub area: std::collections::HashMap<u64, [f32; 2]>,
    pub hover: Option<(u64, usize)>,
    /// Tabs whose wordmark has scrolled off.
    pub hidden: std::collections::HashSet<u64>,
}

impl App {
    /// Step every terminal's wheel glide one frame and place its scroll; true while moving.
    pub fn step_terminal_glides(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let mut moving = false;
        for (tab, term) in self.terminals.iter_mut() {
            let Some(glide) = term.glide.as_mut() else { continue };
            glide.target = glide.target.min(term.scroll.get().1);
            let more = glide.step(clock);
            tx.scroll_to_instant(format!("terminal-scroll:{tab}"), 0.0, glide.pos);
            if more { moving = true; } else { term.glide = None; }
        }
        moving
    }

    /// The Home splash is showing in the active tab (not scrolled off, not replaced by a grid).
    pub fn splash_on_screen(&self) -> bool {
        !self.splash.hidden.contains(&self.active_tab().id) && self.active_terminal().is_none_or(|t| !t.raw())
    }
}

/// Terminal cell metrics for the configured font: Geist Mono advances 0.6 em; rows are 1.3 em.
#[derive(Clone, Copy)]
pub struct Cells {
    pub cell: f32,
    pub row: f32,
    /// The blocks' horizontal inset (16 pt at scale 1).
    pub inset: f32,
}

impl Cells {
    pub fn of(app: &App) -> Cells {
        let size = app.ty.size;
        Cells { cell: size * 0.6, row: size * 1.3, inset: 16.0 * app.ty.scale() }
    }

    /// Pixel cell size for Ghostty's size reports and mouse encoding.
    fn px(self) -> (u32, u32) { (self.cell.round() as u32, self.row.round() as u32) }
}

impl App {
    /// A splash menu button: the same actions as its `Alt` shortcut.
    pub fn splash_action(&mut self, action: MenuAction, _tx: &mut rune::Tx) {
        match action {
            MenuAction::Agent => { self.open_tab(TabKind::Agent { session: None }); }
            MenuAction::AgentPanel => self.toggle_conversations(),
            MenuAction::CommandPalette => self.open_picker(crate::palette::Mode::Commands),
            MenuAction::NewTerminal => { self.open_tab(TabKind::Terminal); }
            MenuAction::ChangeDirectory => self.open_picker(crate::palette::Mode::Directories),
            MenuAction::Notes => self.toggle_tree(crate::app::Sidebar::Notes),
            MenuAction::FileTree => self.toggle_file_tree(),
            MenuAction::Search => self.open_finder(crate::finder::Mode::Files),
        }
    }

    pub(crate) fn open_terminal(&mut self, tab_id: u64) {
        let mut term = Terminal::new(self.cwd.clone(), self.terminal_colors());
        // A plain Terminal tab follows `terminal.block-ui`; Home is always blocks.
        term.grid = self.tabs.iter().any(|t| t.id == tab_id && t.kind == TabKind::Terminal)
            && crate::config::read()["terminal"]["block-ui"] == serde_json::Value::Bool(false);
        self.terminals.insert(tab_id, term);
        self.create_pty(tab_id);
    }

    pub(crate) fn close_terminal(&mut self, tab_id: u64) {
        let Some(term) = self.terminals.remove(&tab_id) else { return };
        if let Some(session_id) = term.shell.session { self.send_pty(PtyClient::ClosePty { session_id }); }
    }

    /// After a (re)connect: reattach each shell from the next byte it needs (the daemon drops a
    /// socket that falls behind and expects this cursor replay), or start one for new tabs.
    pub(crate) fn terminals_connected(&mut self) {
        self.pending_pty.clear();
        let tabs: Vec<u64> = self.terminals.keys().copied().collect();
        for tab_id in tabs {
            let Some(term) = self.terminals.get(&tab_id) else { continue };
            match term.shell.session.clone() {
                Some(session_id) => self.send_pty(PtyClient::AttachPty { session_id, cursor: term.shell.next_offset() }),
                None => self.create_pty(tab_id),
            }
        }
    }

    fn send_pty(&self, message: PtyClient) {
        if let (Some(daemon), Ok(value)) = (&self.daemon, serde_json::to_value(&message)) { daemon.send(Family::Pty, value); }
    }

    fn create_pty(&mut self, tab_id: u64) {
        if self.daemon.is_none() || self.connection != Connection::Connected || !self.workspace_ready || self.pending_pty.contains(&tab_id) { return; }
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        let (cols, rows) = term.shell.size();
        let cwd = term.shell.cwd.clone();
        let presentation = if term.grid { Presentation::Grid } else { Presentation::Blocks };
        // The composer's ghost text and history menu read the shell's history file.
        let history = !std::mem::replace(&mut term.composer.history_requested, true);
        self.send_pty(PtyClient::CreatePty { cwd: Some(cwd), cols, rows, shell: None, shell_args: None, presentation: Some(presentation) });
        self.pending_pty.push_back(tab_id);
        if let (true, Some(daemon)) = (history, &self.daemon) {
            daemon.send_scoped(Family::Files, json!({"ReadShellHistory": {"max_entries": 1000}}), &self.cwd);
        }
    }

    pub(crate) fn terminal_colors(&self) -> crate::ghostty::Colors {
        let p = self.pal;
        crate::ghostty::Colors {
            palette: [p.black, p.red, p.green, p.yellow, p.blue, p.magenta, p.cyan, p.white,
                p.muted, p.red, p.green, p.yellow, p.blue, p.magenta, p.cyan, p.fg],
            fg: p.fg,
            bg: p.bg,
        }
    }

    pub fn copy_terminal_selection(&self, tx: &mut Tx) {
        if let Some(selection) = tx.text_selection() { let text = selection.text().to_owned(); tx.copy(text); return; }
        let Some(term) = self.active_terminal() else { return };
        if let Some(block) = term.shell.blocks.iter().chain(term.shell.live.iter()).find(|b| Some(b.id) == term.selected) {
            tx.copy(block.lines.iter().map(|row| crate::ghostty::row_text(row)).collect::<Vec<_>>().join("\n"));
        } else {
            let draft = &self.active_tab().draft;
            let (a,b) = (draft.anchor.min(draft.focus), draft.anchor.max(draft.focus));
            if let Some(text) = draft.text.get(a..b).filter(|t| !t.is_empty()) { tx.copy(text.to_owned()); }
        }
    }

    pub fn terminal_paste(&mut self, text: &str) {
        let Some(term) = self.active_terminal() else { return };
        let Some(session) = term.shell.session.clone() else { return };
        if term.direct() { self.pty_input(&session, &term.shell.encode_paste(text)); }
    }

    fn pty_input(&self, session_id: &str, bytes: &[u8]) {
        if bytes.is_empty() { return; }
        self.send_pty(PtyClient::PtyInput { session_id: session_id.into(), bytes: bytes.to_vec() });
    }

    fn active_terminal(&self) -> Option<&Terminal> {
        let tab = self.active_tab();
        if !matches!(tab.kind, TabKind::Home | TabKind::Terminal) { return None; }
        self.terminals.get(&tab.id)
    }

    fn active_terminal_mut(&mut self) -> Option<&mut Terminal> {
        let tab = self.tabs.get(self.active)?;
        if !matches!(tab.kind, TabKind::Home | TabKind::Terminal) { return None; }
        self.terminals.get_mut(&tab.id)
    }

    /// Run a composer line in the tab's shell. When the shell is not at its prompt the line
    /// stays in the composer (the composer is read-only then, as in GPUiX).
    pub fn run_command(&mut self, index: usize, text: String) {
        let tab_id = self.tabs[index].id;
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        term.composer.menu = None;
        let Some(session_id) = term.shell.session.clone() else {
            self.tabs[index].draft.replace_text(text);
            return;
        };
        match term.shell.submission(&text) {
            Some(bytes) => {
                term.wheel_target = None;
                term.glide = None;
                term.selected = None;
                term.tick();
                self.pty_input(&session_id, &bytes);
            }
            None => self.tabs[index].draft.replace_text(text),
        }
    }

    /// Per frame: re-read live blocks, size each shell to its pane, and keep busy timers ticking.
    pub fn sync_terminals(&mut self, tx: &mut Tx) -> bool {
        let cells = Cells::of(self);
        let mut changed = false;
        let mut resizes = Vec::new();
        for term in self.terminals.values_mut() {
            changed |= term.sync();
            let [width, height] = term.area;
            if width > 0.0 && height > 0.0 {
                let inset = if term.raw() && term.shell.alt() { 0.0 } else { cells.inset };
                let cols = ((width - 2.0 * inset) / cells.cell).floor().max(2.0) as u16;
                let rows = (height / cells.row).floor().max(1.0) as u16;
                if term.shell.resize(cols, rows, cells.px()) {
                    if let Some(session_id) = &term.shell.session { resizes.push((session_id.clone(), cols, rows)); }
                    changed = true;
                }
            }
            if let Some(since) = term.busy_since {
                // Wake for the long-running switch, then each second for the duration label.
                let elapsed = since.elapsed();
                let next = if elapsed < terminal::LONG_RUNNING { terminal::LONG_RUNNING - elapsed } else { Duration::from_millis(1000 - (elapsed.as_millis() % 1000) as u64) };
                tx.wake_after(next);
            }
        }
        for (session_id, cols, rows) in resizes { self.send_pty(PtyClient::Resize { session_id, cols, rows }); }
        changed
    }

    /// Keys while the active Home terminal takes the keyboard directly (a TUI, a REPL, a running
    /// command). Returns whether the key was sent.
    pub fn terminal_key(&mut self, event: &KeyEvent) -> bool {
        let Some(term) = self.active_terminal() else { return false };
        if !term.direct() { return false; }
        let Some(key) = TermKey::from_event(event) else { return false };
        let Some(term) = self.active_terminal_mut() else { return false };
        let Some(session_id) = term.shell.session.clone() else { return false };
        let bytes = term.shell.encode_key(&key.input());
        term.wheel_target = None;
        term.glide = None;
        self.pty_input(&session_id, &bytes);
        !bytes.is_empty()
    }

    /// Composer keys before the text field (`TerminalCommandInput`'s keyDown): history and
    /// completion menus, ghost acceptance, Ctrl+C and Ctrl+L.
    pub fn terminal_composer_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if crate::keys::shortcut(self, event, tx) { return true; }
        let Some(key) = TermKey::from_event(event) else { return false };
        let index = self.active;
        let Some(tab) = self.tabs.get(index) else { return false };
        let tab_id = tab.id;
        let draft = tab.draft.text.clone();
        let at_end = tab.draft.anchor == tab.draft.focus && tab.draft.focus == draft.len();
        let Some(term) = self.terminals.get_mut(&tab_id) else { return false };
        let recent = term.shell.recent.clone();
        let plain = !key.ctrl() && !key.alt() && !key.logo();
        if key.ctrl_only() && key.is("c") {
            term.composer.menu = None;
            let bytes = term.shell.encode_key(&key.input());
            let session = term.shell.session.clone();
            self.tabs[index].draft.clear();
            if let Some(session_id) = session { self.pty_input(&session_id, &bytes); }
            return true;
        }
        if key.ctrl_only() && key.is("l") {
            term.composer.menu = None;
            term.shell.clear();
            return true;
        }
        if key.ctrl_only() && key.is("r") {
            term.composer.open(MenuKind::History, &draft, &recent);
            return true;
        }
        if key.is("escape") {
            let Some(menu) = term.composer.menu.take() else { return false };
            self.set_draft(index, menu.query);
            return true;
        }
        if plain && !key.shift() && (key.is("up") || key.is("down")) {
            let up = key.is("up");
            match &term.composer.menu {
                None => term.composer.open(MenuKind::History, &draft, &recent),
                // History opens above the composer, newest nearest it: Up goes further back.
                Some(menu) => {
                    let delta = if menu.kind == MenuKind::History { if up { 1 } else { -1 } } else if up { -1 } else { 1 };
                    if let Some(item) = term.composer.step(delta).map(|i| i.value.clone()) { self.set_draft(index, item); }
                    return true;
                }
            }
            if let Some(item) = term.composer.chosen().map(|i| i.value.clone()) { self.set_draft(index, item); }
            return true;
        }
        if key.is("tab") && plain && (term.composer.menu.is_some() || at_end) {
            match &term.composer.menu {
                Some(menu) if menu.kind == MenuKind::Completion && !menu.loading => {
                    let delta = if key.shift() { -1 } else { 1 };
                    if let Some(item) = term.composer.step(delta).map(|i| i.value.clone()) { self.set_draft(index, item); }
                }
                Some(menu) if menu.kind == MenuKind::Completion => {}
                _ => {
                    term.composer.open(MenuKind::Completion, &draft, &recent);
                    if let Some(session_id) = term.shell.session.clone() {
                        self.send_pty(PtyClient::CompleteTerminal { session_id, cursor: draft.len() as u32, line: draft });
                    }
                }
            }
            return true;
        }
        if key.is("right") && plain && !key.shift() && term.composer.menu.is_none() && at_end {
            if let Some(ghost) = term.composer.ghost(&draft, &recent) {
                self.set_draft(index, ghost);
                return true;
            }
        }
        if key.is("enter") && plain && !key.shift() {
            if let Some(menu) = &term.composer.menu {
                if menu.kind == MenuKind::History {
                    let chosen = term.composer.chosen().map(|i| i.value.clone());
                    term.composer.menu = None;
                    if let Some(value) = chosen { self.set_draft(index, value); }
                    return true;
                }
            }
        }
        false
    }

    /// A typed edit closes any open menu (the menu's query is the draft it opened on).
    pub fn composer_edited(&mut self) {
        if let Some(term) = self.active_terminal_mut() { term.composer.menu = None; }
    }

    fn set_draft(&mut self, index: usize, text: String) {
        let draft = &mut self.tabs[index].draft;
        let end = text.len();
        draft.replace_text(text);
        draft.set_caret(end);
    }

    /// Wheel over the raw grid: mouse reports when the program tracks the mouse, arrow keys on
    /// the alternate screen in alternate-scroll mode (as terminals do), else nothing.
    pub fn grid_wheel(&mut self, tab_id: u64, steps: i32, col: u16, row: u16) {
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        let Some(session_id) = term.shell.session.clone() else { return };
        let button = if steps > 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
        let mut bytes = Vec::new();
        for _ in 0..steps.unsigned_abs().min(200) {
            let report = term.shell.encode_mouse(&MouseInput { action: MouseAction::Press, button: Some(button), col, row, ctrl: false, alt: false });
            match report {
                Some(report) => bytes.extend(report),
                None if term.shell.alt() && term.shell.alternate_scroll() => {
                    let key = if steps > 0 { "up" } else { "down" };
                    bytes.extend(term.shell.encode_key(&crate::ghostty::KeyInput { key, text: "", shift: false, ctrl: false, alt: false, logo: false }));
                }
                None => break,
            }
        }
        self.pty_input(&session_id, &bytes);
    }

    /// A press, drag or release over the raw grid, for programs that track the mouse.
    pub fn grid_mouse(&mut self, tab_id: u64, action: MouseAction, col: u16, row: u16) {
        // Pressing into another pane's program focuses that pane first.
        if action == MouseAction::Press && self.active_tab().id != tab_id {
            self.select_tab(tab_id);
            self.focus = crate::app::Focus::Content;
        }
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        let Some(session_id) = term.shell.session.clone() else { return };
        if let Some(bytes) = term.shell.encode_mouse(&MouseInput { action, button: Some(MouseButton::Left), col, row, ctrl: false, alt: false }) {
            self.pty_input(&session_id, &bytes);
        }
    }

    /// A `Pty` message. `live` is false for backlog a reattach replays: Ghostty re-parses it, but
    /// its answers to historical queries must not reach the shell.
    pub(crate) fn apply_pty(&mut self, message: PtyServer, live: bool) {
        match message {
            PtyServer::PtyCreated { session_id, .. } => {
                // A reattach acknowledges a session a tab already has; a new PTY fills the
                // oldest pending tab.
                let known = self.terminals.values().find(|t| t.shell.session.as_deref() == Some(&session_id));
                let size = match known {
                    Some(term) => Some(term.shell.size()),
                    None => self.pending_pty.pop_front().and_then(|tab_id| self.terminals.get_mut(&tab_id)).map(|term| {
                        term.shell.attach(session_id.clone());
                        term.shell.size()
                    }),
                };
                // The pane may have been measured (or resized) after the PTY was requested.
                if let Some((cols, rows)) = size { self.send_pty(PtyClient::Resize { session_id, cols, rows }); }
            }
            PtyServer::PtyOutput { session_id, bytes, offset } => {
                let replies = self.terminals.values_mut().find(|t| t.shell.session.as_deref() == Some(&session_id))
                    .map(|term| term.shell.feed(&bytes, offset)).unwrap_or_default();
                if live { self.pty_input(&session_id, &replies); }
            }
            // The daemon restarted (or reaped the shell): a reattach finds nothing, so start over.
            PtyServer::Error { message } => {
                let tab = self.terminals.iter()
                    .find(|(_, t)| t.shell.session.as_ref().is_some_and(|id| message == format!("unknown session {id}") || message == format!("No such PTY: {id}")))
                    .map(|(id, _)| *id);
                if let Some(tab_id) = tab {
                    if let Some(term) = self.terminals.get_mut(&tab_id) { term.shell.session = None; }
                    self.create_pty(tab_id);
                }
            }
            PtyServer::SessionProgram { session_id, program } => {
                if let Some(term) = self.terminals.values_mut().find(|t| t.shell.session.as_deref() == Some(&session_id)) {
                    term.program = terminal::agent_program(&program);
                }
            }
            PtyServer::SessionCwd { session_id, cwd } => {
                if let Some(term) = self.terminals.values_mut().find(|t| t.shell.session.as_deref() == Some(&session_id)) { term.shell.cwd = cwd; }
            }
            PtyServer::TerminalCompletions { session_id, line, entries, .. } => {
                let Some((&tab_id, term)) = self.terminals.iter_mut().find(|(_, t)| t.shell.session.as_deref() == Some(&session_id)) else { return };
                let recent = term.shell.recent.clone();
                let native = entries.into_iter().map(|e| (e.value, e.label, e.detail, e.kind)).collect();
                let preview = term.composer.completions(&line, native, &recent);
                let index = self.tabs.iter().position(|t| t.id == tab_id);
                if let (Some(value), Some(index)) = (preview, index) { self.set_draft(index, value); }
            }
            PtyServer::PtyClosed { session_id, .. } => {
                let tab = self.terminals.iter().find(|(_, t)| t.shell.session.as_deref() == Some(&session_id)).map(|(id, _)| *id);
                if let Some(tab_id) = tab {
                    if let Some(term) = self.terminals.get_mut(&tab_id) { term.shell.session = None; }
                    // A shell that exited (`exit`, crash) is replaced so Home always has a prompt.
                    self.create_pty(tab_id);
                }
            }
            PtyServer::TerminalDirectoryChanged { request_id, directory, .. } => self.directory_changed(&request_id, Some(&directory)),
            PtyServer::TerminalDirectoryChangeFailed { request_id, .. } => self.directory_changed(&request_id, None),
            _ => {}
        }
    }

    /// `Files` replies the Home terminals asked for (shell history).
    pub(crate) fn apply_files(&mut self, message: &Value) {
        let Some(entries) = message["ShellHistory"]["entries"].as_array() else { return };
        let history: Vec<String> = entries.iter().filter_map(|e| e.as_str().map(str::to_owned)).collect();
        let start = history.len().saturating_sub(1000);
        for term in self.terminals.values_mut().filter(|t| t.composer.history.is_empty()) {
            term.composer.history = history[start..].to_vec();
        }
    }

    pub fn tab_cwd(&self, tab: &Tab) -> String {
        let cwd = self.terminals.get(&tab.id).map(|t| t.shell.cwd.as_str()).unwrap_or(&self.cwd);
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() && cwd.starts_with(&home) { format!("~{}", &cwd[home.len()..]) } else { cwd.to_owned() }
    }
}
