//! The application state every view reads, and the daemon event loop that changes it. Feature
//! modules (`agent`, `home`, `tabs`) add their own `impl App` blocks next to their views.

use crate::agent::Session;
use crate::daemon::{Daemon, Family, Inbound, Inbox};
use crate::ui::status::GitSummary;
use neoism_protocol::git::GitRepoStatus;
use serde_json::json;
use crate::home::Terminal;
use crate::tabs::{Tab, TabKind};
use crate::ui::theme::{Palette, Type, palette};
use crate::wordmark::Wordmark;
use neoism_protocol::agent::ThreadSummary;
use neoism_protocol::pty::ServerMessage as PtyServer;
use rune::ImageCache;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub enum Connection {
    Connecting,
    Connected,
    Disconnected(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sidebar {
    Conversations,
    Files,
    Notes,
}

/// Window keyboard focus zones (`rune/README.md`, "Keyboard focus and the cursor").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Content,
    Sidebar,
    Buffers(usize),
    Workspaces(usize),
}

const GIT_WATCH_TOKEN: &str = "neoism-rune-git-status";

pub struct App {
    pub workspaces: Vec<crate::workspace::Entry>,
    pub workspace_active: usize,
    pub workspace_id: String,
    pub workspace_ready: bool,
    pub workspace_closing: Option<u64>,
    pub quit_requested: bool,
    /// The agent transcript's selecting drag, and the text its last release copied.
    pub selection: crate::agent::timeline::Selecting,
    pub copied_selection: Option<String>,
    /// The activity rail button under the pointer (its icon grows).
    pub rail_hover: Option<&'static str>,
    /// Every scroll view's wheel and trackpad motion (`ui/scroll.rs`).
    pub scrollers: crate::ui::scroll::Scrollers,
    /// The activity line is pinned above the composer (following an overflowing transcript).
    pub activity_pinned: bool,
    /// The field-focus sync has run once (a composer is focused before it does).
    pub field_synced: bool,
    /// The next commit lands every track (`Tx::settle`): a conversation's first page opens at
    /// its end instead of growing its rows in and gliding down to them.
    pub settle_pending: bool,
    /// Keeps Rune drawing every vblank while the app steps a motion itself (a wheel glide, a
    /// trackpad coast, the cursor trail), so `poll` steps it once a frame.
    pub heartbeat: crate::ui::chrome::Heartbeat,
    /// The question-rail tick under the pointer: (conversation, tick).
    pub question_hover: Option<(String, usize)>,
    /// The status line's mode pill transition (scramble and colour cross-fade).
    pub mode_pill: crate::ui::status::ModePill,
    /// The composer chips' last labels (agent, model, thinking) and when each changed.
    pub chip_scramble: [(String, f64); 3],
    /// The first Esc of an Esc-Esc stop on a running chat.
    pub esc_at: Option<std::time::Instant>,
    /// The hovered Systems/Servers category tab (or the settings back button).
    pub palette_category_hover: Option<usize>,
    /// Window recovery writes (`session.rs`), and whether the window closed its last workspace.
    pub session_writer: crate::session::Writer,
    pub session_closed: bool,
    pub tab_hover: Option<u64>,
    pub workspace_hover: Option<String>,
    pub chrome_reveal: Vec<String>,
    /// A tab strip's wheel glide target (strip key, offset, when set).
    pub strip_target: Option<(String, f32, std::time::Instant)>,
    pub chrome_scroll: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, [f32; 2]>>>,
    pub tab_drag: Option<crate::ui::tab_drag::TabDrag>,
    pub suppress_tab_press: bool,
    /// The live turn's animated status line ("Crafting . . . · 4.2s model").
    pub activity: Option<crate::agent::Activity>,
    pub replay: Option<crate::replay::Replay>,
    /// The open universal modal (About, confirmations).
    pub modal: Option<crate::ui::modal::Modal>,
    /// A `cd` waiting for the shell to confirm it (request id).
    pub pending_cd: Option<String>,
    /// The terminal tab whose directory the workspace follows (`mainTerminals`); `None` falls back
    /// to the first eligible terminal.
    pub main_terminal: Option<u64>,
    /// Dashboard project folders the user closed in the conversations panel.
    pub collapsed_projects: std::collections::HashSet<String>,
    /// The app clipboard: the last yank, also what `p` falls back to.
    pub clipboard: Option<String>,
    /// The notes sidebar (the workspace's notes vault).
    pub notes: crate::tree::Tree,
    /// The open app menu (rail hamburger).
    pub menu: Option<crate::ui::menu::Menu>,
    /// The open Settings page.
    pub settings: Option<crate::settings::Settings>,
    pub keybindings: serde_json::Value,
    pub databases: HashMap<String, crate::database::Database>,
    /// The window wallpaper (`ui.window.background-image`).
    pub wallpaper: crate::ui::wallpaper::Wallpaper,
    /// The window size in points, from the root layout.
    pub window: [f32; 2],
    /// Background previews by image path (`None` while decoding).
    pub background_previews: std::cell::RefCell<HashMap<std::path::PathBuf, Option<rune::Image>>>,
    /// Pixels decoded off-thread, waiting for the image cache.
    pub decoded_previews: std::sync::Arc<std::sync::Mutex<Vec<(std::path::PathBuf, (u32, u32, Vec<u8>))>>>,
    /// The file tree sidebar.
    pub tree: crate::tree::Tree,
    /// The editor cursor moved: scroll it into view after its next layout.
    pub editor_follow: bool,
    /// The file tree's scroll (offset, max, viewport) and its pending follow target.
    pub tree_follow: crate::ui::follow::Follow,
    /// The conversations panel's keyboard row and its follow.
    pub conv_selected: usize,
    /// The conversations panel's keyboard stop above the list (New chat or the directory row).
    pub conv_header: Option<crate::agent::ConvHeader>,
    /// The conversations panel's project dropdown, while open.
    pub project_menu: Option<crate::agent::ProjectMenu>,
    pub conv_follow: crate::ui::follow::Follow,
    /// A `g` waiting for its second key (`gg`).
    pub pending_g: bool,
    /// A vim count typed in the file tree (`12G`, `5j`).
    pub tree_count: Option<usize>,
    /// Open files by workspace-relative path.
    pub buffers: HashMap<String, crate::editor::Buffer>,
    /// The open project finder (`Alt+S`).
    pub finder: Option<crate::finder::Finder>,
    /// This person's presence name (their orb seed and message author).
    pub local_name: String,
    /// The active workspace's presence and shared-document plane (`collab`).
    pub collab: crate::collab::Collab,
    /// The rail orb under the pointer, whose name shows beside it.
    pub peer_hover: Option<String>,
    /// Other people's carets blink on this, lit for the first half of each half second.
    pub peer_blink: rune::Live<f32>,
    /// Presence orbs (profile pictures) shown so far.
    pub orbs: crate::ui::orb::Orbs,
    /// Wakes the UI thread (poll) from views and background work.
    pub waker: Option<rune::Waker>,
    /// The window cursor: focus bar on chrome, block caret in fields, one trail between them.
    pub cursor: crate::ui::cursor::Cursor,
    /// Laid-out rects of focusable chrome (`pane-tab:N`, `workspace-tab:N`), for the cursor.
    pub rects: HashMap<String, [f32; 4]>,
    /// Sub-agent families: child sessions, their roots and roster order.
    pub families: crate::agent::Families,
    pub header_hover: Option<crate::agent::HeaderStep>,
    /// Branch and line counts from the daemon's Git `RepoStatus` watch.
    pub git: Option<crate::ui::status::GitSummary>,
    pub splash: crate::home::SplashState,
    /// The open centered picker (`Alt+P`, themes), above everything else.
    pub picker: Option<crate::palette::Picker>,
    /// The file browser modal (Change Directory, attaching a picture).
    pub browser: Option<crate::browser::Browser>,
    /// The open form (Add / Edit Server, Add system).
    pub form: Option<crate::servers::form::Form>,
    /// The window's server and any connection in progress (kept across a server switch).
    pub server: crate::servers::flow::ServerState,
    /// The palette's and finder's list slides, mirrored for the window cursor.
    pub list_residual: crate::ui::rows::Residual,
    /// The open diagnostics popup over a status pill (`editor/diag_popup.rs`).
    pub diag_popup: Option<crate::editor::diag_popup::DiagPopup>,
    /// Code blocks whose Copy button was just pressed: their two labels' opacities and when
    /// the button goes back to "Copy" (`agent::timeline`, `poll`).
    pub copy_feedback: Vec<(rune::Live<f32>, rune::Live<f32>, std::time::Instant)>,
    /// When a streaming reply's text last re-ran the views (`agent::StreamPace`).
    pub stream_pace: crate::agent::StreamPace,
    /// A scripted workload for frame-pacing checks (`NEOISM_RUNE_DRIVE`, `drive.rs`).
    pub drive: Option<crate::drive::Drive>,
    pub daemon: Option<Daemon>,
    pub inbox: Option<Inbox>,
    pub images: Option<ImageCache>,
    pub connection: Connection,
    pub pal: &'static Palette,
    pub ty: Type,
    pub workspace_name: String,
    pub cwd: String,
    pub threads: Vec<ThreadSummary>,
    pub sessions: HashMap<String, Session>,
    pub tabs: Vec<Tab>,
    /// Index of the focused pane's active tab.
    pub active: usize,
    /// The split panes and the tabs each one holds.
    pub layout: crate::panes::Layout,
    /// The tab an unfocused pane is drawing while its content is built (see `active_tab`).
    pub render_tab: std::cell::Cell<Option<usize>>,
    /// The tab whose composer `sync_field_focus` last left focused.
    pub field_tab: Option<u64>,
    pub next_tab: u64,
    pub sidebar: Option<Sidebar>,
    /// The `/` commands, the pickers above the composer, and the model/agent/effort choice.
    pub composer: crate::agent::slash::Composer,
    /// Composer attachments and the image preview (`agent::attach`).
    pub attach: crate::agent::attach::Attachments,
    /// The composer whose paperclip the pointer is over.
    pub composer_attach_hover: Option<u64>,
    pub focus: Focus,
    pub agent: String,
    pub model: String,
    pub effort: String,
    pub wordmark: Wordmark,
    pub fps: Option<u32>,
    /// The FPS pill's reading: live text, so a new reading never re-runs the views.
    pub fps_live: rune::LiveText,
    /// When the FPS pill last changed (it refreshes at most twice a second).
    pub fps_at: Option<std::time::Instant>,
    /// Tool rows: expansion, selected group children, spinner clock, service icons.
    pub mcp_icons: crate::agent::mcp::Icons,
    pub tools: crate::agent::tools::ToolState,
    /// Home block terminals by tab id, and the tabs whose `CreatePty` is still in flight.
    pub terminals: HashMap<u64, Terminal>,
    pub pending_pty: VecDeque<u64>,
}

impl App {
    pub fn new() -> App {
        let config = crate::config::load();
        let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        let workspace_name = std::path::Path::new(&cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("~".into());
        let mut app = App {
            workspaces: Vec::new(), workspace_active: 0, workspace_id: crate::workspace::new_id(), workspace_ready: false, workspace_closing: None, quit_requested: false, selection: Default::default(), copied_selection: None, rail_hover: None, scrollers: Default::default(), activity_pinned: false, field_synced: false, settle_pending: false, heartbeat: Default::default(), question_hover: None, mode_pill: Default::default(), chip_scramble: Default::default(), esc_at: None, palette_category_hover: None, session_writer: Default::default(), session_closed: false, tab_hover: None, workspace_hover: None, chrome_reveal: Vec::new(), strip_target: None, chrome_scroll: Default::default(), tab_drag: None, suppress_tab_press: false,
            diag_popup: None,
            copy_feedback: Vec::new(),
            stream_pace: Default::default(),
            drive: crate::drive::Drive::from_env(),
            daemon: None,
            inbox: None,
            images: None,
            connection: Connection::Connecting,
            pal: palette(&config.theme),
            ty: Type { size: config.font_size, weight: config.font_weight },
            workspace_name: if cwd == home { "~".into() } else { workspace_name },
            cwd,
            threads: Vec::new(),
            sessions: HashMap::new(),
            tabs: Vec::new(),
            active: 0,
            layout: Default::default(),
            render_tab: Default::default(),
            field_tab: None,
            next_tab: 1,
            sidebar: None,
            attach: Default::default(),
            composer_attach_hover: None,
            composer: crate::agent::slash::Composer::new(config.model.as_deref(), config.variant.as_deref()),
            focus: Focus::Content,
            agent: "Build".into(),
            model: crate::agent::title_case(config.model.as_deref().unwrap_or("openai/gpt-6.1-sol")),
            effort: crate::agent::title_case(config.variant.as_deref().unwrap_or("medium")),
            wordmark: Wordmark::default(),
            fps: None,
            fps_live: rune::LiveText::new(""),
            fps_at: None,
            mcp_icons: Default::default(),
            tools: Default::default(),
            picker: None,
            browser: None,
            form: None,
            server: Default::default(),
            list_residual: Default::default(),
            activity: None, replay: None,
            modal: None,
            pending_cd: None,
            main_terminal: None,
            collapsed_projects: Default::default(),
            clipboard: None,
            notes: Default::default(),
            menu: None,
            settings: None,
            databases: HashMap::new(),
            keybindings: crate::config::read()["keybinds"]["keys"].clone(),
            wallpaper: crate::ui::wallpaper::Wallpaper::from_config(&crate::config::read()),
            window: [1252.0, 674.0],
            background_previews: Default::default(),
            decoded_previews: Default::default(),
            tree: Default::default(),
            editor_follow: false,
            tree_follow: Default::default(),
            conv_selected: 0,
            conv_header: None,
            project_menu: None,
            conv_follow: Default::default(),
            pending_g: false,
            tree_count: None,
            buffers: HashMap::new(),
            finder: None,
            local_name: crate::ui::orb::local_display_name(config.display_name.as_deref()),
            collab: crate::collab::Collab::new(&crate::ui::orb::local_display_name(config.display_name.as_deref())),
            peer_hover: None,
            peer_blink: rune::Live::new(1.0),
            orbs: Default::default(),
            waker: None,
            cursor: Default::default(),
            rects: HashMap::new(),
            families: Default::default(),
            header_hover: None,
            // The local branch until the daemon's Git watch reports counts.
            git: crate::config::git_branch().map(|branch| crate::ui::status::GitSummary { branch, ..Default::default() }),
            splash: Default::default(),
            terminals: HashMap::new(),
            pending_pty: Default::default(),
        };
        app.open_tab(TabKind::Home);
        app.workspaces.push(crate::workspace::Entry { id: app.workspace_id.clone(), title: app.workspace_name.clone(), server: None, state: None });
        app
    }

    pub fn display_cwd(&self) -> String {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() && self.cwd.starts_with(&home) { format!("~{}", &self.cwd[home.len()..]) } else { self.cwd.clone() }
    }

    pub fn drain_current(&mut self) -> bool {
        let Some(inbox) = &self.inbox else { return false };
        let events: Vec<Inbound> = inbox.lock().unwrap().drain(..).collect();
        let mut changed = false;
        let owner = self.workspace_id.clone();
        for event in events {
            // A reply's text is applied now and shown on the stream's own pace (`StreamPace`).
            if matches!(&event, Inbound::Agent(message) if matches!(**message, neoism_protocol::agent::AgentServerMessage::ContentDelta { .. })) {
                self.stream_pace.pending = true;
            } else if let Inbound::Crdt { request_id, message } = event {
                // A heartbeat or our own echo changes nothing on screen.
                changed |= self.apply_crdt(request_id, *message);
                continue;
            } else {
                changed = true;
            }
            self.apply(event);
            // Close acknowledgement may dispose this workspace and select its neighbour.
            // The remaining old-socket events must never be applied to that neighbour.
            if self.workspace_id != owner { break; }
        }
        changed
    }

    pub(crate) fn apply(&mut self, event: Inbound) {
        if let Inbound::Reply { family, request_id, message } = &event {
            if matches!(family.as_str(), "Files" | "Config") {
                if let Some(error) = message.get("Error") { self.editor_save_error(Some(*request_id), error["message"].as_str().unwrap_or("Save rejected")); }
            }
        }
        match event {
            Inbound::Connecting => self.connection = Connection::Connecting,
            Inbound::Connected => {
                self.connection = Connection::Connected;
                self.workspace_ready = false;
                self.register_workspace();
                self.collab_reconnect();
            }
            Inbound::Disconnected(reason) => { self.connection = Connection::Disconnected(reason); self.database_disconnected(); self.editor_save_error(None, "Connection lost; buffer kept open. Verify the file before retrying."); },
            Inbound::Agent(message) => self.apply_agent(*message),
            Inbound::Crdt { request_id, message } => { self.apply_crdt(request_id, *message); }
            Inbound::Editor(message) => self.apply_editor_message(*message),
            // Live PTY output is uncorrelated (request 0); a reattach replays backlog under its request.
            Inbound::Reply { family, message, request_id } if family == "Pty" => {
                if let Ok(typed) = serde_json::from_value::<PtyServer>(message) { self.apply_pty(typed, request_id == 0); }
            }
            Inbound::Reply { family, message, .. } if family == "Git" => {
                if let Some(status) = message.get("RepoStatus") { self.apply_git_status(status); }
            }
            Inbound::Reply { family, message, .. } if family == "Search" => {
                if let Ok(typed) = serde_json::from_value(message) { self.apply_search(typed); }
            }
            Inbound::Reply { family, message, request_id } if family == "Files" => {
                self.apply_files(&message);
                if let Some(listing) = message.get("DirListing") {
                    self.apply_dir_listing(listing["path"].as_str().unwrap_or_default(), &listing["entries"]);
                }
                if let Some(written) = message.get("FileWritten") {
                    self.apply_file_written(written["path"].as_str().unwrap_or_default(), request_id);
                }
                if let Some(paths) = message["Changed"]["paths"].as_array() {
                    let paths: Vec<String> = paths.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect();
                    self.apply_files_changed(&paths);
                }
                if let Some(file) = message.get("FileContent") {
                    let path = file["path"].as_str().unwrap_or_default().to_owned();
                    let bytes: Vec<u8> = serde_json::from_value(file["bytes"].clone()).unwrap_or_default();
                    self.apply_file_content(&path, &bytes);
                    self.apply_opened_file(&path, &bytes);
                }
            }
            Inbound::Reply { family, message, request_id } if family == "Workspace" => {
                self.workspace_reply(request_id, &message);
                if let Some(dir) = message["NotesVaultResolved"]["notes_vault_dir"].as_str() { self.apply_notes_vault(dir); }
            }
            Inbound::Database { request_id, message } => self.apply_database_reply(request_id, message),
            Inbound::Reply { family, message, request_id } if family == "Config" => self.apply_config_reply(request_id, &message),
            Inbound::Reply { family, message, .. } if family == "Editor" => self.apply_editor_reply(message),
            Inbound::Reply { .. } => {}
        }
    }

    /// Subscribe to the workspace's Git status (branch and changed line counts).
    pub(crate) fn watch_git(&self) {
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Git, json!({"WatchStatus": {"token": GIT_WATCH_TOKEN}}), &self.cwd);
        }
    }

    fn apply_git_status(&mut self, status: &serde_json::Value) {
        if status["token"] != GIT_WATCH_TOKEN { return; }
        let Ok(snapshot) = serde_json::from_value::<GitRepoStatus>(status["snapshot"].clone()) else { return };
        if snapshot.error.is_some() { return; }
        self.git = snapshot.branch.filter(|_| snapshot.repo_root.is_some()).map(|branch| GitSummary {
            branch,
            added: snapshot.files.iter().map(|f| u64::from(f.additions)).sum(),
            deleted: snapshot.files.iter().map(|f| u64::from(f.deletions)).sum(),
        });
    }
}
