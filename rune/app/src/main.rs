//! Neoism on Rune: a native daemon client that draws every surface with Rune elements.
//! See `rune/README.md` for the plan, the look and the motion this must match.

mod agent;
mod app;
mod config;
mod daemon;
mod database;
mod editor;
mod finder;
mod ghostty;
mod home;
mod keys;
mod palette;
mod replay;
mod panes;
mod settings;
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
}

/// `src/demo.rs` open with rust-analyzer attached and two diagnostics.
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
    // Terminal output is parsed as it arrives; blocks and grids are re-read once per frame.
    let changed = replay::tick(app, clock, tx) | app.poll_usage() | app.drain() | app.take_decoded_previews() | app.sync_terminals(tx);
    if app.quit_requested { tx.quit(); return true; }
    // The wordmark eases hover, shimmers and plays click effects off the frame clock.
    let animating = app.wordmark.tick(app.visible_wordmark(), clock);
    if animating { tx.wake_after(std::time::Duration::ZERO); }
    // Running tool spinners step every ~90 ms (8 steps per 1/1.35 s lap).
    app.tools.clock = clock;
    let spinning = app.running_tools();
    if spinning { tx.wake_after(std::time::Duration::from_millis(45)); }
    // Presence orbs flow every frame while a transcript with people in it is on screen.
    if app.orbs_on_screen() && app.orbs.tick() { tx.wake_after(std::time::Duration::ZERO); }
    app.editor_follow_cursor(tx);
    app.tree_follow_selection(tx);
    app.palette_follow_selection(tx);
    app.finder_follow_selection(tx);
    app.conversation_follow(tx);
    let panes_changed = app.sync_panes();
    let flashing = app.editor_tick();
    let mouse_changed = app.editor_mouse_tick(tx);
    // Edited buffers re-sync to the language servers once typing settles.
    if let Some(delay) = app.lsp_tick() { tx.wake_after(delay); }
    if flashing { tx.wake_after(std::time::Duration::ZERO); }
    let focus_changed = app.sync_field_focus();
    // The live turn's status line animates on live bindings; it rebuilds only on state changes.
    let (busy, activity_changed) = app.tick_activity();
    if busy { tx.wake_after(std::time::Duration::ZERO); }
    // The finder searches once its query settles.
    if let Some(delay) = app.finder_tick(std::time::Instant::now()) { tx.wake_after(delay); }
    // A `/piss` … `/praise` skit plays on the frame clock.
    let skit = app.tick_effect();
    if skit { tx.wake_after(std::time::Duration::ZERO); }
    // The cursor trails and blinks on its own schedule.
    let cursor_was = (app.cursor.visible, app.cursor.trail.quad());
    let target = app.cursor_target();
    if let Some(delay) = app.cursor.tick(target, std::time::Instant::now()) { tx.wake_after(delay); }
    let caret_changed = cursor_was != (app.cursor.visible, app.cursor.trail.quad());
    // Refresh the FPS pill when the measured rate moves; check again as old frames age out.
    let fps = ui::fps::rate();
    let fps_changed = app.fps != Some(fps);
    if fps_changed {
        app.fps = Some(fps);
        tx.wake_after(std::time::Duration::from_millis(500));
    }
    animating || changed || spinning || fps_changed || caret_changed || focus_changed || panes_changed || skit || activity_changed || flashing || mouse_changed
}

fn on_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    keys::on_key(app, event, tx)
}

fn main() {
    let spec = AppSpec::<App> {
        fonts: FONTS,
        families: ("Geist Mono", "Geist Mono"),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        scenes: &[
            ("welcome", |_| {}),
            ("agent-new", |s| { s.open_tab(tabs::TabKind::Agent { session: None }); }),
            ("sidebar", |s| s.sidebar = Some(app::Sidebar::Conversations)),
            ("slash", |s| {
                s.open_tab(tabs::TabKind::Agent { session: None });
                let i = s.active;
                s.tabs[i].draft.replace_text("/");
                s.composer_changed();
            }),
            ("splits", |s| {
                s.split(panes::Axis::Horizontal);
                s.open_tab(tabs::TabKind::Agent { session: None });
                s.split(panes::Axis::Vertical);
            }),
            ("palette", |s| s.open_picker(palette::Mode::Commands)),
            ("themes", |s| { s.open_picker(palette::Mode::Themes); s.move_picker(2); }),
            ("backgrounds", |s| { s.open_picker(palette::Mode::Backgrounds); s.move_picker(2); }),
            // `NEOISM_RUNE_FIXTURE=history.json`: render any recorded history page.
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
    let mut app = rune::App::new(spec).init(App::new);
    // Encoded frame callbacks measure submission cadence only. They are not compositor
    // presentation timestamps and cannot establish missed-vblank parity (see rune/README.md).
    let trace = std::env::var("NEOISM_RUNE_FRAME_TRACE").as_deref() == Ok("1");
    let origin = std::time::Instant::now();
    app = app.on_frame_encoded(move |frame| {
        ui::fps::record(frame.at());
        if trace {
            eprintln!("[rune-frame] seq={} at_us={} outcome={:?} hz={:?}", frame.seq(),
                frame.at().duration_since(origin).as_micros(), frame.outcome(), frame.refresh_hz());
        }
    });
    app.run();
}
