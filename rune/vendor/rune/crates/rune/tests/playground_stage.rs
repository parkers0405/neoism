//! The story list's counts and motion marks, and how a stage lays a story out: centred, a
//! scrolling page, or edge to edge.
use rune::{App, AppSpec, Cx, El, Env, Playground, Runtime, Stage, Story, col, el, text};

const SIZE: (f32, f32) = (1200.0, 700.0);
/// The stage's width: the window less the sidebar, the side column and their hairlines.
const STAGE_W: f32 = 1200.0 - 200.0 - 264.0 - 2.0;

#[derive(Default)]
struct Pad {
    rows: usize,
}

fn pad(s: &Pad, _: &mut Cx<Pad>) -> El {
    col().key("pad").w_full().children((0..s.rows).map(|_| el().h(40.0)))
}

static SCENES: &[(&str, fn(&mut Pad))] = &[("Pad/Empty", |_| {}), ("Pad/Full", |s| s.rows = 3), ("Long/Scene", |s| s.rows = 40)];

#[derive(Default)]
struct Card;

fn card(_: &Card, _: &mut Cx<Card>) -> El {
    text("card").key("card")
}

fn tall(_: &Card, _: &mut Cx<Card>) -> El {
    col().key("tall").w_full().children((0..40).map(|_| el().h(40.0)))
}

fn screen(_: &Card, _: &mut Cx<Card>) -> El {
    el().key("screen")
}

/// Scenes 0 to 2, then stories 3 to 6.
fn playground() -> Playground<Pad> {
    Playground::<Pad>::new()
        .trigger("Full", 'd', "Double", |s, _| s.rows *= 2)
        .stage("Long/Scene", Stage::Scroll)
        .story(Story::new("Cards/Plain", Card::default, card))
        .story(Story::new("Cards/Raised", Card::default, card).trigger('p', "Pop", |_, _| {}))
        .story(Story::new("Cards/Tall", Card::default, tall).stage(Stage::Scroll))
        .story(Story::new("Screens/Window", Card::default, screen).stage(Stage::Fill))
}

fn runtime() -> Runtime<Pad> {
    let mut spec = AppSpec::new("stage", SIZE, Pad::default, pad);
    spec.scenes = SCENES;
    App::new(spec).playground(playground()).headless_runtime(SIZE, Env::new(false, 1.0))
}

fn element(rt: &Runtime<Pad>, key: &str) -> Option<rune::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

fn rect(rt: &Runtime<Pad>, key: &str) -> [f32; 4] {
    element(rt, key).unwrap_or_else(|| panic!("no {key}")).rect
}

/// The stage's rect: the element keyed `playground-stage-<generation>`.
fn stage(rt: &Runtime<Pad>) -> [f32; 4] {
    rt.elements().into_iter()
        .find(|e| !e.exiting && e.key.as_deref().and_then(|k| k.strip_prefix("playground-stage-")).is_some_and(|g| g.parse::<u32>().is_ok()))
        .expect("a stage")
        .rect
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.5)
}

#[test]
fn each_header_counts_its_stories() {
    let mut rt = runtime();
    rt.open_playground("Cards/Plain", None);
    let count = |key: &str| element(&rt, key).and_then(|e| e.text).unwrap_or_else(|| panic!("no {key}"));
    assert_eq!(count("playground-count-0"), "2", "Pad");
    assert_eq!(count("playground-count-2"), "1", "Long");
    assert_eq!(count("playground-count-3"), "3", "Cards");
    assert_eq!(count("playground-count-6"), "1", "Screens");
}

#[test]
fn a_folded_group_still_counts_its_stories() {
    let mut rt = runtime();
    rt.open_playground("Pad/Empty", None);
    let (x, y) = rt.find("playground-group-3").expect("the Cards header");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(element(&rt, "playground-item-3").is_none(), "Cards folded");
    assert_eq!(element(&rt, "playground-count-3").and_then(|e| e.text).as_deref(), Some("3"));
}

#[test]
fn only_stories_with_triggers_have_a_motion_mark() {
    let mut rt = runtime();
    rt.open_playground("Cards/Plain", None);
    let marked: Vec<usize> = (0..7).filter(|i| element(&rt, &format!("playground-motion-{i}")).is_some()).collect();
    assert_eq!(marked, [1, 4], "the scene with a trigger and the story with one");
}

#[test]
fn a_scroll_stage_starts_its_content_at_the_top_and_scrolls() {
    let mut rt = runtime();
    rt.open_playground("Cards/Tall", None);
    let stage = stage(&rt);
    assert!(close([stage[0], stage[1], stage[2], stage[3]], [201.0, 0.0, STAGE_W, SIZE.1]), "the stage fills its room: {stage:?}");
    let top = rect(&rt, "tall");
    assert!(close(top, [stage[0] + 24.0, stage[1] + 24.0, stage[2] - 48.0, 1600.0]), "inside the margin, at its own height: {top:?}");
    // A viewport learns how far it scrolls on a frame that moves the clock.
    rt.frame(1.0 / 60.0);
    rt.wheel(stage[0] + stage[2] / 2.0, stage[1] + stage[3] / 2.0, -300.0);
    rt.frame(1.0 / 60.0);
    let scrolled = rect(&rt, "tall");
    assert!((scrolled[1] - (top[1] - 300.0)).abs() < 0.5, "the stage scrolled: {scrolled:?}");
}

#[test]
fn a_scene_takes_its_stage_from_the_playground() {
    let mut rt = runtime();
    rt.open_playground("Long/Scene", None);
    let stage = stage(&rt);
    let top = rect(&rt, "pad");
    assert!((top[1] - (stage[1] + 24.0)).abs() < 0.5 && (top[3] - 1600.0).abs() < 0.5, "a scrolling page: {top:?}");
    rt.open_playground("Pad/Full", None);
    assert!(close(rect(&rt, "pad"), stage), "the others stay centred, the size of the stage");
}

#[test]
fn a_fill_stage_gives_the_root_the_whole_stage() {
    let mut rt = runtime();
    rt.open_playground("Screens/Window", None);
    let stage = stage(&rt);
    assert!(close(stage, [201.0, 0.0, STAGE_W, SIZE.1]), "the stage fills its room: {stage:?}");
    assert!(close(rect(&rt, "screen"), stage));
}
