use super::*;
use crate::{Key, KeyEvent, Mods};

#[test]
fn the_menu_key_opens_nothing_for_a_focused_link_whose_subtree_is_inert() {
    #[derive(Default)]
    struct State { menus: Vec<String> }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().key("row").size(300.0, 100.0)
            .on_secondary_press(|s: &mut State, _, _| s.menus.push("row".into()))
            .child(crate::rich("see the docs", Vec::new()).key("para").w(200.0)
                .link(crate::Link::new(8..12, "docs"))
                .on_link_secondary(|s: &mut State, id, _, _| s.menus.push(id.into())))
    }
    let mut rt = Runtime::new(State::default(), view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.focused_key().as_deref(), Some("docs"));
    // Focus normally drops at the commit that makes it inert; the key must not depend on that.
    for n in &mut rt.nodes { n.inert = true; }
    for (key, shift) in [(Key::ContextMenu, false), (Key::Function(10), true)] {
        rt.key(KeyEvent { key, mods: Mods { shift, ..Mods::default() } });
        rt.frame(0.0);
    }
    assert!(rt.state.menus.is_empty(), "{:?}", rt.state.menus);
}
