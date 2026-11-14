use super::*;

fn lab_variants() -> Playground<Real> {
    let story = Story::new("Variants/Pressed", Lab::default, lab)
        .variant("Few", |p| p.presses = 5)
        .variant("Many", |p| p.presses = 50)
        .trigger('x', "Clear", |p, _| p.presses = 0);
    base().story(story)
}

#[test]
fn digits_switch_a_storys_variant_and_r_remakes_the_one_its_on() {
    let mut rt = runtime_with(lab_variants(), Some(Some("Pressed".into())), true);
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 5", "a story with variants starts on its first");
    assert!(find(&rt, "playground-variant-0").is_some() && find(&rt, "playground-variant-1").is_some(), "the side column lists them");
    assert_eq!(shows(&rt, "playground-hint").replace('\u{a0}', " "), "↑↓ stories · ←→ groups · / filter · Tab controls · Esc back");
    let (hint, side) = (find(&rt, "playground-hint").unwrap().rect, find(&rt, "playground-side").unwrap().rect);
    let (one_line, line) = rt.text.measure(&shows(&rt, "playground-hint"), crate::tok::type_::SECTION, 1.0);
    assert!(hint[0] + hint[2] <= side[0] + side[2], "{hint:?} in {side:?}");
    assert!(one_line <= hint[2] - 8.0 || hint[3] > 1.5 * line, "a hint too long for one line wraps: {one_line} wide, {hint:?}");
    let all = rt.elements();
    let caps: Vec<_> = all.iter().filter(|e| e.parent.is_some_and(|p| all[p].key.as_deref() == Some("cap"))).filter_map(|e| e.text.clone()).collect();
    assert_eq!(caps, ["1", "2", "R", "X"], "each variant's and trigger's key, by its row");
    letter(&mut rt, "2");
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 5", "from the list, a digit isn't a variant");
    press(&mut rt, Key::Tab);
    assert_eq!(shows(&rt, "playground-hint").replace('\u{a0}', " "), "↑↓ controls · 1–2 variants · Esc stories");
    letter(&mut rt, "2");
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 50");
    tap(&mut rt, "bump");
    escape(&mut rt);
    letter(&mut rt, "r");
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 50", "R makes the variant again");
    press(&mut rt, Key::Tab);
    letter(&mut rt, "3");
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 50", "there's no third");
    letter(&mut rt, "x");
    rt.frame(DT);
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 0", "triggers still run");
    tap(&mut rt, "playground-variant-0");
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 5", "a row picks one too");
    letter(&mut rt, "2");
    escape(&mut rt);
    letter(&mut rt, "k");
    letter(&mut rt, "j");
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 50", "J and K still move, and the variant stays with its story");
    escape(&mut rt);
    rt.update(|_, tx| tx.open_playground());
    rt.frame(DT);
    assert_eq!(shows(&rt, "lab"), "ticks 0 presses 5", "reopened, it's back on its first");
}

#[test]
fn a_scene_has_variants_too_applied_after_its_scene_fn() {
    let scenes = base()
        .variant("Still", "Ten", |s| s.count = 10)
        .variant("Still", "Twenty", |s| s.count = 20)
        .trigger("Still", 'd', "Double", |s, _| s.count *= 2);
    let mut rt = runtime_with(scenes, Some(Some("Still".into())), true);
    assert_eq!(shows(&rt, "count"), "count 10");
    press(&mut rt, Key::Tab);
    letter(&mut rt, "2");
    assert_eq!(shows(&rt, "count"), "count 20");
    letter(&mut rt, "d");
    rt.frame(DT);
    assert_eq!(shows(&rt, "count"), "count 40");
    letter(&mut rt, "r");
    assert_eq!(shows(&rt, "count"), "count 20");
    escape(&mut rt);
    letter(&mut rt, "k");
    assert!(find(&rt, "playground-variant-0").is_none(), "another scene has none");
    press(&mut rt, Key::Tab);
    letter(&mut rt, "1");
    assert_eq!(shows(&rt, "count"), "count 0", "and a digit isn't the playground's there");
}
