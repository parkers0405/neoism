//! Replaying and triggering the open story from a handler: `Tx::replay_playground` and
//! `Tx::playground_trigger`, as a deep link does after `Tx::open_playground_at`.
use rune::{App, AppSpec, Cx, El, Env, Playground, Runtime, Story, text};

#[derive(Default)]
struct Real;

#[derive(Default)]
struct Count {
    n: u32,
}

fn count(c: &Count, _: &mut Cx<Count>) -> El {
    text(format!("{}", c.n)).key("count")
}

fn runtime() -> Runtime<Real> {
    let story = Story::new("Motion/Count", Count::default, count)
        .trigger('r', "Replay", |c: &mut Count, _| c.n += 1)
        .trigger('s', "Step", |c: &mut Count, _| c.n += 100);
    App::new(AppSpec::new("replay", (400.0, 300.0), Real::default, |_: &Real, _: &mut Cx<Real>| text("app")))
        .playground(Playground::<Real>::new().story(story))
        .headless_runtime((400.0, 300.0), Env::new(false, 1.0))
}

fn n(rt: &Runtime<Real>) -> Option<u32> {
    rt.playground_state::<Count>().map(|c| c.n)
}

fn handler(rt: &mut Runtime<Real>, f: impl Fn(&mut rune::Tx) + 'static) {
    rt.update(move |_, tx| f(tx));
    rt.frame(0.0);
    rt.frame(0.0);
}

#[test]
fn a_handler_triggers_and_replays_the_open_story() {
    let mut rt = runtime();
    rt.open_playground("motion-count", None);
    assert_eq!(n(&rt), Some(1), "opening runs 'r' once");
    handler(&mut rt, |tx| tx.playground_trigger('s'));
    assert_eq!(n(&rt), Some(101));
    handler(&mut rt, |tx| tx.replay_playground());
    assert_eq!(n(&rt), Some(1), "replay makes the stage again, then runs 'r'");
    handler(&mut rt, |tx| tx.playground_trigger('S'));
    handler(&mut rt, |tx| tx.playground_trigger('R'));
    assert_eq!(n(&rt), Some(1), "'r' is replay");
    handler(&mut rt, |tx| tx.playground_trigger('x'));
    assert_eq!(n(&rt), Some(1), "an unknown key does nothing");
}

#[test]
fn a_link_opens_then_replays_in_one_handler() {
    let mut rt = runtime();
    handler(&mut rt, |tx| {
        tx.open_playground_at("motion-count", None);
        tx.playground_trigger('s');
        tx.replay_playground();
    });
    assert_eq!(n(&rt), Some(1), "the replay applies to the story the link opened");
}

#[test]
fn closed_it_does_nothing() {
    let mut rt = runtime();
    handler(&mut rt, |tx| { tx.replay_playground(); tx.playground_trigger('s'); });
    assert_eq!(n(&rt), None);
}
