//! An app's tests driving its playground headless: open a story, run its triggers and their
//! `Play::after` steps on the logical clock, and read the stage's state and elements.
use rune::{App, AppSpec, Cx, El, Env, Play, Playground, Runtime, Story, Tx, col, el, text};

const DT: f64 = 1.0 / 60.0;

#[derive(Default)]
struct Pad {
    count: u32,
}

fn pad(s: &Pad, _: &mut Cx<Pad>) -> El {
    col().w_full().h_full()
        .child(text(format!("count {}", s.count)).key("count"))
        .child(el().key("inc").size(40.0, 20.0).on_press(|s: &mut Pad| s.count += 1))
}

static SCENES: &[(&str, fn(&mut Pad))] = &[("Pad/Empty", |_| {}), ("Pad/Full", |s| s.count = 10)];

#[derive(Default)]
struct Upload {
    bytes: u32,
    phase: &'static str,
}

fn upload(u: &Upload, _: &mut Cx<Upload>) -> El {
    text(format!("{} {}", u.phase, u.bytes)).key("upload")
}

fn upload_story() -> Story<Upload> {
    Story::new("Progress/Upload", Upload::default, upload)
        .trigger('r', "Run", |u, play| {
            u.phase = "sending";
            play.after(0.5, |u, play| {
                u.phase = "sent";
                play.after(0.25, |u, _| u.phase = "done");
            });
        })
        .trigger('f', "Fail", |u, _| u.phase = "failed")
        .variant("Small", |u| u.bytes = 10)
        .variant("Large", |u| u.bytes = 1_000_000)
}

fn playground() -> Playground<Pad> {
    Playground::<Pad>::new()
        .story(upload_story())
        .story(Story::from_real("Progress/Mine", |real: &Pad| Upload { bytes: real.count, phase: "mine" }, upload))
        .trigger("Full", 'd', "Double", |s, _| s.count *= 2)
        .variant("Full", "Few", |s| s.count = 3)
        .variant("Full", "Many", |s| s.count = 99)
}

fn runtime(playground: Playground<Pad>) -> Runtime<Pad> {
    let mut spec = AppSpec::new("pad", (800.0, 500.0), Pad::default, pad);
    spec.scenes = SCENES;
    App::new(spec).playground(playground).headless_runtime((800.0, 500.0), Env::new(false, 1.0))
}

/// Frames of `DT` until `secs` more have passed on the logical clock.
fn advance(rt: &mut Runtime<Pad>, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        rt.frame(DT);
    }
}

fn phase(rt: &Runtime<Pad>) -> &'static str {
    rt.playground_state::<Upload>().expect("the upload story's state").phase
}

#[test]
fn a_story_opens_by_path_and_its_r_trigger_runs_at_once() {
    let mut rt = runtime(playground());
    assert!(rt.playground_state::<Upload>().is_none(), "closed until opened");
    rt.open_playground("Progress/Upload", None);
    assert_eq!(phase(&rt), "sending");
    assert_eq!(rt.playground_element("upload").and_then(|e| e.text).as_deref(), Some("sending 10"), "on its first variant");
    assert!(rt.playground_state::<Pad>().is_none(), "the stage's type is the story's own");
}

#[test]
fn play_after_steps_run_on_the_logical_clock() {
    let mut rt = runtime(playground());
    rt.open_playground("Upload", None);
    advance(&mut rt, 0.4);
    assert_eq!(phase(&rt), "sending");
    advance(&mut rt, 0.2);
    assert_eq!(phase(&rt), "sent", "half a second after the trigger");
    advance(&mut rt, 0.1);
    assert_eq!(phase(&rt), "sent");
    advance(&mut rt, 0.1);
    assert_eq!(phase(&rt), "done", "a quarter second after the step that queued it");
}

#[test]
fn a_trigger_runs_by_its_key_and_r_replays() {
    let mut rt = runtime(playground());
    rt.open_playground("Progress/Upload", None);
    advance(&mut rt, 1.0);
    assert_eq!(phase(&rt), "done");
    rt.playground_trigger('f');
    assert_eq!(phase(&rt), "failed");
    rt.playground_trigger('r');
    assert_eq!(phase(&rt), "sending", "the stage made again, its 'r' run");
    advance(&mut rt, 0.6);
    assert_eq!(phase(&rt), "sent", "and its steps queued again");
}

#[test]
fn a_scene_trigger_acts_on_the_stage_and_never_the_real_state() {
    let mut rt = runtime(playground());
    rt.state.count = 4;
    rt.open_playground("Pad/Full", None);
    rt.playground_trigger('d');
    assert_eq!(rt.playground_state::<Pad>().map(|s| s.count), Some(6), "on its first variant, Few");
    assert_eq!(rt.playground_element("count").and_then(|e| e.text).as_deref(), Some("count 6"));
    assert_eq!(rt.state.count, 4, "the real state is untouched");
}

#[test]
fn a_variant_opens_with_its_own_setup() {
    let mut rt = runtime(playground());
    rt.open_playground("Progress/Upload", Some("Large"));
    assert_eq!(rt.playground_state::<Upload>().map(|u| u.bytes), Some(1_000_000));
    rt.open_playground("Progress/Upload", Some("1"));
    assert_eq!(rt.playground_state::<Upload>().map(|u| u.bytes), Some(10), "by number, and made again");
    rt.open_playground("Full", Some("many"));
    assert_eq!(rt.playground_state::<Pad>().map(|s| s.count), Some(99), "the scene fn, then the variant's");
}

#[test]
fn a_story_from_real_reads_the_real_state_and_leaves_it() {
    let mut rt = runtime(playground());
    rt.state.count = 7;
    rt.open_playground("Progress/Mine", None);
    assert_eq!(rt.playground_state::<Upload>().map(|u| u.bytes), Some(7));
    assert_eq!(rt.state.count, 7);
}

fn compare(stage: &Pad, real: &Pad, _: &mut Cx<Pad>) -> El {
    col()
        .child(text(format!("stage {} real {}", stage.count, real.count)).key("compare"))
        .child(el().key("keep").size(40.0, 20.0).on_press_tx(|_: &mut Pad, tx: &mut Tx| tx.keep_playground()))
}

#[test]
fn only_keep_from_a_side_real_control_writes_the_real_state() {
    let playground = playground().keep(|stage, real| real.count = stage.count).side_real("Full", compare);
    let mut rt = runtime(playground);
    rt.open_playground("Full", None);
    rt.playground_trigger('d');
    rt.frame(DT);
    assert_eq!(rt.state.count, 0, "the trigger changed the stage only");
    assert!(rt.playground_element("compare").is_none(), "the side column isn't the stage");
    let (x, y) = rt.find("keep").expect("the side column's control");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
    assert_eq!(rt.state.count, 6, "kept");
    let compare = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("compare")).and_then(|e| e.text);
    assert_eq!(compare.as_deref(), Some("stage 6 real 6"));
}

#[derive(Default)]
struct Pulse {
    beats: u32,
    settled: bool,
}

fn pulse_view(p: &Pulse, _: &mut Cx<Pulse>) -> El {
    text(format!("beats {}", p.beats)).key("pulse")
}

/// A loop: each beat queues the next.
fn beat(p: &mut Pulse, play: &mut Play<'_, Pulse>) {
    p.beats += 1;
    play.after(0.25, beat);
}

fn pulse() -> Story<Pulse> {
    Story::new("Motion/Pulse", Pulse::default, pulse_view)
        .trigger('r', "Pulse", beat)
        .trigger('s', "Settle", |_, play| {
            play.clear();
            play.after(0.2, |p, _| p.settled = true);
        })
}

#[test]
fn a_trigger_that_clears_stops_the_loop_and_runs_only_its_own_steps() {
    let mut rt = runtime(playground().story(pulse()));
    rt.open_playground("Pulse", None);
    advance(&mut rt, 1.1);
    let beats = |rt: &Runtime<Pad>| rt.playground_state::<Pulse>().map(|p| (p.beats, p.settled));
    assert_eq!(beats(&rt), Some((5, false)), "a beat every quarter second from the open");
    rt.playground_trigger('s');
    assert_eq!(beats(&rt), Some((5, false)), "clearing leaves the state alone");
    advance(&mut rt, 0.1);
    assert_eq!(beats(&rt), Some((5, false)), "the loop's next beat is gone");
    advance(&mut rt, 0.15);
    assert_eq!(beats(&rt), Some((5, true)), "the step queued after the clear ran");
    advance(&mut rt, 1.0);
    assert_eq!(beats(&rt), Some((5, true)));
    assert_eq!(rt.playground_element("pulse").and_then(|e| e.text).as_deref(), Some("beats 5"));
}

#[derive(Default)]
struct Glow {
    lit: bool,
}

#[test]
fn replays_land_their_steps_on_the_same_frame_every_time() {
    let glow = Story::new("Motion/Glow", Glow::default, |g: &Glow, _: &mut Cx<Glow>| text(if g.lit { "lit" } else { "dark" }).key("glow"))
        .trigger('r', "Glow", |g, play| {
            g.lit = false;
            play.after(0.4, |g, _| g.lit = true);
        });
    let mut rt = runtime(playground().story(glow));
    rt.open_playground("Motion/Glow", None);
    let dt = 1.0 / 120.0;
    let mut frames = Vec::new();
    for _ in 0..3 {
        rt.playground_trigger('r');
        let lit_on = (1..=120).find(|_| {
            rt.frame(dt);
            rt.playground_state::<Glow>().is_some_and(|g| g.lit)
        });
        frames.push(lit_on);
        for _ in 0..(120 - lit_on.unwrap_or(120)) {
            rt.frame(dt);
        }
    }
    assert_eq!(frames, vec![Some(48); 3], "0.4 s at 120 Hz is frame 48 on every replay");
}

#[test]
#[should_panic(expected = "no story \"Nope\" in the playground")]
fn an_unknown_story_panics_with_the_list() {
    runtime(playground()).open_playground("Nope", None);
}

#[test]
#[should_panic(expected = "has no trigger on 'z'")]
fn an_unknown_trigger_panics_with_the_keys() {
    let mut rt = runtime(playground());
    rt.open_playground("Progress/Upload", None);
    rt.playground_trigger('z');
}
