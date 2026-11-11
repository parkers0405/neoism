//! Neoism on Rune: a native daemon client that draws every surface with Rune elements.
//! See `rune/README.md` for the plan, the look and the motion this must match.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent;
mod app;
mod browser;
mod cli;
mod collab;
mod clipboard;
mod session;
mod config;
mod daemon;
mod database;
mod drive;
mod editor;
mod finder;
mod ghostty;
mod home;
mod keys;
mod palette;
mod replay;
mod panes;
mod servers;
mod settings;
// GPUiX's native server and SSH layers, compiled in as they are (`neoism-frontend/gpuix/native`);
// they sit at the crate root because they name each other through `crate::`.
#[allow(dead_code)]
#[path = "../../../neoism-frontend/gpuix/native/src/ssh_hosts.rs"]
mod ssh_hosts;
#[allow(dead_code)]
#[path = "../../../neoism-frontend/gpuix/native/src/system_registry.rs"]
mod system_registry;
#[allow(dead_code)]
#[path = "../../../neoism-frontend/gpuix/native/src/fast_servers.rs"]
mod fast_servers;
#[allow(dead_code)]
#[path = "servers/service_process.rs"]
mod service_process;
// The shared Yrs replica and presence plane (`neoism-frontend/shared/src/editor/crdt`), as
// every Neoism client compiles them; `cursor_style` is the one name they reach for outside.
#[allow(dead_code, unused_imports)]
#[path = "../../../neoism-frontend/shared/src/editor/crdt/mod.rs"]
mod crdt;
mod cursor_style {
    /// Seconds since first use: the process clock rainbow carets and presence orbs share.
    pub fn rainbow_now_seconds() -> f32 {
        static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        EPOCH.get_or_init(std::time::Instant::now).elapsed().as_secs_f32()
    }
}
mod startup;
mod tabs;
mod tree;
mod ui;
mod workspace;
mod wordmark;

use rune::{AppSpec, ImageCache, KeyEvent, Tx};
use app::App;

pub const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/GeistMono-Regular.otf"),
    include_bytes!("../assets/fonts/GeistMono-Bold.otf"),
    include_bytes!("../assets/fonts/GeistMono-Italic.otf"),
    include_bytes!("../assets/fonts/GeistMono-BoldItalic.otf"),
    include_bytes!("../assets/fonts/CascadiaCodeNF-Regular.otf"),
    include_bytes!("../assets/fonts/PressStart2P-Regular.ttf"),
    include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf"),
];

fn setup(app: &mut App, waker: rune::Waker) {
    app.waker = Some(waker.clone());
    let images = ImageCache::new(waker.clone());
    app.wordmark.load(&images, app.pal.fg);
    app.tools.icons.load(&images);
    app.images = Some(images);
    // Headless renders (`--snap`, `--tree`) draw the reference look without a daemon.
    if std::env::var_os("NEOISM_RUNE_OFFLINE").is_none() {
        let wake = waker.clone();
        let (daemon, inbox) = daemon::Daemon::start(move || wake());
        app.daemon = Some(daemon);
        app.inbox = Some(inbox);
    }
    // Reopen the last window's workspaces before the daemon handshake registers them.
    if session::enabled() && let Some(saved) = session::read() { app.restore_session(saved); }
    // `neoism file.rs`: the files named on the command line open in the first workspace.
    for file in startup::files() { app.open_file(&file.to_string_lossy(), None); }
}

/// `src/demo.rs` open with rust-analyzer attached and two diagnostics.
/// Finished blocks for the terminal scenes: command, output lines, exit status.
const TERMINAL_FIXTURE: [(&str, &[&str], i32); 3] = [
    ("git status --short", &[" M rune/app/src/home/view.rs", " M rune/app/src/workspace.rs", "?? rune/app/src/home/actions.rs"], 0),
    ("ls src", &["agent", "app.rs", "daemon.rs", "editor", "home", "keys.rs", "main.rs", "session.rs"], 0),
    ("cargo test --release", &["running 132 tests", "test result: ok. 132 passed; 0 failed"], 0),
];

fn lsp_fixture(s: &mut App) {
    s.open_file("src/demo.rs", None);
    s.apply_opened_file("src/demo.rs", b"fn main() {\n    let mut items = Vec::new();\n    items.pu\n    println!(\"{}\", items.len());\n}\n");
    s.apply_editor_reply(serde_json::json!({"LspSnapshot": {"surface_id": "rune-editor:src/demo.rs", "filetype": "rust",
        "servers": [{"name": "rust-analyzer", "binary": "rust-analyzer", "filetype": "rust", "state": "active"}]}}));
    let diag = |severity: &str, line: u32, col: u32, end: u32, message: &str| serde_json::json!({
        "severity": severity, "message": message, "source": "rust-analyzer", "line": line, "col": col, "end_line": line, "end_col": end, "lnum": line + 1});
    s.apply_editor_reply(serde_json::json!({"Diagnostics": {"surface_id": "rune-editor:src/demo.rs", "items": [
        diag("Warn", 1, 12, 17, "variable does not need to be mutable"),
        diag("Error", 2, 10, 12, "no field `pu` on type `Vec<_>`"),
    ]}}));
}

fn poll(app: &mut App, clock: f64, tx: &mut Tx) -> bool {
    let started = std::time::Instant::now();
    let rerun = poll_frame(app, clock, tx);
    ui::fps::polled(started, rerun);
    rerun
}

fn poll_frame(app: &mut App, clock: f64, tx: &mut Tx) -> bool {
    static TRACE_POLL: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("NEOISM_RUNE_TRACE_POLL").is_some());
    let mut wakes: Vec<(&str, std::time::Duration)> = Vec::new();
    macro_rules! wake { ($name:expr, $d:expr) => {{ let d = $d; if *TRACE_POLL { wakes.push(($name, d)); } tx.wake_after(d); }} }
    // Terminal output is parsed as it arrives; blocks and grids are re-read once per frame.
    let changed = drive::tick(app, clock, tx) | replay::tick(app, clock, tx) | app.poll_usage() | app.drain() | app.take_decoded_previews() | app.take_picked_attachments() | app.poll_server() | app.sync_terminals(tx);
    // A streaming reply's text shows at most thirty times a second, however fast it arrives.
    let (stream_due, stream_wait) = app.stream_pace.due(std::time::Instant::now());
    if let Some(wait) = stream_wait { wake!("stream", wait); }
    if std::mem::take(&mut app.settle_pending) { tx.settle(); }
    if app.quit_requested {
        if !app.session_closed { app.save_session(true); }
        // Fast Servers this window hosts stop with it.
        servers::shutdown();
        tx.quit();
        return true;
    }
    if let Some(wait) = app.save_session(false) { wake!("session", wait); }
    // The wordmark eases hover, shimmers and plays click effects off the frame clock.
    let visible_wordmarks = app.visible_wordmarks();
    // The wordmark's idle motion runs on live bindings: frames keep coming without re-running
    // the views; only a newly shown wordmark or a click effect rebuilds.
    let (wordmark_moving, animating) = app.wordmark.tick(&visible_wordmarks, clock);
    if wordmark_moving { wake!("wordmark", std::time::Duration::ZERO); }
    if animating { wake!("wordmark-effect", std::time::Duration::ZERO); }
    // Running tool spinners step every ~90 ms (8 steps per 1/1.35 s lap), on live offsets.
    app.tools.clock = clock;
    if app.running_tools() {
        app.tools.spinner.tick(clock, app.ty.scale());
        wake!("spinner", std::time::Duration::from_millis(45));
    }
    // Presence orbs flow (at 30 Hz) while one is in view.
    if app.orbs_on_screen() && let Some(next) = app.orbs.tick() { wake!("orbs", next); }
    app.editor_follow_cursor(tx);
    app.tree_follow_selection(tx);
    app.conversation_follow(tx);
    let mode_changing = app.tick_mode_pill() | app.tick_chip_scramble();
    // Wheel and trackpad motion on every scroll view, stepped on the frame clock.
    let gliding = app.scrollers.step(clock, tx) | app.editor_jump_springs(tx);
    // A transcript's steps hold a row rather than an offset (`agent/anchor.rs`).
    app.place_conversations(tx);
    let rail_bottom = app.rail_follow_bottom() | app.timeline_away_tick();
    if gliding { wake!("glide", std::time::Duration::ZERO); }
    if mode_changing { wake!("mode", std::time::Duration::ZERO); }
    let panes_changed = app.sync_panes();
    // A file that just came back on screen goes back to where it was scrolled.
    let scroll_restoring = app.editor_keep_scroll(tx);
    if scroll_restoring { wake!("scroll-restore", std::time::Duration::ZERO); }
    app.tree_ensure_listed();
    // The directory-owning terminal's `cd` moves the workspace view with it.
    let panes_changed = app.follow_main_terminal() || panes_changed;
    app.follow_tab_chrome(tx);
    // Shared documents: send what was typed, carry out undo; then who is where.
    let shared_changed = app.collab_flush();
    let (presence_wake, presence_changed) = app.presence_tick();
    if let Some(delay) = presence_wake { wake!("presence", delay); }
    if let Some(delay) = app.peer_blink_tick() { wake!("peer-blink", delay); }
    let flashing = app.editor_tick();
    let mouse_changed = app.editor_mouse_tick(tx);
    // Edited buffers re-sync to the language servers once typing settles.
    if let Some(delay) = app.lsp_tick() { wake!("lsp", delay); }
    if flashing { wake!("flash", std::time::Duration::ZERO); }
    let focus_changed = app.sync_field_focus();
    // A selecting drag resting at the transcript's edge keeps scrolling it.
    let edge_scrolling = app.timeline_edge_scroll(tx);
    if edge_scrolling { wake!("edge", std::time::Duration::ZERO); }
    // The live turn's status line animates on live bindings; it rebuilds only on state changes.
    let (busy, activity_changed) = app.tick_activity();
    if busy { wake!("activity", std::time::Duration::ZERO); }
    // The finder searches once its query settles.
    if let Some(delay) = app.finder_tick(std::time::Instant::now()) { wake!("finder", delay); }
    // A pressed Copy button reads "Copied" for a moment.
    if let Some(wait) = app.copy_feedback_tick() { wake!("copied", wait); }
    // A `/piss` … `/praise` skit plays on the frame clock.
    let skit = app.tick_effect();
    if skit { wake!("skit", std::time::Duration::ZERO); }
    // The cursor trails and blinks on its own schedule.
    app.browser_follow(tx);
    app.settings_reveal(tx);
    let list_sliding = app.step_list_residual(clock);
    // In a file the cursor is part of the text: it rides the scroll rather than chasing it.
    let riding = app.cursor_riding();
    app.cursor.carry(riding.as_ref().map(|(key, offset)| (key.as_str(), *offset)));
    let target = app.cursor_target();
    let moved_at = app.cursor.trail.moved_at;
    let cursor_wake = app.cursor.tick(target, std::time::Instant::now());
    if let Some(delay) = cursor_wake { wake!("cursor", delay); }
    if std::env::var_os("NEOISM_RUNE_TRACE_CURSOR").is_some() {
        let q = app.cursor.trail.quad();
        eprintln!("cursor {:.3} target {:?} quad {:?}", clock, target.map(|t| t.map(|v| (v * 10.0).round() / 10.0)), q.map(|c| c.map(|v| (v * 10.0).round() / 10.0)));
    }
    // Everything the app steps itself once a frame (glides, the cursor trail, the yank flash,
    // the mode pill, the wordmark, the activity line, skits) keeps Rune drawing every vblank
    // while it runs; a poll wake alone is held to a 16.7 ms timer on Linux, i.e. 60 fps.
    let per_frame = gliding || list_sliding || cursor_wake == Some(std::time::Duration::ZERO) || flashing || mode_changing
        || wordmark_moving || animating || busy || skit || edge_scrolling;
    app.keep_vsync(per_frame);
    let clip = app.cursor_clip();
    app.cursor.track_clip(clip, app.cursor.trail.moved_at != moved_at);
    // The blink is a live opacity and travel a live offset: only the block appearing, leaving,
    // changing shape (a trail) or clip re-runs the views (`Cursor::place`).
    let caret_changed = app.cursor.place(clip.filter(|_| !app.cursor.crossing()));
    // Refresh the FPS pill when the measured rate moves; check again as old frames age out.
    // While animating, the pill shows the presentation rate; at rest it holds the last one.
    // At most twice a second, and through the pill's live text: only its first reading mounts
    // the pill (a view run); later readings repaint the label without re-running anything or
    // moving the pill chain (the label is three digits wide).
    let fps = ui::fps::rate().or(app.fps);
    let due = app.fps_at.is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(500));
    let fps_moved = fps.is_some() && app.fps != fps && due;
    let fps_changed = fps_moved && app.fps.is_none();
    if fps_moved {
        app.fps = fps;
        app.fps_at = Some(std::time::Instant::now());
        app.fps_live.set(ui::status::fps_label(fps.unwrap_or(0)));
    }
    if fps.is_some() && app.fps != fps && !due {
        wake!("fps", std::time::Duration::from_millis(500).saturating_sub(app.fps_at.map_or(std::time::Duration::ZERO, |at| at.elapsed())));
    }
    let rerun = animating || changed || scroll_restoring || shared_changed || presence_changed || stream_due || fps_changed || caret_changed || focus_changed || panes_changed || skit || activity_changed || flashing || mouse_changed || mode_changing || rail_bottom;
    // `NEOISM_RUNE_TRACE_POLL=1`: which term re-ran the views (each is a full view run).
    if *TRACE_POLL && (rerun || !wakes.is_empty()) {
        let terms = [("animating", animating), ("changed", changed), ("stream", stream_due), ("fps", fps_changed), ("caret", caret_changed),
            ("focus", focus_changed), ("panes", panes_changed), ("skit", skit), ("activity", activity_changed), ("flashing", flashing), ("mouse", mouse_changed), ("mode", mode_changing), ("rail", rail_bottom)];
        eprintln!("[poll] {:.3} rerun={} wakes={}", clock, terms.iter().filter(|(_, on)| *on).map(|(n, _)| *n).collect::<Vec<_>>().join(","),
            wakes.iter().map(|(n, d)| format!("{n}:{:.1}", d.as_secs_f64() * 1000.0)).collect::<Vec<_>>().join(","));
    }
    if rerun { app.stream_pace.ran(std::time::Instant::now()); }
    rerun
}

fn on_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    keys::on_key(app, event, tx)
}

fn main() {
    // `neoism --version`, `neoism update` and the rest answer before any window or daemon.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = match cli::dispatch(&args) { Ok(launch) => launch, Err(code) => std::process::exit(code) };
    startup::apply(&launch);
    let spec = AppSpec::<App> {
        fonts: FONTS,
        families: ("Geist Mono", "Geist Mono"),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        scenes: &[
            ("welcome", |_| {}),
            ("agent-new", |s| { s.open_tab(tabs::TabKind::Agent { session: None }); }),
            // A sent message with an image and another waiting in the composer;
            // `NEOISM_RUNE_PREVIEW=1` opens the sent one full-window.
            ("agent-attach", |s| {
                s.load_fixture("attach", "Attachments", "[]");
                let png = |w: u32, h: u32, tint: u8| {
                    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, tint, 255]));
                    let mut out = std::io::Cursor::new(Vec::new());
                    img.write_to(&mut out, image::ImageFormat::Png).expect("png");
                    out.into_inner()
                };
                s.attach_bytes("diagram.png", "image/png", png(480, 300, 200));
                let i = s.active;
                let text = format!("{}what does this diagram show?", s.tabs[i].draft.text);
                s.tabs[i].draft.replace_text(text);
                s.submit();
                s.attach_bytes("screenshot.png", "image/png", png(300, 420, 60));
                if std::env::var_os("NEOISM_RUNE_PREVIEW").is_some() {
                    s.attach.preview = Some(agent::attach::Preview { key: "history:".to_owned(), name: "diagram.png".into() });
                    let id = s.sessions.get("attach").and_then(|x| x.messages.last()).map(|m| m.id.clone()).unwrap_or_default();
                    if let Some(p) = &mut s.attach.preview { p.key = format!("history:{id}:0"); }
                }
            }),
            ("sidebar", |s| s.sidebar = Some(app::Sidebar::Conversations)),
            ("slash", |s| {
                s.open_tab(tabs::TabKind::Agent { session: None });
                let i = s.active;
                s.tabs[i].draft.replace_text("/");
                s.composer_changed();
            }),
            ("mcp-icons", |s| {
                s.open_tab(tabs::TabKind::Agent { session: None });
                s.open_mcp();
                s.apply_agent(neoism_protocol::agent::AgentServerMessage::McpCatalog {
                    status: serde_json::from_str(include_str!("../fixtures/mcp-catalog.json")).unwrap(), directory: None,
                });
            }),
            ("mcp-timeline", |s| {
                s.load_fixture("mcp-icons", "Server-supplied artwork", include_str!("../fixtures/mcp-timeline.json"));
                s.sessions.get_mut("mcp-icons").unwrap().live_from = Some(0);
            }),
            ("tab-overflow", |s| {
                for i in 0..16 { let path = format!("source_{i}.rs"); s.open_file(&path, None); s.apply_opened_file(&path, b"let value = 1;\n"); }
                for i in 0..12 { s.new_workspace(); s.workspace_name = format!("Workspace {i}"); }
                s.select_workspace(0);
            }),
            ("tab-reorder", |s| {
                s.workspace_name = "First".into();
                s.open_file("first.rs", None); s.apply_opened_file("first.rs", b"let first = 1;\n");
                s.open_tab(tabs::TabKind::Agent { session: None });
                s.new_workspace(); s.workspace_name = "Second".into();
                s.new_workspace(); s.workspace_name = "Third".into();
                s.select_workspace(0);
            }),
            ("splits", |s| {
                s.split(panes::Axis::Horizontal);
                s.open_tab(tabs::TabKind::Agent { session: None });
                s.split(panes::Axis::Vertical);
            }),
            ("palette", |s| s.open_picker(palette::Mode::Commands)),
            // Change Directory's folder browser, from the launch directory.
            ("browser", |s| s.open_file_browser(browser::Mode::ChooseDirectory)),
            // Forty file hits, for the finder's list and highlight springs.
            // The real Alt+S: the daemon's file search over the working directory (`--live`).
            ("finder", |s| {
                if let Ok(dir) = std::env::current_dir() { s.cwd = dir.display().to_string(); }
                s.open_finder(finder::Mode::Files);
            }),
            ("finder-hits", |s| {
                s.open_finder(finder::Mode::Files);
                if let Some(f) = &mut s.finder {
                    let count = std::env::var("NEOISM_RUNE_FINDER_HITS").ok().and_then(|n| n.parse().ok()).unwrap_or(40);
                    f.hits = (0..count).map(|i| finder::Hit::File { path: format!("src/module_{i:02}/file_{i:02}.rs") }).collect();
                    f.loading = false;
                }
            }),
            ("themes", |s| { s.open_picker(palette::Mode::Themes); s.move_picker(2); }),
            ("backgrounds", |s| { s.open_picker(palette::Mode::Backgrounds); s.move_picker(2); }),
            // `NEOISM_RUNE_FIXTURE=history.json`: render any recorded history page.
            // `NEOISM_RUNE_FILE=<path>`: a real file in the editor, for profiling it.
            ("file", |s| {
                let path = std::env::var("NEOISM_RUNE_FILE").unwrap_or_default();
                let bytes = std::fs::read(&path).unwrap_or_default();
                s.open_file(&path, None);
                s.apply_opened_file(&path, &bytes);
            }),
            ("fixture", |s| {
                let json = std::env::var("NEOISM_RUNE_FIXTURE").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
                s.load_fixture("fixture", "Fixture", &json);
            }),
            ("app-menu", |s| { s.toggle_menu(); if let Some(m) = &mut s.menu { m.selected = 1; } }),
            ("settings", |s| {
                s.open_settings();
                if let Some(settings) = &mut s.settings {
                    settings.set_descriptors(serde_json::from_str(include_str!("../fixtures/settings-schema.json")).expect("settings fixture"));
                }
            }),
            ("keybinds", |s| { s.open_settings(); if let Some(settings) = &mut s.settings { settings.category = settings::Category::Keybinds; settings.loading = false; } }),
            ("database", database::fixture),
            ("editor-splits", |s| {
                s.open_file("left.rs", None); s.apply_opened_file("left.rs", b"let left_value = 1;\n");
                s.split(panes::Axis::Horizontal);
                s.open_file("right.rs", None); s.apply_opened_file("right.rs", b"let right_value = 2;\n");
            }),
            ("about", |s| s.modal = Some(ui::modal::Modal::about())),
            // Systems/Servers from the rail's server button, with fixed entries (not the
            // desktop registries) so renders are reproducible.
            ("systems", |s| { s.open_picker(palette::Mode::Systems); palette::systems_fixture(s); }),
            ("servers", |s| { s.open_picker(palette::Mode::Servers); palette::systems_fixture(s); s.move_picker(1); }),
            // The Add Server form (Host), its Join + Fast tunnel variant, and Add system.
            ("server-form", |s| s.open_server_form()),
            ("server-form-join", |s| {
                s.open_server_form();
                if let Some(form) = &mut s.form { let mut draft = form.draft().unwrap_or_default(); draft.host = false; draft.fast = true; *form = servers::form::Form::server(draft); }
            }),
            ("system-form", |s| s.open_system_form()),
            ("system-settings", |s| {
                s.open_picker(palette::Mode::Systems);
                palette::systems_fixture(s);
                s.move_picker(1);
                s.accept_picker_now(None);
            }),
            // Terminal blocks with the toolbar on the newest (hovered), its actions menu, and a
            // filtered block; the Home terminal owns the working directory (filled star).
            ("terminal-actions", |s| {
                s.fixture_blocks(&TERMINAL_FIXTURE);
                let tab = s.active_tab().id;
                if let Some(term) = s.terminals.get_mut(&tab) { term.hovered = Some(9002); term.more = Some(9002); }
            }),
            ("terminal-filter", |s| {
                s.fixture_blocks(&TERMINAL_FIXTURE);
                let tab = s.active_tab().id;
                s.toggle_block_filter(tab, 9001);
                if let Some(term) = s.terminals.get_mut(&tab) { term.filter.as_mut().unwrap().1.replace_text("rs"); }
            }),
            // A second terminal next to Home: Home keeps the directory until the star moves it.
            // A full-screen program with coloured rows (status bar, selection, panels).
            ("terminal-tui", |s| {
                let tab = s.active_tab().id;
                let Some(term) = s.terminals.get_mut(&tab) else { return };
                term.shell.session = Some(format!("fixture-{tab}"));
                term.shell.ready = true;
                let mut out = String::from("\x1b[?1049h\x1b[2J\x1b[H\x1b[44;97m Codex  ~/projects/neoism                              model: gpt-6.1 \x1b[0m\r\n");
                for i in 0..14 {
                    let bg = if i == 5 { "\x1b[48;5;238m" } else if i % 4 == 0 { "\x1b[48;5;235m" } else { "" };
                    out.push_str(&format!("{bg}  {i:>2}  line of program output with \x1b[32mcolour\x1b[39m and a \x1b[43;30m tag \x1b[0m{bg}   \x1b[0m\r\n"));
                }
                out.push_str("\x1b[42;30m NORMAL \x1b[0m\x1b[48;5;236m  main.rs  \x1b[0m");
                term.shell.feed(out.as_bytes(), None);
            }),
            // Plain finished blocks: hovering one swaps its status for the toolbar.
            ("terminal-scroll", |s| s.fixture_blocks(&TERMINAL_FIXTURE)),
            // A Home tab running Codex next to a Terminal running Claude Code: logos and names.
            ("terminal-agent-tabs", |s| {
                s.fixture_blocks(&TERMINAL_FIXTURE[..1]);
                let home = s.active_tab().id;
                if let Some(term) = s.terminals.get_mut(&home) { term.program = Some("codex"); }
                s.open_tab(tabs::TabKind::Terminal);
                let terminal = s.active_tab().id;
                if let Some(term) = s.terminals.get_mut(&terminal) { term.program = Some("claude"); }
            }),
            ("terminal-owner", |s| {
                s.fixture_blocks(&TERMINAL_FIXTURE[..1]);
                let home = s.active_tab().id;
                s.split(panes::Axis::Horizontal);
                s.fixture_blocks(&TERMINAL_FIXTURE[1..2]);
                let second = s.active_tab().id;
                if let Some(term) = s.terminals.get_mut(&second) { term.shell.cwd = "/tmp".into(); }
                s.set_main_terminal(second);
                let _ = home;
            }),
            // The `~` dashboard: chats from every project, grouped by folder.
            // The panel's project dropdown under the directory row, over chats from three projects.
            ("projects-menu", |s| {
                s.threads = [("a", "Neoism Release", "/projects/neoism", 9), ("b", "Swift-Cursor", "/projects/neoism", 7),
                    ("d", "Rune layout", "/projects/rune", 5), ("e", "Dotfiles", "/projects/dotfiles", 3)]
                    .iter().map(|(id, title, dir, at)| serde_json::from_value(serde_json::json!({
                        "session_id": id, "title": title, "directory": dir, "updated_at": 1_700_000_000_000u64 + at * 1000,
                    })).unwrap()).collect();
                s.sidebar = Some(app::Sidebar::Conversations);
                s.focus = app::Focus::Sidebar;
                s.tabs.iter_mut().for_each(|t| t.draft.focused = false);
                s.conv_header = Some(agent::ConvHeader::Projects);
                s.open_project_menu();
            }),
            ("conversations-dashboard", |s| {
                let home = crate::startup::home();
                s.cwd = home.clone();
                s.workspace_name = "~".into();
                s.threads = [("a", "Neoism Release", "/projects/neoism", 9), ("b", "Swift-Cursor", "/projects/neoism", 7),
                    ("c", "Dotfiles cleanup", home.as_str(), 8), ("d", "Rune layout", "/projects/rune", 5), ("e", "Pinned plan", "/projects/rune", 1)]
                    .iter().map(|(id, title, dir, at)| serde_json::from_value(serde_json::json!({
                        "session_id": id, "title": title, "directory": dir, "updated_at": 1_700_000_000_000u64 + at * 1000, "pinned": *id == "e",
                    })).unwrap()).collect();
                s.collapsed_projects.insert("/projects/rune".into());
                s.sidebar = Some(app::Sidebar::Conversations);
                s.focus = app::Focus::Sidebar;
                s.conv_selected = 1;
            }),
            // A long file and a long tree, for walking with held arrows.
            ("editor-long", |s| {
                s.open_file("src/app.rs", None);
                s.apply_opened_file("src/app.rs", include_bytes!("app.rs"));
                let entries: Vec<_> = (0..80).map(|i| serde_json::json!({"name": format!("file_{i:02}.rs"), "is_dir": false})).collect();
                s.apply_dir_listing("", &serde_json::Value::Array(entries));
                s.sidebar = Some(app::Sidebar::Files);
            }),
            // Large, wrapped file in a narrow split: scrolling must only mount visible rows.
            ("editor-stress", |s| {
                s.open_file("left.rs", None);
                s.apply_opened_file("left.rs", b"// Adjacent pane stays independent.\n");
                s.split(panes::Axis::Horizontal);
                let source: String = (0..20_000).map(|i| format!("pub fn item_{i}(value: usize) -> String {{ format!(\"value: {{}} — Unicode and a wrapped source line\", value + {i}) }}\n")).collect();
                s.open_file("stress.rs", None);
                s.apply_opened_file("stress.rs", source.as_bytes());
            }),
            ("editor-md", |s| {
                s.open_file("README.md", None);
                s.apply_opened_file("README.md", include_bytes!("../../README.md"));
            }),
            ("sidebar-long", |s| {
                s.threads = (0..40).map(|i| serde_json::from_value(serde_json::json!({
                    "session_id": format!("s{i}"), "title": format!("Conversation number {i}"), "pinned": i < 2,
                }))).collect::<Result<_, _>>().expect("fixture threads");
                s.sidebar = Some(app::Sidebar::Conversations);
                s.focus = app::Focus::Sidebar;
                // The opening composer would otherwise read as a click into it (sync_field_focus).
                s.tabs.iter_mut().for_each(|t| t.draft.focused = false);
            }),
            // A Rust buffer with a live language server: diagnostics and the `K` hover card.
            ("lsp", |s| {
                lsp_fixture(s);
                if let Some(b) = s.buffers.get_mut("src/demo.rs") {
                    b.doc.set_cursor(editor::vim::Pos { line: 1, col: 24 });
                    b.lsp.hover = Some(editor::lsp::Hover { seq: 7, text: None, anchor: None });
                }
                s.apply_editor_reply(serde_json::json!({"LspHoverResult": {"seq": 7, "line": 1, "character": 24,
                    "contents": "```rust\nalloc::vec::Vec\n\npub struct Vec<T, A = Global>\n```\n\n---\n\nA contiguous growable array type, written as `Vec<T>`, short for 'vector'."}}));
                // `NEOISM_RUNE_LONG_HOVER=1`: a card taller than its limit, with links (wheel and clicks).
                if std::env::var_os("NEOISM_RUNE_LONG_HOVER").is_some() {
                    let mut text = String::from("```rust\npub struct Vec<T>\n```\n\nSee [the Vec docs](https://doc.rust-lang.org/std/vec/struct.Vec.html).\n\n");
                    for i in 0..60 { text.push_str(&format!("Paragraph {i} of the documentation, long enough to wrap in the card.\n\n")); }
                    s.apply_editor_reply(serde_json::json!({"LspHoverResult": {"seq": 7, "line": 1, "character": 24, "contents": text}}));
                }
            }),
            // The real daemon: open the launch directory's `src/main.rs` (language servers attach).
            ("lsp-live", |s| s.open_file("src/main.rs", Some(2))),
            // Insert mode after `items.pu`: the completion menu.
            ("lsp-complete", |s| {
                lsp_fixture(s);
                if let Some(b) = s.buffers.get_mut("src/demo.rs") {
                    b.doc.mode = editor::document::Mode::Insert;
                    b.doc.set_cursor(editor::vim::Pos { line: 2, col: 12 });
                    b.lsp.completion = Some(editor::lsp::Completion { seq: 9, line: 2, start: 10, items: Vec::new(), selected: 0 });
                }
                let item = |label: &str, kind: &str, detail: &str| serde_json::json!({"label": label, "kind": kind, "detail": detail, "insert_text": label});
                s.apply_editor_reply(serde_json::json!({"LspCompletions": {"seq": 9, "items": [
                    item("push", "method", "fn(&mut self, T)"), item("push_within_capacity", "method", "fn(&mut self, T) -> Result"),
                    item("pop", "method", "fn(&mut self) -> Option<T>"), item("capacity", "method", "fn(&self) -> usize"),
                    item("PUBLIC", "constant", "usize"),
                ]}}));
            }),
            // Nine questions and answers: the question rail in the right gutter.
            ("question-rail", |s| {
                let messages: Vec<serde_json::Value> = (0..9).flat_map(|i| [
                    serde_json::json!({"id": format!("u{i}"), "role": "User", "kind": "User",
                        "text": format!("Question {i}: how does the **frame scheduler** keep `120 Hz` while the transcript streams and the wheel glides through long answers?")}),
                    serde_json::json!({"id": format!("a{i}"), "role": "Assistant", "kind": "Assistant",
                        "text": format!("## Answer {i}\n\n{}", "The scheduler presents on the display clock and never busy-waits. ".repeat(8))}),
                ]).collect();
                s.load_fixture("fixture-rail", "Question Rail", &serde_json::Value::Array(messages).to_string());
            }),
            // Company in a shared workspace: six people on the rail (four shown, this user first,
            // "+2"), five in the open file (three orbs and "+2" on its tree row; a selection, an
            // insert beam, a block and a rainbow caret in the editor), one in another file.
            ("presence", |s| {
                use neoism_protocol::crdt::{CrdtCursorPosition as At, CrdtPeerPresence, CrdtPresenceColor, CrdtSelectionRange};
                let entries: Vec<_> = ["notes.md", "server.rs", "shared.rs"].iter().map(|name| serde_json::json!({"name": name, "is_dir": false})).collect();
                s.apply_dir_listing("", &serde_json::Value::Array(entries));
                s.sidebar = Some(app::Sidebar::Files);
                s.open_file("shared.rs", None);
                s.apply_opened_file("shared.rs", b"use std::collections::HashMap;\n\nfn main() {\n    let mut seen = HashMap::new();\n    for word in [\"shared\", \"edits\", \"shared\"] {\n        *seen.entry(word).or_insert(0) += 1;\n    }\n    println!(\"{seen:?}\");\n}\n");
                let at = |line, column| At { line, column, offset: None };
                let file = |name: &str| app::App::buffer_id(&format!("{}/{name}", s.cwd.trim_end_matches('/')));
                let peer = |name: &str, buffer: String, color: [u8; 3], cursor: (u32, u32), selection: Option<((u32, u32), (u32, u32))>, insert: bool, rainbow: bool| CrdtPeerPresence {
                    buffer_id: buffer, peer_id: format!("{}@scene", name.to_lowercase()), display_name: name.to_owned(), client_os: (name == "Grace").then(|| "windows".to_owned()),
                    color: CrdtPresenceColor { r: color[0], g: color[1], b: color[2] }, cursor: at(cursor.0, cursor.1),
                    selection: selection.map(|(a, b)| CrdtSelectionRange { anchor: at(a.0, a.1), head: at(b.0, b.1) }), insert, rainbow, updated_at_ms: 0,
                };
                s.collab.stage(vec![
                    peer("Ada", file("shared.rs"), [0xe0, 0x6c, 0x75], (5, 24), Some(((4, 8), (5, 24))), false, false),
                    peer("Bob", file("shared.rs"), [0x61, 0xaf, 0xef], (3, 17), None, true, false),
                    peer("Cy", file("shared.rs"), [0xc6, 0x78, 0xdd], (7, 4), None, false, false),
                    peer("Dee", file("shared.rs"), [0xe5, 0xc0, 0x7b], (0, 4), None, true, true),
                    peer("Eli", file("shared.rs"), [0x56, 0xb6, 0xc2], (8, 1), None, true, false),
                    peer("Grace", file("server.rs"), [0x98, 0xc3, 0x79], (0, 0), None, false, false),
                ]);
                s.peer_hover = Some("bob@scene".into());
            }),
            ("tree-long", |s| {
                let entries: Vec<_> = (0..80).map(|i| serde_json::json!({"name": format!("file_{i:02}.rs"), "is_dir": false})).collect();
                s.apply_dir_listing("", &serde_json::Value::Array(entries));
                s.sidebar = Some(app::Sidebar::Files);
                s.focus = app::Focus::Sidebar;
            }),
            // A live turn mid-stream: the animated "Crafting . . ." status line.
            ("streaming", replay::start),
            ("activity", |s| {
                s.load_fixture("fixture-activity", "Frame Pacing in Rust", include_str!("../fixtures/code-timeline.json"));
                if let Some(session) = s.sessions.get_mut("fixture-activity") {
                    session.live_from = Some(session.messages.len());
                    session.streaming = true;
                }
            }),
            // The first installed background behind the Home splash (no config is written).
            ("wallpaper", |s| {
                let image = std::fs::read_dir(config::dir().join("bg")).ok()
                    .and_then(|mut files| files.find_map(|f| f.ok().map(|f| f.path())));
                if let Some(path) = image {
                    s.wallpaper = ui::wallpaper::Wallpaper::from_config(&serde_json::json!({"ui": {"window": {"background-image": {"path": path, "opacity": 0.35}}}}));
                }
            }),
            ("timeline-report", |s| s.load_fixture("fixture-report", "Read-Only GPU Frame Analysis and Documentation Report", include_str!("../fixtures/report-timeline.json"))),
            ("code", |s| s.load_fixture("fixture-code", "Frame Pacing in Rust", include_str!("../fixtures/code-timeline.json"))),
            ("tool-cards", |s| {
                s.load_fixture("fixture-tool-cards", "Tool Cards", include_str!("../fixtures/tool-cards-timeline.json"));
                // Shown as the live turn, so its whole trace is visible.
                if let Some(session) = s.sessions.get_mut("fixture-tool-cards") { session.live_from = Some(0); }
            }),
            ("tool-cards-open", |s| {
                s.load_fixture("fixture-tool-cards", "Tool Cards", include_str!("../fixtures/tool-cards-timeline.json"));
                if let Some(session) = s.sessions.get_mut("fixture-tool-cards") { session.live_from = Some(0); }
                for id in ["prt_1c14b7a66001aYzMV0RbE1Psr2..", "prt_1c2505048001v1gMCUm75EWj9h", "prt_1c15712740011BAWLf1gNhsbMf:0",
                    "prt_1c250073a001g03n2pGaegM5tS", "prt_1c198dac10011iePQvpkkfkmwM",
                    "prt_1c198dac10011iePQvpkkfkmwM/compose/prt_1c198e302002zGwGCWKs45Ca8Z"] {
                    s.tools.expand(id);
                }
            }),
            ("timeline-tools", |s| s.load_fixture("fixture-tools", "Inspect Git History and GPU Sources; Edit Scratchpad", include_str!("../fixtures/tools-timeline.json"))),
        ],
        ..AppSpec::new("Neoism", (1252.0, 674.0), App::new, ui::chrome::root)
    };
    // A distinct id per instance lets several builds run side by side (and be found by class).
    let app_id = std::env::var("NEOISM_RUNE_APP_ID").unwrap_or_else(|_| "ai.neoism.rune".into());
    rune::set_app_id(Box::leak(app_id.into_boxed_str()));
    // The app draws its own title band: macOS keeps its traffic lights in it and Windows its
    // caption buttons; on Linux the compositor's frame stays and there is no band.
    let mut app = rune::App::new(spec).titlebar(rune::Titlebar::new(ui::chrome::TITLEBAR_H)).init(App::new);
    // Encoded frame callbacks measure submission cadence only. They are not compositor
    // presentation timestamps and cannot establish missed-vblank parity (see rune/README.md).
    let trace = std::env::var("NEOISM_RUNE_FRAME_TRACE").as_deref() == Ok("1");
    let origin = std::time::Instant::now();
    app = app.on_frame_encoded(move |frame| {
        ui::fps::record(frame.at());
        if trace {
            // `mono_us` is CLOCK_MONOTONIC, the clock Wayland presentation feedback reports in;
            // `work_us` runs from this frame's poll to the end of its encode (the UI thread's
            // share of the frame), `poll_us` is the poll alone.
            let (poll_us, work_us, rerun) = ui::fps::last_poll(frame.at());
            eprintln!("[rune-frame] seq={} at_us={} mono_us={} poll_us={} work_us={} rerun={} outcome={:?} hz={:?}", frame.seq(),
                frame.at().duration_since(origin).as_micros(), ui::fps::monotonic_us(frame.at()), poll_us, work_us, rerun,
                frame.outcome(), frame.refresh_hz());
        }
    });
    app.run();
}

