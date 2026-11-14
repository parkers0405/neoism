use super::*;
use crate::{GrowthSpec, ScrollSpec, col, el};

/// A chat's rows, top to bottom, with their heights: messages, the last bubble `p` (36 pt) and a
/// "working" row `w` (77 pt).
type Rows = Vec<(&'static str, f32)>;

fn chat_rows() -> Rows {
    vec![("m0", 120.0), ("m1", 60.0), ("m2", 90.0), ("p", 36.0), ("w", 77.0)]
}

fn chat(rows: &Rows, cx: &mut Cx<Rows>) -> El {
    chat_in(rows, cx, true)
}

fn chat_in(rows: &Rows, _: &mut Cx<Rows>, pinned: bool) -> El {
    let stack = col().key("stack").w_full().padding(12.0).gap(8.0).growth(GrowthSpec::default())
        .children(rows.iter().map(|&(k, h)| el().key(k).w_full().h(h).fill(Color::WHITE)));
    col().w_full().h_full()
        .child(col().key("scroll").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: pinned }).child(stack))
}

fn settled(rows: Rows) -> Runtime<Rows> {
    let mut rt = Runtime::new(rows, chat, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    assert!(!rt.animating);
    rt
}

fn y(rt: &Runtime<Rows>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap().rect[1]
}

fn offset_tracked(rt: &Runtime<Rows>, key: &str) -> bool {
    rt.nodes.iter().find(|n| n.path.ends_with(&format!("/{key}"))).is_some_and(|n| rt.scene.store().has(n.id, prop::OFFSET))
}

fn swap_last_two(rows: &mut Rows) {
    let n = rows.len();
    rows.swap(n - 2, n - 1);
}

/// Each row's top every frame for `frames` frames at 120 Hz.
fn trace(rt: &mut Runtime<Rows>, keys: &[&str], frames: usize) -> Vec<Vec<f32>> {
    (0..frames).map(|_| { rt.frame(1.0 / 120.0); keys.iter().map(|k| y(rt, k)).collect() }).collect()
}

/// Asserts `ys` leaves `from` without a jump, heads one way to `to` in steps no bigger than
/// `step`, and lands there.
fn slides(name: &str, from: f32, to: f32, ys: &[f32], step: f32) {
    let mut last = from;
    for (f, &y) in ys.iter().enumerate() {
        assert!((y - last).abs() <= step, "{name} jumps {} pt at frame {f}: {ys:?}", y - last);
        assert!((y - last) * (to - from) >= -0.01, "{name} heads back at frame {f}: {ys:?}");
        last = y;
    }
    assert!((ys[0] - from).abs() > 0.01, "{name} moves on the commit's frame: {ys:?}");
    assert!((last - to).abs() < 0.01, "{name} lands at {to}: {ys:?}");
}

#[test]
fn a_reordered_growth_row_and_the_row_it_passes_slide_into_place() {
    let mut rt = settled(chat_rows());
    let (p0, w0, m2) = (y(&rt, "p"), y(&rt, "w"), y(&rt, "m2"));
    rt.update(|rows, _| swap_last_two(rows));
    let frames = trace(&mut rt, &["p", "w", "m2"], 120);
    let col = |k: usize| frames.iter().map(|f| f[k]).collect::<Vec<_>>();
    slides("p", p0, p0 + 77.0 + 8.0, &col(0), 8.0);
    slides("w", w0, p0, &col(1), 8.0);
    assert!(col(2).iter().all(|&y| y == m2), "the rows above hold still: {:?}", col(2));
    assert!(!rt.animating);
    // Swapping back mid-slide turns both rows around from where they show.
    rt.update(|rows, _| swap_last_two(rows));
    let mid = trace(&mut rt, &["p", "w"], 12);
    let (p1, w1) = (mid[11][0], mid[11][1]);
    rt.update(|rows, _| swap_last_two(rows));
    let back = trace(&mut rt, &["p", "w"], 120);
    assert!((back[0][0] - p1).abs() <= 8.0 && (back[0][1] - w1).abs() <= 8.0, "no jump turning around: {:?} from {p1}, {w1}", back[0]);
    assert!((back[119][0] - (p0 + 85.0)).abs() < 0.01 && (back[119][1] - p0).abs() < 0.01, "{:?}", back[119]);
}

#[test]
fn an_instant_or_reduced_motion_reorder_lands_at_once() {
    for reduced in [false, true] {
        let mut rt = settled(chat_rows());
        rt.set_system_reduced_motion(reduced);
        let p0 = y(&rt, "p");
        rt.update(|rows, tx| { if !reduced { tx.instant(); } swap_last_two(rows) });
        rt.frame(1.0 / 120.0);
        assert_eq!((y(&rt, "w"), y(&rt, "p")), (p0, p0 + 85.0), "reduced={reduced}");
        assert!(!offset_tracked(&rt, "p") && !offset_tracked(&rt, "w"));
    }
}

#[test]
fn rows_reordered_out_of_sight_land_at_once() {
    // Read from the top, the last two rows are below the viewport before and after.
    let rows = vec![("m0", 400.0), ("p", 36.0), ("w", 77.0)];
    let mut rt = Runtime::new(rows, |rows, cx| chat_in(rows, cx, false), (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    let p0 = y(&rt, "p");
    assert!(p0 >= 400.0, "{p0}");
    rt.update(|rows, _| swap_last_two(rows));
    rt.frame(1.0 / 120.0);
    assert_eq!((y(&rt, "w"), y(&rt, "p")), (p0, p0 + 85.0));
    assert!(!offset_tracked(&rt, "p") && !offset_tracked(&rt, "w"));
}

#[test]
fn a_row_moved_past_others_slides_them_and_only_them() {
    let mut rt = settled(chat_rows());
    let before: Vec<f32> = ["m0", "m1", "m2", "p", "w"].iter().map(|k| y(&rt, k)).collect();
    // m1 moves below m2 and p; m0 and w keep their places.
    rt.update(|rows, _| { let r = rows.remove(1); rows.insert(3, r); });
    let frames = trace(&mut rt, &["m0", "m1", "m2", "p", "w"], 120);
    let col = |k: usize| frames.iter().map(|f| f[k]).collect::<Vec<_>>();
    slides("m1", before[1], before[1] + 90.0 + 36.0 + 16.0, &col(1), 12.0);
    slides("m2", before[2], before[2] - 68.0, &col(2), 8.0);
    slides("p", before[3], before[3] - 68.0, &col(3), 8.0);
    assert!(col(0).iter().all(|&y| y == before[0]) && col(4).iter().all(|&y| y == before[4]));
    assert!(!offset_tracked(&rt, "m0") && !offset_tracked(&rt, "w"), "rows nobody passed don't slide");
}

#[test]
fn rows_inserted_or_removed_above_a_row_move_it_on_growth_not_a_slide() {
    // Growth carries a visible row through an insert or a removal above it: no jump, and no slide.
    for insert in [true, false] {
        let mut rt = settled(chat_rows());
        let (p0, w0) = (y(&rt, "p"), y(&rt, "w"));
        rt.update(|rows, _| if insert { rows.insert(3, ("n", 50.0)) } else { rows.remove(2); });
        let frames = trace(&mut rt, &["p", "w"], 120);
        let mut last = (p0, w0);
        for f in &frames {
            assert!((f[0] - last.0).abs() <= 8.0 && (f[1] - last.1).abs() <= 8.0, "insert={insert}: {frames:?}");
            last = (f[0], f[1]);
        }
        assert!(!offset_tracked(&rt, "p") && !offset_tracked(&rt, "w"), "insert={insert}");
    }
}

#[test]
fn reorder_measures_only_rows_that_changed_order() {
    let ids: Vec<NodeId> = (1..=5).map(NodeId).collect();
    let h = |id: NodeId| Some(id.0 as f32 * 10.0);
    let mut r = Reorder::default();
    // Inserting, removing and keeping order: nothing moves.
    r.measure(true, &ids, &[ids[0], NodeId(9), ids[1], ids[3], ids[4]], 2.0, |id| if id.0 == 9 { Some(0.0) } else { h(id) });
    assert!(r.jumps.is_empty());
    // Moving the first row last passes every other row.
    r.measure(true, &ids, &[ids[1], ids[2], ids[3], ids[4], ids[0]], 2.0, h);
    assert_eq!(r.jump(ids[0]), -(20.0 + 30.0 + 40.0 + 50.0 + 4.0 * 2.0));
    assert_eq!(r.jump(ids[1]), 12.0);
    assert_eq!(r.jump(ids[4]), 12.0);
    // A commit that lands its changes measures nothing.
    r.measure(false, &ids, &[ids[1], ids[0], ids[2], ids[3], ids[4]], 2.0, h);
    assert!(r.jumps.is_empty());
}

/// 30 virtual rows of alternating heights, in a 300 pt viewport.
fn list(rows: &Rows, _: &mut Cx<Rows>) -> El {
    let stack = col().key("stack").w_full().gap(4.0).growth(GrowthSpec::default()).virtualize(40.0)
        .children(rows.iter().map(|&(k, h)| el().key(k).w_full().h(h).fill(Color::WHITE)));
    col().w_full().h_full()
        .child(col().key("scroll").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: false }).child(stack))
}

fn virtual_rows() -> Runtime<Rows> {
    const KEYS: [&str; 30] = ["v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "v10", "v11", "v12", "v13", "v14",
        "v15", "v16", "v17", "v18", "v19", "v20", "v21", "v22", "v23", "v24", "v25", "v26", "v27", "v28", "v29"];
    let rows = KEYS.iter().enumerate().map(|(i, &k)| (k, if i % 2 == 0 { 36.0 } else { 77.0 })).collect();
    let mut rt = Runtime::new(rows, list, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    rt
}

#[test]
fn reordered_virtual_rows_slide_and_a_scroll_moves_none() {
    // Rows below the reader's first row, then the first row itself.
    for (a, b) in [(2, 3), (0, 1)] {
        let mut rt = virtual_rows();
        let (ka, kb) = (rt.state[a].0, rt.state[b].0);
        let (ya, yb, hb) = (y(&rt, ka), y(&rt, kb), rt.state[b].1);
        let below = rt.state[b + 1].0;
        let y_below = y(&rt, below);
        rt.update(|rows, _| rows.swap(a, b));
        let frames = trace(&mut rt, &[ka, kb, below], 120);
        let col = |k: usize| frames.iter().map(|f| f[k]).collect::<Vec<_>>();
        slides(ka, ya, ya + hb + 4.0, &col(0), 8.0);
        slides(kb, yb, ya, &col(1), 8.0);
        assert!(col(2).iter().all(|&y| y == y_below), "rows nobody passed hold still: {:?}", col(2));
    }
    // Scrolling shifts the window and mounts rows: nothing slides.
    let mut rt = virtual_rows();
    for _ in 0..30 { rt.wheel(100.0, 100.0, -40.0); rt.frame(1.0 / 120.0); }
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("v0")), "scrolled past the first rows");
    let keys: Vec<&str> = rt.state.iter().map(|r| r.0).collect();
    assert!(keys.iter().all(|k| !offset_tracked(&rt, k)), "a window shift slides no row");
}

#[test]
fn a_row_that_moves_and_grows_slides_without_a_step() {
    // A delivered prompt grows from 53 to 77 pt and moves above the Working row in one commit.
    let mut rt = settled(vec![("m0", 120.0), ("m1", 60.0), ("m2", 90.0), ("w", 77.0), ("a", 53.0), ("b", 53.0)]);
    let (a0, w0, b0) = (y(&rt, "a"), y(&rt, "w"), y(&rt, "b"));
    rt.update(|rows, _| { rows[4].1 = 77.0; let a = rows.remove(4); rows.insert(3, a); });
    let frames = trace(&mut rt, &["a", "w", "b"], 120);
    assert!(!rt.animating);
    let col = |k: usize| frames.iter().map(|f| f[k]).collect::<Vec<_>>();
    let end = &frames[119];
    assert!((end[1] - end[0] - 85.0).abs() < 0.01, "Working rests under the grown row: {end:?}");
    slides("a", a0, end[0], &col(0), 8.0);
    slides("w", w0, end[1], &col(1), 8.0);
    let mut last = b0;
    for (f, &y) in col(2).iter().enumerate() {
        assert!((y - last).abs() <= 8.0, "b jumps {} pt at frame {f}: {:?}", y - last, col(2));
        last = y;
    }
}

/// Three rows whose contents sit under a gap, with a foot under each body; the middle row's gap
/// and body height are the state.
fn gapped(&(gap, body): &(f32, f32), _: &mut Cx<(f32, f32)>) -> El {
    let row = |k: &'static str, pt: f32, h: f32| col().key(k).w_full().pt(pt)
        .child(el().key(format!("{k}-body")).w_full().h(h).fill(Color::WHITE))
        .child(el().key(format!("{k}-foot")).w_full().h(20.0).mt(4.0).fill(Color::WHITE));
    let stack = col().key("stack").w_full().padding(12.0).gap(8.0).growth(GrowthSpec::default())
        .child(row("a", 16.0, 37.0)).child(row("b", gap, body)).child(row("c", 16.0, 37.0));
    col().w_full().h_full().child(col().key("scroll").w_full().h(400.0).scroll(ScrollSpec { stick_bottom: false }).child(stack))
}

#[test]
fn contents_whose_gap_in_their_row_changes_slide_from_where_they_were_drawn() {
    // A steer stops joining the prompt above it: its gap grows from 4 to 16 pt.
    let mut rt = Runtime::new((4.0f32, 37.0f32), gapped, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    let at = |rt: &Runtime<(f32, f32)>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).unwrap().rect[1];
    let keys = ["b-body", "b-foot", "c-body"];
    let ys0 = keys.map(|k| at(&rt, k));
    rt.update(|s, _| s.0 = 16.0);
    let frames: Vec<[f32; 3]> = (0..120).map(|_| { rt.frame(1.0 / 120.0); keys.map(|k| at(&rt, k)) }).collect();
    assert!(!rt.animating);
    for (k, key) in keys.iter().enumerate() {
        slides(key, ys0[k], ys0[k] + 12.0, &frames.iter().map(|f| f[k]).collect::<Vec<_>>(), 3.0);
    }
    // A body that grows moves the foot under it at once, as the row's growth reveals it.
    let foot = at(&rt, "b-foot");
    rt.update(|s, _| s.1 = 77.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(at(&rt, "b-foot"), foot + 40.0);
    // Instant, the gap lands at once.
    rt.update(|s, tx| { tx.instant(); s.0 = 4.0 });
    rt.frame(1.0 / 120.0);
    assert_eq!(at(&rt, "b-body"), ys0[0]);
}

/// A tall row, then a last row whose contents sit under a gap it fits around, in a list pinned to
/// its end; the gap is the state.
fn pinned_gap(&gap: &f32, _: &mut Cx<f32>) -> El {
    let last = col().key("b").w_full().pt(gap)
        .child(el().key("b-body").w_full().h(37.0).fill(Color::WHITE))
        .child(el().key("b-foot").w_full().h(20.0).mt(4.0).fill(Color::WHITE));
    let stack = col().key("stack").w_full().padding(12.0).gap(8.0).growth(GrowthSpec::default())
        .child(el().key("a").w_full().h(400.0).fill(Color::WHITE)).child(last);
    col().w_full().h_full().child(col().key("scroll").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: true }).child(stack))
}

#[test]
fn contents_whose_gap_opens_as_their_row_grows_at_a_pinned_end_hold_still() {
    // The last steer stops joining the prompt above it: its gap and its height grow by 12 pt, so
    // its row rises 12 pt and its contents keep their place on every frame.
    let mut rt = Runtime::new(4.0f32, pinned_gap, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    let at = |rt: &Runtime<f32>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).unwrap().rect[1];
    let (row0, body0, foot0) = (at(&rt, "b"), at(&rt, "b-body"), at(&rt, "b-foot"));
    rt.update(|s, _| *s = 16.0);
    let frames: Vec<[f32; 3]> = (0..120).map(|_| { rt.frame(1.0 / 120.0); ["b", "b-body", "b-foot"].map(|k| at(&rt, k)) }).collect();
    assert!(!rt.animating);
    slides("b", row0, row0 - 12.0, &frames.iter().map(|f| f[0]).collect::<Vec<_>>(), 3.0);
    for (f, ys) in frames.iter().enumerate() {
        assert!((ys[1] - body0).abs() < 1.0 / 64.0 && (ys[2] - foot0).abs() < 1.0 / 64.0, "the contents move at frame {f}: {frames:?}");
    }
}
