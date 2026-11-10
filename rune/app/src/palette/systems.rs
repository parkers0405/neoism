//! Systems and Servers (`command_palette` Systems/Servers/SystemSettings modes): the rail's
//! server button and `workspace: Systems` / `workspace: Servers` open the palette on a
//! category header. Systems lists this computer and the SSH computers saved in
//! `systems.json`; Servers lists the daemon this window uses and the saved `servers.json`
//! entries. Both registries are the desktop app's (`$XDG_CONFIG_HOME/neoism`) and are only
//! read here: managing SSH systems and switching servers still lives in the desktop host.

use crate::app::Connection;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemStatus {
    Connected,
    /// The desktop host's managed SSH connection in progress; Rune holds none yet.
    #[allow(dead_code)]
    Connecting,
    Disconnected,
}

impl SystemStatus {
    pub fn label(self) -> &'static str {
        match self {
            SystemStatus::Connected => "Connected",
            SystemStatus::Connecting => "Connecting",
            SystemStatus::Disconnected => "Disconnected",
        }
    }
}

/// `ServerIndicatorStatus`: only the server this window uses has a live status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    Online,
    Connecting,
    Offline,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct System {
    pub id: String,
    pub name: String,
    pub address: String,
    pub local: bool,
    /// The durable "Active" preference (always on for this computer).
    pub enabled: bool,
    pub status: SystemStatus,
}

impl System {
    /// The row's right-hand state (`PaletteRow::shortcut` for systems).
    pub fn state(&self) -> &'static str {
        if !self.local && !self.enabled { "Deactivated" } else { self.status.label() }
    }

    /// `address · state`, or the state alone.
    pub fn detail(&self) -> String {
        if self.address.is_empty() { self.state().to_owned() } else { format!("{} \u{b7} {}", self.address, self.state()) }
    }

    /// The settings heading: `name · address · status[ · Read only]`.
    pub fn heading(&self) -> String {
        format!("{} \u{b7} {} \u{b7} {}{}", self.name, self.address, self.status.label(), if self.local { " \u{b7} Read only" } else { "" })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub address: String,
    pub local: bool,
    /// The server this window is connected to.
    pub active: bool,
    pub status: ServerStatus,
}

/// One row of a system's settings (`PaletteMode::SystemSettings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Active,
    Disconnect,
    Connect,
    Ssh,
    Remove,
}

impl Setting {
    pub fn title(self) -> &'static str {
        match self {
            Setting::Active => "Active",
            Setting::Disconnect => "Disconnect",
            Setting::Connect => "Connect",
            Setting::Ssh => "SSH to system",
            Setting::Remove => "Remove",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Setting::Active => "Keep agent chats connected in the background. Deactivating disconnects this system.",
            Setting::Disconnect => "Disconnect this computer's live connections and remote workspaces",
            Setting::Connect => "Retry the managed background connection",
            Setting::Ssh => "Open a remote workspace for this computer's terminal and files; does not change Active",
            Setting::Remove => "Disconnect and forget this system; remote files are not deleted",
        }
    }
}

/// This computer's settings are read only; a saved system has Active, then Disconnect or
/// Connect by its state, then SSH and Remove.
pub fn settings(system: &System) -> Vec<Setting> {
    if system.local { return Vec::new(); }
    let mut out = vec![Setting::Active];
    match system.status {
        SystemStatus::Connected | SystemStatus::Connecting => out.push(Setting::Disconnect),
        SystemStatus::Disconnected if system.enabled => out.push(Setting::Connect),
        SystemStatus::Disconnected => {}
    }
    out.extend([Setting::Ssh, Setting::Remove]);
    out
}

/// The desktop registries' directory (`server_registry::registry_directory`).
pub fn registry_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("NEOISM_CONFIG_HOME") { return path.into(); }
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("neoism")
}

#[derive(Deserialize)]
struct SavedSystem {
    id: String,
    name: String,
    target: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

fn enabled_by_default() -> bool { true }

#[derive(Deserialize)]
struct SavedServer {
    id: String,
    name: String,
    endpoint: String,
}

fn read_list<T: for<'de> Deserialize<'de>>(file: &str, key: &str) -> Vec<T> {
    let Ok(bytes) = std::fs::read(registry_directory().join(file)) else { return Vec::new() };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return Vec::new() };
    value.get(key).and_then(|list| serde_json::from_value(list.clone()).ok()).unwrap_or_default()
}

/// This computer first, then the saved SSH systems. Rune holds no SSH connections, so saved
/// systems read as Disconnected (or Deactivated).
pub fn systems() -> Vec<System> {
    let mut out = vec![System { id: "local".into(), name: "This computer".into(), address: "Local system".into(),
        local: true, enabled: true, status: SystemStatus::Connected }];
    out.extend(read_list::<SavedSystem>("systems.json", "systems").into_iter().map(|s| System {
        id: s.id, name: s.name, address: s.target, local: false, enabled: s.enabled, status: SystemStatus::Disconnected,
    }));
    out
}

/// The local daemon this window uses, then the saved servers.
pub fn servers(connection: &Connection) -> Vec<Server> {
    let status = match connection {
        Connection::Connected => ServerStatus::Online,
        Connection::Connecting => ServerStatus::Connecting,
        Connection::Disconnected(_) => ServerStatus::Offline,
    };
    let address = crate::daemon::public_endpoint().unwrap_or_else(|| "This computer".into());
    let mut out = vec![Server { id: "local".into(), name: "Local Server".into(), address, local: true, active: true, status }];
    out.extend(read_list::<SavedServer>("servers.json", "servers").into_iter().map(|s| Server {
        id: s.id, name: s.name, address: s.endpoint, local: false, active: false, status: ServerStatus::Unknown,
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(enabled: bool, status: SystemStatus) -> System {
        System { id: "r".into(), name: "Remote".into(), address: "user@host".into(), local: false, enabled, status }
    }

    #[test]
    fn settings_follow_the_system_state() {
        assert_eq!(settings(&systems()[0]), vec![]);
        assert_eq!(settings(&remote(true, SystemStatus::Connected)), vec![Setting::Active, Setting::Disconnect, Setting::Ssh, Setting::Remove]);
        assert_eq!(settings(&remote(true, SystemStatus::Disconnected)), vec![Setting::Active, Setting::Connect, Setting::Ssh, Setting::Remove]);
        assert_eq!(settings(&remote(false, SystemStatus::Disconnected)), vec![Setting::Active, Setting::Ssh, Setting::Remove]);
        assert_eq!(remote(false, SystemStatus::Disconnected).detail(), "user@host \u{b7} Deactivated");
        assert_eq!(systems()[0].heading(), "This computer \u{b7} Local system \u{b7} Connected \u{b7} Read only");
    }

    #[test]
    fn registries_are_read_from_the_config_home() {
        let dir = std::env::temp_dir().join(format!("neoism-rune-systems-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("systems.json"), br#"{"systems":[{"id":"a","name":"Build box","target":"me@box","ssh_args":[],"enabled":false}]}"#).unwrap();
        std::fs::write(dir.join("servers.json"), br#"{"servers":[{"id":"s","name":"Team","endpoint":"ws://team:9877/session"}],"window_profiles":{}}"#).unwrap();
        // SAFETY: tests in this module are the only readers of NEOISM_CONFIG_HOME.
        unsafe { std::env::set_var("NEOISM_CONFIG_HOME", &dir); }
        let systems = systems();
        let servers = servers(&Connection::Connected);
        unsafe { std::env::remove_var("NEOISM_CONFIG_HOME"); }
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(systems.len(), 2);
        assert_eq!((systems[1].name.as_str(), systems[1].state()), ("Build box", "Deactivated"));
        assert_eq!(servers.len(), 2);
        assert!(servers[0].active && servers[0].status == ServerStatus::Online);
        assert_eq!((servers[1].name.as_str(), servers[1].status), ("Team", ServerStatus::Unknown));
    }
}
