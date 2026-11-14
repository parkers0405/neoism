//! `Tx::keep_on_screen` on a header stuck to the top of a `stick_bottom` timeline: shutting
//! its group holds the header where it's drawn, and the fold and the scroll agree each frame.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el};

const STEPS: usize = 40;
const STEP_H: f32 = 28.0;
const HDR_H: f32 = 28.0;
/// Where the group starts in the timeline's content.
const GROUP_TOP: f32 = 200.0;

#[derive(Clone, Copy, Debug)]
struct Fold {
    open: bool,
    /// Steps still shown once shut.
    left: usize,
    /// The header stays sticky once shut (else, like a disclosure, only while open).
    sticky_shut: bool,
    /// Hidden steps leave the tree when shut (else they stay laid out under a box that ends at
    /// the last step still shown).
    drop_steps: bool,
    virtualized: bool,
}

impl Fold {
    fn new(sticky_shut: bool, drop_steps: bool, virtualized: bool) -> Self {
        Fold { open: true, left: 0, sticky_shut, drop_steps, virtualized }
    }
}

fn group(s: &Fold) -> El {
    let shown = if s.open { STEPS } else { s.left };
    let mut hdr = el().key("hdr").w_full().h(HDR_H);
    if shown > 0 || s.sticky_shut {
        hdr = hdr.sticky_top(0.0);
    }
    let mut g = col().key("group").w_full().child(hdr);
    let laid = if s.drop_steps { shown } else { STEPS };
    if laid > 0 {
        let steps = col().key("steps").w_full()
            .children((0..laid).map(|k| el().key(format!("step-{k}")).w_full().h(STEP_H)));
        let held = col().key("steps-box").w_full();
        g = g.child(if shown == laid { held } else { held.h(shown as f32 * STEP_H) }.child(steps));
    }
    g
}

fn row(s: &Fold, i: usize) -> El {
    match i {
        0 => el().key("before").w_full().h(GROUP_TOP),
        1 => col().key("turn").w_full().child(group(s)),
        2 => el().key("after").w_full().h(100.0),
        _ => el().key("tail").w_full().h(100.0),
    }
}

fn view(s: &Fold, _: &mut Cx<Fold>) -> El {
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
    if s.virtualized {
        let keys = ["before", "turn", "after", "tail"];
        stack = stack.virtual_rows(4, 100.0, move |_: &Fold, i| keys[i].into(), row);
    } else {
        for i in 0..4 { stack = stack.child(row(s, i)); }
    }
    col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
}

fn find(els: &[ElementInfo], key: &str) -> Option<ElementInfo> {
    els.iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).cloned()
}

/// How much of `e` shows between the header's bottom and the next row's top (both paint over
/// the steps), inside the viewport.
fn shows(e: &ElementInfo, from: f32, to: f32) -> f32 {
    let [_, y, _, h] = e.visible;
    ((y + h).min(to).min(200.0) - y.max(from)).max(0.0)
}

/// One frame: the header's top, the next row's top, and the steps that show with their tops.
type Frame = (f32, Option<f32>, Vec<(usize, f32)>);

/// Opens the group, scrolls `into` points past its header's place (so it sticks when that's
/// above the top), shuts the group down to `left` steps with `keep_on_screen("hdr")`, and
/// samples each frame for two seconds.
fn shut(init: Fold, into: f32, left: usize) -> Vec<Frame> {
    let mut rt = Runtime::new(init, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(move |_, tx| tx.scroll_to_instant("scroll", 0.0, GROUP_TOP + into));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let hdr = find(&rt.elements(), "hdr").unwrap();
    assert_eq!(hdr.rect[1], (-into).max(0.0), "{init:?}");
    assert!((hdr.stuck - into.max(0.0)).abs() < 0.5, "{init:?}: stuck {}", hdr.stuck);
    let sample = |rt: &Runtime<Fold>| -> Frame {
        let els = rt.elements();
        let hdr = find(&els, "hdr").map_or(f32::NAN, |h| h.rect[1]);
        let after = find(&els, "after").map(|a| a.rect[1]);
        let to = after.unwrap_or(f32::INFINITY);
        let steps = (0..STEPS)
            .filter_map(|k| find(&els, &format!("step-{k}")).filter(|e| shows(e, hdr + HDR_H, to) > 0.5).map(|e| (k, e.rect[1])))
            .collect();
        (hdr, after, steps)
    };
    let mut frames = vec![sample(&rt)];
    rt.update(move |s, tx| { s.open = false; s.left = left; tx.keep_on_screen("hdr"); });
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
        frames.push(sample(&rt));
    }
    frames
}

/// The first way `frames` breaks the hold of a header drawn at `at` whose group ends with the
/// next row at `after`, if any.
fn first_break(frames: &[Frame], at: f32, after: f32) -> Option<String> {
    for (f, w) in frames.windows(2).enumerate() {
        let ((h0, a0, s0), (h1, a1, s1)) = (&w[0], &w[1]);
        if (h1 - at).abs() >= 0.5 { return Some(format!("frame {f}: the header moved from {h0} to {h1}")); }
        // The steps under the header stay put while the fold covers them: none flashes in.
        for (k, y) in s1 {
            let was = s0.iter().find(|(j, _)| j == k).map(|s| s.1);
            if !was.is_some_and(|was| (y - was).abs() < 0.5) {
                return Some(format!("frame {f}: step {k} showed at {y} (was {was:?}; first step before {:?})", s0.first()));
            }
        }
        // The rows below only rise, at the growth spring's pace, never dipping first.
        if let (Some(a0), Some(a1)) = (a0, a1) && !(-12.0..0.5).contains(&(a1 - a0)) {
            return Some(format!("frame {f}: the row below went from {a0} to {a1}"));
        }
    }
    let (h, a, _) = frames.last().unwrap();
    if (*h, *a) != (at, Some(after)) {
        return Some(format!("lands with the header at {h} and the row below at {a:?}"));
    }
    None
}

fn check(into: f32, left: usize) {
    let mut breaks = vec![];
    for sticky_shut in [false, true] {
        for drop_steps in [false, true] {
            for virtualized in [false, true] {
                let init = Fold::new(sticky_shut, drop_steps, virtualized);
                let frames = shut(init, into, left);
                if std::env::var_os("KEEP_STUCK_TRACE").is_some() {
                    for (f, (h, a, s)) in frames.iter().enumerate() {
                        eprintln!("{init:?} {f}: hdr {h:.2} after {a:?} steps {:?}", s.first());
                    }
                }
                let at = (-into).max(0.0);
                let bottom = GROUP_TOP + HDR_H + left as f32 * STEP_H;
                let after = (bottom - GROUP_TOP - into).max(at + HDR_H);
                if let Some(b) = first_break(&frames, at, after) { breaks.push(format!("{init:?}: {b}")); }
            }
        }
    }
    assert!(breaks.is_empty(), "{}", breaks.join("\n"));
}

#[test]
fn shutting_from_a_stuck_header_holds_it_and_folds_the_rows_under_it() {
    // Stuck 500 points into the group, which ends 420 above the top once shut: the header
    // unsticks as its group shrinks, and the rows below rise to it before the scroll follows.
    check(500.0, 0);
}

#[test]
fn shutting_part_of_a_group_leaves_its_header_stuck() {
    // Twenty steps still reach 88 points below the header: it stays stuck and the scroll
    // doesn't move.
    check(500.0, 20);
}

#[test]
fn shutting_from_a_header_that_is_not_stuck_holds_it_as_before() {
    check(-40.0, 0);
}
