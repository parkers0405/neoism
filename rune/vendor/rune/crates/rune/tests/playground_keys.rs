//! Keys the playground's list doesn't use reach the open story: its `AppSpec::on_key` hears ⌘K
//! and digits while the list has the keyboard, and the list keeps its arrows, J K, Esc and the
//! filter.
use rune::{App, AppSpec, Cx, El, Env, Key, KeyEvent, Mods, Playground, Runtime, text};

#[derive(Default)]
struct Real {
    scene: &'static str,
    palette: bool,
    typed: String,
}

fn real(s: &Real, _: &mut Cx<Real>) -> El {
    text(format!("{} {} {}", s.scene, s.palette, s.typed)).key("real")
}

/// ⌘K opens a palette, Esc closes it, and every typed key is noted.
fn on_key(s: &mut Real, ev: &KeyEvent, _: &mut rune::Tx) -> bool {
    match &ev.key {
        Key::Command('k') if ev.mods.cmd => s.palette = !s.palette,
        Key::Escape if s.palette => s.palette = false,
        Key::Text(t) => s.typed.push_str(t),
        _ => return false,
    }
    true
}

const SCENES: &[(&str, fn(&mut Real))] = &[("Launch", |s| s.scene = "launch"), ("Palette/Search", |s| s.scene = "search")];

fn runtime() -> Runtime<Real> {
    let spec = AppSpec { scenes: SCENES, on_key: Some(on_key), ..AppSpec::new("keys", (900.0, 600.0), Real::default, real) };
    let mut rt = App::new(spec).playground(Playground::<Real>::new()).headless_runtime((900.0, 600.0), Env::new(false, 1.0));
    rt.open_playground("Launch", None);
    rt
}

fn press_with(rt: &mut Runtime<Real>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

fn press(rt: &mut Runtime<Real>, key: Key) {
    press_with(rt, key, Mods::default());
}

fn typed(rt: &mut Runtime<Real>, s: &str) {
    press(rt, Key::Text(s.into()));
}

fn command_k(rt: &mut Runtime<Real>) {
    press_with(rt, Key::Command('k'), Mods { cmd: true, ..Mods::default() });
}

fn stage(rt: &Runtime<Real>) -> (&'static str, bool, String) {
    let s = rt.playground_state::<Real>().expect("a scene on the stage");
    (s.scene, s.palette, s.typed.clone())
}

fn shown(rt: &Runtime<Real>, key: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

fn hint(rt: &Runtime<Real>) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("playground-hint")).and_then(|e| e.text).unwrap_or_default().replace('\u{a0}', " ")
}

#[test]
fn a_story_opened_from_the_list_hears_the_keys_the_list_leaves() {
    let mut rt = runtime();
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Down);
    assert_eq!(stage(&rt).0, "search", "↓ opens the next story, the keyboard still on the list");
    assert!(hint(&rt).starts_with("↑↓ stories"));
    command_k(&mut rt);
    assert!(stage(&rt).1, "⌘K reaches the story's on_key");
    typed(&mut rt, "3");
    assert_eq!(stage(&rt).2, "3", "and so does a digit");
    assert!(!rt.state.palette && rt.state.typed.is_empty(), "the stage's state, not the app's");
    press(&mut rt, Key::Escape);
    assert_eq!(stage(&rt).1, false, "Esc goes to the story first");
    typed(&mut rt, "k");
    assert_eq!(stage(&rt), ("launch", false, String::new()), "K is the list's: it moves to the story above");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Up);
    press(&mut rt, Key::Up);
    assert_eq!(stage(&rt), ("launch", false, String::new()), "so are the arrows, and none reach the story");
    press(&mut rt, Key::Tab);
    command_k(&mut rt);
    assert!(stage(&rt).1, "from the side column too");
    press(&mut rt, Key::Escape);
    assert!(stage(&rt).1 && hint(&rt).starts_with("↑↓ stories"), "Esc there goes back to the list");
    press(&mut rt, Key::Escape);
    assert!(!stage(&rt).1, "where Esc closes the story's palette");
    press(&mut rt, Key::Escape);
    assert!(rt.playground_state::<Real>().is_none(), "then leaves");
}

#[test]
fn typing_in_the_filter_filters() {
    let mut rt = runtime();
    typed(&mut rt, "/");
    typed(&mut rt, "sea");
    typed(&mut rt, "2");
    assert!(!shown(&rt, "playground-item-0") && !shown(&rt, "playground-item-1"), "nothing matches \"sea2\"");
    assert_eq!(stage(&rt), ("launch", false, String::new()), "the field keeps what's typed");
    press(&mut rt, Key::Backspace);
    assert!(shown(&rt, "playground-item-1") && !shown(&rt, "playground-item-0"));
    command_k(&mut rt);
    assert!(stage(&rt).1, "a chord the field doesn't use reaches the story");
    press(&mut rt, Key::Enter);
    assert_eq!(stage(&rt).0, "search", "Return opens the first match");
}
