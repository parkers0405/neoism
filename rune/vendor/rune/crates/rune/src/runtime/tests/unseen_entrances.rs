use super::*;
use crate::el::ScrollSpec;

/// A 100 pt scroll over 40 rows; `n` odd lights every row.
fn lit(n: &u32, _cx: &mut Cx<u32>) -> El {
    let fill = if n % 2 == 1 { Color::WHITE } else { Color::TRANSPARENT };
    let rows = (0..40).map(|i| col().key(format!("r{i}")).w_full().h(40.0).fill(fill));
    col().w_full().h_full().child(col().key("scroll").w_full().h(100.0).scroll(ScrollSpec { stick_bottom: false }).child(col().children(rows)))
}

#[test]
fn colours_out_of_sight_land_at_once() {
    let mut rt = Runtime::new(0u32, lit, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.update(|n, _| *n = 1);
    rt.frame(1.0 / 120.0);
    let fading = |rt: &Runtime<u32>, key: &str| {
        let id = rt.nodes.iter().find(|n| n.named(key)).unwrap().id;
        rt.scene.store().is_animating(id, P_FILL)
    };
    assert!(fading(&rt, "r0"), "a row in sight fades its fill");
    assert!(!fading(&rt, "r30"), "a row out of sight lands it");
}

/// A 100 pt scroll over 40 rows, each with a digit; `n` ticks every row's digit.
fn ticking(n: &u32, _cx: &mut Cx<u32>) -> El {
    let rows = (0..40).map(|i| {
        let digit = el().key(format!("d{i}-{n}")).size(10.0, 10.0).fill(Color::WHITE).transition(Transition::opacity());
        col().key(format!("r{i}")).w_full().h(40.0).child(digit)
    });
    col().w_full().h_full().child(col().key("scroll").w_full().h(100.0).scroll(ScrollSpec { stick_bottom: false }).child(col().children(rows)))
}

#[test]
fn what_mounts_out_of_sight_enters_at_once() {
    let mut rt = Runtime::new(0u32, ticking, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.update(|n, _| *n = 1);
    rt.frame(1.0 / 120.0);
    let opacity = |key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.opacity);
    assert!(opacity("d0-1").is_some_and(|o| o < 1.0), "a digit in sight fades in: {:?}", opacity("d0-1"));
    assert_eq!(opacity("d30-1"), Some(1.0), "a digit out of sight lands at once");
    assert_eq!(opacity("d30-0"), None, "and the one it replaced goes at once");
}
