use std::cell::RefCell;
use std::time::Duration;

use super::names::normalize;
use super::rollup::Rollup;
use super::*;
use crate::{Cx, El, Env, Ime, Runtime, TextEdit, TextStyle, col, el, text_input};

thread_local! {
    pub(super) static SENT: RefCell<Vec<Slow>> = const { RefCell::new(Vec::new()) };
    pub(super) static FRAMES: RefCell<Vec<SlowFrame>> = const { RefCell::new(Vec::new()) };
    pub(super) static OVERLOADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn norm(path: &str, keep: usize) -> String {
    let mut out = String::new();
    normalize(path, keep, &mut out);
    out
}

#[test]
fn names_drop_unkeyed_segments_ids_and_prose() {
    assert_eq!(norm("#0/sidebar/row-x1208", 4), "sidebar/row-x*");
    assert_eq!(norm("#0/#2/list/item-3f9a2c41d7e0/open", 4), "list/item-*/open");
    assert_eq!(norm("#0/a/b/c/d/e", 4), "b/c/d/e");
    assert_eq!(norm("#0/notes/Buy milk tomorrow", 4), "notes/~");
    assert_eq!(norm("#0/k-palette", 4), "k-palette");
    assert_eq!(norm("#0/a\u{e9}b", 4), "a_b");
    assert_eq!(norm(&format!("#0/{}", "x".repeat(50)), 4).len(), 32);
}

#[test]
fn names_are_interned() {
    let mut names = Names::default();
    let a = names.control("#0/list/row-1/open");
    let b = names.control("#0/list/row-22/open");
    assert_eq!(a.hash, b.hash);
    assert!(Arc::ptr_eq(&a.text, &b.text), "the second sighting reuses the first's text");
    assert_eq!(&*names.route(Some("#0/settings"), Some("#0/settings/confirm-7")).text, "settings+modal:confirm-*");
    assert_eq!(&*names.route(None, None).text, "root");
}

#[test]
fn the_gate_wants_perf_or_a_loopback_collector() {
    use gate::{loopback, wanted};
    assert!(wanted(Some("1"), None, true), "RUNE_PERF=1 turns it on, headless too");
    assert!(!wanted(Some("0"), Some("http://127.0.0.1:27686"), false), "RUNE_PERF=0 wins");
    assert!(wanted(None, Some("http://127.0.0.1:27686"), false));
    assert!(wanted(None, Some("http://localhost:4318/"), false));
    assert!(!wanted(None, Some("http://127.0.0.1:27686"), true), "a headless run needs RUNE_PERF=1");
    assert!(!wanted(None, Some("https://otel.example.com:4318"), false), "a remote collector isn't a dev Mac");
    assert!(!wanted(None, None, false), "a user's Mac has neither");
    assert_eq!(loopback("[::1]:4318").map(|a| a.port()), Some(4318));
    assert_eq!(gate::endpoint(None).map(|a| a.to_string()).as_deref(), Some(gate::DEFAULT_ENDPOINT));
}

#[test]
fn rollup_quantiles_land_within_a_bucket() {
    let mut names = Names::default();
    let (route, control) = (names.fixed("root"), names.fixed("save"));
    let mut rollup = Rollup::default();
    for i in 1..=100 { rollup.add(&route, &control, "on_press", i as f64, i > 98, false); }
    let rolls = rollup.take();
    assert_eq!(rolls.len(), 1);
    let r = &rolls[0];
    assert_eq!((r.count, r.slow, r.max_ms), (100, 2, 100.0));
    assert!((r.quantile(0.5) - 50.0).abs() / 50.0 < 0.16, "p50 {}", r.quantile(0.5));
    assert!((r.quantile(0.98) - 98.0).abs() / 98.0 < 0.16, "p98 {}", r.quantile(0.98));
    assert!(rollup.is_empty());
}

#[derive(Default)]
struct App {
    note: TextEdit,
    presses: u32,
    slow: bool,
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    let list = (0..30).fold(col().key("list"), |c, i| c.child(el().key(format!("row-{i}")).h(4.0)));
    col().key("settings").w_full().h_full()
        .child(el().key("save-42").size(80.0, 30.0).on_press(|s: &mut App| {
            s.presses += 1;
            if s.slow { std::thread::sleep(Duration::from_millis(25)); }
        }))
        .child(text_input(&s.note, TextStyle::new(14.0, 400.0)).single_line().key("note").size(200.0, 24.0).on_edit(|s: &mut App| &mut s.note))
        .child(el().key("count").child(crate::text(format!("{}", s.presses))))
        .child(list)
}

fn runtime(slow: bool) -> Runtime<App> {
    force_on();
    SENT.take();
    let mut rt = Runtime::new(App { slow, ..App::default() }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn force_on() { gate::force_on(); }

fn s_route(rt: &mut Runtime<App>) -> Name { rt.interaction_tracker().names.route(Some("#0/settings"), None) }
fn s_control(rt: &mut Runtime<App>) -> Name { rt.interaction_tracker().names.control("#0/settings/save-42") }

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.interaction_presented(Instant::now(), None);
}

#[test]
fn a_slow_click_names_its_page_and_control() {
    let mut rt = runtime(true);
    click(&mut rt, "save-42");
    let sent = SENT.take();
    assert_eq!(sent.len(), 1, "one slow interaction");
    let s = &sent[0];
    assert_eq!((&*s.route, &*s.control, s.action), ("settings", "settings/save-*", "on_press"));
    assert!(s.handler_ms >= 25.0, "the handler's sleep is its time: {}", s.handler_ms);
    let note = Note { route: s_route(&mut rt), control: s_control(&mut rt), action: "on_press", rank: 3 };
    assert!(rt.interaction_tracker().breached(&note), "a slow signature is sampled sooner next time");
    assert!(s.commits >= 1 && s.why & crate::runtime::CommitWhy::INPUT != 0);
    assert!(s.phases[Phase::View as usize] > 0.0, "the view phase timed itself");
    assert!(s.present >= s.submitted && s.submitted >= s.built && s.built >= s.frame_start && s.frame_start >= s.arrived);
    assert_eq!(s.rebuilt.first().map(|r| &*r.0), Some("settings"), "nothing is memoized: the whole page rebuilt");
    assert!(!active(), "phases stop timing once the frame is built");
}

#[test]
fn a_slow_click_says_whose_time_it_took() {
    let mut rt = runtime(true);
    click(&mut rt, "save-42");
    let s = SENT.take().pop().unwrap();
    assert_eq!(s.cause, Cause::Blocked, "the handler slept: little CPU on an idle machine");
    assert!(s.cpu_ms.is_some_and(|cpu| cpu < 25.0), "the frame's CPU, without the sleep: {:?}", s.cpu_ms);
    let rolls = rt.interaction_tracker().rollup.take();
    assert_eq!((rolls[0].slow, rolls[0].contended), (1, 0));

    OVERLOADED.set(true);
    let mut rt = runtime(true);
    click(&mut rt, "save-42");
    OVERLOADED.set(false);
    assert_eq!(SENT.take().pop().unwrap().cause, Cause::SchedulerWait, "on a busy machine, the same frame is the machine's");
    let tracker = rt.interaction_tracker();
    let rolls = tracker.rollup.take();
    assert_eq!((rolls[0].slow, rolls[0].contended), (0, 1), "not counted against the app");
    assert!(tracker.breached.is_empty(), "nor sampled sooner next time");
}

#[test]
fn a_fast_click_only_counts() {
    let mut rt = runtime(false);
    click(&mut rt, "save-42");
    assert!(SENT.take().is_empty());
    let rolls = rt.interaction_tracker().rollup.take();
    assert_eq!(rolls.len(), 1);
    assert_eq!((&*rolls[0].control.text, rolls[0].count), ("settings/save-*", 1));
}

#[test]
fn typing_in_a_field_says_key_edit_never_the_text() {
    let mut rt = runtime(false);
    click(&mut rt, "note");
    rt.interaction_tracker().rollup.take();
    rt.ime(Ime::Commit("secret".into()));
    rt.frame(1.0 / 60.0);
    rt.interaction_presented(Instant::now(), None);
    let rolls = rt.interaction_tracker().rollup.take();
    assert_eq!(rolls.iter().map(|r| (&*r.control.text, r.action)).collect::<Vec<_>>(), [("settings/note", "key_edit")]);
}

#[test]
fn a_frame_no_host_presents_ends_at_its_build() {
    let mut rt = runtime(true);
    let (x, y) = rt.find("save-42").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(SENT.take().is_empty(), "waits for the host");
    rt.frame(1.0 / 60.0);
    let sent = SENT.take();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].present, sent[0].built);
}

#[test]
fn off_nothing_is_timed() {
    SENT.take();
    let mut rt = Runtime::new(App { slow: true, ..App::default() }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, "save-42");
    assert!(SENT.take().is_empty());
    let tracker = rt.interaction_tracker();
    assert!(tracker.rollup.is_empty() && tracker.timed.is_none());
}

#[test]
fn slow_spans_carry_the_signature_and_stretches() {
    let mut rt = runtime(true);
    click(&mut rt, "save-42");
    let slow = SENT.take().pop().unwrap();
    let spans = export::slow_spans(&slow);
    let names: Vec<_> = spans.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names[0], "ui.interaction");
    assert!(names.contains(&"update"));
    let attr = |key: &str| spans[0]["attributes"].as_array().unwrap().iter().find(|a| a["key"] == key).map(|a| a["value"].clone());
    assert_eq!(attr("rune.route").unwrap()["stringValue"], "settings");
    assert_eq!(attr("rune.control").unwrap()["stringValue"], "settings/save-*");
    let root = spans[0]["spanId"].clone();
    assert!(spans[1..].iter().all(|s| s["parentSpanId"] == root && s["traceId"] == spans[0]["traceId"]));
}
