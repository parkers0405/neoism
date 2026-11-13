//! Servers and Systems, as the GPUiX desktop runs them (`native/src/server_management.rs`,
//! `ssh_transport.rs`, `menu-actions/ChromeActionsController.ts`, `server-actions.ts`):
//!
//! - Servers: the saved registry (`servers.json` with its credentials store), servers hosted on
//!   this machine (a persistent workspace daemon, `host` / `rehost`), Fast Servers hosted
//!   through OpenTunnel (`host-fast`, temporary) and joined from an invite (`join-fast`).
//! - Systems: saved SSH recipes (`systems.json`); connecting bootstraps the remote daemon and a
//!   loopback tunnel (`attach_workspace_over_ssh`).
//!
//! Every side effect runs on a worker thread; the UI polls the result each frame. Selecting a
//! server or connecting a system dials it first (`probe`) and only then switches the window's
//! connection (`daemon::switch_target`), so a host that can't be reached leaves the current
//! workspace as it was.

pub mod form;
pub mod flow;

use crate::daemon::Target;
use neoism_agent_service_api::server_registry::{HostedServerSpec, SavedServer, ServerRegistry, registry_directory};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How long a selected server gets to finish its handshake (`waitForConnection`).
const HANDSHAKE: Duration = Duration::from_secs(10);

/// Process-owned temporary servers, shared by all Rune windows. Switching targets or closing
/// one window must not revoke a shareable invite; application exit calls `shutdown`.
#[derive(Default)]
struct Store {
    fast: HashMap<String, crate::fast_servers::FastHost>,
    joins: HashMap<String, Selection>,
}

impl Store {
    fn add_host(&mut self, host: crate::fast_servers::FastHost) -> Selection {
        let selection = fast_selection(&host);
        self.fast.insert(host.id.clone(), host);
        selection
    }

    fn remove_temporary(&mut self, id: &str) -> bool {
        if let Some(mut host) = self.fast.remove(id) {
            host.stop();
            return true;
        }
        self.joins.remove(id).is_some()
    }

    fn temporary_entries(&mut self) -> Vec<ServerEntry> {
        // `is_running` survives relay reconnects and zero connected clients. Only terminal
        // failure/explicit stop withdraws the row; dropping the guard reaps its worker.
        self.fast.retain(|_, host| host.is_running());
        self.fast.values().map(|host| ServerEntry {
            id: host.id.clone(), name: host.name.clone(), address: host.endpoint.clone(),
            temporary: true, hosted: true, online: Some(true),
        }).chain(self.joins.values().map(|join| ServerEntry {
            id: join.id.clone(), name: join.name.clone(), address: join.endpoint.clone(),
            temporary: true, hosted: false, online: None,
        })).collect()
    }
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
fn store() -> std::sync::MutexGuard<'static, Store> {
    STORE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// A server row (`server.list`).
#[derive(Clone, Debug, PartialEq)]
pub struct ServerEntry {
    pub id: String,
    pub name: String,
    pub address: String,
    pub temporary: bool,
    pub hosted: bool,
    /// A Fast Server this process hosts: whether its tunnel still runs.
    pub online: Option<bool>,
}

/// What to dial (`ServerSelection`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub pairing_token: Option<String>,
    pub auth_token: Option<String>,
    pub temporary: bool,
    pub hosted: bool,
    pub invite: Option<String>,
    /// A saved SSH system to tunnel to first (`ssh.systemId`).
    pub system: Option<String>,
}

impl Selection {
    pub fn local() -> Selection { Selection { id: "local".into(), name: "Local Server".into(), ..Selection::default() } }
    pub fn is_local(&self) -> bool { self.id == "local" }
}

fn registry() -> Result<ServerRegistry, String> { ServerRegistry::load(registry_directory()).map_err(|e| e.to_string()) }

/// The public listing reads only the atomically replaced metadata file (no credentials).
fn saved_entries() -> Result<Vec<SavedServer>, String> {
    let path = registry_directory().join("servers.json");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let data: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    serde_json::from_value(data.get("servers").cloned().unwrap_or_else(|| json!([]))).map_err(|e| e.to_string())
}

/// Saved servers (not the agent API's), then this process's live Fast hosts and joins.
pub fn list() -> Vec<ServerEntry> {
    let mut out: Vec<ServerEntry> = saved_entries().unwrap_or_default().into_iter().filter(|s| !s.agent_api).map(|s| ServerEntry {
        id: s.id, name: s.name, address: s.endpoint, temporary: false, hosted: s.hosted.is_some(), online: None,
    }).collect();
    out.extend(store().temporary_entries());
    out
}

fn selection(registry: &ServerRegistry, id: &str) -> Result<Selection, String> {
    let server = registry.server(id).ok_or("Saved server no longer exists")?;
    Ok(Selection { id: id.into(), name: server.name.clone(), endpoint: server.endpoint.clone(),
        pairing_token: registry.token(id).map(str::to_owned), hosted: server.hosted.is_some(), ..Selection::default() })
}

/// `server.resolve` / `server.edit`: a temporary host or join, else the saved entry.
pub fn resolve(id: &str) -> Result<Selection, String> {
    {
        let store = store();
        if let Some(host) = store.fast.get(id) {
            if !host.is_running() { return Err("Fast Server has stopped".into()); }
            return Ok(fast_selection(host));
        }
        if let Some(join) = store.joins.get(id) { return Ok(join.clone()); }
    }
    selection(&registry()?, id)
}

fn fast_selection(host: &crate::fast_servers::FastHost) -> Selection {
    Selection { id: host.id.clone(), name: host.name.clone(), endpoint: host.endpoint.clone(), pairing_token: Some(host.token.clone()),
        auth_token: Some(host.token.clone()), temporary: true, hosted: true, invite: Some(host.public_url.clone()), system: None }
}

fn expand_home(raw: &str) -> Result<PathBuf, String> {
    let home = || dirs::home_dir().ok_or_else(|| "Cannot resolve home folder".to_owned());
    Ok(if raw == "~" { home()? } else if let Some(rest) = raw.strip_prefix("~/") { home()?.join(rest) } else { PathBuf::from(raw) })
}

/// Where this machine's agent server listens: the environment's choice, else the endpoint the
/// local daemon published beside its own.
fn local_agent_server() -> Option<String> {
    if let Ok(server) = std::env::var("NEOISM_AGENT_SERVER").or_else(|_| std::env::var("NEOISM_SERVER")) { return Some(server); }
    let published = neoism_agent_service_api::daemon_credential::canonical_daemon_token_path().with_file_name("agent-endpoint.json");
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(published).ok()?).ok()?;
    Some(format!("http://{}", record["address"].as_str()?))
}

fn host(spec: &HostedServerSpec, password: Option<&str>) -> Result<(), String> {
    std::fs::create_dir_all(&spec.state_dir).map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(crate::service_process::standalone_executable().map_err(|e| e.to_string())?);
    command.args(["--neoism-internal-hosted-workspace-daemon", "--addr", &format!("0.0.0.0:{}", spec.port), "--no-unix-socket", "--state-dir"])
        .arg(&spec.state_dir).arg("--workspace").arg(&spec.workspace_dir);
    command.env_remove("NEOISM_FAST_READINESS_NONCE").env("NEOISM_REQUIRE_AUTH", if spec.require_auth { "1" } else { "0" });
    if let Some(password) = password { command.env("NEOISM_DAEMON_TOKEN", password); } else { command.env_remove("NEOISM_DAEMON_TOKEN"); }
    // The hosted daemon shares this machine's agent. The local daemon may have put it on
    // another port than the default (`--auto-agent-port`): say where it is.
    if let Some(agent) = local_agent_server() { command.env("NEOISM_AGENT_SERVER", &agent).env("NEOISM_SERVER", &agent); }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid only detaches the child into its own session before exec.
        unsafe { command.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }); }
    }
    crate::service_process::launch_persistent_host(&mut command, spec.port, password, || {
        let mut file = tempfile::NamedTempFile::new_in(&spec.state_dir)?;
        file.write_all(&serde_json::to_vec_pretty(spec)?)?;
        file.as_file().sync_all()?;
        file.persist(spec.state_dir.join("hosted-spec.json")).map_err(|e| e.error)?;
        Ok(())
    }).map(|_| ()).map_err(|e| e.to_string())
}

/// Only the exact persistent daemon recipe may be stopped. Never signal a process just because
/// it owns the port.
fn stop_hosted(spec: &HostedServerSpec) {
    #[cfg(target_os = "linux")]
    if let Ok(entries) = std::fs::read_dir("/proc") {
        let state = spec.state_dir.to_string_lossy().into_owned();
        let address = format!("0.0.0.0:{}", spec.port);
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else { continue };
            let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else { continue };
            let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.trim_end_matches(" (deleted)") != "neoism-workspace-daemon" { continue; }
            let Ok(bytes) = std::fs::read(entry.path().join("cmdline")) else { continue };
            let args: Vec<String> = bytes.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
            if args.iter().any(|a| a == "--neoism-internal-hosted-workspace-daemon")
                && args.windows(2).any(|p| p[0] == "--state-dir" && p[1] == state)
                && args.windows(2).any(|p| p[0] == "--addr" && p[1] == address)
            {
                // SAFETY: a plain signal to a process matched by its exact launch recipe.
                unsafe { libc::kill(pid, libc::SIGTERM); }
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = spec;
}

/// The Add / Edit Server form's operation (`serverFormOperation` + the native `work`).
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Save (add or update) a server to join.
    Save { id: Option<String>, address: String, name: String, password: String },
    /// Host the project folder on this machine (saved, relaunchable).
    Host { directory: String, name: String, password: String },
    /// Host a Fast Server (OpenTunnel, temporary; password required).
    HostFast { directory: String, name: String, password: String },
    /// Join a Fast Server invite (temporary).
    JoinFast { address: String, name: String, password: String },
}

fn optional(value: &str) -> Option<&str> { Some(value).filter(|v| !v.is_empty()) }

/// Run one form operation; returns the server to dial.
pub fn run(operation: Operation) -> Result<Selection, String> {
    match operation {
        Operation::Save { id, address, name, password } => {
            let mut registry = registry()?;
            let server = match id {
                Some(id) => registry.update(&id, &address, optional(&name), optional(&password))?,
                None => registry.add(&address, optional(&name), optional(&password))?,
            };
            selection(&registry, &server.id)
        }
        Operation::JoinFast { address, name, password } => {
            let join = crate::fast_servers::normalize_fast_join(address, optional(&password).map(str::to_owned))?;
            if join.token.as_ref().is_none_or(|t| t.trim().is_empty()) {
                return Err("Enter the host's password or an invite containing access".into());
            }
            let id = format!("fast-{}", uuid::Uuid::new_v4());
            let selection = Selection { id: id.clone(), name: optional(&name).map(str::to_owned).unwrap_or(join.name), endpoint: join.endpoint,
                pairing_token: join.token.clone(), auth_token: join.token, temporary: true, ..Selection::default() };
            store().joins.insert(id, selection.clone());
            Ok(selection)
        }
        Operation::HostFast { directory, name, password } => {
            let agent = local_agent_server().unwrap_or_else(|| "http://127.0.0.1:4096".into());
            let hosted = crate::fast_servers::create_fast_server(directory, optional(&name).map(str::to_owned), optional(&password).map(str::to_owned), agent)?;
            Ok(store().add_host(hosted))
        }
        Operation::Host { directory, name, password } => {
            let directory = std::fs::canonicalize(expand_home(&directory)?).map_err(|_| "Project folder does not exist")?;
            if !directory.is_dir() { return Err("Project folder is not a directory".into()); }
            let port = (9877..9900).find(|port| std::net::TcpListener::bind(("0.0.0.0", *port)).is_ok())
                .ok_or("No free hosted-server port between 9877 and 9899")?;
            let spec = HostedServerSpec { state_dir: registry_directory().join("hosted-servers").join(port.to_string()),
                workspace_dir: directory, port, require_auth: optional(&password).is_some() };
            host(&spec, optional(&password))?;
            let saved = (|| {
                let mut registry = registry()?;
                let server = registry.add_hosted(&format!("ws://127.0.0.1:{port}/session"), optional(&name), optional(&password), spec.clone())?;
                selection(&registry, &server.id)
            })();
            if saved.is_err() { stop_hosted(&spec); }
            saved
        }
    }
}

/// Relaunch a saved server hosted on this machine whose daemon stopped (`rehost`).
fn rehost(id: &str) -> Result<Selection, String> {
    let registry = registry()?;
    let spec = registry.hosted_spec(id).ok_or("This server is not hosted on this machine")?;
    host(&spec, registry.token(id))?;
    selection(&registry, id)
}

/// Forget a server: stop a Fast host or leave a join; a saved server hosted here stops too.
pub fn remove(id: &str) -> Result<(), String> {
    if store().remove_temporary(id) { return Ok(()); }
    let mut registry = registry()?;
    let hosted = registry.hosted_spec(id);
    registry.remove(id).map_err(|e| e.to_string())?;
    if let Some(spec) = hosted { stop_hosted(&spec); }
    Ok(())
}

/// The Fast Server invite to share (`server-invite`).
pub fn invite(id: &str) -> Result<String, String> {
    resolve(id)?.invite.ok_or_else(|| "This server has no Fast Server invite".into())
}

/// Stop every Fast Server this process hosts (on application exit, not individual window close).
pub fn shutdown() {
    let hosts: Vec<_> = store().fast.drain().map(|(_, h)| h).collect();
    for mut host in hosts { host.stop_and_wait(); }
    store().joins.clear();
}

// -- systems --------------------------------------------------------------------------------

/// A saved SSH system (`ssh.systems.list`).
#[derive(Clone, Debug, PartialEq)]
pub struct SystemEntry {
    pub id: String,
    pub name: String,
    pub target: String,
    pub enabled: bool,
}

pub fn systems() -> Vec<SystemEntry> {
    crate::system_registry::SystemRegistry::load().map(|r| r.systems().iter().map(|s| SystemEntry {
        id: s.id.clone(), name: s.name.clone(), target: s.target.clone(), enabled: s.enabled,
    }).collect()).unwrap_or_default()
}

/// Save an SSH system (`ssh.systems.add`): new systems are saved deactivated.
pub fn add_system(target: &str, name: &str) -> Result<(), String> {
    let target = target.trim();
    if target.is_empty() || target.starts_with('-') || target.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("Enter an SSH host alias or user@host".into());
    }
    let mut registry = crate::system_registry::SystemRegistry::load().map_err(|e| e.to_string())?;
    registry.upsert(name.trim(), target, Vec::new()).map(|_| ()).map_err(|e| e.to_string())
}

pub fn remove_system(id: &str) -> Result<(), String> {
    let mut registry = crate::system_registry::SystemRegistry::load().map_err(|e| e.to_string())?;
    registry.remove(id).map_err(|e| e.to_string())
}

pub fn set_system_enabled(id: &str, enabled: bool) -> Result<(), String> {
    let mut registry = crate::system_registry::SystemRegistry::load().map_err(|e| e.to_string())?;
    registry.set_enabled(id, enabled).map(|_| ()).map_err(|e| e.to_string())
}

// -- connecting -----------------------------------------------------------------------------

/// A verified connection, ready to switch to.
pub struct Connected {
    pub selection: Selection,
    /// `None` dials the local daemon again.
    pub target: Option<Target>,
    /// The workspace to open there: the SSH home workspace (created), else the daemon's choice.
    pub workspace: Option<(String, String)>,
    /// The SSH tunnel; dropping it closes the connection.
    pub tunnel: Option<crate::ssh_hosts::DaemonAttach>,
}

/// Dial `selection` and complete the handshake (`prepareServerConnection`): tunnel to an SSH
/// system first; a saved server hosted here whose daemon stopped is relaunched and retried.
pub fn dial(selection: Selection) -> Result<Connected, String> {
    if selection.is_local() { return Ok(Connected { selection, target: None, workspace: None, tunnel: None }); }
    if let Some(system_id) = selection.system.clone() {
        let registry = crate::system_registry::SystemRegistry::load().map_err(|e| e.to_string())?;
        let system = registry.system(&system_id).ok_or("Saved system not found")?.clone();
        let attach = crate::ssh_hosts::attach_workspace_over_ssh(&system.target, system.ssh_args.clone()).map_err(|e| e.to_string())?;
        let target = Target { url: attach.daemon_url.clone(), auth_token: Some(attach.credential.clone()), pairing_token: None };
        crate::daemon::probe(&target, HANDSHAKE)?;
        let workspace = Some((attach.workspace_id.clone(), format!("{} \u{b7} Home", attach.alias)));
        return Ok(Connected { selection, target: Some(target), workspace, tunnel: Some(attach) });
    }
    let target = |s: &Selection| Target { url: s.endpoint.clone(), auth_token: s.auth_token.clone(), pairing_token: s.pairing_token.clone() };
    match crate::daemon::probe(&target(&selection), HANDSHAKE) {
        Ok(()) => Ok(Connected { target: Some(target(&selection)), selection, workspace: None, tunnel: None }),
        // A server hosted here whose persistent daemon stopped: relaunch it, then dial again.
        Err(_) if selection.hosted && !selection.temporary => {
            let ready = rehost(&selection.id)?;
            crate::daemon::probe(&target(&ready), HANDSHAKE)?;
            Ok(Connected { target: Some(target(&ready)), selection: ready, workspace: None, tunnel: None })
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_host_is_listed_without_clients_until_its_worker_ends() {
        let mut store = Store::default();
        let (host, ended) = crate::fast_servers::test_owned_host();
        let selected = store.add_host(host);
        let entries = store.temporary_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], ServerEntry {
            id: selected.id.clone(), name: selected.name.clone(), address: selected.endpoint.clone(),
            temporary: true, hosted: true, online: Some(true),
        });
        assert!(selected.invite.is_some());
        // Listing repeatedly (with no attached clients) must not revoke the invite.
        assert_eq!(store.temporary_entries(), entries);
        ended.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while store.fast[&selected.id].is_running() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(store.temporary_entries().is_empty());
        assert!(!store.fast.contains_key(&selected.id));
    }

    #[test]
    fn stopped_fast_host_is_pruned_without_forgetting_guest_invites() {
        let mut store = Store::default();
        let (host, _ended) = crate::fast_servers::test_owned_host();
        let selected = store.add_host(host);
        let join = Selection { id: "guest".into(), temporary: true, ..Selection::default() };
        store.joins.insert(join.id.clone(), join.clone());
        store.fast.get_mut(&selected.id).unwrap().stop_and_wait();
        let entries = store.temporary_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, join.id);
        assert!(!entries[0].hosted);
        assert!(store.fast.is_empty());
    }

    #[test]
    fn removing_fast_host_withdraws_row_and_signals_owned_worker() {
        let mut store = Store::default();
        let (host, ended) = crate::fast_servers::test_owned_host();
        let selected = store.add_host(host);
        assert!(store.remove_temporary(&selected.id));
        assert!(store.temporary_entries().is_empty());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !ended.is_closed() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(ended.is_closed(), "removed host's worker must receive shutdown");
        assert!(!store.remove_temporary(&selected.id));
    }

    /// Registry tests run against a scratch config home, never the user's.
    fn scratch() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the registry tests below are the only NEOISM_CONFIG_HOME users in this binary
        // that write; they run in one test (sequentially).
        unsafe { std::env::set_var("NEOISM_CONFIG_HOME", dir.path()); }
        dir
    }

    #[test]
    fn servers_and_systems_save_list_and_remove_in_a_scratch_home() {
        let dir = scratch();
        let saved = run(Operation::Save { id: None, address: "ws://192.168.1.20:7981/session".into(), name: "Team".into(), password: "pw".into() }).unwrap();
        assert_eq!((saved.name.as_str(), saved.pairing_token.as_deref()), ("Team", Some("pw")));
        assert!(list().iter().any(|s| s.id == saved.id && s.address == "ws://192.168.1.20:7981/session" && !s.temporary));
        let edited = run(Operation::Save { id: Some(saved.id.clone()), address: "ws://10.0.0.2:7981/session".into(), name: "Team 2".into(), password: String::new() }).unwrap();
        assert_eq!(edited.id, saved.id);
        assert_eq!(resolve(&saved.id).unwrap().endpoint, "ws://10.0.0.2:7981/session");
        remove(&saved.id).unwrap();
        assert!(list().iter().all(|s| s.id != saved.id));

        assert!(add_system("-oProxyCommand=x", "").is_err());
        add_system("me@build-box", "Build box").unwrap();
        let system = systems().into_iter().find(|s| s.target == "me@build-box").unwrap();
        assert!(!system.enabled, "new systems are saved deactivated");
        set_system_enabled(&system.id, true).unwrap();
        assert!(systems()[0].enabled);
        remove_system(&system.id).unwrap();
        assert!(systems().is_empty());
        unsafe { std::env::remove_var("NEOISM_CONFIG_HOME"); }
        drop(dir);
    }

    #[test]
    fn a_fast_join_needs_a_complete_invite_and_access() {
        assert!(run(Operation::JoinFast { address: "not a url".into(), name: String::new(), password: String::new() }).is_err());
    }

    #[test]
    fn dialing_an_unreachable_server_fails() {
        let selection = Selection { id: "x".into(), endpoint: "ws://127.0.0.1:1/session".into(), ..Selection::default() };
        assert!(dial(selection).is_err());
    }

    /// A one-connection fake daemon on loopback: answers `Hello` with `reply`, and reports the
    /// pairing token and URL token it was given.
    fn fake_daemon(reply: serde_json::Value) -> (String, std::thread::JoinHandle<(Option<String>, String)>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/session", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let path = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let seen = path.clone();
            let mut socket = tungstenite::accept_hdr(stream, move |request: &tungstenite::handshake::server::Request, response| {
                *seen.lock().unwrap() = request.uri().to_string();
                Ok(response)
            }).unwrap();
            let hello: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            let token = hello["Workspace"]["message"]["Hello"]["token"].as_str().map(str::to_owned);
            socket.send(tungstenite::Message::text(reply.to_string())).unwrap();
            let _ = socket.read();
            let uri = path.lock().unwrap().clone();
            (token, uri)
        });
        (url, handle)
    }

    #[test]
    fn a_daemon_that_acknowledges_is_dialled_with_its_credentials() {
        let (url, daemon) = fake_daemon(json!({"WorkspaceReply": {"request_id": 0, "message": {"HelloAck": {"server_version": "test"}}}}));
        let selection = Selection { id: "s".into(), endpoint: url.clone(), pairing_token: Some("pair".into()), auth_token: Some("auth".into()), ..Selection::default() };
        let connected = dial(selection).unwrap();
        let target = connected.target.unwrap();
        assert_eq!((target.url.as_str(), target.pairing_token.as_deref()), (url.as_str(), Some("pair")));
        let (token, uri) = daemon.join().unwrap();
        assert_eq!(token.as_deref(), Some("pair"));
        assert!(uri.ends_with("/session?token=auth"), "{uri}");
    }

    #[test]
    fn a_daemon_that_rejects_the_pairing_says_why() {
        let (url, daemon) = fake_daemon(json!({"WorkspaceReply": {"request_id": 0, "message": {"Error": {"message": "pairing token rejected"}}}}));
        let error = dial(Selection { id: "s".into(), endpoint: url, ..Selection::default() }).err().unwrap();
        assert_eq!(error, "pairing token rejected");
        let _ = daemon.join();
    }
}
