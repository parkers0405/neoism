//! Neoism on Rune: a native daemon client that draws every surface with Rune elements.
//! See `rune/README.md` for the plan, the look and the motion this must match.

mod agent;
mod config;
mod daemon;
mod ghostty;
mod keys;
mod prose;
mod state;
mod terminal;
mod theme;
mod theme_data;
mod view;
mod wordmark;

use rune::{AppSpec, ImageCache, KeyEvent, Tx};
use state::App;

pub const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/GeistMono-Regular.otf"),
    include_bytes!("../assets/fonts/GeistMono-Bold.otf"),
    include_bytes!("../assets/fonts/GeistMono-Italic.otf"),
    include_bytes!("../assets/fonts/GeistMono-BoldItalic.otf"),
    include_bytes!("../assets/fonts/CascadiaCodeNF-Regular.otf"),
    include_bytes!("../assets/fonts/PressStart2P-Regular.ttf"),
];

fn setup(app: &mut App, waker: rune::Waker) {
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

fn poll(app: &mut App, _clock: f64, _tx: &mut Tx) -> bool {
    app.drain()
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
            ("agent-new", |s| { s.open_tab(state::TabKind::Agent { session: None }); }),
            ("sidebar", |s| s.sidebar = Some(state::Sidebar::Conversations)),
            ("slash", |s| { s.open_tab(state::TabKind::Agent { session: None }); s.overlay = Some(state::Overlay::Slash); }),
            ("timeline-report", |s| s.load_fixture("fixture-report", "Read-Only GPU Frame Analysis and Documentation Report", include_str!("../fixtures/report-timeline.json"))),
            ("code", |s| s.load_fixture("fixture-code", "Frame Pacing in Rust", include_str!("../fixtures/code-timeline.json"))),
            ("timeline-tools", |s| s.load_fixture("fixture-tools", "Inspect Git History and GPU Sources; Edit Scratchpad", include_str!("../fixtures/tools-timeline.json"))),
        ],
        ..AppSpec::new("Neoism", (1252.0, 674.0), App::new, view::root)
    };
    rune::set_app_id("ai.neoism.rune");
    let mut app = rune::App::new(spec).init(App::new);
    // `NEOISM_RUNE_FRAME_TRACE=1`: one stderr line per frame, for the same missed-frame analysis
    // the GPUiX investigation used (`docs/gpuix-live-rendering-baseline.md`).
    if std::env::var("NEOISM_RUNE_FRAME_TRACE").as_deref() == Ok("1") {
        let origin = std::time::Instant::now();
        app = app.on_frame_encoded(move |frame| {
            eprintln!("[rune-frame] seq={} at_us={} outcome={:?} hz={:?}", frame.seq(),
                frame.at().duration_since(origin).as_micros(), frame.outcome(), frame.refresh_hz());
        });
    }
    app.run();
}
