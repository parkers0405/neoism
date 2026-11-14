//! A reply streaming into a pinned timeline at any pace: the climb is steady, not one kick per
//! line or per word.
use super::*;
use crate::el::{GrowthSpec, ScrollSpec};

const WORDS: &[&str] = &[
    "the", "follower", "keeps", "pace", "with", "whatever", "arrives,", "so", "a", "slow", "reply",
    "creeps", "steadily", "and", "a", "fast", "one", "keeps", "up", "without", "kicking", "for",
    "each", "line;", "words", "fade", "in", "under", "the", "edge", "as", "they", "come,", "and",
    "nothing", "stops", "dead", "until", "the", "stream", "pauses.",
];

#[derive(Default)]
struct Chat {
    text: String,
    streaming: bool,
}

fn view(s: &Chat, _: &mut Cx<Chat>) -> El {
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
    for i in 0..12 {
        stack = stack.child(el().key(format!("r{i}")).w_full().h(60.0));
    }
    stack = stack.child(col().key("reply").w_full().streaming(s.streaming)
        .child(crate::el::text(s.text.clone()).wrap().w_full().reveal(0.18)));
    col().w_full().h_full().child(col().key("scroll").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: true }).child(stack))
}

/// When each word arrives, in seconds.
fn pace(name: &str) -> Vec<f64> {
    let n = 160;
    match name {
        "slow" | "slow, wide" => (0..n).map(|i| i as f64 * 0.3).collect(),
        "medium" => (0..n).map(|i| i as f64 * 0.05).collect(),
        "fast" => (0..n).map(|i| (i / 6) as f64 / 60.0).collect(),
        // Bursts of 4–12 words a few ms apart, then pauses of 0.1–0.9 s, as real models stream.
        _ => {
            let (mut t, mut out, mut seed) = (0.0, Vec::new(), 7u32);
            let mut rand = || { seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345); (seed >> 16) as f64 / 65_536.0 };
            while out.len() < n {
                for _ in 0..4 + (rand() * 9.0) as usize { out.push(t); t += 0.012; }
                t += 0.1 + rand() * 0.8;
            }
            out.truncate(n);
            out
        }
    }
}

/// The first row's top each frame at 120 Hz while words arrive on `at`, the stream ending with
/// the last, then 1.5 s after, and how far the reply's drawn height trails its text.
fn climb(at: &[f64], width: f32) -> (Vec<f32>, Vec<f32>) {
    let mut rt = Runtime::new(Chat { text: String::new(), streaming: true }, view, (width, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    let top = |rt: &Runtime<Chat>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r0")).unwrap().rect[1];
    let reply = rt.nodes.iter().find(|n| n.key.as_deref() == Some("reply")).unwrap().id;
    let (mut ys, mut lags, mut next, mut t) = (Vec::new(), Vec::new(), 0, 0.0);
    let end = at.last().copied().unwrap_or(0.0) + 1.5;
    while t < end {
        t += 1.0 / 120.0;
        while next < at.len() && at[next] <= t {
            if !rt.state.text.is_empty() { rt.state.text.push(' '); }
            rt.state.text.push_str(WORDS[next % WORDS.len()]);
            next += 1;
            rt.state.streaming = next < at.len();
            rt.invalidate();
        }
        rt.frame(1.0 / 120.0);
        ys.push(top(&rt));
        lags.push(rt.grows.get(&reply).map_or(0.0, |g| g.target - g.h));
    }
    (ys, lags)
}

/// Once a reply overflows, a pinned timeline climbs without resting until its last word lands,
/// however fast or unevenly the words come: no stop and kick per line or per word. A steady
/// stream trails its text by under a line, and the last of it lands within half a second.
#[test]
fn a_stream_climbs_steadily_at_any_pace() {
    // A wide window's last line fills, and its reveal rises, a fraction of a point a word.
    for name in ["slow", "slow, wide", "medium", "fast", "bursty"] {
        let at = pace(name);
        let (ys, lags) = climb(&at, if name.ends_with("wide") { 1000.0 } else { 420.0 });
        let steps: Vec<f32> = ys.windows(2).map(|w| w[0] - w[1]).collect();
        // (Drawn rects move in 1/128 pt steps, so a climb has to be underway to show each frame.)
        let first = steps.iter().position(|&d| d > 0.02).expect("the reply overflows");
        let last = (at[at.len() - 1] * 120.0) as usize;
        let rests: Vec<usize> = (first..last).filter(|&k| steps[k] < 0.005).collect();
        assert!(rests.is_empty(), "{name}: rests {} times, first at frame {:?}", rests.len(), rests.first());
        if name != "fast" {
            let most = lags.iter().copied().fold(0.0, f32::max);
            assert!(most < 20.0, "{name}: trails its text by {most} pt");
        }
        let landed = last + 60;
        assert!(lags[landed..].iter().all(|&l| l < 0.05), "{name}: still {} pt short 0.5 s after the last word", lags[landed]);
    }
}
