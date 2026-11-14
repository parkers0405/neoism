//! The story list as a tree, nested by each story's path, and the keyboard in one column at a
//! time: the list, the stage or the side column.
use std::cell::RefCell;
use std::rc::Rc;

use rune::{App, AppSpec, Cx, El, ElementInfo, Env, Key, KeyEvent, Mods, Playground, Runtime, Story, Tx, text};

#[derive(Default)]
struct Real;

fn real(_: &Real, _: &mut Cx<Real>) -> El {
    text("the app")
}

#[derive(Default)]
struct Card {
    variant: u8,
    knob: String,
}

fn card(c: &Card, _: &mut Cx<Card>) -> El {
    text(format!("card {}", c.variant)).key("card").on_press(|_: &mut Card| {})
}

/// Digits turn the knob; it leaves everything else.
fn knob_key(c: &mut Card, ev: &KeyEvent, _: &mut Tx) -> bool {
    match &ev.key {
        Key::Text(t) if t.chars().all(|c| c.is_ascii_digit()) => c.knob.push_str(t),
        _ => return false,
    }
    true
}

fn playground() -> Playground<Real> {
    Playground::<Real>::new()
        .story(Story::new("Sidebar/Rows/Plain", Card::default, card).variant("One", |c| c.variant = 1).variant("Two", |c| c.variant = 2))
        .story(Story::new("Sidebar/Rows/Pinned", Card::default, card))
        .story(Story::new("Sidebar/Filter buttons/Compact", Card::default, card))
        .story(Story::new("Sidebar/Filter buttons/Wide", Card::default, card))
        .story(Story::new("Sidebar/Header", Card::default, card))
        .story(Story::new("Dock/Icons", Card::default, card).side_keys(|c, active, _| text(format!("{} {}", if active { "on" } else { "off" }, c.knob)).key("knob"), knob_key))
}

fn runtime(playground: Playground<Real>, open: &str) -> Runtime<Real> {
    let mut rt = App::new(AppSpec::new("tree", (900.0, 600.0), Real::default, real)).playground(playground).headless_runtime((900.0, 600.0), Env::new(false, 1.0));
    rt.open_playground(open, None);
    rt
}

fn press(rt: &mut Runtime<Real>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
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

fn says(rt: &Runtime<Real>, key: &str) -> String {
    element(rt, key).and_then(|e| e.text).unwrap_or_else(|| panic!("no {key}")).replace('\u{a0}', " ")
}

/// The first text inside the element keyed `key`, and where it starts.
fn label(rt: &Runtime<Real>, key: &str) -> (String, f32) {
    let all = rt.elements();
    let at = all.iter().position(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("no {key}"));
    let inside = |e: &ElementInfo| std::iter::successors(e.parent, |&p| all[p].parent).any(|p| p == at);
    let e = all.iter().find(|e| e.text.is_some() && inside(e)).expect("a label");
    (e.text.clone().unwrap(), e.rect[0])
}

/// The open story's name, as the side column titles it.
fn title(rt: &Runtime<Real>) -> String {
    let all = rt.elements();
    let at = all.iter().position(|e| e.key.as_deref() == Some("playground-title")).expect("a title");
    all.iter().filter(|e| e.parent == Some(at)).filter_map(|e| e.text.clone()).nth(1).unwrap_or_default()
}

fn tap(rt: &mut Runtime<Real>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
    tap_at(rt, x, y);
}

fn tap_at(rt: &mut Runtime<Real>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn focused(rt: &Runtime<Real>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn card_state(rt: &Runtime<Real>) -> (u8, String) {
    rt.playground_state::<Card>().map(|c| (c.variant, c.knob.clone())).expect("a card story")
}

const LIST_HINT: &str = "↑↓ stories · ←→ groups · / filter · Tab controls · Esc back";

#[test]
fn a_path_nests_groups_one_level_per_slash() {
    let rt = runtime(playground(), "Sidebar/Filter buttons/Compact");
    let order = ["playground-group-0", "playground-group-0-1", "playground-item-0", "playground-item-1", "playground-group-2-1", "playground-item-2", "playground-item-3", "playground-item-4", "playground-group-5", "playground-item-5"];
    let ys: Vec<f32> = order.iter().map(|k| element(&rt, k).unwrap_or_else(|| panic!("no {k}")).rect[1]).collect();
    assert!(ys.windows(2).all(|w| w[0] < w[1]), "rows in tree order: {ys:?}");
    assert_eq!(label(&rt, "playground-group-2-1").0, "Filter buttons", "a group inside another shows its own name");
    let (sidebar, rows, compact, header) = (label(&rt, "playground-group-0").1, label(&rt, "playground-group-0-1").1, label(&rt, "playground-item-2").1, label(&rt, "playground-item-4").1);
    assert_eq!(header, sidebar, "a story lines up with its group's name");
    assert_eq!((rows - sidebar, compact - header), (12.0, 12.0), "each level indents 12 pt");
    assert_eq!([says(&rt, "playground-count-0"), says(&rt, "playground-count-0-1"), says(&rt, "playground-count-2-1"), says(&rt, "playground-count-5")], ["5", "2", "2", "1"], "a header counts every story inside it");
    assert_eq!(label(&rt, "playground-title").0, "Sidebar › Filter buttons");
    assert_eq!(title(&rt), "Compact");
}

#[test]
fn every_level_folds_and_a_fold_inside_one_outlasts_its_parents() {
    let mut rt = runtime(playground(), "Dock/Icons");
    tap(&mut rt, "playground-group-2-1");
    assert!(!shown(&rt, "playground-item-2") && !shown(&rt, "playground-item-3"), "Filter buttons folds");
    assert!(shown(&rt, "playground-item-0") && shown(&rt, "playground-item-4"), "the rest of Sidebar doesn't");
    tap(&mut rt, "playground-group-0");
    assert!(!shown(&rt, "playground-group-0-1") && !shown(&rt, "playground-group-2-1") && !shown(&rt, "playground-item-4"), "Sidebar folds what's inside it");
    assert!(shown(&rt, "playground-item-5"), "Dock stays");
    tap(&mut rt, "playground-group-0");
    assert!(shown(&rt, "playground-item-0") && shown(&rt, "playground-group-2-1"));
    assert!(!shown(&rt, "playground-item-2"), "Filter buttons is still folded");
}

#[test]
fn folds_keep_groups_at_any_level_by_their_path() {
    let saved: Rc<RefCell<Vec<Vec<String>>>> = Rc::default();
    let saves = saved.clone();
    let playground = playground().folds(|| vec!["Sidebar/Filter buttons".to_owned(), "Dock".to_owned()], move |groups| saves.borrow_mut().push(groups.to_vec()));
    let mut rt = runtime(playground, "Sidebar/Header");
    assert!(!shown(&rt, "playground-item-2") && shown(&rt, "playground-group-2-1"), "a nested group loads folded");
    assert!(!shown(&rt, "playground-item-5"), "and a top one, by name as before");
    tap(&mut rt, "playground-group-0-1");
    assert_eq!(saved.borrow().last().cloned(), Some(vec!["Sidebar/Filter buttons".to_owned(), "Dock".to_owned(), "Sidebar/Rows".to_owned()]));
    rt.open_playground("Sidebar/Filter buttons/Wide", None);
    assert!(shown(&rt, "playground-item-3"), "opening a story unfolds the groups around it");
    assert_eq!(saved.borrow().last().cloned(), Some(vec!["Dock".to_owned(), "Sidebar/Rows".to_owned()]));
}

#[test]
fn arrows_walk_into_and_out_of_nested_groups() {
    let mut rt = runtime(playground(), "Sidebar/Filter buttons/Compact");
    press(&mut rt, Key::Left);
    press(&mut rt, Key::Left);
    assert!(!shown(&rt, "playground-item-2"), "← goes to the story's group, then folds it");
    press(&mut rt, Key::Left);
    press(&mut rt, Key::Left);
    assert!(!shown(&rt, "playground-group-0-1"), "then up to the group around it, and folds that");
    press(&mut rt, Key::Left);
    assert!(!shown(&rt, "playground-group-0-1"), "at the top, ← stays");
    press(&mut rt, Key::Right);
    assert!(shown(&rt, "playground-group-0-1") && !shown(&rt, "playground-item-2"), "→ unfolds one level");
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Down);
    assert_eq!(title(&rt), "Plain", "→ steps into the group: Rows, then its first story");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Right);
    assert!(shown(&rt, "playground-item-2"), "→ on a folded header unfolds it");
    press(&mut rt, Key::Down);
    assert_eq!(title(&rt), "Compact");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Down);
    assert_eq!(title(&rt), "Header", "down past a subgroup to the stories beside it");
}

#[test]
fn the_filter_keeps_matches_and_the_groups_above_them() {
    let mut rt = runtime(playground(), "Sidebar/Rows/Plain");
    typed(&mut rt, "/");
    assert_eq!(focused(&rt).as_deref(), Some("playground-filter"), "/ focuses the filter");
    typed(&mut rt, "filter wi");
    assert!(shown(&rt, "playground-group-0") && shown(&rt, "playground-group-2-1") && shown(&rt, "playground-item-3"));
    assert!(!shown(&rt, "playground-item-0") && !shown(&rt, "playground-item-2") && !shown(&rt, "playground-group-5"), "the rest goes");
    assert_eq!(says(&rt, "playground-count-0"), "1", "counts count matches");
    press(&mut rt, Key::Enter);
    assert_eq!((title(&rt), focused(&rt)), ("Wide".to_owned(), None), "Return opens the first match and gives the list the keyboard");
    typed(&mut rt, "/");
    press(&mut rt, Key::Escape);
    assert!(shown(&rt, "playground-item-0"), "Esc empties it");
}

#[test]
fn the_keyboard_moves_from_the_list_to_the_side_column_and_back() {
    let mut rt = runtime(playground(), "Sidebar/Rows/Plain");
    assert_eq!(says(&rt, "playground-hint"), LIST_HINT);
    typed(&mut rt, "2");
    assert_eq!(card_state(&rt).0, 1, "from the list, a digit isn't a variant");
    press(&mut rt, Key::Tab);
    assert_eq!(says(&rt, "playground-hint"), "↑↓ controls · 1–2 variants · Esc stories");
    assert_eq!(focused(&rt).as_deref(), Some("playground-variant-0"));
    typed(&mut rt, "2");
    assert_eq!(card_state(&rt).0, 2, "in the side column it is");
    press(&mut rt, Key::Escape);
    assert_eq!((says(&rt, "playground-hint"), focused(&rt)), (LIST_HINT.to_owned(), None), "Esc goes back to the list");
    press(&mut rt, Key::Down);
    assert_eq!(title(&rt), "Pinned", "whose arrows work again");
    press(&mut rt, Key::Right);
    assert!(says(&rt, "playground-hint").starts_with("↑↓ controls"), "→ from a story goes over too");
}

#[test]
fn side_keys_hear_only_the_side_column() {
    let mut rt = runtime(playground(), "Dock/Icons");
    typed(&mut rt, "3");
    assert_eq!(card_state(&rt).1, "", "the list has the keyboard, so the knob hears nothing");
    press(&mut rt, Key::Right);
    typed(&mut rt, "3");
    assert_eq!(says(&rt, "knob"), "on 3");
    press(&mut rt, Key::Escape);
    typed(&mut rt, "4");
    assert_eq!(says(&rt, "knob"), "off 3");
}

#[test]
fn a_click_gives_its_column_the_keyboard() {
    let mut rt = runtime(playground(), "Sidebar/Rows/Plain");
    tap(&mut rt, "card");
    assert_eq!(says(&rt, "playground-hint"), "Keys go to the story · Esc stories");
    assert!(shown(&rt, "playground-stage-ring"), "a ring shows the stage has it");
    press(&mut rt, Key::Down);
    typed(&mut rt, "j");
    assert_eq!(title(&rt), "Plain", "the list's keys aren't the stage's");
    let side = element(&rt, "playground-side").unwrap().rect;
    tap_at(&mut rt, side[0] + side[2] * 0.5, side[1] + side[3] * 0.6);
    assert!(says(&rt, "playground-hint").starts_with("↑↓ controls"), "a click on the side column's background takes it there");
    assert!(!shown(&rt, "playground-stage-ring"));
    typed(&mut rt, "2");
    assert_eq!(card_state(&rt).0, 2);
    tap(&mut rt, "playground-item-1");
    assert_eq!((says(&rt, "playground-hint"), title(&rt)), (LIST_HINT.to_owned(), "Pinned".to_owned()));
    press(&mut rt, Key::Escape);
    assert!(rt.playground_state::<Card>().is_none(), "Esc from the list goes back to the app");
}
