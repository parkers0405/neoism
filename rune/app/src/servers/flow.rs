//! The window side of Servers and Systems (`ChromeActionsController`'s server actions):
//! connecting with a cancellable "Connecting" message, the forms' submit, removal with a
//! confirmation, invites, and switching the window to the new connection.
//!
//! Rune keeps one connection per window: switching to another server stashes the local
//! window's workspaces, opens the remote daemon's workspace (the SSH home workspace, or the
//! one the daemon picks), and switching back to Local Server restores them.

use super::form::{Draft, Form, Kind};
use super::{Connected, Operation, Selection};
use crate::app::App;
use crate::ui::modal::{Action, Button, Modal};
use std::sync::mpsc::{Receiver, TryRecvError};

/// What a finished background job does next.
enum Outcome {
    Connected(Connected),
}

struct Pending {
    /// The server or `system:<id>` being connected (the Systems list shows it Connecting).
    id: String,
    rx: Receiver<Result<Outcome, String>>,
    /// A form's draft: a failure reopens it with the error (`beginServer(draft)`).
    draft: Option<Draft>,
}

/// A removal waiting for its confirmation.
#[derive(Clone, Debug, PartialEq)]
pub enum Confirm {
    Server(String),
    System(String),
}

/// How the connected daemon's workspace is chosen once it handshakes.
#[derive(Clone, Debug, PartialEq)]
pub enum Adopt {
    /// Ask the daemon (`ResolveInitialWorkspace`).
    Resolve,
}

#[derive(Default)]
pub struct ServerState {
    /// The window's server; `None` is Local Server.
    pub current: Option<Selection>,
    pending: Option<Pending>,
    tunnel: Option<crate::ssh_hosts::DaemonAttach>,
    /// The local window's workspaces while it visits another server.
    stash: Option<crate::session::Saved>,
    pub confirm: Option<Confirm>,
    pub adopt: Option<Adopt>,
}

impl ServerState {
    pub fn current_id(&self) -> &str { self.current.as_ref().map_or("local", |s| s.id.as_str()) }
    pub fn remote(&self) -> bool { self.current.is_some() }
    pub fn connecting(&self) -> Option<&str> { self.pending.as_ref().map(|p| p.id.as_str()) }
    /// The system this window is connected to over SSH, while its tunnel is up.
    pub fn system(&mut self) -> Option<String> {
        let id = self.current.as_ref()?.system.clone()?;
        let tunnel = self.tunnel.as_mut()?;
        tunnel.is_running().then_some(id)
    }
}

fn busy(title: &str, body: &str) -> Modal {
    Modal { title: title.into(), body: body.into(),
        buttons: vec![Button { label: "Cancel".into(), key: "Esc".into(), action: Action::CancelConnect }], selected: 0 }
}

impl App {
    fn start_job(&mut self, id: String, title: &str, body: &str, job: impl FnOnce() -> Result<Outcome, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = self.waker.clone();
        std::thread::Builder::new().name("neoism-server".into()).spawn(move || {
            let _ = tx.send(job());
            if let Some(wake) = wake { wake(); }
        }).ok();
        self.server.pending = Some(Pending { id, rx, draft: None });
        self.modal = Some(busy(title, body));
    }

    /// Dial a server (or a system, or Local Server) and switch to it once it answers.
    pub fn connect_server(&mut self, selection: Selection) {
        if selection.id == self.server.current_id() && selection.system.is_none() { return; }
        let id = selection.system.as_ref().map_or_else(|| selection.id.clone(), |s| format!("system:{s}"));
        let body = if selection.name.is_empty() { "Connecting to the selected system\u{2026}".to_owned() } else { selection.name.clone() };
        self.start_job(id, "Connecting", &body, move || super::dial(selection).map(Outcome::Connected));
    }

    /// Select a server row: the local daemon, or a saved / temporary server by id.
    pub fn select_server(&mut self, id: &str) {
        if id == "local" { return self.connect_server(Selection::local()); }
        match super::resolve(id) {
            Ok(selection) => self.connect_server(selection),
            Err(error) => self.modal = Some(Modal::notice("Servers", &error)),
        }
    }

    /// Connect a saved SSH system (`system-connect`).
    pub fn connect_system(&mut self, id: &str, name: &str) {
        self.connect_server(Selection { id: format!("system:{id}"), name: name.into(), system: Some(id.into()), ..Selection::default() });
    }

    /// The background job finished: switch, or say why not. Returns whether anything changed.
    pub fn poll_server(&mut self) -> bool {
        let Some(pending) = &self.server.pending else { return false };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("Server worker stopped".into()),
        };
        let draft = self.server.pending.take().and_then(|p| p.draft);
        if matches!(self.modal.as_ref().map(|m| m.buttons.first().map(|b| b.action)), Some(Some(Action::CancelConnect))) { self.modal = None; }
        match result {
            Ok(Outcome::Connected(connected)) => self.switch_connection(connected),
            Err(error) => match draft {
                Some(draft) => {
                    let mut form = Form::server(draft);
                    form.error = Some(error);
                    self.form = Some(form);
                }
                None => self.modal = Some(Modal::notice("Could not connect", &error)),
            },
        }
        true
    }

    /// Cancel the connection in progress: its result is dropped (a late tunnel closes with it).
    pub fn cancel_connect(&mut self) {
        self.server.pending = None;
        self.modal = None;
    }

    /// Move the window to a verified connection.
    fn switch_connection(&mut self, connected: Connected) {
        let Connected { selection, target, workspace, tunnel } = connected;
        let remote = target.is_some();
        // Leaving Local Server: keep its workspaces to come back to.
        if remote && !self.server.remote() && self.server.stash.is_none() {
            // Saved first: the snapshot stays the local window while it's away. (Never from a
            // unit test, which would write the user's own session file.)
            #[cfg(not(test))]
            self.save_session(true);
            self.server.stash = Some(self.capture_session());
        }
        crate::daemon::switch_target(target);
        // The old tunnel closes once nothing dials it.
        self.server.tunnel = tunnel;
        self.server.current = (!selection.is_local()).then_some(selection);
        self.reset_window();
        if remote {
            match workspace {
                // The SSH home workspace (`prepareSshWorkspace`): created at the remote home.
                Some((id, title)) => {
                    self.workspace_id = id;
                    self.workspace_name = title;
                    self.cwd = "~".into();
                    self.server.adopt = None;
                }
                None => self.server.adopt = Some(Adopt::Resolve),
            }
        } else if let Some(saved) = self.server.stash.take() {
            self.restore_session(saved);
        }
    }

    /// A fresh window on the same renderer: the connection's workspaces replace this one's.
    fn reset_window(&mut self) {
        let mut fresh = App::new();
        macro_rules! keep { ($($f:ident),*) => { $( std::mem::swap(&mut fresh.$f, &mut self.$f); )* } }
        keep!(daemon, inbox, waker, images, pal, ty, wallpaper, window, background_previews, decoded_previews, wordmark, tools,
            mcp_icons, keybindings, fps, fps_at, orbs, local_name, server, mode_pill, chip_scramble, cursor, composer, agent, model, effort,
            session_writer);
        fresh.connection = crate::app::Connection::Connecting;
        fresh.open_tab(crate::tabs::TabKind::Home);
        *self = fresh;
    }

    /// The daemon picked this window's workspace (`InitialWorkspaceResolved`): open it.
    pub(crate) fn adopt_workspace(&mut self, workspace: &serde_json::Value) {
        if self.server.adopt.take().is_none() { return; }
        let Some(id) = workspace["id"].as_str() else { return };
        self.workspace_id = id.to_owned();
        if let Some(title) = workspace["title"].as_str() { self.workspace_name = title.to_owned(); }
        self.cwd = workspace["root_dir"].as_str().unwrap_or("~").to_owned();
        self.register_workspace();
    }

    // -- forms ------------------------------------------------------------------------------

    /// `+ Add Server`: host this project, or join (`beginServer`).
    pub fn open_server_form(&mut self) {
        let directory = if self.server.remote() { std::env::var("HOME").unwrap_or_default() } else { self.cwd.clone() };
        self.form = Some(Form::server(Draft { host: true, directory, ..Draft::default() }));
    }

    pub fn open_edit_server(&mut self, id: &str) {
        match super::resolve(id) {
            Ok(entry) => self.form = Some(Form::server(Draft {
                host: false, server_id: Some(id.into()), address: entry.endpoint, name: entry.name,
                password: entry.pairing_token.unwrap_or_default(), ..Draft::default()
            })),
            Err(error) => self.modal = Some(Modal::notice("Edit Server", &error)),
        }
    }

    pub fn open_system_form(&mut self) { self.form = Some(Form::system()); }

    /// Submit the open form (`form-submit`): a system saves at once; a server operation runs in
    /// the background and its server is then dialled.
    pub fn submit_form(&mut self, _tx: &mut rune::Tx) {
        let Some(form) = &mut self.form else { return };
        match &form.kind {
            Kind::System => {
                let target = form.value("system_target").unwrap_or("").to_owned();
                let name = form.value("system_name").unwrap_or("").to_owned();
                match super::add_system(&target, &name) {
                    Ok(()) => { self.form = None; self.open_picker(crate::palette::Mode::Systems); }
                    Err(error) => form.error = Some(error),
                }
            }
            Kind::Server(_) => {
                let Some(draft) = form.draft() else { return };
                let operation = match operation(&draft) {
                    Ok(operation) => operation,
                    Err(error) => { form.error = Some(error); return; }
                };
                let (title, body) = (if draft.host { "Starting server" } else { "Joining server" },
                    if draft.fast { "Preparing the Fast Server connection\u{2026}" } else { "Preparing the workspace connection\u{2026}" });
                self.form = None;
                self.start_job("server-form".into(), title, body, move || {
                    let selection = super::run(operation)?;
                    super::dial(selection).map(Outcome::Connected)
                });
                if let Some(pending) = &mut self.server.pending { pending.draft = Some(draft); }
            }
        }
    }

    // -- server rows ------------------------------------------------------------------------

    /// Remove a server or system after confirming (`server-confirm`).
    pub fn ask_remove(&mut self, confirm: Confirm) {
        let (title, body) = match &confirm {
            Confirm::System(_) => ("Remove system?", "Remove this saved SSH connection?"),
            Confirm::Server(_) => ("Remove server?", "Remove this saved connection? A server hosted here will stop and disconnect its clients."),
        };
        self.server.confirm = Some(confirm);
        self.picker = None;
        self.modal = Some(Modal { title: title.into(), body: body.into(), selected: 0, buttons: vec![
            Button { label: "Remove".into(), key: "Enter".into(), action: Action::ConfirmRemove },
            Button { label: "Cancel".into(), key: "Esc".into(), action: Action::Close },
        ] });
    }

    pub fn confirm_remove(&mut self) {
        let Some(confirm) = self.server.confirm.take() else { return };
        let (current, result, mode) = match &confirm {
            Confirm::Server(id) => (self.server.current_id() == id, super::remove(id), crate::palette::Mode::Servers),
            Confirm::System(id) => (self.server.current.as_ref().and_then(|s| s.system.as_deref()) == Some(id), super::remove_system(id), crate::palette::Mode::Systems),
        };
        if let Err(error) = result { self.modal = Some(Modal::notice("Could not remove", &error)); return; }
        // Removing the server this window uses goes back to Local Server.
        if current { self.connect_server(Selection::local()); } else { self.open_picker(mode); }
    }

    /// Copy a Fast Server's invite (`server-invite`).
    pub fn copy_invite(&mut self, id: &str, tx: &mut rune::Tx) {
        match super::invite(id) {
            Ok(invite) => { tx.copy(invite.clone()); self.clipboard = Some(invite); }
            Err(error) => self.modal = Some(Modal::notice("Servers", &error)),
        }
    }

    /// The Active switch (`system-enabled`): turning it on connects, off disconnects.
    pub fn set_system_active(&mut self, id: &str, name: &str, enabled: bool) {
        if let Err(error) = super::set_system_enabled(id, enabled) { self.modal = Some(Modal::notice("Systems", &error)); return; }
        let here = self.server.current.as_ref().and_then(|s| s.system.as_deref()) == Some(id);
        if enabled { self.picker = None; self.connect_system(id, name); }
        else if here { self.picker = None; self.connect_server(Selection::local()); }
    }

    /// Disconnect (`system-disconnect`): back to Local Server when it's this window's system.
    pub fn disconnect_system(&mut self, id: &str) {
        if self.server.current.as_ref().and_then(|s| s.system.as_deref()) == Some(id) { self.connect_server(Selection::local()); }
    }
}

/// `serverFormOperation`: validate the draft and pick the operation.
pub fn operation(draft: &Draft) -> Result<Operation, String> {
    let (name, password) = (draft.name.trim().to_owned(), draft.password.clone());
    if let Some(id) = &draft.server_id {
        return Ok(Operation::Save { id: Some(id.clone()), address: draft.address.trim().to_owned(), name, password });
    }
    if draft.host {
        if draft.directory.trim().is_empty() { return Err("Project folder is required".into()); }
        if draft.fast && password.trim().is_empty() { return Err("Password is required to host a Fast Server".into()); }
        let directory = draft.directory.trim().to_owned();
        return Ok(if draft.fast { Operation::HostFast { directory, name, password } } else { Operation::Host { directory, name, password } });
    }
    if draft.address.trim().is_empty() { return Err("Server address is required".into()); }
    let address = draft.address.trim().to_owned();
    Ok(if draft.fast { Operation::JoinFast { address, name, password } } else { Operation::Save { id: None, address, name, password } })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The switching tests move the window-wide daemon target: one at a time.
    static SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_form_picks_its_operation_and_validates() {
        let draft = |host: bool, fast: bool| Draft { host, fast, ..Draft::default() };
        assert_eq!(operation(&draft(true, false)).unwrap_err(), "Project folder is required");
        assert_eq!(operation(&Draft { directory: "~/p".into(), ..draft(true, true) }).unwrap_err(), "Password is required to host a Fast Server");
        assert!(matches!(operation(&Draft { directory: "~/p".into(), password: "pw".into(), ..draft(true, true) }), Ok(Operation::HostFast { .. })));
        assert!(matches!(operation(&Draft { directory: "~/p".into(), ..draft(true, false) }), Ok(Operation::Host { .. })));
        assert_eq!(operation(&draft(false, false)).unwrap_err(), "Server address is required");
        assert!(matches!(operation(&Draft { address: "wss://x".into(), ..draft(false, true) }), Ok(Operation::JoinFast { .. })));
        assert!(matches!(operation(&Draft { address: "ws://h/session".into(), ..draft(false, false) }), Ok(Operation::Save { id: None, .. })));
        assert!(matches!(operation(&Draft { server_id: Some("s".into()), ..draft(false, false) }), Ok(Operation::Save { id: Some(_), .. })));
    }

    #[test]
    fn visiting_a_server_stashes_the_local_window_and_coming_back_restores_it() {
        let _serial = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = App::new();
        app.workspace_name = "local-ws".into();
        let local_cwd = app.cwd.clone();
        let remote = Selection { id: "team".into(), name: "Team".into(), endpoint: "ws://team:9877/session".into(), ..Selection::default() };
        let target = crate::daemon::Target { url: remote.endpoint.clone(), ..Default::default() };
        app.switch_connection(Connected { selection: remote, target: Some(target), workspace: None, tunnel: None });
        assert!(app.server.remote());
        assert_eq!(app.server.adopt, Some(Adopt::Resolve));
        assert_eq!(crate::daemon::current_target().map(|t| t.url).as_deref(), Some("ws://team:9877/session"));
        // The daemon picks the workspace; the window opens it.
        app.adopt_workspace(&serde_json::json!({"id": "w-remote", "title": "Team project", "root_dir": "/srv/project"}));
        assert_eq!((app.workspace_id.as_str(), app.workspace_name.as_str(), app.cwd.as_str()), ("w-remote", "Team project", "/srv/project"));
        assert_eq!(app.server.adopt, None);
        // Back to Local Server: the stashed window returns.
        app.switch_connection(Connected { selection: Selection::local(), target: None, workspace: None, tunnel: None });
        assert!(!app.server.remote());
        assert_eq!(crate::daemon::current_target(), None);
        assert_eq!((app.workspace_name.as_str(), app.cwd.as_str()), ("local-ws", local_cwd.as_str()));
    }

    #[test]
    fn an_ssh_system_opens_its_home_workspace() {
        let _serial = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = App::new();
        let target = crate::daemon::Target { url: "ws://127.0.0.1:40000/session".into(), auth_token: Some("cred".into()), pairing_token: None };
        let system = Selection { id: "system:s1".into(), system: Some("s1".into()), ..Selection::default() };
        app.switch_connection(Connected { selection: system, target: Some(target), workspace: Some(("neoism-quick-ssh-home-x".into(), "box \u{b7} Home".into())), tunnel: None });
        assert_eq!((app.workspace_id.as_str(), app.cwd.as_str(), app.server.adopt.clone()), ("neoism-quick-ssh-home-x", "~", None));
        app.switch_connection(Connected { selection: Selection::local(), target: None, workspace: None, tunnel: None });
        assert!(!app.server.remote());
    }

    #[test]
    fn an_unreachable_server_reports_and_keeps_the_window() {
        let mut app = App::new();
        let id = app.workspace_id.clone();
        app.connect_server(Selection { id: "x".into(), name: "Nowhere".into(), endpoint: "ws://127.0.0.1:1/session".into(), ..Selection::default() });
        assert_eq!(app.server.connecting(), Some("x"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !app.poll_server() { assert!(std::time::Instant::now() < deadline); std::thread::sleep(std::time::Duration::from_millis(20)); }
        assert_eq!(app.modal.as_ref().map(|m| m.title.as_str()), Some("Could not connect"));
        assert_eq!(app.workspace_id, id);
        assert!(!app.server.remote());
    }
}
