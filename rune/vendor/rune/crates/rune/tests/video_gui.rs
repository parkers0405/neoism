//! A window plays a short clip muted through AVPlayer: the poster arrives before play, frames
//! advance and present while playing, the clip ends, nothing redraws while stopped, and
//! looping keeps frames coming. Then an audio clip with no element loads, plays, advances and
//! ends. The audio is silent and muted, so nothing is ever heard. Opens a native window, so it
//! only runs with RUNE_GUI_TESTS=1 in a logged-in GUI session on macOS.

use rune::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Gui;

static CLIP: OnceLock<Video> = OnceLock::new();
static FRAMES: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

fn clip() -> &'static Video {
    CLIP.get_or_init(|| Video::file(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rgbw.mp4")))
}

fn view(_: &Gui, _: &mut Cx<Gui>) -> El {
    el().key("video").w(320.0).h(180.0).video(clip(), SurfaceFit::Fit)
}

fn wait(what: &str, limit: Duration, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < limit,
            "timed out waiting for {what}: {:?}, draws {:?}, frames {}, error {:?}",
            clip(),
            clip().surface().counts(),
            frames(),
            clip().surface().last_error()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for(what: &str, limit: Duration, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < limit, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn frames() -> usize {
    FRAMES.lock().unwrap().len()
}

fn main() {
    if std::env::var_os("RUNE_GUI_TESTS").is_none() || !cfg!(target_os = "macos") {
        println!("video_gui: skipped (set RUNE_GUI_TESTS=1 in a macOS GUI session)");
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(60));
        eprintln!("timeout");
        std::process::exit(2)
    });
    clip().surface().set_observer(Arc::new(|t| {
        FRAMES.lock().unwrap().insert(t.frame_id);
    }));
    let app = App::new(AppSpec::new("video gui", (320.0, 180.0), || Gui, view)).args(Vec::new());
    let main = app.handle();
    let result = Arc::new(Mutex::new(None));
    let out = result.clone();
    std::thread::spawn(move || {
        let run = std::panic::catch_unwind(|| {
            let clip = clip();
            let secs = Duration::from_secs;
            wait("main window", secs(5), || main.is_visible());
            wait("poster", secs(5), || clip.state() == PlayerState::Ready && frames() >= 1);
            assert_eq!(clip.natural_size(), Some((64, 36)));
            assert!((clip.duration().unwrap() - 1.0).abs() < 0.01, "{:?}", clip.duration());
            assert!(clip.muted(), "videos start muted");

            // The load re-runs the view once; after that the window goes idle.
            std::thread::sleep(Duration::from_millis(300));
            let quiet = clip.surface().counts();
            std::thread::sleep(Duration::from_millis(400));
            assert_eq!(clip.surface().counts(), quiet, "a loaded, paused video requests no frames");

            let before = frames();
            let presented = clip.surface().counts().1;
            clip.play();
            wait("playing", secs(5), || clip.state() == PlayerState::Playing);
            wait("ended", secs(5), || clip.state() == PlayerState::Ended);
            let played = frames() - before;
            assert!(played >= 3, "the four-frame clip presented {played} new frames");
            assert!(clip.surface().counts().1 > presented, "draws completed while playing");
            assert!((clip.time() - 1.0).abs() < 0.1, "ends at the end: {}", clip.time());

            std::thread::sleep(Duration::from_millis(300));
            let quiet = clip.surface().counts();
            std::thread::sleep(Duration::from_millis(600));
            assert_eq!(clip.surface().counts(), quiet, "an ended video requests no frames");

            let before = frames();
            clip.set_looping(true);
            clip.play();
            wait("playing again", secs(5), || clip.state() == PlayerState::Playing);
            std::thread::sleep(Duration::from_millis(2500));
            assert_eq!(clip.state(), PlayerState::Playing, "looping keeps playing");
            let looped = frames() - before;
            assert!(looped >= 6, "two and a half loops presented {looped} new frames");

            clip.pause();
            wait("paused", secs(5), || clip.state() == PlayerState::Ready);
            // The frame that applies the pause reads where the player stopped.
            std::thread::sleep(Duration::from_millis(200));
            let at = clip.time();
            let quiet = clip.surface().counts();
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(clip.time(), at, "paused time holds");
            assert_eq!(clip.surface().counts(), quiet, "a paused video requests no frames");

            // Two of the four 0.25 s frames on from where it paused, so a new frame shows.
            let target = ((at / 0.25).floor() + 2.0) % 4.0 * 0.25 + 0.1;
            let before = frames();
            clip.seek(target);
            wait("seek frame", secs(3), || frames() > before);
            std::thread::sleep(Duration::from_millis(300));
            assert!((clip.time() - target).abs() < 0.01, "a paused seek lands: {} for {target}", clip.time());
            assert_eq!(clip.state(), PlayerState::Ready);
            // A paused seek within the frame on screen has no frame to wait for: no redraws.
            clip.seek(target + 0.05);
            std::thread::sleep(Duration::from_millis(200));
            let quiet = clip.surface().counts();
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(clip.surface().counts(), quiet, "a same-frame paused seek requests no frames");
            assert!((clip.time() - target - 0.05).abs() < 0.01, "{}", clip.time());
            println!("video_gui: {played} frames in one play, {looped} while looping");

            let wakes = Arc::new(AtomicUsize::new(0));
            let heard = wakes.clone();
            let tap = Player::audio(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/silence.m4a"));
            assert!(!tap.muted(), "audio starts audible");
            tap.set_muted(true);
            tap.set_waker(Arc::new(move || {
                heard.fetch_add(1, Ordering::Relaxed);
            }));
            wait_for("audio duration", secs(5), || tap.state() == PlayerState::Ready);
            let duration = tap.duration().unwrap();
            assert!((duration - 1.0).abs() < 0.1, "{duration}");
            assert_eq!(tap.natural_size(), None);
            let draws = clip.surface().counts();
            tap.play();
            wait_for("audio playing", secs(5), || tap.state() == PlayerState::Playing);
            wait_for("audio time advancing", secs(5), || tap.time() > 0.2);
            assert_eq!(tap.state(), PlayerState::Playing);
            wait_for("audio ended", secs(5), || tap.state() == PlayerState::Ended);
            assert!((tap.time() - duration).abs() < 0.1, "ends at the end: {}", tap.time());
            assert!(tap.muted(), "stayed muted");
            assert!(wakes.load(Ordering::Relaxed) >= 2, "the waker heard the load and the end");
            assert_eq!(clip.surface().counts(), draws, "audio drew nothing");

            let ogg = Player::audio("https://example.com/clip.ogg");
            assert!(matches!(ogg.state(), PlayerState::Failed(e) if e.contains(".ogg")), "{:?}", ogg.state());
            println!("video_gui: audio played {duration:.2} s to the end, muted");
        });
        *out.lock().unwrap() = Some(run.is_ok());
        main.close();
    });
    app.run();
    assert_eq!(*result.lock().unwrap(), Some(true), "video_gui failed");
    println!("video_gui: ok");
}
