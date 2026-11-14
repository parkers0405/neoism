//! `rune::text::with_families` scopes `measure` to one thread, so tests that switch families
//! run in parallel.
use std::sync::Once;

use rune::text::{TextSystem, font_epoch, measure, with_families};
use rune::{Env, Runtime, TextStyle, col, text};

const INTER: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
const STYLE: TextStyle = TextStyle::new(14.0, 400.0);
const CODE: &str = "let mono = 42; // iiiiiiiiii";

/// The process-wide families every test here starts from: Inter in both slots.
fn process() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        TextSystem::new(&[INTER, PLEX], ("Inter", "Inter"));
    });
}

fn mono_width() -> f32 {
    measure(CODE, &[], STYLE.mono(), None).0
}

#[test]
fn threads_keep_their_own_families() {
    process();
    let run = |scoped: bool| {
        std::thread::spawn(move || {
            process();
            let _g = scoped.then(|| with_families("Inter", "IBM Plex Mono"));
            let widths: Vec<f32> = (0..100).map(|_| mono_width()).collect();
            assert!(widths.iter().all(|w| *w == widths[0]), "stable on one thread");
            widths[0]
        })
    };
    let (a, b) = (run(true), run(false));
    let (plex, inter) = (a.join().unwrap(), b.join().unwrap());
    assert_ne!(plex, inter, "the guard's mono is wider than the process's");
}

#[test]
fn nests_and_restores_on_drop() {
    process();
    let outside = mono_width();
    let epoch = font_epoch();
    {
        let _outer = with_families("Inter", "IBM Plex Mono");
        let plex = mono_width();
        let outer_epoch = font_epoch();
        assert_ne!(plex, outside);
        assert_ne!(outer_epoch, epoch, "caches keyed on the epoch keep scoped sizes apart");
        {
            let _inner = with_families("Inter", "Inter");
            assert_eq!(mono_width(), outside);
            assert_ne!(font_epoch(), outer_epoch);
            assert_ne!(font_epoch(), epoch);
        }
        assert_eq!(mono_width(), plex);
        assert_eq!(font_epoch(), outer_epoch);
    }
    assert_eq!(mono_width(), outside);
    assert_eq!(font_epoch(), epoch);
}

#[test]
fn a_window_switch_in_a_guard_stays_on_the_thread() {
    process();
    let outside = mono_width();
    let other = std::thread::spawn(|| {
        process();
        mono_width()
    });
    let _g = with_families("Inter", "Inter");
    let mut rt = Runtime::with_text(
        (),
        |_: &(), _: &mut rune::Cx<()>| col().child(text(CODE).font(STYLE.mono())),
        (600.0, 400.0),
        Env::new(true, 2.0),
        TextSystem::new(&[INTER, PLEX], ("Inter", "Inter")),
    );
    rt.update_if(|_, tx| {
        tx.set_font_families("Inter", "IBM Plex Mono");
        false
    });
    rt.frame(0.0);
    assert_ne!(mono_width(), outside, "this thread's measure followed the switch");
    assert_eq!(other.join().unwrap(), outside, "no other thread's did");
    drop(_g);
    assert_eq!(mono_width(), outside, "and it ended with the guard");
}
