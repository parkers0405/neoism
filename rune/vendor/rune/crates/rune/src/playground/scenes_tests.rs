use super::*;

#[test]
fn the_scene_filter_lists_only_the_scenes_it_keeps() {
    let mut rt = runtime_with(base().scenes(|name| name.contains('/')), Some(None), true);
    assert!(find(&rt, "playground-item-0").is_none() && find(&rt, "playground-item-1").is_none());
    assert!(find(&rt, "playground-item-2").is_some(), "Motion/Still stays");
    assert_eq!(selected(&rt), 2, "the list starts at its first listed story");
    let live = rt.playground.as_ref().unwrap();
    assert_eq!(live.find("big"), None, "a hidden scene can't be chosen by name");
    assert_eq!(live.find("Still"), Some(2));
    letter(&mut rt, "k");
    assert_eq!(selected(&rt), 4, "K wraps past the hidden scenes");
}

fn count_up() -> Playground<Real> {
    base().trigger("Still", 'r', "Count up", |s, play| {
        s.count = 1;
        play.after(0.5, |s, _| s.count = 2);
    })
}

fn run_for(rt: &mut Runtime<Real>, secs: f64) {
    for _ in 0..(secs * 60.0).round() as usize { rt.frame(1.0 / 60.0); }
}

#[test]
fn choosing_a_scene_runs_its_r_trigger() {
    let mut rt = runtime_with(count_up(), Some(Some("Still".into())), false);
    rt.frame(DT);
    assert_eq!(shows(&rt, "count"), "count 1", "it ran as the scene opened, headless included");
    run_for(&mut rt, 0.6);
    assert_eq!(shows(&rt, "count"), "count 2", "and its steps follow on the clock");
    letter(&mut rt, "j");
    letter(&mut rt, "k");
    rt.frame(DT);
    assert_eq!(shows(&rt, "count"), "count 1", "a scene starts afresh every time it's chosen");
}

#[test]
fn a_story_runs_its_r_trigger_when_its_state_is_made_not_when_it_comes_back() {
    let mut rt = runtime(Some(Some("Fade".into())));
    rt.frame(DT);
    assert_eq!(shows(&rt, "fade"), "[\"start\"]");
    run_for(&mut rt, 1.0);
    assert_eq!(shows(&rt, "fade"), "[\"start\", \"half\", \"end\"]");
    letter(&mut rt, "j");
    letter(&mut rt, "k");
    run_for(&mut rt, 1.0);
    assert_eq!(shows(&rt, "fade"), "[\"start\", \"half\", \"end\"]", "the kept state isn't played again");
    letter(&mut rt, "r");
    rt.frame(DT);
    assert_eq!(shows(&rt, "fade"), "[\"start\"]", "R plays it once, not twice");
}
