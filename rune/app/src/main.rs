//! Neoism on Rune: a native daemon client that draws every surface with Rune elements.
//! See `rune/README.md` for the plan, the look and the motion this must match.

mod agent;
mod app;
mod config;
mod daemon;
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

fn poll(app: &mut App, clock: f64, tx: &mut Tx) -> bool {
    let changed = app.drain();
    // The wordmark eases hover, shimmers and plays click effects off the frame clock.
    let animating = app.wordmark.tick(app.visible_wordmark(), clock);
    if animating { tx.wake_after(std::time::Duration::ZERO); }
    // Running tool spinners step every ~90 ms (8 steps per 1/1.35 s lap).
    app.tools.clock = clock;
    let spinning = app.running_tools();
    if spinning { tx.wake_after(std::time::Duration::from_millis(45)); }
    // Refresh the FPS pill when the measured rate moves; check again as old frames age out.
    let fps = ui::fps::rate();
    let fps_changed = app.fps != Some(fps);
    if fps_changed {
        app.fps = Some(fps);
        tx.wake_after(std::time::Duration::from_millis(500));
    }
    animating || changed || spinning || fps_changed
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
    // `NEOISM_RUNE_APP_ID` gives a test instance its own window class next to a running one.
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
