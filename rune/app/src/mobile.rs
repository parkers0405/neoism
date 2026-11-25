//! Neoism's phone surface. The workspace daemon and chat state are shared with desktop.

use crate::app::{App, Connection, Focus, Sidebar};
use crate::tabs::TabKind;
use crate::ui::{ChildIf, feather::Icon, theme::rgb};
use rune::{AppSpec, Cx, El, Key, KeyEvent, Tx, col, el, row, text};

const WIDTH: f32 = 390.0;
const HEIGHT: f32 = 844.0;
pub(crate) const DEFAULT_FONT_SIZE: f32 = 16.0;

#[derive(Default)]
pub(crate) struct Pairing {
    local_name: Option<String>,
    gate: bool,
    adding: bool,
    devices: bool,
    invite: rune::TextEdit,
    password: rune::TextEdit,
    pending: Option<Device>,
    claim: Option<PendingClaim>,
    saved: Vec<Device>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct Device { name: String, endpoint: String, token: String }

#[derive(serde::Deserialize)]
struct ClaimReply { token: String }

struct PendingClaim { reply: crate::device_pair::Reply<ClaimReply>, name: String, endpoint: String }

impl Device {
    fn selection(&self) -> crate::servers::Selection {
        let device = crate::device_pair::is_session(&self.endpoint);
        crate::servers::Selection { id: format!("mobile:{}", self.endpoint), name: self.name.clone(), endpoint: self.endpoint.clone(),
            pairing_token: (!device).then(|| self.token.clone()), auth_token: Some(self.token.clone()), temporary: true, ..Default::default() }
    }
}

pub(crate) fn accept_scanned_invite(app: &mut App, value: &str) {
    if !(app.mobile_pairing.gate || app.mobile_pairing.adding) || app.mobile_pairing.pending.is_some() || app.mobile_pairing.claim.is_some() { return; }
    if normalize_invite(value, "").is_err() && crate::device_pair::parse(value).is_err() {
        app.modal = Some(crate::ui::modal::Modal::notice("Invalid pairing QR code", "Scan the code shown by Open on phone in desktop Neoism."));
        return;
    }
    app.mobile_pairing.password = Default::default();
    app.mobile_pairing.invite.replace_text(value.trim());
    app.mobile_pairing.invite.focused = false;
}

fn devices_path() -> std::path::PathBuf { crate::config::dir().join("mobile-devices.json") }

fn save_devices(devices: &[Device]) -> std::io::Result<()> {
    let path = devices_path();
    let dir = path.parent().expect("mobile config directory");
    std::fs::create_dir_all(dir)?;
    // NamedTempFile creates a private file (0600 on Unix); never write invite credentials to the desktop config.
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, devices)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(crate) fn restore_device(app: &mut App) {
    if std::env::var_os("NEOISM_RUNE_OFFLINE").is_some() { return; }
    app.mobile_pairing.gate = app.mobile_pairing.local_name.is_none();
    app.mobile_pairing.saved = std::fs::read(devices_path()).ok()
        .and_then(|contents| serde_json::from_slice(&contents).ok()).unwrap_or_default();
    if let Some(device) = app.mobile_pairing.saved.first().cloned() {
        app.mobile_pairing.gate = false;
        select_device(app, device);
    } else if app.mobile_pairing.local_name.is_some() {
        select_local_device(app);
    }
}

fn select_local_device(app: &mut App) {
    let Some(index) = app.workspaces.iter().position(|entry| entry.server.is_none()) else { return };
    close_history(app);
    app.select_workspace(index);
    if app.workspace_active != index || app.daemon.is_some() || std::env::var_os("NEOISM_RUNE_OFFLINE").is_some() { return; }
    if let Some(wake) = app.waker.clone() {
        let (daemon, inbox) = crate::daemon::Daemon::start(move || wake());
        app.daemon = Some(daemon);
        app.inbox = Some(inbox);
    }
}

fn select_device(app: &mut App, device: Device) {
    app.mobile_pairing.devices = false;
    app.mobile_pairing.adding = false;
    close_history(app);
    let selection = device.selection();
    if let Some(index) = app.workspaces.iter().position(|entry| entry.server.as_deref() == Some(&selection.id)) {
        app.select_workspace(index);
    } else {
        let target = crate::daemon::Target { url: selection.endpoint.clone(), auth_token: selection.auth_token.clone(), pairing_token: selection.pairing_token.clone() };
        let mut link = crate::servers::flow::Link::default();
        link.current = Some(selection.clone());
        link.adopt = Some(crate::servers::flow::Adopt::Resolve);
        app.open_workspace(crate::workspace::Opening { id: crate::workspace::new_id(), title: selection.name.clone(), cwd: "~".into(), target: Some(target),
            link });
    }
    remember_device(app, device);
}

fn remember_device(app: &mut App, device: Device) {
    let saved = &mut app.mobile_pairing.saved;
    if saved.first().is_some_and(|first| first.endpoint == device.endpoint && first.token == device.token && first.name == device.name) { return; }
    saved.retain(|entry| entry.endpoint != device.endpoint);
    saved.insert(0, device);
    if let Err(error) = save_devices(saved) {
        app.modal = Some(crate::ui::modal::Modal::notice("Could not remember device", &error.to_string()));
    }
}

fn pair_device(app: &mut App) {
    if app.server.progress().is_some() || app.mobile_pairing.pending.is_some() || app.mobile_pairing.claim.is_some() { return; }
    if crate::device_pair::is_device_link(&app.mobile_pairing.invite.text) {
        match crate::device_pair::parse(&app.mobile_pairing.invite.text) {
            Ok(invite) if !app.mobile_pairing.password.text.is_empty() => {
                let mut claim = invite.origin.clone();
                claim.set_path("/mobile-device/claim");
                let mut socket = invite.origin;
                if socket.set_scheme("wss").is_err() { return; }
                socket.set_path(crate::device_pair::SESSION);
                socket.set_fragment(invite.ca.as_deref());
                let reply = crate::device_pair::request(claim, serde_json::json!({
                    "code": invite.code, "device_label": "Neoism Mobile", "password": app.mobile_pairing.password.text,
                }), None, invite.ca, app.waker.clone());
                app.mobile_pairing.invite.focused = false;
                app.mobile_pairing.password = Default::default();
                app.mobile_pairing.claim = Some(PendingClaim { reply, name: invite.name, endpoint: socket.to_string() });
            }
            Ok(_) => {},
            Err(error) => app.modal = Some(crate::ui::modal::Modal::notice("Could not pair device", &error)),
        }
        return;
    }
    match parse_invite(&app.mobile_pairing.invite.text, &app.mobile_pairing.password.text) {
        Ok(device) => {
            let selection = device.selection();
            if app.mobile_pairing.saved.iter().any(|saved| saved.endpoint == device.endpoint && saved.token == device.token) {
                select_device(app, device);
                return;
            }
            app.mobile_pairing.invite.focused = false;
            app.mobile_pairing.password.focused = false;
            if app.server.link.current.as_ref().is_some_and(|current| current.id == selection.id) {
                app.modal = Some(crate::ui::modal::Modal::notice("Device already paired", "Use a fresh QR from the desktop if the host stopped sharing."));
                return;
            }
            app.mobile_pairing.pending = Some(device);
            app.connect_server(selection);
        }
        Err(error) => app.modal = Some(crate::ui::modal::Modal::notice("Could not pair device", &error)),
    }
}

fn normalize_invite(input: &str, password: &str) -> Result<crate::fast_servers::FastJoin, String> {
    let input = input.trim();
    if input.len() > 8192 { return Err("Pairing link is too long.".into()); }
    let invite = url::Url::parse(input).ok().filter(|url| url.scheme() == "neoism" && url.host_str() == Some("join"))
        .and_then(|url| url.query_pairs().find(|(key, _)| key == "invite").map(|(_, value)| value.into_owned())).unwrap_or_else(|| input.to_owned());
    crate::fast_servers::normalize_fast_join(invite, (!password.is_empty()).then(|| password.to_owned()))
}

fn parse_invite(input: &str, password: &str) -> Result<Device, String> {
    let join = normalize_invite(input, password)?;
    Ok(Device { name: join.name, endpoint: join.endpoint, token: join.token.ok_or("Enter the host password or use the desktop pairing QR.")? })
}

pub(crate) fn poll_pairing(app: &mut App) -> bool {
    if let Some(result) = app.mobile_pairing.claim.as_ref().and_then(|claim| claim.reply.lock().unwrap().take()) {
        let claim = app.mobile_pairing.claim.take().unwrap();
        match result {
            Ok(reply) if !reply.token.is_empty() => {
                let device = Device { name: claim.name, endpoint: claim.endpoint, token: reply.token };
                let selection = device.selection();
                app.mobile_pairing.pending = Some(device);
                app.connect_server(selection);
            }
            Ok(_) => app.modal = Some(crate::ui::modal::Modal::notice("Could not pair device", "The host returned no device credential.")),
            Err(error) => app.modal = Some(crate::ui::modal::Modal::notice("Could not pair device", &error)),
        }
        return true;
    }
    let Some(device) = &app.mobile_pairing.pending else { return false };
    if app.server.progress().is_some() { return false; }
    let current = app.server.link.current.as_ref().is_some_and(|current| {
        current.id == device.selection().id && current.auth_token.as_deref() == Some(device.token.as_str())
    });
    if current && (!app.workspace_ready || !matches!(app.connection, Connection::Connected)) { return false; }
    let device = app.mobile_pairing.pending.take().unwrap();
    if current {
        app.mobile_pairing.gate = false;
        app.mobile_pairing.adding = false;
        app.mobile_pairing.devices = false;
        app.mobile_pairing.invite = Default::default();
        app.mobile_pairing.password = Default::default();
        remember_device(app, device);
    }
    true
}

pub fn run() {
    rune::set_app_id("ai.neoism.mobile");
    let spec = AppSpec {
        min_size: Some((300.0, 360.0)),
        fonts: crate::FONTS,
        families: (crate::ui::fonts::CHAT_SLOT, crate::ui::fonts::MONO),
        setup: Some(crate::setup),
        poll: Some(crate::poll),
        on_key: Some(on_key),
        scenes: &[
            ("welcome", |_| {}),
            ("pair", |app| app.mobile_pairing.gate = true),
            ("pair-add", |app| { fixture(app); open_history(app); app.mobile_pairing.adding = true; }),
            ("splash-replay", |app| { app.replay_empty_wordmark(); }),
            ("settings", |app| { fixture(app); open_history(app); open_settings(app); }),
            ("themes", |app| { fixture(app); open_history(app); open_settings(app); app.settings.as_mut().unwrap().category = crate::settings::Category::Themes; }),
            ("backgrounds", |app| { fixture(app); open_history(app); open_settings(app); app.settings.as_mut().unwrap().category = crate::settings::Category::Backgrounds; }),
            ("composer-menu", |app| { fixture(app); app.mobile_composer_menu = true; }),
            ("chat", fixture),
            ("history", |app| { fixture(app); open_history(app); }),
            ("models", |app| app.open_selection(crate::agent::slash::Kind::Model)),
            ("systems", |app| { fixture(app); open_history(app); app.mobile_pairing.saved.push(Device { name: "Studio / neoism".into(), endpoint: "wss://example.com/session".into(), token: String::new() }); open_systems(app); }),
            ("crafting", |app| {
                fixture(app);
                app.apply_agent(neoism_protocol::agent::AgentServerMessage::CatalogActivity { session_id: "mobile-preview".into(), activity: "running".into() });
            }),
        ],
        ..AppSpec::new("Neoism", (WIDTH, HEIGHT), init, root)
    };
    let app = rune::App::new(spec);
    #[cfg(target_os = "macos")]
    let app = {
        use crate::palette::commands::Action;
        use rune::MenuItem;
        // AppKit handles Command key equivalents before sending keys to the focused view.
        app.titlebar(rune::Titlebar::new(44.0))
            .menu_item(MenuItem::new("Zoom In", |a: &mut App, tx| a.run_action(Action::IncreaseFontSize, tx))
                .menu("View").key_equivalent("cmd+=".parse().unwrap()))
            .menu_item(MenuItem::new("Zoom In (+)", |a: &mut App, tx| a.run_action(Action::IncreaseFontSize, tx))
                .menu("View").key_equivalent("cmd+shift+=".parse().unwrap()))
            .menu_item(MenuItem::new("Zoom Out", |a: &mut App, tx| a.run_action(Action::DecreaseFontSize, tx))
                .menu("View").key_equivalent("cmd+-".parse().unwrap()))
            .menu_item(MenuItem::new("Reset Zoom", |a: &mut App, tx| a.run_action(Action::ResetFontSize, tx))
                .menu("View").key_equivalent("cmd+0".parse().unwrap()))
    };
    app
        .on_close(|app, _| app.close_window_resources())
        .on_quit(|app, _| app.close_window_resources())
        .run();
    crate::workspace::finish_window_cleanup();
}

fn init() -> App {
    let directory = std::env::var("NEOISM_WORKSPACE_ROOT").unwrap_or_else(|_|
        std::env::current_dir().map(|path| path.display().to_string()).unwrap_or_default());
    let mut app = App::new_surface(directory, true);
    if !cfg!(target_os = "ios") {
        let name = crate::startup::hostname();
        app.mobile_pairing.local_name = Some(if name.is_empty() { "This computer".into() } else { name });
    }
    app.load_mobile_background();
    if let Ok(theme) = std::fs::read_to_string(theme_path()) {
        if let Some(theme) = crate::ui::theme::PALETTES.iter().find(|p| p.name == theme.trim()) { app.pal = theme; }
    }
    app.window = [WIDTH, HEIGHT];
    app.ty.size = std::fs::read_to_string(zoom_path()).ok()
        .and_then(|text| text.trim().parse::<f32>().ok())
        .filter(|size| (8.0..=40.0).contains(size))
        .unwrap_or(DEFAULT_FONT_SIZE);
    app.ty.weight = 450.0;
    app.focused_view = true;
    app.conv_all = true;
    app.tabs[0].draft.focused = false;
    app
}

pub(crate) fn theme_path() -> std::path::PathBuf { crate::config::dir().join("mobile-theme") }

fn zoom_path() -> std::path::PathBuf { crate::config::dir().join("mobile-zoom") }

pub(crate) fn save_zoom(size: f32) {
    use std::io::Write;
    let path = zoom_path();
    let Some(dir) = path.parent() else { return };
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        writeln!(file, "{size}")?;
        file.persist(&path)?;
        Ok(())
    })();
    if let Err(error) = result { eprintln!("could not save mobile zoom: {error}"); }
}

fn fixture(app: &mut App) {
    app.cwd = "/projects/neoism".into();
    app.workspace_name = "neoism".into();
    for (id, title, directory) in [
        ("preview-history-3", "Review the release notes", "/projects/neoism"),
        ("preview-history-2", "Map the agent event flow", "/projects/neoism"),
        ("preview-history-1", "Plan the next release", "/projects/studio"),
    ] {
        app.cwd = directory.into();
        app.load_fixture(id, title, "[]");
    }
    app.cwd = "/projects/neoism".into();
    app.load_fixture("mobile-preview", "A calmer onboarding", r#"[
        {"id":"prompt","kind":"User","role":"User","text":"How could we make onboarding feel lighter?"},
        {"id":"answer","kind":"Assistant","role":"Assistant","text":"Start with one action: **open a workspace**. Show the rest when it is useful.\n\n- Keep the first screen quiet.\n- Explain what the agent can do in context.\n- Make every setup step skippable."}
    ]"#);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    for (index, thread) in app.threads.iter_mut().enumerate() {
        thread.updated_at = now.saturating_sub((index as u64 + 1) * 3_600_000);
    }
    app.threads.iter_mut().find(|thread| thread.session_id == "preview-history-3").unwrap().pinned = true;
    app.tabs[app.active].draft.focused = false;
}

pub(crate) fn open_history(app: &mut App) {
    app.mobile_composer_menu = false;
    app.close_prompt_picker();
    app.tabs[app.active].draft.focused = false;
    app.focus = Focus::Sidebar;
    app.sidebar = Some(Sidebar::Conversations);
    app.refresh_threads();
}

pub(crate) fn close_history(app: &mut App) {
    app.sidebar = None;
    app.mobile_pairing.devices = false;
    app.close_project_menu();
    app.conv_query.focused = false;
    app.focus = Focus::Content;
}

fn compact_width(width: f32) -> bool { width < 640.0 }

pub(crate) fn select_chat_from_history(app: &mut App) {
    if !app.mobile { return; }
    if compact_width(app.window[0]) { close_history(app); }
    else {
        app.conv_query.focused = false;
        app.focus = Focus::Content;
    }
}

pub(crate) fn pairing_wordmark(app: &App) -> Option<String> {
    (app.mobile && app.settings.is_none() && (app.mobile_pairing.gate || app.mobile_pairing.adding))
        .then(|| crate::wordmark::host_key("pairing", app.active_tab().id))
}

pub(crate) fn new_chat(app: &mut App) {
    close_history(app);
    app.close_prompt_picker();
    let draft = app.tabs.iter().find(|tab| matches!(tab.kind, TabKind::Agent { session: None })).map(|tab| tab.id);
    if let Some(id) = draft { app.select_tab(id); }
    else { app.open_tab(TabKind::Agent { session: None }); }
    app.tabs[app.active].draft.focused = false;
}

fn open_systems(app: &mut App) {
    app.close_project_menu();
    app.conv_query.focused = false;
    app.mobile_pairing.devices = !app.mobile_pairing.devices;
}

fn open_settings(app: &mut App) {
    app.mobile_pairing.devices = false;
    app.close_project_menu();
    app.conv_query.focused = false;
    app.mobile_composer_menu = false;
    app.close_prompt_picker();
    app.tabs[app.active].draft.focused = false;
    let mut settings = crate::settings::Settings::default();
    settings.category = crate::settings::Category::General;
    settings.loading = false;
    settings.values = app.mobile_backgrounds.config.clone();
    settings.backgrounds = crate::settings::mobile_backgrounds::catalog();
    app.settings = Some(settings);
}

fn on_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    if app.image_preview_key(event) { return true; }
    if app.modal.is_none() && crate::keys::zoom_shortcut(app, event, tx) { return true; }
    if event.key == Key::Escape && app.modal.is_none() {
        if app.settings.is_some() { crate::settings::mobile_back(app); return true; }
        if app.mobile_pairing.adding { app.mobile_pairing.adding = false; return true; }
        if app.mobile_pairing.devices { app.mobile_pairing.devices = false; return true; }
    }
    if app.mobile_composer_menu && event.key == Key::Escape {
        app.mobile_composer_menu = false;
        return true;
    }
    if app.sidebar.is_some() && event.key == Key::Escape && app.picker.is_none()
        && app.project_menu.is_none() && app.project_add.is_none() && app.browser.is_none()
        && app.form.is_none() && app.modal.is_none() && app.context.is_none() && app.prompt.is_none() {
        close_history(app);
        return true;
    }
    if matches!(app.focus, Focus::Content | Focus::Sidebar) {
        return crate::keys::on_key(app, event, tx);
    }
    false
}

fn root(app: &App, cx: &mut Cx<App>) -> El {
    let p = app.pal;
    let [top, right, bottom, left] = cx.safe_area();
    let chrome = cx.chrome_insets();
    let native_titlebar = chrome.height > 0.0 && (chrome.leading > 0.0 || chrome.trailing > 0.0);
    if app.settings.is_some() {
        if native_titlebar { cx.caption_color(rgb(p.fg)); }
        return col().key("app-shell").w_full().h_full().fill(rgb(p.bg))
            .pt(top).pr(right).pb(bottom).pl(left)
            .on_layout(|a: &mut App, [_, _, w, h]: [f32; 4]| a.window = [w, h])
            .child_if(native_titlebar, || el().key("settings-titlebar").w_full().h(chrome.height).window_drag())
            .child(crate::settings::mobile_view(app, left + right).grow(1.0).shrink())
            .child_if(app.modal.is_some(), || crate::ui::modal::view(app));
    }
    let history = app.sidebar == Some(Sidebar::Conversations);
    let pairing = app.mobile_pairing.gate || app.mobile_pairing.adding;
    let session = match &app.active_tab().kind {
        TabKind::Agent { session } => session.as_deref(),
        _ => None,
    };
    let link = if app.daemon.is_none() { "Not paired" } else { match &app.connection {
        Connection::Connected if app.workspace_ready => "Connected",
        Connection::Connected | Connection::Connecting => "Connecting",
        Connection::Reconnecting { .. } => "Reconnecting",
        Connection::Disconnected(_) => "Retrying",
    }};
    if native_titlebar { cx.caption_color(rgb(p.fg)); }
    let available = (app.window[0] - left - right).max(0.0);
    let compact = compact_width(app.window[0]);
    let sidebar_width = if compact { (available - 48.0).clamp(0.0, 380.0) } else { 350.0 };
    let docked = history && !compact;
    let chat_width = if docked && !pairing { (available - sidebar_width).max(0.0) } else { available };
    let nav = row().key("mobile-controls").w_full().h(56.0).px(12.0).center_y()
        .child(button(app, "mobile-history", if history { "Close chats" } else { "Open chats" }, Icon::Menu)
            .on_press(move |a: &mut App| if history { close_history(a); } else { open_history(a); }))
        .child_if(docked, || el().w(sidebar_width))
        .child(col().grow(1.0).shrink().min_w(0.0).px(12.0).items_center().gap(2.0)
            .child_if(session.is_some() && (!history || docked), || text(app.tab_title(app.active_tab())).key("mobile-chat-title")
                .max_w(f32::INFINITY).font(app.ty.sans(15.0, 600.0)).color(rgb(p.fg)).truncate())
            .child_if(app.server.link.current.is_some() && link != "Connected", || text(link).font(app.ty.sans(11.0, 550.0)).color(rgb(p.dim))))
        .child(if history { el().w(44.0) } else { button(app, "mobile-new-chat", "New chat", Icon::Edit).on_press(new_chat) });
    let chat = row().key("mobile-chat-body").w_full().grow(1.0).shrink().clip_rounded(0.0)
        .child_if(history && !pairing, || sidebar(app).w(sidebar_width))
        .child(col().key("mobile-chat-content").h_full().w(chat_width).clip_rounded(0.0)
            // On a phone, push the full-width chat offscreen rather than crushing its layout.
            .min_w(chat_width)
            .child_if(!pairing && app.wallpaper.active(), || crate::ui::wallpaper::view(app))
            .child(if pairing {
                let height = app.window[1] - top - bottom - if native_titlebar { chrome.height } else { 0.0 };
                pairing_view(app, chat_width, height)
            } else { crate::agent::timeline::mobile_pane(app, session, chat_width) }.w_full().grow(1.0).shrink())
            .child_if(history && compact && !pairing, || el().key("mobile-close-chats-edge").label("Close chats")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).on_press(close_history)));
    col().key("app-shell").w_full().h_full().fill(rgb(p.bg))
        .pt(top).pr(right).pb(bottom).pl(left)
        .on_layout(|a: &mut App, [_, _, w, h]: [f32; 4]| a.window = [w, h])
        .child_if(native_titlebar, || el().key("mobile-titlebar").w_full().h(chrome.height).window_drag())
        .child_if(!pairing, || nav)
        .child(chat)
        .child_if(history && app.mobile_pairing.devices && !pairing, || device_picker(app, top + 56.0 + if native_titlebar { chrome.height } else { 0.0 }, bottom))
        .child_if((!history || !compact) && !pairing, || crate::agent::overlay(app, [top, right, bottom, left]))
        .child_if(app.mobile_composer_menu && (!history || !compact) && !pairing, || crate::agent::mobile_menu(app))
        .child_if(app.project_menu.is_some(), || crate::agent::project_menu(app))
        .child_if(app.project_add.is_some(), || crate::agent::project_add(app))
        .child_if(app.picker.is_some(), || crate::palette::view(app))
        .child_if(app.browser.is_some(), || crate::browser::view::view(app))
        .child_if(app.form.is_some(), || crate::servers::form::view(app))
        .child_if(app.modal.is_some(), || crate::ui::modal::view(app))
        .child_if(app.context.is_some(), || crate::ui::context::menu_view(app))
        .child_if(app.prompt.is_some(), || crate::ui::context::prompt_view(app))
        .child_if(app.server.progress().is_some(), || crate::servers::flow::progress_view(app))
}

fn sidebar(app: &App) -> El {
    let p = app.pal;
    col().key("mobile-chat-sidebar").h_full().fill(rgb(p.bg))
        .border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child(row().w_full().h(39.0).px(16.0).center_y()
            .child(text("Chats").font(app.ty.sans(20.0, 650.0)).color(rgb(p.fg)).grow(1.0)))
        .child(crate::agent::sidebar::conversations(app).w_full().grow(1.0).shrink())
        .child(row().key("mobile-settings").label("Settings").w_full().h(54.0).px(18.0).gap(10.0).center_y()
            .border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)).hover(rgb(p.hover))
            .child(crate::ui::feather::view(app, Icon::Settings, 17.0, p.fg))
            .child(text("Settings").font(app.ty.sans(14.0, 550.0)).color(rgb(p.fg)).grow(1.0))
            .child(crate::ui::feather::view(app, Icon::Right, 14.0, p.dim))
            .on_press(open_settings))
}

fn pairing_view(app: &App, width: f32, height: f32) -> El {
    let p = app.pal;
    let state = &app.mobile_pairing;
    let field = |edit: &rune::TextEdit, key, placeholder| rune::text_input(edit, app.ty.sans(15.0, 450.0))
        .key(key).label(placeholder).w_full().single_line().secure().color(rgb(p.fg))
        .placeholder(placeholder, rgb(p.dim)).caret(rgb(p.fg), crate::ui::cursor::selection(app))
        .on_submit(|a: &mut App, _| pair_device(a));
    let surface = |child| row().w_full().h(48.0).px(14.0).center_y().radius(14.0)
        .fill(rgb(p.surface)).border(1.0, rgb(p.border)).child(child);
    let needs_password = crate::device_pair::parse(&state.invite.text).is_ok()
        || normalize_invite(&state.invite.text, "").is_ok_and(|invite| invite.token.is_none());
    let ready = !state.invite.text.trim().is_empty() && state.pending.is_none() && state.claim.is_none()
        && (!needs_password || !state.password.text.is_empty());
    let wordmark = crate::wordmark::view(app, &pairing_wordmark(app).expect("pairing view is visible"), splash_width(width));
    let form = col().w_full().max_w(450.0).px(26.0).gap(14.0).items_center()
            .child(text("Pair a device").font(app.ty.sans(28.0, 650.0)).color(rgb(p.fg)))
            .child(text("Connect to Tailscale, then open the QR button in desktop Neoism. Scan the code and enter its pairing password, or paste an invite link.")
                .font(app.ty.sans(14.0, 450.0).lh(21.0)).color(rgb(p.dim)).w_full().wrap())
            .child(surface(field(&state.invite, "pair-invite", "Pairing link").on_edit(|a: &mut App| &mut a.mobile_pairing.invite)))
            .child_if(needs_password, || {
                surface(field(&state.password, "pair-password", "Host password").on_edit(|a: &mut App| &mut a.mobile_pairing.password))
            })
            .child(row().w_full().gap(8.0)
                .child(pair_button(app, "pair-scan", "Scan QR Code", !ready).grow(1.0)
                    .on_press_tx(|a: &mut App, tx| {
                        a.mobile_pairing.invite.focused = false;
                        a.mobile_pairing.password.focused = false;
                        if a.mobile_pairing.pending.is_none() { crate::mobile_qr::scan(a, tx); }
                    }))
                .child(pair_button(app, "pair-submit", if state.pending.is_some() || state.claim.is_some() { "Pairing..." } else { "Pair device" }, ready).grow(1.0)
                    .on_press(move |a: &mut App| if ready { pair_device(a) })));
    col().key("mobile-pair").w_full().h_full()
        .child(col().w_full().grow(1.0).shrink().min_h(0.0).items_center().scroll(rune::ScrollSpec { stick_bottom: false })
            .child(col().key("pair-content").w_full().min_h(height.max(0.0)).py(if state.adding { 56.0 } else { 20.0 })
                .justify_center().items_center().gap(22.0).child(wordmark).child(form)))
        .child_if(state.adding, || button(app, "pair-back", "Back to chats", Icon::Close).absolute(12.0, 6.0)
            .on_press(|a: &mut App| a.mobile_pairing.adding = false))
}

fn pair_button(app: &App, key: &'static str, title: &'static str, primary: bool) -> El {
    let p = app.pal;
    el().key(key).label(title).h(46.0).px(12.0).radius(14.0).center()
        .fill(rgb(if primary { p.fg } else { p.surface })).border(1.0, rgb(p.border))
        .child(text(title).font(app.ty.sans(14.0, 600.0)).color(rgb(if primary { p.bg } else { p.fg })))
}

pub(crate) fn device_trigger(app: &App) -> El {
    let p = app.pal;
    let open = app.mobile_pairing.devices;
    let current = match app.server.link.current.as_ref() {
        Some(device) => Some(device.name.as_str()).filter(|name| !name.is_empty()),
        None => app.mobile_pairing.local_name.as_deref(),
    };
    let connected = matches!(app.connection, Connection::Connected) && app.workspace_ready;
    row().key("mobile-systems").label("Choose device").h(44.0).mx(12.0).mb(4.0).px(4.0).gap(8.0).radius(6.0).center_y()
        .hover(rgb(p.hover))
        .child(crate::ui::feather::view(app, Icon::Server, 15.0, p.dim))
        .child(text(current.unwrap_or("Choose device")).font(app.ty.sans(13.0, 550.0)).color(rgb(p.fg)).grow(1.0).shrink().truncate())
        .child_if(current.is_some(), || el().w(6.0).h(6.0).radius(3.0).fill(rgb(if connected { p.green } else { p.muted })))
        .child(crate::ui::feather::view(app, if open { Icon::Up } else { Icon::Down }, 14.0, p.dim))
        .on_layout(|a: &mut App, r| { a.rects.insert("mobile-systems".into(), r); })
        .on_press(open_systems)
}

fn device_picker(app: &App, top: f32, bottom: f32) -> El {
    let Some([x, y, w, h]) = app.rects.get("mobile-systems").copied() else { return el() };
    let p = app.pal;
    let below = y + h + 6.0;
    let list_h = (app.window[1] - bottom - below - 98.0).clamp(44.0, 224.0);
    let mut devices = col().w_full().max_h(list_h).shrink().scroll(rune::ScrollSpec { stick_bottom: false });
    if let Some(name) = app.mobile_pairing.local_name.as_ref() {
        let selected = app.server.link.current.is_none();
        devices = devices.child(row().key("device-local").w_full().h(44.0).px(10.0).gap(9.0).center_y()
            .radius(7.0).fill(if selected { rgb(p.fg).alpha(0.07) } else { rune::Color::TRANSPARENT }).hover(rgb(p.hover))
            .child(crate::ui::feather::view(app, Icon::Server, 15.0, if selected { p.blue } else { p.dim }))
            .child(text(name.clone()).font(app.ty.sans(13.0, 550.0)).color(rgb(p.fg)).grow(1.0).shrink().truncate())
            .child(text("This computer").font(app.ty.sans(11.0, 500.0)).color(rgb(p.dim)))
            .child_if(selected, || text(crate::ui::theme::glyph::CHECK).font(app.ty.icon_sized(12.0)).color(rgb(p.blue)))
            .on_press(select_local_device));
    } else if app.mobile_pairing.saved.is_empty() {
        devices = devices.child(el().px(10.0).py(10.0).child(text("No paired devices").font(app.ty.sans(13.0, 450.0)).color(rgb(p.muted))));
    }
    for device in &app.mobile_pairing.saved {
        let selected = app.server.link.current.as_ref().is_some_and(|s| s.endpoint == device.endpoint);
        let device = device.clone();
        devices = devices.child(row().key(format!("device:{}", device.endpoint)).w_full().h(44.0).px(10.0).gap(9.0).center_y()
            .radius(7.0).fill(if selected { rgb(p.fg).alpha(0.07) } else { rune::Color::TRANSPARENT }).hover(rgb(p.hover))
            .child(crate::ui::feather::view(app, Icon::Server, 15.0, if selected { p.blue } else { p.dim }))
            .child(text(device.name.clone()).font(app.ty.sans(13.0, 550.0)).color(rgb(p.fg)).grow(1.0).shrink().truncate())
            .child_if(selected, || text(crate::ui::theme::glyph::CHECK).font(app.ty.icon_sized(12.0)).color(rgb(p.blue)))
            .on_press(move |a: &mut App| select_device(a, device.clone())));
    }
    let panel = col().key("mobile-devices").absolute(x, below - top).w(w)
        .padding(6.0).radius(13.0).fill(rgb(p.bg)).border(1.0, rgb(p.fg).alpha(0.13))
        .shadow(8.0, 28.0, rune::Color::BLACK.alpha(0.45))
        .on_press(|_: &mut App| {})
        .child(row().w_full().h(32.0).px(10.0).center_y()
            .child(text("Devices").font(app.ty.sans(13.0, 600.0)).color(rgb(p.dim))))
        .child(el().h(1.0).mx(4.0).mb(4.0).fill(rgb(p.blue).alpha(0.75)))
        .child(devices)
        .child(row().key("mobile-pair-another").label("Pair another device").w_full().h(44.0).px(10.0).gap(9.0).center_y().radius(7.0).hover(rgb(p.hover))
            .child(text("+").font(app.ty.sans(19.0, 500.0)).color(rgb(p.blue)))
            .child(text("Pair another device").font(app.ty.sans(13.0, 550.0)).color(rgb(p.fg)))
            .on_press(|a: &mut App| { a.mobile_pairing.devices = false; a.mobile_pairing.adding = true; }));
    el().key("mobile-devices-scrim").transition(rune::Transition::none()).pin(Some(0.0), Some(top), Some(0.0), Some(0.0))
        .on_press(|a: &mut App| a.mobile_pairing.devices = false).child(panel)
}

pub(crate) fn welcome(app: &App) -> El {
    let tab = app.active_tab().id;
    let available = app.rects.get(&format!("agent-area:{tab}")).map_or(app.window[0], |r| r[2]);
    col().items_center().justify_center()
        .child(crate::wordmark::view(app, &crate::wordmark::host_key("agent", tab), splash_width(available)))
}

fn splash_width(available: f32) -> f32 { (available - 72.0).clamp(100.0, 460.0) }

fn button(app: &App, key: &'static str, label: &'static str, icon: Icon) -> El {
    let p = app.pal;
    el().key(key).label(label).w(44.0).h(44.0).radius(22.0).center()
        .backdrop_blur(8.0).fill(rgb(p.bg).mix(rgb(p.fg), 0.07).alpha(0.78))
        .border(1.0, rgb(p.fg).alpha(0.16)).shadow(2.0, 8.0, rune::Color::BLACK.alpha(0.12)).hover(rgb(p.hover))
        .child(crate::ui::feather::view(app, icon, 18.0, p.fg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_protected_desktop_qr_is_accepted_before_password_entry() {
        let mut app = App::new_surface(String::new(), true);
        app.mobile_pairing.gate = true;
        let invite = "https://sample.opentunnel.xyz/?name=Studio";
        accept_scanned_invite(&mut app, invite);
        assert!(app.modal.is_none());
        assert_eq!(app.mobile_pairing.invite.text, invite);
        assert!(parse_invite(&app.mobile_pairing.invite.text, "").is_err());
        let device = parse_invite(&app.mobile_pairing.invite.text, "host-password").unwrap();
        assert_eq!(device.endpoint, "wss://sample.opentunnel.xyz/session");
        assert_eq!(device.token, "host-password");

        accept_scanned_invite(&mut app, "not a pairing link");
        assert!(app.modal.is_some());
        assert_eq!(app.mobile_pairing.invite.text, invite);
    }

    #[test]
    fn qr_capability_is_kept_out_of_the_endpoint() {
        let token = "a".repeat(64);
        let device = parse_invite(&format!("https://example.com/?name=Studio#neoism-fast={token}"), "").unwrap();
        assert_eq!(device.name, "Studio");
        assert_eq!(device.endpoint, "wss://example.com/session");
        assert_eq!(device.selection().auth_token.as_deref(), Some(token.as_str()));
        assert!(parse_invite("https://example.com/", "").is_err());
    }
}