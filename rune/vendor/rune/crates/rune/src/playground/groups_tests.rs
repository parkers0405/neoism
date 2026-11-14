use super::*;

fn shown(rt: &Runtime<Real>, i: usize) -> bool {
    find(rt, &format!("playground-item-{i}")).is_some()
}

#[test]
fn a_click_on_a_header_folds_its_group_and_another_unfolds_it() {
    let mut rt = runtime(Some(Some("big".into())));
    assert!(shown(&rt, 2) && shown(&rt, 5));
    tap(&mut rt, "playground-group-2");
    assert!(!shown(&rt, 2) && !shown(&rt, 5), "Motion's stories are hidden");
    assert!(shown(&rt, 3), "other groups aren't");
    assert_eq!((header(&rt), selected(&rt)), (Some("Motion"), 0), "the header is the selection; the stage keeps its story");
    tap(&mut rt, "playground-group-2");
    assert!(shown(&rt, 2) && shown(&rt, 5));
}

#[test]
fn left_right_and_return_fold_the_selected_header() {
    let mut rt = runtime(Some(Some("Still".into())));
    press(&mut rt, Key::Up);
    assert_eq!(header(&rt), Some("Motion"));
    press(&mut rt, Key::Left);
    assert!(!shown(&rt, 2));
    press(&mut rt, Key::Down);
    assert_eq!(header(&rt), Some("Live"), "↓ skips the folded stories");
    press(&mut rt, Key::Up);
    press(&mut rt, Key::Right);
    assert!(shown(&rt, 2));
    press(&mut rt, Key::Enter);
    assert!(!shown(&rt, 2));
    press(&mut rt, Key::Enter);
    assert!(shown(&rt, 2));
    assert_eq!(selected(&rt), 2, "the stage never changed story");
}

#[test]
fn folding_the_open_storys_group_selects_its_header_and_stays_folded_after_close() {
    let mut rt = runtime(Some(Some("Fade".into())));
    tap(&mut rt, "playground-group-2");
    assert_eq!((header(&rt), selected(&rt)), (Some("Motion"), 5));
    letter(&mut rt, "r");
    assert_eq!(header(&rt), Some("Motion"), "a replay keeps the list as it is");
    assert!(!shown(&rt, 5));
    escape(&mut rt);
    rt.update(|_, tx| tx.open_playground());
    rt.frame(DT);
    assert!(!shown(&rt, 2) && !shown(&rt, 5), "reopened, the group is still folded");
    assert_eq!((header(&rt), selected(&rt)), (Some("Motion"), 5), "and its header stands in for the story");
}

#[test]
fn choosing_a_story_by_name_unfolds_its_group() {
    let mut rt = runtime(Some(Some("big".into())));
    tap(&mut rt, "playground-group-2");
    rt.update(|_, tx| tx.playground.push(Request::Open(Some("Fade".into()))));
    rt.frame(DT);
    assert!(shown(&rt, 5));
    assert_eq!((header(&rt), selected(&rt)), (None, 5));
}
