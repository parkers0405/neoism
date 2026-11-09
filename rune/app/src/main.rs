//! Neoism on Rune: a native daemon client that draws every surface with Rune elements.
//! See `rune/README.md` for the plan, the look and the motion this must match.

mod agent;
mod app;
mod config;
mod daemon;
mod editor;
mod finder;
mod ghostty;
mod home;
mod keys;
mod palette;
mod tabs;
mod ui;
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
    app.images = Some(images);
    // Headless renders (`--snap`, `--tree`) draw the reference look without a daemon.
    if std::env::var_os("NEOISM_RUNE_OFFLINE").is_none() {
        let wake = waker.clone();
        let (daemon, inbox) = daemon::Daemon::start(move || wake());
        app.daemon = Some(daemon);
        app.inbox = Some(inbox);
    }
}

fn poll(app: &mut App, clock: f64, tx: &mut Tx) -> bool {
    let changed = app.drain();
    // The wordmark eases hover, shimmers and plays click effects off the frame clock.
    let animating = app.wordmark.tick(app.visible_wordmark(), clock);
    if animating { tx.wake_after(std::time::Duration::ZERO); }
    // Presence orbs flow every frame while a transcript with people in it is on screen.
    if app.orbs_on_screen() && app.orbs.tick() { tx.wake_after(std::time::Duration::ZERO); }
    app.editor_follow_cursor(tx);
    // The finder searches once its query settles.
    if let Some(delay) = app.finder_tick(std::time::Instant::now()) { tx.wake_after(delay); }
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
    animating || changed || fps_changed || caret_changed
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
            ("slash", |s| { s.open_tab(tabs::TabKind::Agent { session: None }); s.overlay = Some(app::Overlay::Slash); }),
            ("palette", |s| s.open_picker(palette::Mode::Commands)),
            ("themes", |s| { s.open_picker(palette::Mode::Themes); s.move_picker(2); }),
            ("timeline-report", |s| s.load_fixture("fixture-report", "Read-Only GPU Frame Analysis and Documentation Report", include_str!("../fixtures/report-timeline.json"))),
            ("code", |s| s.load_fixture("fixture-code", "Frame Pacing in Rust", include_str!("../fixtures/code-timeline.json"))),
            ("timeline-tools", |s| s.load_fixture("fixture-tools", "Inspect Git History and GPU Sources; Edit Scratchpad", include_str!("../fixtures/tools-timeline.json"))),
        ],
        ..AppSpec::new("Neoism", (1252.0, 674.0), App::new, ui::chrome::root)
    };
    // A distinct id per instance lets several builds run side by side (and be found by class).
    let app_id = std::env::var("NEOISM_RUNE_APP_ID").unwrap_or_else(|_| "ai.neoism.rune".into());
    rune::set_app_id(Box::leak(app_id.into_boxed_str()));
    let mut app = rune::App::new(spec).init(App::new);
    // Every encoded frame feeds the FPS pill; `NEOISM_RUNE_FRAME_TRACE=1` also logs each one for
    // the missed-frame analysis the GPUiX investigation used (`docs/gpuix-live-rendering-baseline.md`).
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
