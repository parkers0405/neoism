use super::*;

fn open(rt: &mut Runtime<Real>, name: &str) {
    let name = name.to_string();
    rt.update(move |_, tx| tx.playground.push(Request::Open(Some(name))));
    rt.frame(DT);
}

fn copy(real: &Real) -> Real {
    Real { count: real.count, ..Real::default() }
}

#[test]
fn fresh_scenes_start_from_the_real_state_on_each_choice_and_replay() {
    let mut rt = runtime_with(base().fresh(copy), None, true);
    rt.state.count = 7;
    open(&mut rt, "holding");
    assert_eq!(shows(&rt, "count"), "count 7", "from the real state, then the scene fn");
    open(&mut rt, "big");
    assert_eq!(shows(&rt, "count"), "count 100", "the scene fn still applies");
    rt.state.count = 9;
    open(&mut rt, "holding");
    assert_eq!(shows(&rt, "count"), "count 9");
    rt.state.count = 11;
    letter(&mut rt, "r");
    assert_eq!(shows(&rt, "count"), "count 11", "R starts from it again");
    assert_eq!(rt.state.count, 11, "and never writes to it");
}

#[test]
fn a_story_from_real_derives_its_state_when_it_shows_and_on_replay() {
    let story = Story::from_real("Lab/Derived", |r: &Real| Lab { ticks: r.count, presses: 0 }, lab);
    let mut rt = runtime_with(base().story(story), None, true);
    rt.state.count = 3;
    open(&mut rt, "Derived");
    assert_eq!(shows(&rt, "lab"), "ticks 3 presses 0");
    rt.state.count = 4;
    letter(&mut rt, "j");
    letter(&mut rt, "k");
    assert_eq!(shows(&rt, "lab"), "ticks 3 presses 0", "kept while the playground's open");
    letter(&mut rt, "r");
    assert_eq!(shows(&rt, "lab"), "ticks 4 presses 0", "derived again on R");
}

#[test]
#[should_panic(expected = "derives from another app state")]
fn a_story_from_another_apps_state_is_refused() {
    let _ = base().story(Story::from_real("Lab/Other", |l: &Lab| Lab { ticks: l.ticks, presses: 0 }, lab));
}

#[test]
fn keep_copies_the_stage_back_before_a_replay_a_switch_and_back() {
    let keep = base().fresh(copy).keep(|stage, real| real.count = stage.count);
    let mut rt = runtime_with(keep, None, true);
    rt.state.count = 5;
    open(&mut rt, "Still");
    tap(&mut rt, "inc");
    assert_eq!((shows(&rt, "count"), rt.state.count), ("count 6".into(), 5), "tuned on the stage only");
    escape(&mut rt);
    letter(&mut rt, "r");
    assert_eq!((shows(&rt, "count"), rt.state.count), ("count 6".into(), 6), "kept, and the replay starts from it");
    tap(&mut rt, "inc");
    escape(&mut rt);
    letter(&mut rt, "j");
    assert_eq!(rt.state.count, 7, "kept on a switch");
    letter(&mut rt, "k");
    assert_eq!(shows(&rt, "count"), "count 7", "which the next scene starts from");
    tap(&mut rt, "inc");
    escape(&mut rt);
    escape(&mut rt);
    assert_eq!(rt.state.count, 8, "and on Back");
    assert_eq!(shows(&rt, "count"), "count 8");
}

#[test]
fn a_fresh_stage_shares_the_real_waker_and_a_wake_from_another_thread_reaches_its_poll() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut spec = AppSpec::new("playground", (800.0, 500.0), Real::default, view);
    spec.scenes = SCENES;
    spec.setup = Some(|s, waker| {
        s.waker = Some(waker);
        s.inbox = Some(Arc::default());
    });
    spec.poll = Some(|s, _, _| {
        let got: Vec<u32> = s.inbox.as_ref().map(|i| i.lock().unwrap().drain(..).collect()).unwrap_or_default();
        s.count += got.iter().sum::<u32>();
        !got.is_empty()
    });
    // The stage keeps the real waker and makes its own inbox, as a worker it starts would.
    let fresh = base().fresh(|real| Real { waker: real.waker.clone(), inbox: Some(Arc::default()), ..Real::default() });
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = wakes.clone();
    let mut rt = crate::tooling::runtime_for(&spec, (spec.init)(), spec.size, Env::new(false, 1.0), Arc::new(move || { counted.fetch_add(1, Ordering::SeqCst); }));
    attach(&mut rt, Some(fresh), &spec, Some(Some("Still".into())), None, false);
    rt.frame(0.0);
    rt.frame(DT);
    let stage = &rt.playground.as_ref().unwrap().scene.as_ref().expect("a scene's stage").state;
    let (waker, inbox) = (stage.waker.clone().expect("the real waker"), stage.inbox.clone().unwrap());
    let before = wakes.load(Ordering::SeqCst);
    std::thread::spawn(move || {
        inbox.lock().unwrap().push(41);
        waker();
    })
    .join()
    .unwrap();
    assert_eq!(wakes.load(Ordering::SeqCst), before + 1, "the wake reached the window's waker");
    rt.frame(DT);
    assert_eq!(shows(&rt, "count"), "count 41", "and the next frame's poll of the stage took it");
    assert_eq!(rt.state.count, 0, "the real state's inbox is its own");
}

fn compare(stage: &Real, real: &Real, _: &mut Cx<Real>) -> El {
    col()
        .child(text(format!("stage {} real {}", stage.count, real.count)).key("compare"))
        .child(el().key("keep").size(40.0, 20.0).on_press_tx(|_: &mut Real, tx: &mut Tx| tx.keep_playground()))
}

#[test]
fn a_side_view_reads_the_real_state_and_keep_writes_the_preview_back() {
    let playground = base().fresh(copy).keep(|stage, real| real.count = stage.count).side_real("Still", compare);
    let mut rt = runtime_with(playground, None, true);
    rt.state.count = 3;
    open(&mut rt, "Still");
    assert_eq!(shows(&rt, "compare"), "stage 3 real 3");
    tap(&mut rt, "inc");
    assert_eq!(shows(&rt, "compare"), "stage 4 real 3", "the stage previews; the app's own choice shows beside it");
    let generation = rt.playground.as_ref().unwrap().generation;
    tap(&mut rt, "keep");
    assert_eq!(shows(&rt, "compare"), "stage 4 real 4");
    assert_eq!((rt.state.count, rt.playground.as_ref().unwrap().generation), (4, generation), "kept, and the stage stays");
}
