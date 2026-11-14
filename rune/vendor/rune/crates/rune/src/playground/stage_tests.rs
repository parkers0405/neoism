//! How each stage layout takes the size picked beside the stage.
use super::*;
use crate::{App, AppSpec, Env, Runtime};

const SIZE: (f32, f32) = (1200.0, 700.0);

#[derive(Default)]
struct Real;

fn real(_: &Real, _: &mut Cx<Real>) -> El {
    text("the app")
}

#[derive(Default)]
struct Card;

fn plain(_: &Card, _: &mut Cx<Card>) -> El {
    el().key("plain")
}

fn tall(_: &Card, _: &mut Cx<Card>) -> El {
    col().key("tall").w_full().children((0..40).map(|_| el().h(40.0)))
}

fn screen(_: &Card, _: &mut Cx<Card>) -> El {
    el().key("screen")
}

fn runtime() -> Runtime<Real> {
    let playground = Playground::<Real>::new()
        .story(Story::new("Cards/Plain", Card::default, plain))
        .story(Story::new("Cards/Tall", Card::default, tall).stage(Stage::Scroll))
        .story(Story::new("Screens/Window", Card::default, screen).stage(Stage::Fill))
        .sizes(&[("Small", (300.0, 200.0))]);
    App::new(AppSpec::new("stage", SIZE, Real::default, real)).playground(playground).headless_runtime(SIZE, Env::new(false, 1.0))
}

/// Opens `story` at the "Small" size; the width and height of the element keyed `key`.
fn size_at_small(story: &str, key: &str) -> (f32, f32) {
    let mut rt = runtime();
    rt.open_playground(story, None);
    rt.update(|_, tx| tx.playground.push(Request::Size(1)));
    rt.frame(1.0 / 60.0);
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"));
    (e.rect[2], e.rect[3])
}

#[test]
fn a_centred_story_takes_the_picked_size() {
    assert_eq!(size_at_small("Cards/Plain", "plain"), (300.0, 200.0));
}

#[test]
fn a_scrolling_story_takes_its_width_inside_the_margin() {
    assert_eq!(size_at_small("Cards/Tall", "tall"), (300.0 - 2.0 * SCROLL_MARGIN, 1600.0));
}

#[test]
fn a_scrolling_story_keeps_the_root_handlers() {
    let mut rt = runtime();
    rt.open_playground("Cards/Tall", None);
    let root = rt.playground.as_ref().unwrap().root;
    assert_eq!(rt.nodes[root].key.as_deref(), Some("tall"), "the story's root, not the margin around it");
}

#[test]
fn a_fill_story_keeps_the_whole_stage() {
    assert_eq!(size_at_small("Screens/Window", "screen"), (SIZE.0 - SIDEBAR - SIDE - 2.0, SIZE.1));
}
