//! Flex alignment builders place children where CSS would.
use rune::{Cx, El, Env, Runtime, col, el, row};

fn rect(rt: &Runtime<u8>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().target
}

fn view(case: &u8, _: &mut Cx<u8>) -> El {
    let kids = || [el().key("a").size(20.0, 10.0), el().key("b").size(20.0, 30.0)];
    let parent = row().key("parent").size(200.0, 60.0);
    match case {
        0 => parent.items_end().children(kids()),
        1 => parent.items_center().children(kids()),
        2 => parent.children([el().key("a").size(20.0, 10.0).self_end(), el().key("b").w(20.0).self_stretch()]),
        3 => parent.justify_center().children(kids()),
        4 => parent.justify_around().children(kids()),
        5 => parent.justify_evenly().children(kids()),
        6 => col().key("parent").size(200.0, 60.0).items_end().justify_center().justify_start().children(kids()),
        _ => unreachable!(),
    }
}

#[test]
fn alignment_builders_match_css() {
    let expect: [(&str, [f32; 2], [f32; 2]); 7] = [
        ("items_end", [0.0, 50.0], [20.0, 30.0]),
        ("items_center", [0.0, 25.0], [20.0, 15.0]),
        ("self_end / self_stretch", [0.0, 50.0], [20.0, 0.0]),
        ("justify_center", [80.0, 0.0], [100.0, 0.0]),
        ("justify_around", [40.0, 0.0], [140.0, 0.0]),
        ("justify_evenly", [53.333, 0.0], [126.667, 0.0]),
        ("column items_end + justify_start", [180.0, 0.0], [180.0, 10.0]),
    ];
    for (case, (name, a, b)) in expect.into_iter().enumerate() {
        let mut rt = Runtime::new(case as u8, view, (200.0, 60.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for (key, at) in [("a", a), ("b", b)] {
            let r = rect(&rt, key);
            assert!((r[0] - at[0]).abs() < 0.01 && (r[1] - at[1]).abs() < 0.01, "{name}: {key} at {r:?}, want {at:?}");
        }
        if case == 2 { assert_eq!(rect(&rt, "b")[3], 60.0, "self_stretch fills the cross axis"); }
    }
}
