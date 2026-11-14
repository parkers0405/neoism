//! A story's side controls that take keys (`Story::side_keys`, `Playground::side_keys`): the
//! keyboard goes onto them from the list, their hook gets keys first, and Esc or Tab leave.
use rune::{App, AppSpec, Cx, El, Env, Key, KeyEvent, Mods, Playground, Runtime, Story, Tx, text};

#[derive(Default)]
struct Real {
    level: i32,
}

fn real(s: &Real, _: &mut Cx<Real>) -> El {
    text(format!("level {}", s.level)).key("level")
}

static SCENES: &[(&str, fn(&mut Real))] = &[("Meter/High", |s| s.level = 50)];

#[derive(Default)]
struct Dial {
    value: i32,
    failed: bool,
}

fn dial(d: &Dial, _: &mut Cx<Dial>) -> El {
    text(format!("dial {}", d.value)).key("dial")
}

/// ↑ and ↓ turn the dial; everything else it leaves.
fn dial_key(d: &mut Dial, ev: &KeyEvent, _: &mut Tx) -> bool {
    match ev.key {
        Key::Up => d.value += 1,
        Key::Down => d.value -= 1,
        _ => return false,
    }
    true
}

fn runtime() -> Runtime<Real> {
    let playground = Playground::<Real>::new()
        .story(
            Story::new("Dials/Dial", Dial::default, dial)
                .side_keys(|d, active, _| text(format!("{} {}", if active { "active" } else { "idle" }, d.value)).key("dial-side"), dial_key)
                .trigger('f', "Fail", |d, _| d.failed = true),
        )
        .story(Story::new("Dials/Plain", Dial::default, dial).side(|d, _| text(format!("plain {}", d.value)).key("plain-side")))
        .side_keys(
            "Meter/High",
            |s, active, _| text(format!("{} {}", if active { "active" } else { "idle" }, s.level)).key("meter-side"),
            |s, ev, _| (ev.key == Key::Up).then(|| s.level += 5).is_some(),
        );
    let mut spec = AppSpec::new("dials", (800.0, 500.0), Real::default, real);
    spec.scenes = SCENES;
    App::new(spec).playground(playground).headless_runtime((800.0, 500.0), Env::new(false, 1.0))
}

fn press(rt: &mut Runtime<Real>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

/// What the side column's element keyed `key` says.
fn says(rt: &Runtime<Real>, key: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text).unwrap_or_default()
}

fn dial_state(rt: &Runtime<Real>) -> (i32, bool) {
    rt.playground_state::<Dial>().map(|d| (d.value, d.failed)).expect("a dial story's state")
}

#[test]
fn the_side_view_knows_when_the_keyboard_is_on_it() {
    let mut rt = runtime();
    rt.open_playground("Dials/Dial", None);
    assert_eq!(says(&rt, "dial-side"), "idle 0");
    press(&mut rt, Key::Right);
    assert_eq!(says(&rt, "dial-side"), "active 0", "→ puts the keyboard on it");
    press(&mut rt, Key::Escape);
    assert_eq!(says(&rt, "dial-side"), "idle 0", "Esc leaves");
    assert!(rt.playground_state::<Dial>().is_some(), "and the playground stays open");
    press(&mut rt, Key::Tab);
    assert_eq!(says(&rt, "dial-side"), "active 0", "so does Tab");
    press(&mut rt, Key::Tab);
    assert_eq!(says(&rt, "dial-side"), "idle 0", "and Tab leaves again");
}

#[test]
fn its_hook_gets_keys_first_and_left_no_longer_leaves() {
    let mut rt = runtime();
    rt.open_playground("dials-dial", None);
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Up);
    press(&mut rt, Key::Up);
    press(&mut rt, Key::Down);
    assert_eq!(dial_state(&rt), (1, false), "↑↓ are the hook's, not the side column's");
    press(&mut rt, Key::Left);
    assert_eq!(says(&rt, "dial-side"), "active 1", "← stays");
    press(&mut rt, Key::Text("f".into()));
    // The step it queued runs on the frame after.
    rt.frame(1.0 / 60.0);
    assert_eq!(dial_state(&rt), (1, true), "a key it leaves runs the trigger it's on");
}

#[test]
fn a_side_without_keys_keeps_the_old_keys() {
    let mut rt = runtime();
    rt.open_playground("Dials/Plain", None);
    press(&mut rt, Key::Right);
    let focused = |rt: &Runtime<Real>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    assert!(focused(&rt).is_some_and(|k| k.starts_with("playground-trigger")), "→ goes to its first control");
    press(&mut rt, Key::Left);
    assert_eq!(focused(&rt), None, "← leaves");
}

#[test]
fn a_scene_takes_keys_on_its_stage_state() {
    let mut rt = runtime();
    rt.open_playground("meter-high", None);
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Up);
    assert_eq!(says(&rt, "meter-side"), "active 55");
    assert_eq!(rt.state.level, 0, "the real state is untouched");
}
