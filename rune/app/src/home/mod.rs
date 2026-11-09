//! Home: the welcome splash, then the Warp-style block terminal over a daemon PTY
//! (`reference/01..03`, motion `home-terminal-scroll`).

mod blocks;
mod shell_events;
mod view;

pub use blocks::{Block, Terminal};
pub use view::view;

use crate::app::{App, Connection};
use crate::daemon::Family;
use crate::tabs::Tab;
use neoism_protocol::pty::{ClientMessage as PtyClient, Presentation, ServerMessage as PtyServer};

impl App {
    pub(crate) fn open_terminal(&mut self, tab_id: u64) {
        self.terminals.insert(tab_id, Terminal::new(self.cwd.clone(), self.terminal_colors()));
        self.create_pty(tab_id);
    }

    pub(crate) fn close_terminal(&mut self, tab_id: u64) {
        let Some(term) = self.terminals.remove(&tab_id) else { return };
        if let Some(session_id) = term.session { self.send_pty(PtyClient::ClosePty { session_id }); }
    }

    /// After a (re)connect every Home tab gets a fresh shell.
    pub(crate) fn terminals_connected(&mut self) {
        self.pending_pty.clear();
        let tabs: Vec<u64> = self.terminals.keys().copied().collect();
        for tab_id in tabs {
            if let Some(term) = self.terminals.get_mut(&tab_id) { term.session = None; }
            self.create_pty(tab_id);
        }
    }

    fn send_pty(&self, message: PtyClient) {
        if let (Some(daemon), Ok(value)) = (&self.daemon, serde_json::to_value(&message)) { daemon.send(Family::Pty, value); }
    }

    fn create_pty(&mut self, tab_id: u64) {
        if self.daemon.is_none() || self.connection != Connection::Connected || self.pending_pty.contains(&tab_id) { return; }
        let cwd = self.terminals.get(&tab_id).map_or_else(|| self.cwd.clone(), |t| t.cwd.clone());
        self.send_pty(PtyClient::CreatePty {
            cwd: Some(cwd), cols: blocks::COLS, rows: blocks::ROWS, shell: None, shell_args: None,
            presentation: Some(Presentation::Blocks),
        });
        self.pending_pty.push_back(tab_id);
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

    fn pty_input(&self, session_id: &str, bytes: &[u8]) {
        self.send_pty(PtyClient::PtyInput { session_id: session_id.into(), bytes: bytes.to_vec() });
    }

    /// Run a composer line in the tab's shell (or feed it to the running command).
    pub fn run_command(&mut self, index: usize, text: String) {
        let tab_id = self.tabs[index].id;
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        if text == "clear" {
            term.clear();
            return;
        }
        let Some(session_id) = term.session.clone() else { return };
        // Away from a prompt (a command is reading stdin) the line goes to that command.
        term.prepare(&text);
        self.pty_input(&session_id, format!("{text}\r").as_bytes());
    }

    pub(crate) fn apply_pty(&mut self, message: PtyServer) {
        match message {
            PtyServer::PtyCreated { session_id, .. } => {
                if let Some(tab_id) = self.pending_pty.pop_front() {
                    if let Some(term) = self.terminals.get_mut(&tab_id) { term.attach(session_id); }
                }
            }
            PtyServer::PtyOutput { session_id, bytes, offset } => {
                let replies = self.terminals.values_mut().find(|t| t.session.as_deref() == Some(&session_id))
                    .map(|term| term.feed(&bytes, offset)).unwrap_or_default();
                if !replies.is_empty() { self.pty_input(&session_id, &replies); }
            }
            PtyServer::SessionCwd { session_id, cwd } => {
                if let Some(term) = self.terminals.values_mut().find(|t| t.session.as_deref() == Some(&session_id)) { term.cwd = cwd; }
            }
            PtyServer::PtyClosed { session_id, .. } => {
                let tab = self.terminals.iter().find(|(_, t)| t.session.as_deref() == Some(&session_id)).map(|(id, _)| *id);
                if let Some(tab_id) = tab {
                    if let Some(term) = self.terminals.get_mut(&tab_id) { term.session = None; }
                    // A shell that exited (`exit`, crash) is replaced so Home always has a prompt.
                    self.create_pty(tab_id);
                }
            }
            _ => {}
        }
    }

    pub fn tab_cwd(&self, tab: &Tab) -> String {
        let cwd = self.terminals.get(&tab.id).map(|t| t.cwd.as_str()).unwrap_or(&self.cwd);
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() && cwd.starts_with(&home) { format!("~{}", &cwd[home.len()..]) } else { cwd.to_owned() }
    }
}
