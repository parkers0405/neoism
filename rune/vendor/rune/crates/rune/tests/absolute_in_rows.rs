//! Absolute children size and pin against their parent's padding box, wherever the parent sits:
//! plain columns, growth stacks and virtual lists inside a scroll viewport. Root overlays
//! (scrims, anchored popovers) keep sizing against the window.
use rune::{Align, Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, Side, col, el, row, text};

fn view(case: &u8, _: &mut Cx<u8>) -> El {
    let r = |i: usize| {
        row().key(format!("row{i}")).w_full().h(40.0).padding(4.0)
            .child(text("hello"))
            .child(el().key(format!("cover{i}")).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .child(el().key(format!("full{i}")).absolute(0.0, 0.0).w_full().h_full())
            .child(el().key(format!("half{i}")).absolute(10.0, 2.0).w_frac(0.5).h(4.0))
            .child(el().key(format!("badge{i}")).pin(None, None, Some(6.0), Some(5.0)).size(10.0, 10.0))
            .child(row().key(format!("nest{i}")).absolute(0.0, 0.0).w(100.0).h(20.0)
                .child(el().key(format!("inner{i}")).pin(Some(0.0), None, Some(0.0), None).h(2.0)))
    };
    let mut stack = col().key("stack").w_full();
    match case {
        0 => {}
        1 => stack = stack.growth(GrowthSpec::default()),
        _ => stack = stack.growth(GrowthSpec::default()).virtualize(40.0),
    }
    for i in 0..3 { stack = stack.child(r(i)); }
    col().w_full().h_full().padding(20.0)
        .child(col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack))
        .child(el().key("scrim").absolute(0.0, 0.0).w_full().h_full())
        .child(col().key("pop").w_frac(0.25).h(30.0).anchored("row1", Side::Bottom, Align::End, 4.0))
}

fn target(rt: &Runtime<u8>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}")).target
}

#[test]
fn absolute_children_of_rows_size_against_the_row() {
    for (case, name) in ["column", "growth stack", "virtual list"].into_iter().enumerate() {
        let mut rt = Runtime::new(case as u8, view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(0.5);
        for i in 0..3 {
            let y = 20.0 + 40.0 * i as f32;
            let at = |key: &str| target(&rt, &format!("{key}{i}"));
            // The padding box includes the row's padding.
            assert_eq!(at("row"), [20.0, y, 360.0, 40.0], "{name}: row{i}");
            assert_eq!(at("cover"), [20.0, y, 360.0, 40.0], "{name}: pinned to all four edges");
            assert_eq!(at("full"), [20.0, y, 360.0, 40.0], "{name}: w_full / h_full");
            assert_eq!(at("half"), [30.0, y + 2.0, 180.0, 4.0], "{name}: w_frac(0.5)");
            assert_eq!(at("badge"), [380.0 - 6.0 - 10.0, y + 40.0 - 5.0 - 10.0, 10.0, 10.0], "{name}: right/bottom pin");
            assert_eq!(at("inner"), [20.0, y, 100.0, 2.0], "{name}: nested absolute");
        }
        assert_eq!(target(&rt, "scrim"), [0.0, 0.0, 400.0, 300.0], "{name}: a root scrim still fills the window");
        assert_eq!(target(&rt, "pop"), [380.0 - 100.0, 60.0 + 40.0 + 4.0, 100.0, 30.0], "{name}: anchored popover");
    }
}
