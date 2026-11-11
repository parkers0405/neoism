//! The window side of Servers and Systems (`ChromeActionsController`'s server actions):
//! connecting with a cancellable "Connecting" message, the forms' submit, removal with a
//! confirmation, invites, and opening the new connection.
//!
//! A connection belongs to a workspace: connecting to a server or an SSH system opens a new
//! workspace tab on it (its own daemon socket, a cloud on its tab), next to the local ones,
//! and picking one that is already open goes to its tab. Local Server goes to a local tab.

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

/// A workspace's server: which one, the SSH tunnel that reaches it (dropping it closes the
/// connection), and whether the daemon still has to pick the workspace. It moves with its
/// workspace (`workspace::State`); the default is the local daemon.
#[derive(Default)]
pub struct Link {
    /// `None` is Local Server.
    pub current: Option<Selection>,
    tunnel: Option<crate::ssh_hosts::DaemonAttach>,
    pub adopt: Option<Adopt>,
}

#[derive(Default)]
pub struct ServerState {
    /// The active workspace's server.
    pub link: Link,
    pending: Option<Pending>,
    pub confirm: Option<Confirm>,
}

impl ServerState {
    pub fn current_id(&self) -> &str { self.link.current.as_ref().map_or("local", |s| s.id.as_str()) }
    /// Whether the active workspace is on another server.
    pub fn remote(&self) -> bool { self.link.current.is_some() }
    pub fn connecting(&self) -> Option<&str> { self.pending.as_ref().map(|p| p.id.as_str()) }
    /// The system the active workspace is connected to over SSH, while its tunnel is up.
    pub fn system(&mut self) -> Option<String> {
        let id = self.link.current.as_ref()?.system.clone()?;
        let tunnel = self.link.tunnel.as_mut()?;
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

    /// Open a verified connection: a new workspace tab on that server, or the tab already
    /// on it; Local Server is a local tab.
    fn switch_connection(&mut self, connected: Connected) {
        let Connected { selection, target, workspace, tunnel } = connected;
        let Some(target) = target else {
            // Local Server: the nearest local workspace, or a new one.
            match self.workspace_on(None) {
                Some(index) => self.select_workspace(index),
                None => self.new_workspace(),
            }
            return;
        };
        if let Some(index) = self.workspace_on(Some(&selection.id)) {
            // Already open: its tab, and the tunnel just dialed for it closes again.
            self.select_workspace(index);
            return;
        }
        let (id, title, cwd, adopt) = match workspace {
            // The SSH home workspace (`prepareSshWorkspace`): created at the remote home.
            Some((id, title)) => (id, title, "~".to_owned(), None),
            // The daemon picks the workspace once it handshakes (`adopt_workspace`).
            None => (crate::workspace::new_id(), selection.name.clone(), "~".to_owned(), Some(Adopt::Resolve)),
        };
        self.open_workspace(crate::workspace::Opening { id, title, cwd, target: Some(target), link: Link { current: Some(selection), tunnel, adopt } });
    }

    /// The workspace tab on server `id` (`None`: a local one), nearest the active tab first.
    fn workspace_on(&self, id: Option<&str>) -> Option<usize> {
        let on = |index: usize| -> Option<&str> {
            if index == self.workspace_active { self.server.link.current.as_ref().map(|s| s.id.as_str()) }
            else { self.workspaces[index].server.as_deref() }
        };
        let mut order: Vec<usize> = (0..self.workspaces.len()).collect();
        order.sort_by_key(|index| index.abs_diff(self.workspace_active));
        order.into_iter().find(|index| on(*index) == id)
    }

    /// The daemon picked this window's workspace (`InitialWorkspaceResolved`): open it.
    pub(crate) fn adopt_workspace(&mut self, workspace: &serde_json::Value) {
        if self.server.link.adopt.take().is_none() { return; }
        let Some(id) = workspace["id"].as_str() else { return };
        self.workspace_id = id.to_owned();
        if let Some(title) = workspace["title"].as_str() { self.workspace_name = title.to_owned(); }
        let root = workspace["root_dir"].as_str().unwrap_or("~").to_owned();
        self.resolve_root(&root);
        self.register_workspace();
    }

    // -- forms ------------------------------------------------------------------------------

    /// `+ Add Server`: host this project, or join (`beginServer`).
    pub fn open_server_form(&mut self) {
        let directory = if self.server.remote() { crate::startup::home() } else { self.cwd.clone() };
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
        let (server, result, mode) = match &confirm {
            Confirm::Server(id) => (id.clone(), super::remove(id), crate::palette::Mode::Servers),
            Confirm::System(id) => (format!("system:{id}"), super::remove_system(id), crate::palette::Mode::Systems),
        };
        if let Err(error) = result { self.modal = Some(Modal::notice("Could not remove", &error)); return; }
        // A removed server's workspace tab closes with it.
        if !self.leave_server(&server) { self.open_picker(mode); }
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
        if enabled { self.picker = None; self.connect_system(id, name); }
        else if self.leave_server(&format!("system:{id}")) { self.picker = None; }
    }

    /// Disconnect (`system-disconnect`): the system's workspace tab closes.
    pub fn disconnect_system(&mut self, id: &str) { self.leave_server(&format!("system:{id}")); }

    /// Close the workspace tab on server `id`, hanging up its connection (and its tunnel).
    /// From that tab the window goes to a local one first. Returns whether a tab was open.
    fn leave_server(&mut self, id: &str) -> bool {
        let Some(index) = self.workspace_on(Some(id)) else { return false };
        if index == self.workspace_active {
            match self.workspace_on(None) {
                Some(local) => self.select_workspace(local),
                None => self.new_workspace(),
            }
        }
        // Dropping the parked workspace drops its socket and tunnel.
        let Some(index) = self.workspaces.iter().position(|w| w.server.as_deref() == Some(id)) else { return true };
        if index == self.workspace_active { return true; }
        self.workspaces.remove(index);
        if index < self.workspace_active { self.workspace_active -= 1; }
        self.rects.retain(|key, _| !key.starts_with("workspace-") && !key.starts_with("reorder:workspace:"));
        true
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
    fn a_server_opens_its_own_workspace_tab_and_the_local_one_stays() {
        let mut app = App::new();
        app.workspace_name = "local-ws".into();
        let local = (app.workspace_id.clone(), app.cwd.clone());
        let remote = Selection { id: "team".into(), name: "Team".into(), endpoint: "ws://team:9877/session".into(), ..Selection::default() };
        let target = crate::daemon::Target { url: remote.endpoint.clone(), ..Default::default() };
        app.switch_connection(Connected { selection: remote.clone(), target: Some(target.clone()), workspace: None, tunnel: None });
        // A second tab, on the server, active; the local one is still there behind it.
        assert_eq!((app.workspaces.len(), app.workspace_active), (2, 1));
        assert!(app.server.remote());
        assert_eq!(app.server.link.adopt, Some(Adopt::Resolve));
        assert_eq!((app.workspaces[0].server.as_deref(), app.workspaces[1].server.as_deref()), (None, Some("team")));
        // The daemon picks the workspace; the tab becomes it.
        app.adopt_workspace(&serde_json::json!({"id": "w-remote", "title": "Team project", "root_dir": "/srv/project"}));
        assert_eq!((app.workspace_id.as_str(), app.workspace_name.as_str(), app.cwd.as_str()), ("w-remote", "Team project", "/srv/project"));
        assert_eq!(app.server.link.adopt, None);
        // Local Server: back to the local tab, untouched.
        app.switch_connection(Connected { selection: Selection::local(), target: None, workspace: None, tunnel: None });
        assert_eq!((app.workspaces.len(), app.workspace_active), (2, 0));
        assert!(!app.server.remote());
        assert_eq!((app.workspace_id.clone(), app.workspace_name.as_str(), app.cwd.clone()), (local.0, "local-ws", local.1));
        // The same server again: its tab, not a third one.
        app.switch_connection(Connected { selection: remote, target: Some(target), workspace: None, tunnel: None });
        assert_eq!((app.workspaces.len(), app.workspace_active), (2, 1));
        assert_eq!(app.cwd, "/srv/project");
    }

    #[test]
    fn an_ssh_system_opens_its_home_workspace_in_a_new_tab() {
        let mut app = App::new();
        let target = crate::daemon::Target { url: "ws://127.0.0.1:40000/session".into(), auth_token: Some("cred".into()), pairing_token: None };
        let system = Selection { id: "system:s1".into(), system: Some("s1".into()), ..Selection::default() };
        app.switch_connection(Connected { selection: system, target: Some(target), workspace: Some(("neoism-quick-ssh-home-x".into(), "box \u{b7} Home".into())), tunnel: None });
        assert_eq!(app.workspaces.len(), 2);
        assert_eq!((app.workspace_id.as_str(), app.workspace_name.as_str(), app.cwd.as_str(), app.server.link.adopt.clone()), ("neoism-quick-ssh-home-x", "box \u{b7} Home", "~", None));
        // With only remote tabs left, Local Server opens a local one.
        app.workspaces.remove(0);
        app.workspace_active = 0;
        app.switch_connection(Connected { selection: Selection::local(), target: None, workspace: None, tunnel: None });
        assert_eq!(app.workspaces.len(), 2);
        assert!(!app.server.remote());
    }

    #[test]
    fn disconnecting_a_system_closes_its_tab_and_lands_on_a_local_one() {
        let mut app = App::new();
        let target = crate::daemon::Target { url: "ws://127.0.0.1:40000/session".into(), ..Default::default() };
        let system = Selection { id: "system:s1".into(), system: Some("s1".into()), ..Selection::default() };
        app.switch_connection(Connected { selection: system, target: Some(target), workspace: Some(("home-x".into(), "box".into())), tunnel: None });
        assert_eq!((app.workspaces.len(), app.workspace_active), (2, 1));
        app.disconnect_system("s1");
        assert_eq!((app.workspaces.len(), app.workspace_active), (1, 0));
        assert!(!app.server.remote());
        assert!(app.workspaces[0].server.is_none());
        // Nothing open on it: nothing closes.
        app.disconnect_system("s1");
        assert_eq!(app.workspaces.len(), 1);
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
