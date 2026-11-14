//! Folded groups: kept while the app runs, and through `Playground::folds` from one run to the
//! next.
use std::cell::RefCell;
use std::rc::Rc;

use rune::{App, AppSpec, Cx, El, Env, Playground, Runtime, Story, text};

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

fn playground() -> Playground<Real> {
    Playground::<Real>::new()
        .story(Story::new("Cards/Plain", Card::default, card))
        .story(Story::new("Cards/Raised", Card::default, card))
        .story(Story::new("Lists/Short", Card::default, card))
}

fn runtime(playground: Playground<Real>) -> Runtime<Real> {
    App::new(AppSpec::new("folds", (800.0, 500.0), Real::default, real)).playground(playground).headless_runtime((800.0, 500.0), Env::new(false, 1.0))
}

/// Whether the list shows a row keyed `key`.
fn shown(rt: &Runtime<Real>, key: &str) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some(key) && !e.exiting)
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
fn a_folded_group_stays_folded_after_the_playground_closes() {
    let mut rt = runtime(playground());
    rt.open_playground("Lists/Short", None);
    assert!(shown(&rt, "playground-item-1"));
    tap(&mut rt, "playground-group-0");
    assert!(!shown(&rt, "playground-item-1"), "Cards folded");
    reopen(&mut rt);
    assert!(!shown(&rt, "playground-item-1"), "and still folded, reopened");
    assert!(shown(&rt, "playground-item-2"), "Lists isn't");
}

#[test]
fn folds_loads_once_and_saves_each_change() {
    let saved: Rc<RefCell<Vec<Vec<String>>>> = Rc::default();
    let loads = Rc::new(RefCell::new(0));
    let (saves, count) = (saved.clone(), loads.clone());
    let playground = playground().folds(
        move || {
            *count.borrow_mut() += 1;
            vec!["Cards".to_owned(), "Gone".to_owned()]
        },
        move |groups| saves.borrow_mut().push(groups.to_vec()),
    );
    let mut rt = runtime(playground);
    assert_eq!(*loads.borrow(), 0, "nothing loads until the playground opens");
    rt.open_playground("Lists/Short", None);
    assert!(!shown(&rt, "playground-item-0") && shown(&rt, "playground-item-2"), "Cards loads folded");
    tap(&mut rt, "playground-group-2");
    tap(&mut rt, "playground-group-0");
    assert_eq!(*saved.borrow(), [vec!["Cards".to_owned(), "Lists".to_owned()], vec!["Lists".to_owned()]], "a group it doesn't have is dropped");
    reopen(&mut rt);
    assert_eq!(*loads.borrow(), 1, "loaded once");
    rt.open_playground("Lists/Short", None);
    assert_eq!(saved.borrow().last().map(Vec::len), Some(0), "opening a story in a folded group unfolds it, and saves that");
}
