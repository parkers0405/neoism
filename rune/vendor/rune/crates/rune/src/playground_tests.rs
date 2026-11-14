use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::{AppSpec, Env, Key, KeyEvent, Mods, Runtime, TextEdit, TextStyle, Transition, text_input};

#[derive(Default)]
struct Real {
    count: u32,
    polls: u32,
    holds_escape: bool,
    field: TextEdit,
    pastes: Vec<String>,
    drops: usize,
    focus: Vec<bool>,
    waker: Option<crate::Waker>,
    inbox: Option<Arc<std::sync::Mutex<Vec<u32>>>>,
    typing: bool,
    typed: String,
}

fn view(s: &Real, _: &mut Cx<Real>) -> El {
    col().w_full().h_full()
        .on_paste(|s: &mut Real, text: &str, _| s.pastes.push(text.into()))
        .on_drop_files(|s: &mut Real, files: &[std::path::PathBuf], _| s.drops += files.len())
        .on_window_focus(|s: &mut Real, focused, _| s.focus.push(focused))
        .child(text(format!("count {}", s.count)).key("count"))
        .child(el().key("inc").size(40.0, 20.0).on_press(|s: &mut Real| s.count += 1))
        .child(text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 24.0).on_edit(|s: &mut Real| &mut s.field))
}

static SCENES: &[(&str, fn(&mut Real))] = &[("big", |s| s.count = 100), ("holding", |s| s.holds_escape = true), ("Motion/Still", |_| {})];

#[derive(Default)]
struct Lab {
    ticks: u32,
    presses: u32,
}

fn lab(p: &Lab, _: &mut Cx<Lab>) -> El {
    col().w_full().h_full()
        .child(text(format!("ticks {} presses {}", p.ticks, p.presses)).key("lab"))
        .child(el().key("bump").size(40.0, 20.0).on_press(|p: &mut Lab| p.presses += 1))
}

/// A timer: one tick a second, by `wake_after` and the page's poll.
fn lab_poll(p: &mut Lab, t: f64, tx: &mut Tx) -> bool {
    tx.wake_after(Duration::from_secs(1));
    let due = t.floor() as u32;
    let changed = due != p.ticks;
    p.ticks = due;
    changed
}

fn activity(s: &Real, _: &mut Cx<Real>) -> El {
    text(format!("live {}", s.count)).key("activity")
}

/// A story whose replay runs timed steps, logging when each was due after the first.
#[derive(Default)]
struct Fade {
    t0: f64,
    log: Vec<(&'static str, f64)>,
}

fn fade(p: &Fade, _: &mut Cx<Fade>) -> El {
    text(format!("{:?}", p.log.iter().map(|(n, _)| *n).collect::<Vec<_>>())).key("fade")
}

fn fade_story() -> Story<Fade> {
    Story::new("Motion/Fade", Fade::default, fade)
        .trigger('r', "Fade in", |p, play| {
            p.t0 = play.now();
            p.log.push(("start", 0.0));
            play.after(0.5, |p, play| {
                p.log.push(("half", play.now() - p.t0));
                play.after(0.25, |p, play| p.log.push(("end", play.now() - p.t0)));
            });
        })
        .trigger('f', "Finish", |p, _| p.log.push(("finish", 0.0)))
}

fn base() -> Playground<Real> {
    Playground::new()
        .enabled(true)
        .page("Live/Activity", activity)
        .story(Story::new("Lab", Lab::default, lab).poll(lab_poll))
        .story(fade_story())
        .trigger("big", 'd', "Double", |s, _| s.count *= 2)
}

fn runtime_in(open: Option<Option<String>>, windowed: bool) -> Runtime<Real> {
    runtime_with(base(), open, windowed)
}

fn runtime_with(playground: Playground<Real>, open: Option<Option<String>>, windowed: bool) -> Runtime<Real> {
    let mut spec = AppSpec::new("playground", (800.0, 500.0), Real::default, view);
    spec.scenes = SCENES;
    spec.poll = Some(|s, _, _| { s.polls += 1; false });
    spec.on_key = Some(|s, ev, _| match &ev.key {
        Key::Text(t) if s.typing => {
            s.typed.push_str(t);
            true
        }
        key => s.holds_escape && *key == Key::Escape,
    });
    let mut rt = crate::tooling::runtime_for(&spec, (spec.init)(), spec.size, Env::new(false, 1.0), Arc::new(|| {}));
    attach(&mut rt, Some(playground), &spec, open, None, windowed);
    rt.frame(0.0);
    rt
}

fn runtime(open: Option<Option<String>>) -> Runtime<Real> {
    runtime_in(open, true)
}

fn press_with(rt: &mut Runtime<Real>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

fn press(rt: &mut Runtime<Real>, key: Key) {
    press_with(rt, key, Mods::default());
}

fn letter(rt: &mut Runtime<Real>, c: &str) {
    press(rt, Key::Text(c.into()));
}

fn selected(rt: &Runtime<Real>) -> usize {
    rt.playground.as_ref().map_or(usize::MAX, |p| p.selected)
}

fn header(rt: &Runtime<Real>) -> Option<&'static str> {
    rt.playground.as_ref().and_then(|p| p.header)
}

fn focused(rt: &Runtime<Real>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn find(rt: &Runtime<Real>, key: &str) -> Option<crate::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

fn shows(rt: &Runtime<Real>, key: &str) -> String {
    find(rt, key).and_then(|e| e.text).unwrap_or_else(|| panic!("no {key}"))
}

fn tap(rt: &mut Runtime<Real>, key: &str) {
    let r = find(rt, key).unwrap_or_else(|| panic!("no {key}")).rect;
    let (x, y) = (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn escape(rt: &mut Runtime<Real>) {
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn region(rt: &Runtime<Real>) -> Region {
    rt.playground.as_ref().map_or(Region::List, |p| p.region())
}

fn is_open(rt: &Runtime<Real>) -> bool {
    rt.playground.as_ref().is_some_and(|p| p.is_open())
}

#[test]
fn the_bar_shortens_the_app_and_opens_the_playground() {
    let mut rt = runtime(None);
    assert!(find(&rt, "playground-open").is_some());
    let bar = find(&rt, "playground-open").unwrap().rect;
    assert!(bar[1] >= 500.0 - BAR, "the bar sits along the bottom edge");
    tap(&mut rt, "inc");
    assert_eq!(rt.state.count, 1);
    tap(&mut rt, "playground-open");
    assert!(is_open(&rt));
    assert_eq!(shows(&rt, "count"), "count 100", "the first scene, from a fresh init");
}

#[test]
fn a_scene_gets_a_fresh_state_and_back_restores_the_real_one() {
    let mut rt = runtime(None);
    tap(&mut rt, "inc");
    rt.update(|_, tx| tx.open_playground());
    rt.frame(1.0 / 60.0);
    tap(&mut rt, "inc");
    assert_eq!(shows(&rt, "count"), "count 101");
    tap(&mut rt, "playground-trigger-d");
    rt.frame(1.0 / 60.0);
    assert_eq!(shows(&rt, "count"), "count 202", "triggers act on the stage");
    assert_eq!(rt.state.count, 1, "the real state is untouched");
    tap(&mut rt, "playground-back");
    assert!(!is_open(&rt));
    assert_eq!(shows(&rt, "count"), "count 1");
    assert!(find(&rt, "playground-trigger-d").is_none());
}

#[test]
fn the_stage_takes_escape_first() {
    let mut rt = runtime(Some(Some("holding".into())));
    assert!(is_open(&rt));
    escape(&mut rt);
    assert!(is_open(&rt), "the scene's on_key took Escape");
    tap(&mut rt, "playground-item-0");
    tap(&mut rt, "field");
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key).as_deref(), Some("field"));
    escape(&mut rt);
    assert!(is_open(&rt), "the focused field took Escape");
    escape(&mut rt);
    assert!(is_open(&rt) && region(&rt) == Region::List, "unhandled in the stage, Escape gives the list the keyboard");
    escape(&mut rt);
    assert!(!is_open(&rt), "and from the list goes back");
    assert!(!rt.state.holds_escape);
}

#[test]
fn a_page_keeps_its_own_state_and_timer_while_open() {
    let mut rt = runtime(Some(Some("lab".into())));
    tap(&mut rt, "bump");
    tap(&mut rt, "bump");
    for _ in 0..70 { rt.frame(1.0 / 60.0); }
    assert_eq!(shows(&rt, "lab"), "ticks 1 presses 2");
    tap(&mut rt, "playground-item-0");
    tap(&mut rt, "playground-item-4");
    assert_eq!(shows(&rt, "lab"), "ticks 1 presses 2", "kept across selections");
    tap(&mut rt, "playground-trigger-r");
    assert_eq!(rt.wake_deadline(), Some(rt.clock), "a new stage state asks for its first poll");
    rt.frame(1.0 / 60.0);
    assert_eq!(shows(&rt, "lab"), "ticks 1 presses 0", "restart makes it again, polled at once");
    tap(&mut rt, "bump");
    escape(&mut rt);
    escape(&mut rt);
    rt.update(|_, tx| tx.open_playground());
    rt.frame(1.0 / 60.0);
    assert!(shows(&rt, "lab").ends_with("presses 0"), "closing drops it");
}

#[test]
fn a_live_page_draws_and_changes_the_real_state() {
    let mut rt = runtime(Some(Some("Activity".into())));
    assert_eq!(shows(&rt, "activity"), "live 0");
    rt.update(|s, _| s.count = 7);
    rt.frame(1.0 / 60.0);
    assert_eq!(shows(&rt, "activity"), "live 7");
}

#[test]
fn the_real_state_keeps_polling_while_open() {
    let mut rt = runtime(Some(None));
    let before = rt.state.polls;
    for _ in 0..5 { rt.frame(1.0 / 60.0); }
    assert!(rt.state.polls >= before + 5);
    let scene = rt.playground.as_ref().and_then(|p| p.scene.as_ref()).map(|s| s.state.polls);
    assert!(scene.is_some_and(|n| n >= 5), "the scene polls too");
}

#[test]
fn without_the_bar_the_app_lays_out_untouched() {
    let spec = AppSpec::new("playground", (800.0, 500.0), Real::default, view);
    let mut rt = crate::tooling::runtime_for(&spec, (spec.init)(), spec.size, Env::new(false, 1.0), Arc::new(|| {}));
    rt.frame(0.0);
    let plain: Vec<_> = rt.elements().into_iter().map(|e| (e.path, e.rect)).collect();
    let mut rt = crate::tooling::runtime_for(&spec, (spec.init)(), spec.size, Env::new(false, 1.0), Arc::new(|| {}));
    attach(&mut rt, Some(Playground::new().enabled(true).bar(false)), &spec, None, None, true);
    rt.frame(0.0);
    let barless: Vec<_> = rt.elements().into_iter().map(|e| (e.path, e.rect)).collect();
    assert_eq!(plain, barless);
}

#[test]
fn keys_walk_the_stories_in_their_groups_order() {
    let mut rt = runtime(Some(None));
    // Scenes: big, holding; Motion: Still (a scene), Fade; Live: Activity; Pages: Lab.
    for (i, group) in [(0, true), (1, false), (2, true), (5, false), (3, true), (4, true)] {
        assert_eq!(find(&rt, &format!("playground-group-{i}")).is_some(), group, "story {i}");
    }
    // Headers are rows too; on one, the stage keeps its story.
    let walk = [(Key::Down, 1, None), (Key::Down, 1, Some("Motion")), (Key::Down, 2, None), (Key::Down, 5, None), (Key::Down, 5, Some("Live")), (Key::Down, 3, None), (Key::Up, 3, Some("Live")), (Key::Up, 5, None)];
    for (key, at, on) in walk {
        press(&mut rt, key);
        assert_eq!((selected(&rt), header(&rt)), (at, on));
    }
    letter(&mut rt, "j");
    assert_eq!((selected(&rt), header(&rt)), (3, None), "J skips headers");
    letter(&mut rt, "k");
    assert_eq!((selected(&rt), header(&rt)), (5, None));
    let cmd = Mods { cmd: true, ..Mods::default() };
    press_with(&mut rt, Key::Down, cmd);
    assert_eq!(selected(&rt), 4, "⌘↓ goes to the last story");
    press_with(&mut rt, Key::Up, cmd);
    assert_eq!(selected(&rt), 0, "⌘↑ to the first");
    press(&mut rt, Key::Up);
    assert_eq!(header(&rt), Some("Scenes"));
    press(&mut rt, Key::Up);
    assert_eq!(selected(&rt), 4, "and ↑ wraps");
}

#[test]
fn a_replay_runs_its_steps_on_the_logical_clock_the_same_every_time() {
    let mut logs = Vec::new();
    for (start, dt) in [(0.0, 1.0 / 60.0), (3.3, 1.0 / 24.0)] {
        let mut rt = runtime(Some(Some("fade".into())));
        rt.frame(start);
        letter(&mut rt, "f");
        rt.frame(dt);
        letter(&mut rt, "r");
        // A second R part way starts over rather than queueing another run.
        for _ in 0..4 { rt.frame(dt); }
        letter(&mut rt, "r");
        rt.frame(dt);
        assert_eq!(shows(&rt, "fade"), "[\"start\"]", "the replay made the state again");
        for _ in 0..(1.0 / dt) as usize { rt.frame(dt); }
        let log = rt.playground.as_ref().and_then(|p| p.own()).map(|(own, stage)| {
            own.state_ref(stage).downcast_ref::<Fade>().unwrap().log.iter().map(|(n, t)| (*n, (t * 1e6).round() as i64)).collect::<Vec<_>>()
        });
        logs.push(log.unwrap());
    }
    assert_eq!(logs[0], [("start", 0), ("half", 500_000), ("end", 750_000)]);
    assert_eq!(logs[0], logs[1], "whatever the frame rate or start time");
}

#[test]
fn right_goes_into_the_side_column_and_tab_or_left_comes_back() {
    let mut rt = runtime(Some(Some("Fade".into())));
    press(&mut rt, Key::Right);
    assert_eq!(focused(&rt).as_deref(), Some("playground-trigger-r"));
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt).as_deref(), Some("playground-trigger-f"));
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt).as_deref(), Some("playground-trigger-r"), "↑↓ wrap inside it");
    letter(&mut rt, "j");
    assert_eq!(selected(&rt), 5, "J doesn't leave the story from the side column");
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt), None);
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Left);
    assert_eq!((focused(&rt), selected(&rt)), (None, 5), "← comes back too, to the same story");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Down);
    assert_eq!(selected(&rt), 3);
    press(&mut rt, Key::Right);
    assert_eq!(focused(&rt), None, "a live page has no triggers to go into");
}

#[test]
fn the_shortcut_opens_and_closes_it_without_a_menu() {
    let mut rt = runtime_in(Some(None), false);
    let chord = Mods { cmd: true, shift: true, ctrl: cfg!(not(target_os = "macos")), ..Mods::default() };
    assert!(is_open(&rt));
    press_with(&mut rt, Key::Command('d'), chord);
    assert!(!is_open(&rt));
    press_with(&mut rt, Key::Command('d'), chord);
    assert!(is_open(&rt));
    let mut rt = runtime(Some(None));
    let menu = cfg!(target_os = "macos");
    press_with(&mut rt, Key::Command('d'), chord);
    assert_eq!(is_open(&rt), menu, "the View menu's item has it in a Mac window");
}

#[test]
fn leaving_a_story_part_way_drops_its_queued_steps() {
    let mut rt = runtime(Some(Some("Fade".into())));
    letter(&mut rt, "r");
    letter(&mut rt, "j");
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    letter(&mut rt, "k");
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert_eq!(shows(&rt, "fade"), "[\"start\"]", "no burst of overdue steps on return");
}

const DT: f64 = 1.0 / 60.0;

fn center(rt: &Runtime<Real>, key: &str) -> (f32, f32) {
    let r = find(rt, key).unwrap_or_else(|| panic!("no {key}")).rect;
    (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5)
}

fn own_state<P: 'static>(rt: &Runtime<Real>) -> &P {
    let (own, stage) = rt.playground.as_ref().and_then(|p| p.own()).expect("a story with its own state");
    own.state_ref(stage).downcast_ref().unwrap()
}

fn scene_state(rt: &Runtime<Real>) -> &Real {
    &rt.playground.as_ref().and_then(|p| p.scene.as_ref()).expect("a scene").state
}

#[cfg(feature = "hot")]
#[test]
fn a_view_that_panics_falls_back_to_the_open_story() {
    thread_local! { static PANICS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
    fn flaky(_: &Lab, _: &mut Cx<Lab>) -> El {
        if PANICS.replace(PANICS.get().saturating_sub(1)) > 0 { panic!("a patched view panicked"); }
        text("steady").key("flaky")
    }
    let mut rt = runtime_with(base().story(Story::new("Fixes/Flaky", Lab::default, flaky)), Some(Some("Flaky".into())), true);
    assert_eq!(shows(&rt, "flaky"), "steady");
    PANICS.set(1);
    rt.update(|s, _| s.count += 1);
    rt.frame(DT);
    assert_eq!(shows(&rt, "flaky"), "steady", "the fallback builds the story, not the app");
    assert!(find(&rt, "count").is_none());
}

#[derive(Default)]
struct Tip {
    hidden: bool,
}

fn tip(p: &Tip, _: &mut Cx<Tip>) -> El {
    let tip = (!p.hidden).then(|| el().key("tip").size(120.0, 120.0).transition(Transition::opacity()).on_hover_change(|_: &mut Tip, _, _| {}));
    col().w_full().h_full().children(tip)
}

#[test]
fn a_new_stage_ends_the_old_ones_exits() {
    let story = Story::new("Fixes/Tip", Tip::default, tip).trigger('h', "Hide", |p, _| p.hidden = true);
    let mut rt = runtime_with(base().story(story), Some(Some("Tip".into())), true);
    let (x, y) = center(&rt, "tip");
    rt.pointer_move(x, y);
    rt.frame(DT);
    letter(&mut rt, "h");
    rt.frame(DT);
    let exiting = |rt: &Runtime<Real>| rt.elements().into_iter().any(|e| e.key.as_deref() == Some("tip") && e.exiting);
    assert!(exiting(&rt), "the tip fades out");
    letter(&mut rt, "j");
    assert!(!exiting(&rt), "gone with the state its handlers were built for");
    rt.pointer_move(x + 300.0, y);
    rt.frame(DT);
    rt.pointer_move(x, y);
    rt.frame(DT);
}

#[test]
fn root_handlers_reach_the_app_above_the_bar_and_the_stage() {
    let mut rt = runtime(None);
    rt.paste("hi");
    rt.frame(DT);
    rt.drop_files(vec!["a.txt".into()], None);
    rt.frame(DT);
    rt.window_focus(false);
    rt.frame(DT);
    assert_eq!((rt.state.pastes.as_slice(), rt.state.drops, rt.state.focus.last()), (&["hi".to_owned()][..], 1, Some(&false)));
    rt.update(|_, tx| tx.open_playground());
    rt.frame(DT);
    rt.paste("staged");
    rt.frame(DT);
    rt.drop_files(vec!["b.txt".into(), "c.txt".into()], None);
    rt.frame(DT);
    rt.window_focus(true);
    rt.frame(DT);
    let scene = scene_state(&rt);
    assert_eq!((scene.pastes.as_slice(), scene.drops, scene.focus.as_slice()), (&["staged".to_owned()][..], 2, &[true][..]));
    assert_eq!((rt.state.pastes.len(), rt.state.drops), (1, 1), "the real state is untouched");
}

#[test]
fn a_key_after_a_press_that_opens_the_playground_reaches_it() {
    let mut rt = runtime(None);
    let (x, y) = center(&rt, "playground-open");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(DT);
    assert!(is_open(&rt));
    assert_eq!(selected(&rt), 1, "the open landed before the key");
}

#[derive(Default)]
struct Tray {
    hovering: bool,
}

fn tray(_: &Tray, _: &mut Cx<Tray>) -> El {
    el().key("tray").w_full().h_full().on_drop_files(|_: &mut Tray, _, _| {}).on_drop_hover(|p: &mut Tray, on, _| p.hovering = on)
}

#[test]
fn a_new_stage_forgets_what_the_pointer_held_in_the_old_one() {
    let mut rt = runtime_with(base().story(Story::new("Fixes/Tray", Tray::default, tray)), Some(Some("Tray".into())), true);
    let at = center(&rt, "tray");
    rt.drag_files(vec!["a.txt".into()], Some(at));
    rt.frame(DT);
    assert!(own_state::<Tray>(&rt).hovering);
    rt.pointer_down(at.0, at.1);
    press(&mut rt, Key::Escape);
    letter(&mut rt, "j");
    assert_eq!(selected(&rt), 0);
    rt.drag_files_leave();
    rt.frame(DT);
    rt.pointer_move(at.0 + 10.0, at.1);
    rt.pointer_up(at.0 + 10.0, at.1);
    rt.frame(DT);
    assert_eq!(rt.drop_hovered(), None);
}

fn slow(_: &Lab, cx: &mut Cx<Lab>) -> El {
    cx.time_scale(0.25);
    text("slow").key("slow")
}

#[test]
fn a_storys_time_scale_stays_with_it() {
    let mut rt = runtime_with(base().story(Story::new("Fixes/Slow", Lab::default, slow)), Some(Some("Slow".into())), true);
    assert_eq!(rt.time_scale, 0.25);
    letter(&mut rt, "j");
    assert_eq!(rt.time_scale, 1.0, "the next story starts at the app's");
    letter(&mut rt, "k");
    assert_eq!(rt.time_scale, 0.25);
    escape(&mut rt);
    assert!(!is_open(&rt));
    assert_eq!(rt.time_scale, 1.0, "Back restores the app's");
}

#[derive(Default)]
struct Knob {
    value: i32,
}

fn knob(k: &Knob, _: &mut Cx<Knob>) -> El {
    col().w_full().h_full()
        .child(text(format!("value {}", k.value)).key("value"))
        .child(el().key("knob").size(60.0, 20.0).on_step(|k: &mut Knob, by, _| k.value += by))
}

#[test]
fn an_element_focused_in_the_stage_keeps_its_keys_until_escape() {
    let mut rt = runtime_with(base().story(Story::new("Fixes/Knob", Knob::default, knob)), Some(Some("Knob".into())), true);
    let at = selected(&rt);
    tap(&mut rt, "knob");
    assert_eq!(focused(&rt).as_deref(), Some("knob"));
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Up);
    press(&mut rt, Key::Up);
    assert_eq!((shows(&rt, "value"), selected(&rt)), ("value 1".to_owned(), at));
    escape(&mut rt);
    assert!(is_open(&rt) && focused(&rt).is_none(), "Escape leaves the knob");
    letter(&mut rt, "j");
    assert_ne!(selected(&rt), at);
}

#[test]
fn a_replay_from_the_side_column_keeps_the_keyboard_there() {
    let mut rt = runtime(Some(Some("Fade".into())));
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt).as_deref(), Some("playground-trigger-f"));
    letter(&mut rt, "r");
    rt.frame(DT);
    assert_eq!(shows(&rt, "fade"), "[\"start\"]");
    assert_eq!(focused(&rt).as_deref(), Some("playground-trigger-f"));
    press(&mut rt, Key::Up);
    assert_eq!(focused(&rt).as_deref(), Some("playground-trigger-r"));
}

#[test]
fn scene_triggers_and_sides_take_names_as_find_does() {
    let playground = base()
        .trigger("still", 'q', "Add four", |s, _| s.count += 4)
        .side("STILL", |s, _| text(format!("side {}", s.count)).key("still-side"));
    let mut rt = runtime_with(playground, Some(Some("Motion/Still".into())), true);
    assert_eq!(shows(&rt, "still-side"), "side 0");
    letter(&mut rt, "q");
    rt.frame(DT);
    assert_eq!(shows(&rt, "still-side"), "side 4");
}

#[test]
fn a_long_story_list_scrolls_and_reaches_what_was_off_screen() {
    let mut p = base();
    for n in 0..30 {
        let name: &'static str = Box::leak(format!("Many/Story {n}").into_boxed_str());
        p = p.story(Story::new(name, Lab::default, lab));
    }
    let mut rt = runtime_with(p, Some(None), true);
    rt.frame(DT);
    let last = rt.playground.as_ref().unwrap().order.last().copied().unwrap();
    let item = format!("playground-item-{last}");
    let (_, y) = center(&rt, &item);
    assert!(y > 500.0, "the last story starts below the window");
    let list = find(&rt, "playground-list").expect("list").rect;

    rt.wheel(list[0] + 20.0, list[1] + 100.0, -4000.0);
    rt.frame(DT);
    rt.frame(DT);
    let (x2, y2) = center(&rt, &item);
    assert!(y2 < y && y2 < 500.0, "the wheel scrolled the list up to the last story");
    let hit = rt.element_at(x2, y2).and_then(|p| p.key().map(str::to_owned));
    assert!(hit.is_some_and(|k| k == item || find(&rt, &k).is_some_and(|e| e.rect[1] >= y2 - 28.0)), "the last story is hittable");
}

#[path = "playground/groups_tests.rs"]
mod groups;

#[path = "playground/scenes_tests.rs"]
mod scenes;

#[path = "playground/real_tests.rs"]
mod real;

#[path = "playground/variants_tests.rs"]
mod variants;

#[path = "playground/keys_tests.rs"]
mod keys;

#[path = "playground/chrome_tests.rs"]
mod chrome;

#[path = "playground/stage_tests.rs"]
mod stage_layout;

#[path = "playground/press_tests.rs"]
mod press;

#[path = "playground/widths_tests.rs"]
mod widths_tests;
