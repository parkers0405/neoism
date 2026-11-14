//! Shared compact search material. Backdrop blur is frosting, not optical refraction.
use crate::{app::App, ui::theme::rgb};
use rune::{El, Spring};

pub fn expanded(query: &str) -> bool { !query.trim().is_empty() }

pub fn top(app: &App) -> f32 {
    (60.0 + 80.0 * app.ty.scale()).min((app.window[1] - 140.0 * app.ty.scale()).max(8.0))
}

pub fn rows(app: &App, max: usize) -> usize {
    ((app.window[1] - top(app) - 76.0 * app.ty.scale()) / (32.0 * app.ty.scale()))
        .floor().max(1.0).min(max as f32) as usize
}

pub fn query(query: &str) -> &str {
    if query == "?" { "" } else { query.trim() }
}

/// Let Rune's field handle selection and character/word navigation before app shortcuts.
pub fn editing_key(event: &rune::KeyEvent) -> bool {
    matches!(event.key, rune::Key::Left | rune::Key::Right | rune::Key::Home | rune::Key::End
        | rune::Key::Backspace | rune::Key::Delete)
        || ['a', 'c', 'x', 'v', 'z'].into_iter().any(|key| event.is_command(key))
}

pub fn motion() -> Spring {
    if crate::glass_cursor::reduce_motion() || std::env::var("NEOISM_REDUCED_MOTION").as_deref() == Ok("1") {
        Spring::NONE
    } else { Spring::new(0.22, 0.0) }
}

pub fn surface(el: El, app: &App, expanded: bool) -> El {
    let opaque = reduced_transparency()
        || std::env::var("NEOISM_REDUCED_TRANSPARENCY").as_deref() == Ok("1")
        || std::env::var("NEOISM_INCREASED_CONTRAST").as_deref() == Ok("1");
    let s = app.ty.scale();
    let radius = if expanded { 16.0 } else { 28.0 } * s;
    el.motion(motion()).radius(radius).clip_rounded(radius)
        .backdrop_blur(if opaque { 0.0 } else { 12.0 * s })
        // Tint with the theme BACKGROUND, not its raised surface. Neoism Dark is black;
        // a raised gray fill washes out its glass even with a real blurred backdrop.
        .fill(rgb(app.pal.bg).alpha(if opaque { 1.0 } else { 0.90 }))
        .border(1.0, rgb(app.pal.dim).alpha(if opaque { 0.40 } else { 0.24 }))
}

#[cfg(not(target_os = "macos"))]
fn reduced_transparency() -> bool { false }

#[cfg(target_os = "macos")]
fn reduced_transparency() -> bool {
    use std::ffi::{c_char, c_void};
    // Read public AppKit accessibility properties without changing the concurrently owned
    // optical bridge. Only the UI thread may message NSWorkspace; tests fall back opaque.
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
        fn pthread_main_np() -> std::ffi::c_int;
    }
    unsafe {
        if pthread_main_np() == 0 { return true; }
        let class = objc_getClass(c"NSWorkspace".as_ptr());
        if class.is_null() { return true; }
        let object: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void = std::mem::transmute(objc_msgSend as *const ());
        let boolean: unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool = std::mem::transmute(objc_msgSend as *const ());
        let workspace = object(class, sel_registerName(c"sharedWorkspace".as_ptr()));
        workspace.is_null()
            || boolean(workspace, sel_registerName(c"accessibilityDisplayShouldReduceTransparency".as_ptr()))
            || boolean(workspace, sel_registerName(c"accessibilityDisplayShouldIncreaseContrast".as_ptr()))
    }
}

#[cfg(test)]
mod tests {
    use crate::{app::App, palette::{Mode, view}, finder};
    use rune::{Runtime, Env, Cx, el};

    fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
        rt.elements().iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap().rect
    }

    #[test]
    fn compact_query_cursor_edits_in_place_while_arrows_only_select_results() {
        use rune::{Key, KeyEvent};
        for finder in [false, true] {
            let mut app = App::new();
            app.ty.size = 14.0;
            app.window = [800.0, 600.0];
            if finder { app.open_finder(finder::Mode::Files); }
            else { app.open_picker(Mode::Commands); }
            let mut rt = Runtime::new(app, |a: &App, cx: &mut Cx<App>| {
                a.cursor.caret_line.set(cx.caret("modal-test"));
                el().key("modal-test").w_full().h_full().child(if a.finder.is_some() { finder::view(a) } else { view(a) })
            }, (800.0, 600.0), Env::new(true, 1.0));
            let settle = |rt: &mut Runtime<App>| { for _ in 0..80 { rt.frame(1.0 / 120.0); } };
            let press = |rt: &mut Runtime<App>, key| { rt.key(KeyEvent { key, mods: Default::default() }); settle(rt); };
            settle(&mut rt);
            press(&mut rt, Key::Text("theme".into()));
            press(&mut rt, Key::Left);
            press(&mut rt, Key::Left);
            press(&mut rt, Key::Text("X".into()));
            rt.update(|a, _| {
                let query = a.finder.as_ref().map(|f| &f.query).or_else(|| a.picker.as_ref().map(|p| &p.query)).unwrap();
                assert_eq!(query.text, "theXme");
                let caret = a.cursor_target().expect("measured insertion cursor");
                let clip = a.cursor_clip().unwrap();
                assert!(caret[1] >= clip[1] && caret[1] + caret[3] <= clip[1] + clip[3] + 0.1, "caret {caret:?}, clip {clip:?}, finder={finder}");
            });
            press(&mut rt, Key::Backspace);
            press(&mut rt, Key::Delete);
            press(&mut rt, Key::Home);
            press(&mut rt, Key::Text("A".into()));
            press(&mut rt, Key::End);
            press(&mut rt, Key::Text("Z".into()));
            rt.update(|a, _| {
                let query = a.finder.as_ref().map(|f| &f.query).or_else(|| a.picker.as_ref().map(|p| &p.query)).unwrap();
                assert_eq!(query.text, "AtheeZ");
            });
            // Replace through real editing, not a query setter, then test selection navigation.
            let mut mods = rune::Mods::default();
            mods.logo = true;
            mods.cmd = true;
            rt.key(KeyEvent { key: Key::Command('a'), mods });
            settle(&mut rt);
            press(&mut rt, Key::Text("?".into()));
            let mut before = None;
            rt.update(|a, _| {
                if let Some(f) = &mut a.finder {
                    f.hits = vec![finder::Hit::File { path: "a.rs".into() }, finder::Hit::File { path: "b.rs".into() }];
                }
                before = a.cursor_target();
            });
            press(&mut rt, Key::Down);
            rt.update(|a, _| {
                assert_eq!(a.cursor_target(), before, "result navigation must not move insertion cursor");
                let (text, selected) = if let Some(f) = &a.finder { (&f.query.text, f.selected) } else { let p = a.picker.as_ref().unwrap(); (&p.query.text, p.selected) };
                assert_eq!(text, "?");
                assert_eq!(selected, 1);
            });
            if finder {
                rt.update(|a, _| {
                    a.finder_tick(std::time::Instant::now() + std::time::Duration::from_secs(1));
                    a.finder.as_mut().unwrap().loading = false;
                });
            }
            press(&mut rt, Key::Enter);
            rt.update(|a, _| {
                assert!(a.finder.is_none() && a.picker.is_none(), "Enter activates the selected option");
                if finder {
                    assert!(a.buffers.keys().any(|path| path.ends_with("b.rs")));
                    a.open_finder(finder::Mode::Files);
                } else { a.open_picker(Mode::Commands); }
            });
            settle(&mut rt);
            press(&mut rt, Key::Escape);
            rt.update(|a, _| assert!(a.finder.is_none() && a.picker.is_none()));
        }
    }

    #[test]
    fn compact_modal_layout_expands_and_collapses_in_both_themes() {
        for theme in ["pastel_dark", "neoism_light"] {
            for finder in [false, true] {
              for (width, height) in [(800.0, 600.0), (360.0, 300.0)] {
                let mut app = App::new();
                app.pal = crate::ui::theme::palette(theme);
                app.ty.size = 14.0;
                app.window = [width, height];
                if finder { app.open_finder(finder::Mode::Files); }
                else { app.open_picker(Mode::Commands); }
                let mut rt = Runtime::new(app, |a: &App, _: &mut Cx<App>| {
                    el().w_full().h_full().child(if a.finder.is_some() { finder::view(a) } else { view(a) })
                }, (width, height), Env::new(true, 1.0));
                let key = if finder { "finder" } else { "picker" };
                for _ in 0..120 { rt.frame(1.0 / 120.0); }
                let closed = rect(&rt, key);
                assert!(closed[3] <= 56.0, "single line: {closed:?}");
                rt.update(|a, tx| {
                    tx.animated();
                    if let Some(f) = &mut a.finder { f.query.replace_text("?"); }
                    if let Some(p) = &mut a.picker { p.query.replace_text("?"); }
                });
                rt.frame(1.0 / 120.0);
                let first = rect(&rt, key);
                for _ in 0..120 { rt.frame(1.0 / 120.0); }
                let open = rect(&rt, key);
                assert!(open[3] > closed[3] + 24.0, "expanded: {open:?}");
                if super::motion() != rune::Spring::NONE {
                    assert!(first[3] < open[3] - 1.0, "panel expansion must animate: {first:?} -> {open:?}");
                }
                assert_eq!(open[1], closed[1], "header stays anchored");
                assert!(open[0] >= 0.0 && open[0] + open[2] <= width + 0.1);
                assert!(open[1] + open[3] <= height + 0.1);
                rt.update(|a, tx| {
                    tx.animated();
                    if let Some(f) = &mut a.finder { f.query.replace_text(""); }
                    if let Some(p) = &mut a.picker { p.query.replace_text(""); }
                });
                for _ in 0..120 { rt.frame(1.0 / 120.0); }
                assert!((rect(&rt, key)[3] - closed[3]).abs() < 0.1);
              }
            }
        }
    }

    #[test]
    fn compact_query_contract() {
        assert!(!super::expanded(""));
        assert!(!super::expanded("  "));
        assert!(super::expanded("?"));
        assert_eq!(super::query("?"), "");
        assert_eq!(super::query("what?"), "what?");
        assert_eq!(super::query(" ? "), "?");
    }
}