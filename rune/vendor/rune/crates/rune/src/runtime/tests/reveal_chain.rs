//! Streamed words fade in reading order across texts: a chunk that spans several paragraphs or
//! list items fades them one after another, so the fading edge is only ever on the newest words.
use super::*;

#[derive(Default)]
struct Doc {
    paras: Vec<String>,
}

fn view(s: &Doc, _: &mut Cx<Doc>) -> El {
    col().w_full().children(s.paras.iter().enumerate().map(|(i, p)| crate::text(p.clone()).wrap().w_full().reveal(0.3).key(format!("p{i}"))))
}

fn words(n: usize) -> String {
    (0..n).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ")
}

/// Each paragraph's (first, last) word start time.
fn times(rt: &Runtime<Doc>, key: &str) -> Option<(f64, f64)> {
    let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some(key))?.id;
    let w = &rt.reveals.get(&id)?.words;
    Some((w.first()?.1, w.last()?.1))
}

fn runtime() -> Runtime<Doc> {
    let mut rt = Runtime::new(Doc { paras: vec![String::new()] }, view, (600.0, 800.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt
}

#[test]
fn one_chunk_over_three_paragraphs_fades_them_in_order() {
    let mut rt = runtime();
    rt.state.paras = vec![words(12), words(12), words(12)];
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let [a, b, c] = ["p0", "p1", "p2"].map(|k| times(&rt, k).unwrap_or_else(|| panic!("{k} reveals")));
    assert!(b.0 > a.1 && c.0 > b.1, "each paragraph starts after the one above ends: {a:?} {b:?} {c:?}");
}

#[test]
fn a_new_paragraph_waits_for_the_last_ones_tail() {
    let mut rt = runtime();
    rt.state.paras = vec![words(30)];
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let a = times(&rt, "p0").unwrap();
    rt.state.paras.push(words(4));
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let b = times(&rt, "p1").unwrap();
    assert!(b.0 > a.1, "the next paragraph starts after the last word above: {a:?} then {b:?}");
}

#[test]
fn once_everything_has_shown_a_new_paragraph_starts_at_once() {
    let mut rt = runtime();
    rt.state.paras = vec![words(8)];
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
    rt.state.paras.push(words(4));
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let (first, _) = times(&rt, "p1").unwrap();
    assert!((first - rt.t).abs() < 0.02, "no wait once nothing is fading: starts {first}, now {}", rt.t);
}

/// Texts mounting together behind a run longer than a fade (a remount, a restored turn) fade
/// together, as before, rather than one after another through still frames.
#[test]
fn texts_behind_a_long_run_fade_together() {
    let mut rt = runtime();
    rt.state.paras = vec![words(120), words(120), words(120)];
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let [a, b, c] = ["p0", "p1", "p2"].map(|k| times(&rt, k).unwrap_or_else(|| panic!("{k} reveals")));
    assert!(a.1 - a.0 > 0.3, "the first paragraph's run outlasts a fade: {a:?}");
    assert!(b.0 < a.1 && c.0 < a.1, "the rest don't wait for it: {a:?} {b:?} {c:?}");
}
