use super::*;
use crate::edit::{Key, KeyEvent};
use crate::{Cx, El, Env, Mods, Runtime, Tx, el};

struct S {
    y: f32,
    rewind: Rewind,
    scrub: Scrub,
    sent: Vec<Sent>,
    /// Where the scrubber's stage sits in the window.
    inset: f32,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let app = el().size(400.0, 300.0)
        .child(el().key("title").absolute(10.0, s.y).size(100.0, 20.0).fill(crate::Color::WHITE))
        .child(el().key("still").absolute(200.0, 200.0).size(40.0, 40.0).fill(crate::Color::WHITE));
    el().size(400.0, 300.0)
        .child(app.time_scale(if s.scrub.is_open() { 0.0 } else { 1.0 }))
        .child(el().absolute(s.inset, s.inset).size(400.0 - s.inset, 300.0 - s.inset)
            .child(s.scrub.view(|s: &mut S| &mut s.scrub, |s: &mut S, sent: Sent, _: &mut Tx| s.sent.push(sent))))
}

fn app(rewind: Rewind) -> Runtime<S> {
    let state = S { y: 0.0, rewind, scrub: Scrub::default(), sent: Vec::new(), inset: 0.0 };
    let mut rt = Runtime::new(state, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn record(rt: &mut Runtime<S>) {
    rt.update(|s, tx| tx.record_rewind(Some(&s.rewind)));
    rt.frame(1.0 / 60.0);
}

/// Moves the title to `y` at once.
fn jump(rt: &mut Runtime<S>, y: f32) {
    rt.update(|s, tx| { s.y = y; tx.instant(); });
}

fn frames(rt: &mut Runtime<S>, n: usize) {
    for _ in 0..n { rt.frame(1.0 / 60.0); }
}

fn press(key: Key, shift: bool) -> KeyEvent {
    KeyEvent { key, mods: Mods { shift, ..Mods::default() } }
}

#[test]
fn nothing_records_or_allocates_until_a_recorder_is_attached() {
    let mut rt = app(Rewind::new());
    frames(&mut rt, 5);
    assert!(rt.rewind().is_none());
    assert_eq!(rt.state.rewind.frames(), 0);
    let before = crate::tooling::test_alloc::allocs();
    for _ in 0..100 { rt.record_rewind(); }
    assert_eq!(crate::tooling::test_alloc::allocs(), before, "an unattached recorder costs a branch");
}

#[test]
fn a_poll_can_attach_the_recorder() {
    let mut rt = app(Rewind::new());
    rt.poll = Some(|s, _, tx| { if s.y == 0.0 { s.y = 1.0; tx.record_rewind(Some(&s.rewind)); true } else { false } });
    frames(&mut rt, 3);
    assert!(rt.rewind().is_some());
    assert!(rt.state.rewind.frames() > 0);
}

#[test]
fn records_each_frame_and_stops_when_detached() {
    let mut rt = app(Rewind::new());
    record(&mut rt);
    frames(&mut rt, 9);
    let n = rt.state.rewind.frames();
    assert!(n >= 10, "{n} frames");
    rt.update(|_, tx| tx.record_rewind(None));
    frames(&mut rt, 5);
    assert_eq!(rt.state.rewind.frames(), n, "nothing records once detached");
}

#[test]
fn the_ring_keeps_only_its_window_and_rebuilds_old_frames() {
    let mut rt = app(Rewind::new().seconds(1.0));
    record(&mut rt);
    frames(&mut rt, 30);
    jump(&mut rt, 40.0);
    frames(&mut rt, 300);
    let held = rt.state.rewind.frames();
    assert!((59..=62).contains(&held), "a second at 60 Hz, held {held}");
    // The frames that put both elements on screen are long gone; the base still has them.
    let clip = rt.state.rewind.freeze();
    let first = clip.frame(0);
    assert_eq!(first.len(), 2, "{first:?}");
    let title = first.iter().find(|(id, _)| clip.name(*id) == "title").unwrap().1;
    assert_eq!(title.rect, [10.0, 40.0, 100.0, 20.0]);
}

#[test]
fn the_ring_stays_within_its_byte_budget() {
    let budget = 4096;
    let mut rt = app(Rewind::new().max_bytes(budget));
    record(&mut rt);
    for k in 0..400 { jump(&mut rt, (k % 50) as f32); rt.frame(1.0 / 60.0); }
    assert!(rt.state.rewind.bytes() <= budget, "{} bytes", rt.state.rewind.bytes());
    assert!(rt.state.rewind.frames() > 10);
}

#[test]
fn a_one_frame_jump_is_found_and_labelled() {
    let mut rt = app(Rewind::new());
    record(&mut rt);
    frames(&mut rt, 10);
    jump(&mut rt, 7.5);
    frames(&mut rt, 10);
    let clip = rt.state.rewind.freeze();
    let k = (0..clip.len()).find(|&k| clip.has_jump(k)).expect("a jump");
    let changes = clip.changes(k);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(changes[0].jump);
    assert_eq!(changes[0].label(), "title ↓7.5 pt");
    assert!((clip.activity(k) - 7.5).abs() < 0.01);
    assert_eq!(clip.activity(k + 1), 0.0, "nothing moves after");
    let path = clip.path(changes[0].id, 0..clip.len());
    assert_eq!(path.first().unwrap().1.rect[1], 0.0);
    assert_eq!(path.last().unwrap().1.rect[1], 7.5);
}

#[test]
fn a_one_frame_blip_out_and_back_is_a_jump() {
    let mut rt = app(Rewind::new());
    record(&mut rt);
    frames(&mut rt, 10);
    jump(&mut rt, 9.0);
    rt.frame(1.0 / 60.0);
    jump(&mut rt, 0.0);
    frames(&mut rt, 10);
    let clip = rt.state.rewind.freeze();
    let jumps: Vec<usize> = (0..clip.len()).filter(|&k| clip.has_jump(k)).collect();
    assert_eq!(jumps.len(), 1, "only the frame it went out in: {jumps:?}");
    assert_eq!(clip.changes(jumps[0])[0].label(), "title ↓9.0 pt");
}

#[test]
fn smooth_motion_is_not_a_jump() {
    let mut rt = app(Rewind::new());
    record(&mut rt);
    for k in 1..=30 { jump(&mut rt, k as f32 * 3.0); rt.frame(1.0 / 60.0); }
    let clip = rt.state.rewind.freeze();
    assert!((0..clip.len()).all(|k| !clip.has_jump(k)));
    assert!((0..clip.len()).filter(|&k| clip.activity(k) > 0.0).count() >= 29);
}

#[test]
fn freezing_pauses_recording_and_the_keys_step_select_and_resume() {
    let mut rt = app(Rewind::new());
    record(&mut rt);
    frames(&mut rt, 10);
    jump(&mut rt, 20.0);
    frames(&mut rt, 10);
    let rewind = rt.state.rewind.clone();
    rt.state.scrub.open(&rewind);
    let scrub = &mut rt.state.scrub;
    assert!(scrub.is_open() && rewind.is_paused());
    let held = rewind.frames();
    let clip = scrub.clip().unwrap().clone();
    // It opens on the last frame anything moved in: the jump.
    let k = scrub.frame();
    assert!(clip.has_jump(k));
    assert!(matches!(scrub.key(&press(Key::Left, false)), Keyed::Handled));
    assert_eq!(scrub.frame(), k - 1);
    scrub.key(&press(Key::Right, true));
    scrub.key(&press(Key::Right, true));
    assert_eq!(scrub.selection(), k - 1..k + 2);
    let Keyed::Send(sent) = scrub.key(&press(Key::Enter, false)) else { panic!("Return sends") };
    assert_eq!(sent.frames(), k - 1..k + 2);
    frames(&mut rt, 5);
    assert_eq!(rewind.frames(), held, "nothing records while frozen");
    assert!(matches!(rt.state.scrub.key(&press(Key::Escape, false)), Keyed::Closed));
    assert!(!rt.state.scrub.is_open() && !rewind.is_paused());
    frames(&mut rt, 5);
    assert_eq!(rewind.frames(), held + 5);
}

#[test]
fn the_scrubber_draws_the_moved_element_and_sends_from_its_button() {
    let mut rt = app(Rewind::new());
    record(&mut rt);
    frames(&mut rt, 5);
    jump(&mut rt, 30.0);
    frames(&mut rt, 5);
    rt.update(|s, _| { let r = s.rewind.clone(); s.scrub.open(&r); s.scrub.set_note("title jumps"); });
    rt.frame(1.0 / 60.0);
    let els = rt.elements();
    let find = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect;
    let id = rt.state.scrub.clip().unwrap().changes(rt.state.scrub.frame())[0].id;
    assert_eq!(find(&format!("rewind-box-{id}")), [10.0, 30.0, 100.0, 20.0]);
    assert_eq!(find(&format!("rewind-was-{id}")), [10.0, 0.0, 100.0, 20.0]);
    assert!(els.iter().any(|e| e.key.as_deref().is_some_and(|k| k.starts_with("rewind-bin-"))), "the track lights up");
    let (x, y) = rt.find("rewind-send").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.sent.len(), 1, "send at {x},{y}: {:?}", rt.elements().iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("rewind-"))).map(|e| (e.key.clone(), e.rect)).collect::<Vec<_>>());
    assert_eq!(rt.state.sent[0].note(), "title jumps");
}

#[test]
fn a_scrubber_on_an_inset_stage_still_outlines_where_the_window_drew() {
    let mut rt = app(Rewind::new());
    rt.update(|s, tx| { s.inset = 50.0; tx.instant(); });
    record(&mut rt);
    frames(&mut rt, 5);
    jump(&mut rt, 30.0);
    frames(&mut rt, 5);
    rt.update(|s, _| { let r = s.rewind.clone(); s.scrub.open(&r); });
    frames(&mut rt, 2);
    let id = rt.state.scrub.clip().unwrap().changes(rt.state.scrub.frame())[0].id;
    let rect = rt.elements().iter().find(|e| e.key.as_deref() == Some(&format!("rewind-box-{id}"))).unwrap().rect;
    assert_eq!(rect, [10.0, 30.0, 100.0, 20.0]);
    let bar = rt.elements().iter().find(|e| e.key.as_deref() == Some("rewind-bar")).map(|e| e.rect);
    assert!(bar.is_none_or(|b| b[0] >= 50.0), "the bar stays on the stage: {bar:?}");
}

#[test]
fn a_bundle_holds_a_trace_a_contact_sheet_and_a_summary() {
    let mut rt = app(Rewind::new());
    record(&mut rt);
    frames(&mut rt, 10);
    jump(&mut rt, 7.5);
    frames(&mut rt, 10);
    let clip = rt.state.rewind.freeze();
    let dir = std::env::temp_dir().join(format!("rune-rewind-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bundle = clip.write_bundle(&dir, 0..clip.len(), "the title hops").unwrap();
    let trace = std::fs::read_to_string(dir.join("trace.jsonl")).unwrap();
    let mut lines = trace.lines();
    assert!(lines.next().unwrap().contains(r#""type":"header""#));
    let first = lines.next().unwrap();
    assert!(first.contains(r#""nm":"title""#) && first.contains(r#""nm":"still""#), "{first}");
    assert_eq!(trace.lines().count(), clip.len() + 1);
    assert!(trace.contains(",10,7.5,100,20,1,0,0]"), "the jump frame: {trace}");
    let png = std::fs::read(dir.join("contact.png")).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let summary = std::fs::read_to_string(dir.join("summary.md")).unwrap();
    assert_eq!(summary, bundle.summary());
    assert!(summary.contains("> the title hops"));
    assert!(summary.contains("## Jumps") && summary.contains("title ↓7.5 pt"), "{summary}");
    let _ = std::fs::remove_dir_all(&dir);
}
