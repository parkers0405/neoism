//! Rows inserted into a growth stack play an explicit `.transition` on top of growing open, in
//! plain and virtual stacks under a stick-to-bottom scroll. Rows without one only grow; the
//! first frame and rows a virtual list remounts on scroll never replay an entrance.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, Transition, col, text};

struct S { virtual_list: bool, explicit: bool, n: usize }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
    if s.virtual_list { stack = stack.virtualize(40.0); }
    for i in 0..s.n {
        let mut r = col().key(format!("r{i}")).w_full().h(40.0).child(text(format!("row {i}")).key(format!("t{i}")));
        if s.explicit { r = r.transition(Transition::offset(0.0, 12.0).combined(Transition::opacity())); }
        stack = stack.child(r);
    }
    col().w_full().h_full().child(col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack))
}

fn find(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

#[test]
fn inserted_rows_play_their_explicit_transition() {
    for virtual_list in [false, true] {
        for explicit in [false, true] {
            let name = format!("virtual {virtual_list}, explicit {explicit}");
            let mut rt = Runtime::new(S { virtual_list, explicit, n: 40 }, view, (300.0, 300.0), Env::new(true, 1.0));
            rt.frame(0.0);
            let last = find(&rt, "r39").unwrap();
            assert_eq!(last.opacity, 1.0, "{name}: the first frame mounts settled");

            rt.update(|s, _| s.n = 41);
            rt.frame(1.0 / 60.0);
            let row = find(&rt, "r40").unwrap();
            let label = find(&rt, "t40").unwrap();
            if explicit {
                assert!(row.opacity < 0.5 && label.opacity < 0.5, "{name}: fades in, at {}", row.opacity);
                assert!(row.rect[1] + row.rect[3] > 205.0, "{name}: slides up from below the bottom edge");
            } else {
                assert_eq!((row.opacity, label.opacity), (1.0, 1.0), "{name}: growth alone");
                assert!((row.rect[1] + row.rect[3] - 200.0).abs() < 0.01, "{name}: sits on the bottom edge");
            }
            assert!(row.rect[3] < 40.0, "{name}: still grows open");
            for _ in 0..90 { rt.frame(1.0 / 60.0); }
            let row = find(&rt, "r40").unwrap();
            assert_eq!(row.opacity, 1.0, "{name}: settles");
            assert_eq!(row.rect, [0.0, 160.0, 300.0, 40.0], "{name}: lands at the bottom");

            if virtual_list {
                assert!(find(&rt, "r0").is_none(), "{name}: the top row is culled");
                for _ in 0..4 {
                    rt.wheel(150.0, 100.0, 1000.0);
                    rt.frame(1.0 / 60.0);
                }
                let top = find(&rt, "r0").expect("scrolled back to the top");
                assert_eq!(top.opacity, 1.0, "{name}: a remounted row doesn't replay its entrance");
                assert_eq!(top.rect, top.target, "{name}: nor slide");
            }
        }
    }
}

struct Chips { virtual_list: bool, copied: bool }

fn chips(s: &Chips, _: &mut Cx<Chips>) -> El {
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
    if s.virtual_list { stack = stack.virtualize(40.0); }
    for i in 0..40 {
        let mut r = col().key(format!("r{i}")).w_full().h(40.0).child(text(format!("row {i}")).key(format!("t{i}")));
        if s.copied && (i == 0 || i == 39) { r = r.child(text("Copied").key(format!("chip{i}")).transition(Transition::opacity())); }
        stack = stack.child(r);
    }
    col().w_full().h_full().child(col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack))
}

#[test]
fn late_children_of_a_shown_row_play_their_transition() {
    for virtual_list in [false, true] {
        let name = format!("virtual {virtual_list}");
        let mut rt = Runtime::new(Chips { virtual_list, copied: false }, chips, (300.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, _| s.copied = true);
        rt.frame(1.0 / 60.0);
        let chip = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("chip39")).unwrap();
        assert!(chip.opacity < 0.5, "{name}: the chip fades in, at {}", chip.opacity);
        for _ in 0..90 { rt.frame(1.0 / 60.0); }
        let chip = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("chip39")).unwrap();
        assert_eq!(chip.opacity, 1.0, "{name}: settles");

        if virtual_list {
            for _ in 0..4 {
                rt.wheel(150.0, 100.0, 1000.0);
                rt.frame(1.0 / 60.0);
            }
            let chip = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("chip0")).expect("scrolled back to the top");
            assert_eq!(chip.opacity, 1.0, "{name}: a remounted row's chip doesn't replay its entrance");
        }
    }
}

fn nested(_: &(), _: &mut Cx<()>) -> El {
    let mut inner = col().key("inner-stack").w_full().growth(GrowthSpec::default()).virtualize(40.0);
    for i in 0..80 {
        inner = inner.child(col().key(format!("i{i}")).w_full().h(40.0)
            .transition(Transition::offset(0.0, 12.0).combined(Transition::opacity()))
            .child(text(format!("inner {i}"))));
    }
    let inner = col().key("inner").w_full().h(150.0).scroll(ScrollSpec { stick_bottom: true }).child(inner);
    let outer = col().key("outer-stack").w_full().growth(GrowthSpec::default()).virtualize(100.0)
        .child(col().key("o0").w_full().h(100.0))
        .child(col().key("o1").w_full().h(100.0))
        .child(col().key("o2").w_full().child(inner));
    col().w_full().h_full().child(col().key("outer").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: true }).child(outer))
}

#[test]
fn rows_scrolled_into_a_nested_virtual_list_mount_settled() {
    let mut rt = Runtime::new((), nested, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    let get = |rt: &Runtime<()>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    assert!(get(&rt, "i0").is_none(), "the inner list's top row starts culled");
    let inner = get(&rt, "inner").unwrap();
    let (x, y) = (inner.rect[0] + inner.rect[2] / 2.0, inner.rect[1] + inner.rect[3] / 2.0);
    for _ in 0..6 {
        rt.wheel(x, y, 1000.0);
        rt.frame(1.0 / 60.0);
    }
    let top = get(&rt, "i0").expect("scrolled the inner list to its top");
    assert_eq!(top.opacity, 1.0, "a remounted inner row doesn't replay its entrance");
    let stack = get(&rt, "inner-stack").unwrap();
    assert_eq!((top.rect[1], top.rect[3]), (stack.rect[1], 40.0), "nor slide or grow: {:?} in {:?}", top.rect, stack.rect);
}
