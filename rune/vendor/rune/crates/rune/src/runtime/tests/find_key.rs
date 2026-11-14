use super::*;
use crate::el::row;

#[test]
fn a_key_path_skips_elements_without_keys() {
    struct State;
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col()
            .child(el().key("other").child(el().key("knob").size(10.0, 10.0)))
            .child(
                el().key("form").child(
                    row().child(el().size(30.0, 10.0)).child(el().key("slider").child(col().child(el().key("knob").size(20.0, 20.0)))),
                ),
            )
    }
    let mut rt = Runtime::new(State, view, (100.0, 100.0), Env { system_dark: true, scale: 2.0 });
    rt.frame(0.0);
    assert_eq!(rt.find("slider/knob"), Some((40.0, 20.0)));
    assert_eq!(rt.find("form/slider/knob"), Some((40.0, 20.0)));
    assert_eq!(rt.find("other/knob"), Some((5.0, 5.0)));
    assert_eq!(rt.find("form/knob"), None, "keyed segments must still be adjacent");
    assert_eq!(rt.find("knob"), Some((5.0, 5.0)), "a bare key finds the first element with it");
}
