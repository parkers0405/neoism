use super::*;

fn typing() -> Playground<Real> {
    base().typing(|s| s.typing).trigger("big", 't', "Type", |s, _| s.typing = true)
}

fn typed(rt: &Runtime<Real>) -> String {
    rt.playground.as_ref().and_then(|p| p.scene.as_ref()).map(|s| s.state.typed.clone()).unwrap_or_default()
}

#[test]
fn while_the_stage_is_typing_its_on_key_gets_keys_before_the_playground() {
    let mut rt = runtime_with(typing(), Some(Some("big".into())), true);
    letter(&mut rt, "t");
    rt.frame(DT);
    for c in ["j", "r", "t", "2"] { letter(&mut rt, c); }
    assert_eq!((typed(&rt), selected(&rt)), ("jrt2".into(), 0), "letters, R and triggers are the stage's");
    press(&mut rt, Key::Down);
    assert_eq!(selected(&rt), 1, "what on_key refuses still reaches the playground");
}

#[test]
fn without_typing_the_playground_keeps_its_keys() {
    let mut rt = runtime_with(base().trigger("big", 't', "Type", |s, _| s.typing = true), Some(Some("big".into())), true);
    letter(&mut rt, "t");
    rt.frame(DT);
    letter(&mut rt, "j");
    assert_eq!(selected(&rt), 1);
}
