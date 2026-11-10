//! Systems and Servers (`command_palette` Systems/Servers/SystemSettings modes): the rail's
//! server button and `workspace: Systems` / `workspace: Servers` open the palette on a
//! category header. Systems lists this computer and the SSH computers saved in
//! `systems.json`; Servers lists the daemon this window uses and the saved `servers.json`
//! entries, Fast Servers this window hosts and invites it joined. Both registries are the
//! desktop app's (`$XDG_CONFIG_HOME/neoism`); `crate::servers` reads and changes them.

use crate::app::Connection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemStatus {
    Connected,
    /// The SSH connection in progress.
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
    /// A Fast Server hosted here or joined (not saved).
    pub temporary: bool,
    /// Hosted on this machine.
    pub hosted: bool,
}

/// A server row's buttons (`PaletteServerEntry::controls`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Edit,
    Remove,
    CopyInvite,
    StopHosting,
    Leave,
}

impl Control {
    pub fn label(self) -> &'static str {
        match self {
            Control::Edit => "Edit",
            Control::Remove => "Remove",
            Control::CopyInvite => "Copy invite",
            Control::StopHosting => "Stop hosting",
            Control::Leave => "Leave",
        }
    }
    /// The chip's width in points.
    pub fn width(self) -> f32 {
        match self { Control::Edit => 52.0, Control::Remove => 66.0, Control::CopyInvite => 88.0, Control::StopHosting => 96.0, Control::Leave => 56.0 }
    }
}

impl Server {
    pub fn controls(&self) -> Vec<Control> {
        if self.local { return Vec::new(); }
        match (self.temporary, self.hosted) {
            (true, true) => vec![Control::CopyInvite, Control::StopHosting],
            (true, false) => vec![Control::Leave],
            _ => vec![Control::Edit, Control::Remove],
        }
    }
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

impl crate::app::App {
    /// This computer first, then the saved SSH systems with their live state: Connected while
    /// this window's tunnel to it is up, Connecting while one is being made.
    pub fn palette_systems(&mut self) -> Vec<System> {
        let here = self.server.system();
        let connecting = self.server.connecting().and_then(|id| id.strip_prefix("system:")).map(str::to_owned);
        let mut out = vec![System { id: "local".into(), name: "This computer".into(), address: "Local system".into(),
            local: true, enabled: true, status: if self.server.remote() { SystemStatus::Disconnected } else { SystemStatus::Connected } }];
        out.extend(crate::servers::systems().into_iter().map(|s| {
            let status = if here.as_deref() == Some(s.id.as_str()) { SystemStatus::Connected }
                else if connecting.as_deref() == Some(s.id.as_str()) { SystemStatus::Connecting }
                else { SystemStatus::Disconnected };
            System { id: s.id, name: s.name, address: s.target, local: false, enabled: s.enabled, status }
        }));
        out
    }

    /// Local Server, then the saved servers and this window's temporary ones; the one this
    /// window uses carries the live connection state.
    pub fn palette_servers(&self) -> Vec<Server> {
        let live = match &self.connection {
            Connection::Connected => ServerStatus::Online,
            Connection::Connecting => ServerStatus::Connecting,
            Connection::Disconnected(_) => ServerStatus::Offline,
        };
        let current = self.server.current_id().to_owned();
        let connecting = self.server.connecting().map(str::to_owned);
        let status = |id: &str, online: Option<bool>| if id == current { live }
            else if connecting.as_deref() == Some(id) { ServerStatus::Connecting }
            else if online == Some(false) { ServerStatus::Offline } else { ServerStatus::Unknown };
        let address = crate::daemon::public_endpoint().unwrap_or_else(|| "This computer".into());
        let mut out = vec![Server { id: "local".into(), name: "Local Server".into(), address, local: true, active: current == "local",
            status: status("local", None), temporary: false, hosted: false }];
        out.extend(crate::servers::list().into_iter().map(|s| Server {
            active: s.id == current, status: status(&s.id, s.online), id: s.id, name: s.name, address: s.address, local: false,
            temporary: s.temporary, hosted: s.hosted,
        }));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(enabled: bool, status: SystemStatus) -> System {
        System { id: "r".into(), name: "Remote".into(), address: "user@host".into(), local: false, enabled, status }
    }

    #[test]
    fn settings_follow_the_system_state() {
        let local = System { id: "local".into(), name: "This computer".into(), address: "Local system".into(), local: true, enabled: true, status: SystemStatus::Connected };
        assert_eq!(settings(&local), vec![]);
        assert_eq!(settings(&remote(true, SystemStatus::Connected)), vec![Setting::Active, Setting::Disconnect, Setting::Ssh, Setting::Remove]);
        assert_eq!(settings(&remote(true, SystemStatus::Disconnected)), vec![Setting::Active, Setting::Connect, Setting::Ssh, Setting::Remove]);
        assert_eq!(settings(&remote(false, SystemStatus::Disconnected)), vec![Setting::Active, Setting::Ssh, Setting::Remove]);
        assert_eq!(remote(false, SystemStatus::Disconnected).detail(), "user@host \u{b7} Deactivated");
        assert_eq!(local.heading(), "This computer \u{b7} Local system \u{b7} Connected \u{b7} Read only");
    }

    #[test]
    fn server_rows_offer_their_controls() {
        let server = |temporary, hosted| Server { id: "s".into(), name: "S".into(), address: String::new(), local: false, active: false,
            status: ServerStatus::Unknown, temporary, hosted };
        assert_eq!(server(false, false).controls(), vec![Control::Edit, Control::Remove]);
        assert_eq!(server(true, true).controls(), vec![Control::CopyInvite, Control::StopHosting]);
        assert_eq!(server(true, false).controls(), vec![Control::Leave]);
    }

    #[test]
    fn local_server_is_listed_first_and_active() {
        let app = crate::app::App::new();
        let servers = app.palette_servers();
        assert!(servers[0].local && servers[0].active);
    }
}
