use super::*;

const DT: f64 = 1.0 / 60.0;

/// Forty 50-point rows in a 200-point viewport.
fn view(_: &(), _: &mut Cx<()>) -> El {
    let rows = col().key("rows").w_full().children((0..40).map(|k| el().key(format!("r{k}")).w_full().h(50.0)));
    col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

fn offset(rt: &Runtime<()>) -> f64 {
    rt.scrolls.values().next().unwrap().offset
}

/// The offset each frame until the glide stops.
fn glide(rt: &mut Runtime<()>) -> Vec<f64> {
    let mut path = vec![offset(rt)];
    for _ in 0..240 {
        rt.frame(DT);
        path.push(offset(rt));
        if rt.scrolls.values().all(|s| s.glide().is_none()) { break; }
    }
    path
}

fn at(to: f32) -> Runtime<()> {
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, to));
    rt.frame(DT);
    rt
}

#[test]
fn a_glide_sent_nearer_mid_flight_stops_there_without_going_past() {
    let mut rt = at(1000.0);
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 0.0));
    for _ in 0..5 { rt.frame(DT); }
    let mid = offset(&rt);
    assert!(mid < 900.0, "it's well under way: {mid}");
    // A second press sends it to a stop just ahead, while it's still fast.
    let to = mid - 40.0;
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, to as f32));
    let path = glide(&mut rt);
    let past = path.iter().map(|y| to - y).fold(0.0, f64::max);
    assert!(past < 0.05, "it went {past:.2} points past {to:.1} and came back: {path:.1?}");
    assert!(path.windows(2).all(|w| w[1] <= w[0] + 1e-6), "one way only: {path:.1?}");
    assert_eq!(*path.last().unwrap(), to);
}

#[test]
fn a_glide_sent_further_mid_flight_keeps_its_speed() {
    let mut rt = at(1000.0);
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 500.0));
    for _ in 0..5 { rt.frame(DT); }
    let [a, b] = [offset(&rt), { rt.frame(DT); offset(&rt) }];
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 0.0));
    rt.frame(DT);
    assert!(b - offset(&rt) >= (a - b) - 0.5, "it goes on at least as fast: {a:.1} {b:.1} {:.1}", offset(&rt));
}

#[test]
fn a_glide_lands_without_a_last_step() {
    let mut rt = at(1000.0);
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 0.0));
    let path = glide(&mut rt);
    let steps: Vec<f64> = path.windows(2).map(|w| w[0] - w[1]).collect();
    let n = steps.len();
    assert!(n > 10, "it glided: {path:.1?}");
    // The landing frame moves no more than a tenth of a point past the frame before's step.
    assert!(steps[n - 1] <= steps[n - 2] + 0.1, "the last step jumps: {:.2?}", &steps[n - 4..]);
}

#[test]
fn a_centring_glide_sent_nearer_mid_flight_stops_there_without_going_past() {
    let mut rt = at(0.0);
    rt.update(|_, tx| tx.scroll_center("r30"));
    for _ in 0..5 { rt.frame(DT); }
    let mid = offset(&rt);
    // The first row whose centred offset (its top less 75 points) lies just ahead.
    let k = ((mid + 40.0 + 75.0) / 50.0).ceil();
    let to = k * 50.0 - 75.0;
    assert!(to < 1425.0, "it's well short of r30: {mid}");
    rt.update(|_, tx| tx.scroll_center(format!("r{k}")));
    let path = glide(&mut rt);
    let past = path.iter().map(|y| y - to).fold(0.0, f64::max);
    assert!(past < 0.05, "it went {past:.2} points past {to:.1} and came back: {path:.1?}");
    assert!(path.windows(2).all(|w| w[1] >= w[0] - 1e-6), "one way only: {path:.1?}");
    assert_eq!(*path.last().unwrap(), to);
}

#[test]
fn a_centring_glide_sent_further_mid_flight_keeps_its_speed() {
    let mut rt = at(0.0);
    rt.update(|_, tx| tx.scroll_center("r20"));
    for _ in 0..5 { rt.frame(DT); }
    let [a, b] = [offset(&rt), { rt.frame(DT); offset(&rt) }];
    rt.update(|_, tx| tx.scroll_center("r35"));
    rt.frame(DT);
    assert!(offset(&rt) - b >= (b - a) - 0.5, "it goes on at least as fast: {a:.1} {b:.1} {:.1}", offset(&rt));
    glide(&mut rt);
    assert_eq!(offset(&rt), 35.0 * 50.0 - 75.0);
}
