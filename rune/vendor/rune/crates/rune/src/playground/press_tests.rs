//! Steps that press keys with [`Play::press`].
use super::*;
use crate::motion_gate::Run;
use crate::{App, AppSpec, Env, Mods, Runtime};

const SIZE: (f32, f32) = (1200.0, 700.0);

/// A list whose selection ↓ moves, scrolling the new row in.
#[derive(Default)]
struct List {
    selected: usize,
    keys: Vec<(Key, Mods)>,
}

static SCENES: &[(&str, fn(&mut List))] = &[("Keys/Pressed", |_| {}), ("Keys/Stepped", |_| {})];

fn list(s: &List, _: &mut Cx<List>) -> El {
    let rows = (0..30).map(|i| el().key(format!("row{i}")).w_full().h(40.0).fill(if i == s.selected { Color::WHITE } else { Color::BLACK }));
    col().w_full().h_full().child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(col().key("rows").w_full().children(rows)))
}

/// Selects the next row and puts it at the list's bottom: from row 4, a 40 pt scroll that
/// brings row 5 into view.
fn down(s: &mut List, tx: &mut Tx) {
    s.selected += 1;
    tx.scroll_key_to(format!("row{}", s.selected), 160.0);
}

fn runtime(scene: &str) -> Runtime<List> {
    let mut spec = AppSpec::new("press", SIZE, || List { selected: 4, ..List::default() }, list);
    spec.scenes = SCENES;
    spec.on_key = Some(|s, ev, tx| {
        s.keys.push((ev.key.clone(), ev.mods));
        if ev.key != Key::Down { return false; }
        down(s, tx);
        true
    });
    let playground = Playground::new()
        .enabled(true)
        .trigger("Keys/Pressed", 'r', "Press ↓", |_, play| play.after(0.5, |_, play| play.press(Key::Down)))
        .trigger("Keys/Pressed", 's', "Press ⇧↓", |_, play| play.press(KeyEvent { key: Key::Down, mods: Mods { shift: true, ..Mods::default() } }))
        .trigger("Keys/Stepped", 'r', "Step down", |_, play| play.after(0.5, |s, play| down(s, play)));
    let mut rt = App::new(spec).playground(playground).headless_runtime(SIZE, Env::new(false, 1.0));
    rt.frame(0.0);
    rt.open_playground(scene, None);
    rt
}

fn focused(rt: &Runtime<List>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn stage(rt: &Runtime<List>) -> &List {
    rt.playground_state().expect("the scene's state")
}

#[test]
fn a_step_presses_a_key_the_stage_hears_as_input() {
    let mut r = Run::new(runtime("Keys/Pressed"), 120.0);
    let story = r.rt.playground.as_ref().unwrap().selected;
    // The list's filter has the keyboard, so a person's ↓ would go to the list.
    r.rt.key(Key::Text("/".into()).into());
    r.step();
    assert_eq!(focused(&r.rt).as_deref(), Some(FILTER));
    while stage(&r.rt).keys.is_empty() {
        assert!(r.rec.frames < 120, "the step never pressed");
        r.step();
    }
    assert_eq!(stage(&r.rt).keys, [(Key::Down, Mods::default())], "the scene's on_key hears it");
    assert_eq!(stage(&r.rt).selected, 5);
    let frame = r.rec.frames - 1;
    assert_eq!(r.rec.inputs.last().map(|i| i.0), Some(frame), "the frame it lands on is input");
    assert_eq!(focused(&r.rt).as_deref(), Some(FILTER), "the filter keeps the keyboard");
    let p = r.rt.playground.as_ref().unwrap();
    assert_eq!((p.selected, p.controls.filter.text.as_str()), (story, ""), "neither the list nor its filter took it");
    r.rt.playground_trigger('s');
    assert_eq!(stage(&r.rt).keys[1], (Key::Down, Mods { shift: true, ..Mods::default() }), "with its modifiers, on the step's own frame");
}

/// The jumps the gate finds in the list when `scene`'s replay moves its selection.
fn jumps(scene: &str) -> Vec<String> {
    let mut r = Run::new(runtime(scene), 120.0);
    r.idle(0.25);
    r.step();
    r.play(1.0);
    assert_eq!(stage(&r.rt).selected, 5, "the replay ran");
    r.finish().into_iter().filter(|f| ["jump", "offset-jump", "jerk"].contains(&f.kind) && f.element.contains("row")).map(|f| f.to_string()).collect()
}

#[test]
fn a_scroll_a_pressed_key_asks_for_isnt_reported_as_input_less() {
    let stepped = jumps("Keys/Stepped");
    assert!(stepped.iter().any(|f| f.starts_with("offset-jump") && f.contains("list/rows")), "a step that scrolls on its own teleports the rows: {stepped:#?}");
    let pressed = jumps("Keys/Pressed");
    assert!(pressed.is_empty(), "{pressed:#?}");
}

#[derive(Default)]
struct Long;

fn long(_: &Long, _: &mut Cx<Long>) -> El {
    let rows = (0..40).map(|i| el().key(format!("line{i}")).w_full().h(40.0));
    col().w_full().h_full().child(col().key("page").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: false }).child(col().w_full().children(rows)))
}

/// How far the scroll view keyed `key` has carried its content, from its first child.
fn scrolled(rt: &Runtime<List>, key: &str) -> f32 {
    let all = rt.elements();
    let at = all.iter().position(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"));
    all.iter().find(|e| e.parent == Some(at)).expect("its content").scrolled[1]
}

#[test]
fn a_pressed_page_key_pages_the_stage_not_the_story_list() {
    let story = Story::new("Pages/Long", Long::default, long).trigger('r', "Page down", |_, play| play.after(0.1, |_, play| play.press(Key::PageDown)));
    // Enough stories that the list overflows too, and comes first in the tree.
    let playground = (0..60).fold(Playground::new().enabled(true), |p, i| p.story(Story::new(Box::leak(format!("Filler/Story {i}").into_boxed_str()), Long::default, long))).story(story);
    let mut rt = App::new(AppSpec::new("press", SIZE, List::default, list)).playground(playground).headless_runtime(SIZE, Env::new(false, 1.0));
    rt.frame(0.0);
    rt.open_playground("Pages/Long", None);
    let list_at = scrolled(&rt, "playground-list");
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert_eq!(scrolled(&rt, "page"), 270.0, "a page is 90% of the stage's view");
    assert_eq!(scrolled(&rt, "playground-list"), list_at, "the story list stays put");
}
