//! Deep story paths: groups at any depth, a list that starts folded, folds kept across runs,
//! the filter's keys, and top-level groups in the order the app names.
use std::cell::RefCell;
use std::rc::Rc;

use rune::{App, AppSpec, Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, Playground, Runtime, Story, text};

#[derive(Default)]
struct Real;

fn real(_: &Real, _: &mut Cx<Real>) -> El {
    text("the app")
}

#[derive(Default)]
struct Card;

fn card(_: &Card, _: &mut Cx<Card>) -> El {
    text("card").key("card")
}

/// Stories 0 and 1 are scenes; the rest are stories, 2 onwards, declared out of list order.
fn playground() -> Playground<Real> {
    Playground::<Real>::new()
        .story(Story::new("Fixtures/Linux & Windows/General/Portal", Card::default, card))
        .story(Story::new("Settings panes/General/Microphone missing", Card::default, card))
        .story(Story::new("Settings panes/General/Portal", Card::default, card))
        .story(Story::new("Prototypes/HUD/Processing", Card::default, card))
        .story(Story::new("Prototypes/Onboarding/Option · Pill", Card::default, card))
        .story(Story::new("Toasts/Sync", Card::default, card))
}

const SCENES: &[(&str, fn(&mut Real))] = &[("Launch", |_| {}), ("Fixtures/Empty", |_| {})];

fn runtime(playground: Playground<Real>) -> Runtime<Real> {
    let spec = AppSpec { scenes: SCENES, ..AppSpec::new("deep", (900.0, 700.0), Real::default, real) };
    App::new(spec).playground(playground).headless_runtime((900.0, 700.0), Env::new(false, 1.0))
}

fn ordered() -> Playground<Real> {
    playground().group_order(&["Prototypes", "Settings panes", "Not a group", "Fixtures"])
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

fn element(rt: &Runtime<Real>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

fn shown(rt: &Runtime<Real>, key: &str) -> bool {
    element(rt, key).is_some()
}

/// The first text inside the element keyed `key`.
fn label(rt: &Runtime<Real>, key: &str) -> String {
    let all = rt.elements();
    let at = all.iter().position(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("no {key}"));
    let inside = |e: &ElementInfo| std::iter::successors(e.parent, |&p| all[p].parent).any(|p| p == at);
    all.iter().find(|e| e.text.is_some() && inside(e)).and_then(|e| e.text.clone()).expect("a label")
}

/// The open story's group path and name, as the side column titles it.
fn open(rt: &Runtime<Real>) -> String {
    let all = rt.elements();
    let at = all.iter().position(|e| e.key.as_deref() == Some("playground-title")).expect("a title");
    let parts: Vec<String> = all.iter().filter(|e| e.parent == Some(at)).filter_map(|e| e.text.clone()).collect();
    parts.join(" › ")
}

/// The list's rows top to bottom, by key: headers and stories.
fn rows(rt: &Runtime<Real>) -> Vec<String> {
    let mut rows: Vec<(f32, String)> = rt.elements().into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.key.filter(|k| k.starts_with("playground-item-") || k.starts_with("playground-group-")).map(|k| (e.rect[1], k)))
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, k)| k.replace("playground-", "")).collect()
}

fn focused(rt: &Runtime<Real>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn tap(rt: &mut Runtime<Real>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn reopen(rt: &mut Runtime<Real>) {
    rt.update(|_, tx| tx.close_playground());
    rt.frame(1.0 / 60.0);
    rt.update(|_, tx| tx.open_playground());
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_path_nests_as_deep_as_it_goes_and_every_lookup_finds_it() {
    let mut rt = runtime(playground());
    rt.open_playground("Fixtures/Linux & Windows/General/Portal", None);
    assert_eq!(open(&rt), "Fixtures › Linux & Windows › General › Portal");
    let deep = ["group-1", "item-1", "group-2-1", "group-2-2", "item-2"];
    assert!(deep.iter().all(|k| rows(&rt).contains(&k.to_string())), "a header at each level: {:?}", rows(&rt));
    assert_eq!([label(&rt, "playground-group-2-1"), label(&rt, "playground-group-2-2")], ["Linux & Windows", "General"]);
    rt.open_playground("prototypes-onboarding-option-pill", None);
    assert_eq!(open(&rt), "Prototypes › Onboarding › Option · Pill", "by slug");
    rt.open_playground("Microphone missing", None);
    assert_eq!(open(&rt), "Settings panes › General › Microphone missing", "by name");
    rt.open_playground("Launch", None);
    assert_eq!(open(&rt), "Scenes › Launch", "a one-level path as before");
    rt.open_playground("toasts-sync", None);
    assert_eq!(open(&rt), "Toasts › Sync");
}

#[test]
fn group_order_lists_the_named_groups_first_scenes_and_stories_alike() {
    let mut rt = runtime(ordered());
    rt.update(|_, tx| tx.open_playground());
    rt.frame(0.0);
    assert_eq!(open(&rt), "Prototypes › HUD › Processing", "the first story in the list opens first");
    let order = ["group-5", "group-5-1", "item-5", "group-6-1", "item-6", "group-3", "group-3-1", "item-3", "item-4", "group-1", "item-1", "group-2-1", "group-2-2", "item-2", "group-0", "item-0", "group-7", "item-7"];
    assert_eq!(rows(&rt), order, "named groups first, in their order; then the rest as they first appear");
    rt.update(|_, tx| tx.open_playground_at("Portal", None));
    rt.frame(0.0);
    assert_eq!(open(&rt), "Settings panes › General › Portal", "a name two stories have finds the first in the list");
    rt.update(|_, tx| tx.open_playground_at("Processing", None));
    rt.frame(0.0);
    typed(&mut rt, "j");
    assert_eq!(open(&rt), "Prototypes › Onboarding › Option · Pill");
    typed(&mut rt, "j");
    assert_eq!(open(&rt), "Settings panes › General › Microphone missing", "J goes down the list, past its headers");
    press(&mut rt, Key::Down);
    assert_eq!(open(&rt), "Settings panes › General › Portal", "and so does ↓");
    typed(&mut rt, "/");
    typed(&mut rt, "general");
    press(&mut rt, Key::Down);
    assert_eq!(open(&rt), "Settings panes › General › Microphone missing", "the filter's matches are in list order too");
}

#[test]
fn without_group_order_scenes_come_first_as_before() {
    let mut rt = runtime(playground());
    rt.update(|_, tx| tx.open_playground());
    rt.frame(0.0);
    assert_eq!(open(&rt), "Scenes › Launch");
    assert_eq!(rows(&rt)[..4], ["group-0", "item-0", "group-1", "item-1"]);
}

#[test]
fn start_folded_folds_everything_but_the_open_storys_groups() {
    let mut rt = runtime(ordered().start_folded(true));
    // As a link at launch delivers it: an update before the first frame.
    rt.update(|_, tx| tx.open_playground_at("prototypes-onboarding-option-pill", None));
    rt.frame(0.0);
    assert_eq!(rows(&rt), ["group-5", "group-5-1", "group-6-1", "item-6", "group-3", "group-1", "group-0", "group-7"]);
    tap(&mut rt, "playground-group-3");
    assert!(shown(&rt, "playground-group-3-1") && !shown(&rt, "playground-item-3"), "a group unfolds one level, the ones inside it still folded");
    reopen(&mut rt);
    assert!(shown(&rt, "playground-group-3-1"), "and stays unfolded while the app runs");
    assert!(shown(&rt, "playground-item-6"), "the open story's groups unfold as it opens");
}

#[test]
fn start_folded_saves_the_unfolded_groups_and_loads_them_next_run() {
    let saved: Rc<RefCell<Vec<String>>> = Rc::default();
    let saves = saved.clone();
    let mut rt = runtime(ordered().start_folded(true).folds(Vec::new, move |groups| *saves.borrow_mut() = groups.to_vec()));
    rt.open_playground("Processing", None);
    tap(&mut rt, "playground-group-1");
    tap(&mut rt, "playground-group-2-1");
    assert_eq!(*saved.borrow(), ["Prototypes", "Prototypes/HUD", "Fixtures", "Fixtures/Linux & Windows"], "full paths of the groups left open");
    let names = saved.borrow().clone();
    let mut next = runtime(ordered().start_folded(true).folds(move || names.clone(), |_| {}));
    next.update(|_, tx| tx.open_playground_at("Toasts/Sync", None));
    next.frame(0.0);
    assert!(shown(&next, "playground-group-2-2") && !shown(&next, "playground-item-2"), "next run, the same groups open");
    assert!(shown(&next, "playground-item-5") && !shown(&next, "playground-group-3-1"), "and the rest folded");
    assert!(shown(&next, "playground-item-7"), "with the open story's");
}

#[test]
fn folds_save_full_paths_and_load_old_one_level_names() {
    let saved: Rc<RefCell<Vec<String>>> = Rc::default();
    let saves = saved.clone();
    let load = || vec!["Toasts".to_owned(), "Fixtures/Linux & Windows/General".to_owned(), "Gone".to_owned()];
    let mut rt = runtime(ordered().folds(load, move |groups| *saves.borrow_mut() = groups.to_vec()));
    rt.open_playground("Processing", None);
    assert!(!shown(&rt, "playground-item-7") && !shown(&rt, "playground-item-2") && shown(&rt, "playground-group-2-2"), "an old name and a deep path both load");
    tap(&mut rt, "playground-group-6-1");
    assert_eq!(*saved.borrow(), ["Fixtures/Linux & Windows/General", "Toasts", "Prototypes/Onboarding"]);
}

#[test]
fn the_filter_edits_like_a_field_and_down_goes_into_its_matches() {
    let mut rt = runtime(ordered());
    rt.open_playground("Toasts/Sync", None);
    tap(&mut rt, "playground-filter");
    assert_eq!(focused(&rt).as_deref(), Some("playground-filter"), "a click focuses it");
    typed(&mut rt, "hud");
    press_with(&mut rt, Key::Command('a'), Mods { cmd: true, ..Mods::default() });
    typed(&mut rt, "portal");
    assert_eq!(rows(&rt), ["group-3", "group-3-1", "item-4", "group-1", "group-2-1", "group-2-2", "item-2"], "⌘A selects what's typed, which typing replaces");
    press(&mut rt, Key::Down);
    assert_eq!((open(&rt), focused(&rt)), ("Settings panes › General › Portal".to_owned(), None), "↓ goes to the first match, and the list has the keyboard");
    typed(&mut rt, "j");
    assert_eq!(open(&rt), "Fixtures › Linux & Windows › General › Portal");
    typed(&mut rt, "k");
    press(&mut rt, Key::Up);
    assert_eq!(focused(&rt).as_deref(), Some("playground-filter"), "↑ from the first match goes back up to the field");
    press(&mut rt, Key::Escape);
    assert_eq!(focused(&rt).as_deref(), Some("playground-filter"), "Esc empties it first");
    assert!(shown(&rt, "playground-item-5"));
    press(&mut rt, Key::Escape);
    assert_eq!(focused(&rt), None, "then leaves");
    assert!(rt.playground_state::<Card>().is_some(), "with the playground still open");
}
