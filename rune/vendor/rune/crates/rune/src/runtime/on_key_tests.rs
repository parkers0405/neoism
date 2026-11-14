//! `El::on_key`, `El::icon_rotation`, `El::level` and `El::expanded`, `El::help_when_truncated`
//! and `Tx::mods`: what a virtualised tree or table needs from the core.
use super::*;
use crate::{Cx, El, Env, Icon, Key, KeyEvent, Mods, PhysicalInput, TextEdit, TextStyle, col, el, icon, row, text_input};

#[derive(Default)]
struct Keys {
    log: Vec<String>,
    field: TextEdit,
    /// The outer handler takes Down too.
    outer_down: bool,
}

/// A focusable list that owns its keys (with a focus-group child row it must beat to the
/// arrows), inside an outer handler, beside a text field.
fn keyed(s: &Keys, _: &mut Cx<Keys>) -> El {
    let rows = col().key("rows").role("listbox").focus_group().children((0..3).map(|i| {
        row().key(format!("row{i}")).size(100.0, 20.0).focusable(true).on_press(move |s: &mut Keys| s.log.push(format!("press {i}")))
    }));
    let list = col()
        .key("list")
        .role("tree")
        .focusable(true)
        .size(120.0, 80.0)
        .on_key(|s: &mut Keys, ev: &KeyEvent, _: &mut Tx| {
            let hit = matches!(ev.key, Key::Down | Key::Up);
            if hit {
                s.log.push(format!("list {:?}", ev.key));
            }
            hit
        })
        .child(rows);
    let outer = col()
        .key("outer")
        .on_key(|s: &mut Keys, ev: &KeyEvent, _: &mut Tx| {
            let hit = matches!(ev.key, Key::Down | Key::Left) && (ev.key == Key::Left || s.outer_down);
            if hit {
                s.log.push(format!("outer {:?}", ev.key));
            }
            hit
        })
        .child(list)
        .child(text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 30.0).on_edit(|s: &mut Keys| &mut s.field));
    col().size(300.0, 200.0).child(el().key("pad").size(300.0, 10.0)).child(outer)
}

fn press(rt: &mut Runtime<Keys>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

fn focused<S: 'static>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn center<S: 'static>(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
    (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5)
}

fn click<S: 'static>(rt: &mut Runtime<S>, (x, y): (f32, f32)) {
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

#[test]
fn a_keyed_control_takes_its_keys_before_focus_navigation_innermost_first() {
    let mut rt = Runtime::new(Keys::default(), keyed, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("list"), "Tab reaches a control that owns its keys, without Full Keyboard Access");
    rt.state.outer_down = true;
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Left);
    assert_eq!(rt.state.log, ["list Down", "outer Left"], "the list answers Down before the outer handler; Left falls through to it");
    assert_eq!(focused(&rt).as_deref(), Some("list"), "focus navigation never saw the arrow");
}

#[test]
fn a_focused_text_field_keeps_its_keys() {
    let mut rt = Runtime::new(Keys::default(), keyed, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.field.focused = true);
    rt.frame(0.0);
    press(&mut rt, Key::Left);
    press(&mut rt, Key::Text("a".into()));
    assert!(rt.state.log.is_empty(), "{:?}", rt.state.log);
    assert_eq!(rt.state.field.text, "a");
}

#[test]
fn a_click_inside_a_keyed_control_focuses_it() {
    #[derive(Default)]
    struct S {
        pressed: u32,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(300.0, 200.0).child(
            col()
                .key("table")
                .focusable(true)
                .size(200.0, 100.0)
                .on_key(|_: &mut S, _: &KeyEvent, _: &mut Tx| false)
                .child(row().key("row").size(200.0, 20.0).on_press(|s: &mut S| s.pressed += 1)),
        )
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let at = center(&rt, "row");
    click(&mut rt, at);
    assert_eq!(rt.state.pressed, 1);
    assert_eq!(focused(&rt).as_deref(), Some("table"), "the row's click focuses the table, not nothing");
}

#[test]
fn a_press_sees_the_modifiers_held() {
    #[derive(Default)]
    struct S {
        seen: Vec<Mods>,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(100.0, 100.0).child(el().key("b").size(100.0, 100.0).on_press_tx(|s: &mut S, tx: &mut Tx| s.seen.push(tx.mods())))
    }
    let mut rt = Runtime::new(S::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    click(&mut rt, (50.0, 50.0));
    let shift = Mods { shift: true, ..Mods::default() };
    rt.physical_input(PhysicalInput::Modifiers(shift));
    rt.frame(0.0);
    click(&mut rt, (50.0, 50.0));
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    rt.frame(0.0);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(rt.state.seen, [Mods::default(), shift, Mods::default()]);
}

#[test]
fn icon_rotation_springs_and_jumps_under_reduced_motion() {
    struct S {
        open: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        static CHEVRON: Icon = Icon { name: "test-chevron", view: 16.0, body: r#"<path d="M5 6.5L8 9.5L11 6.5" stroke="currentColor"/>"# };
        col().size(100.0, 100.0).child(
            icon(&CHEVRON, 16.0)
                .key("chevron")
                .color(Color::WHITE)
                .icon_rotation(if s.open { 0.0 } else { -std::f32::consts::FRAC_PI_2 })
                .motion(motion::DEFAULT),
        )
    }
    fn turn(rt: &Runtime<S>) -> f32 {
        rt.instances.iter().find(|q| q.params[1] == KIND_GLYPH).expect("the chevron draws").params[0]
    }
    let quarter = -std::f32::consts::FRAC_PI_2;
    for reduced in [false, true] {
        let mut rt = Runtime::new(S { open: false }, view, (100.0, 100.0), Env::new(true, 1.0));
        rt.set_system_reduced_motion(reduced);
        rt.frame(0.0);
        assert!((turn(&rt) - quarter).abs() < 1e-4, "a closed chevron starts turned, without animating in");
        rt.update(|s, _| s.open = true);
        rt.frame(1.0 / 60.0);
        rt.frame(2.0 / 60.0);
        let mid = turn(&rt);
        if reduced {
            assert!(mid.abs() < 1e-4, "reduced motion lands at once: {mid}");
        } else {
            assert!(mid < -0.05 && mid > quarter + 0.05, "two frames in, the turn is on its way: {mid}");
        }
        for f in 3..120 {
            rt.frame(f as f64 / 60.0);
        }
        assert!(turn(&rt).abs() < 1e-3, "it settles open");

        // Mounting open (`0.0`) and then closing (`-FRAC_PI_2`) must also spring from 0.0.
        let mut rt = Runtime::new(S { open: true }, view, (100.0, 100.0), Env::new(true, 1.0));
        rt.set_system_reduced_motion(reduced);
        rt.frame(0.0);
        assert!(turn(&rt).abs() < 1e-4, "an open chevron starts at 0");
        rt.update(|s, _| s.open = false);
        rt.frame(1.0 / 60.0);
        rt.frame(2.0 / 60.0);
        let mid = turn(&rt);
        if reduced {
            assert!((mid - quarter).abs() < 1e-4, "reduced motion lands closed at once: {mid}");
        } else {
            assert!(mid < -0.05 && mid > quarter + 0.05, "two frames in, closing is on its way: {mid}");
        }
    }
}

#[test]
fn a_modal_stops_keys_from_bubbling_to_an_ancestors_on_key() {
    #[derive(Default)]
    struct S {
        outer_keys: u32,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col()
            .size(200.0, 200.0)
            .on_key(|s: &mut S, _: &KeyEvent, _: &mut Tx| {
                s.outer_keys += 1;
                true
            })
            .child(
                col()
                    .key("dialog")
                    .modal(true, |_: &mut S| {})
                    .size(100.0, 100.0)
                    .child(el().key("inside").focusable(true).size(50.0, 20.0)),
            )
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(focused(&rt).as_deref(), Some("inside"));
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.outer_keys, 0, "an on_key ancestor outside the modal must not see keys inside it");
}

#[test]
fn outline_state_reaches_the_element_tree() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .size(200.0, 100.0)
            .role("tree")
            .child(row().key("folder").role("treeitem").level(1).expanded(true).focus_selected(true).size(200.0, 20.0))
            .child(row().key("file").role("treeitem").level(2).size(200.0, 20.0))
    }
    let mut rt = Runtime::new((), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let e = rt.elements();
    let get = |k: &str| e.iter().find(|e| e.key.as_deref() == Some(k)).unwrap();
    let (folder, file) = (get("folder"), get("file"));
    assert_eq!((folder.level, folder.expanded, folder.item_selected), (Some(1), Some(true), true));
    assert_eq!((file.level, file.expanded, file.item_selected), (Some(2), None, false));
}

#[cfg(target_os = "macos")]
#[test]
fn a_truncation_tooltip_shows_only_while_the_text_is_cut() {
    struct S {
        w: f32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(400.0, 100.0).child(
            row().size(s.w, 20.0).child(
                crate::text("A rather long file name.rs").key("cell").font(TextStyle::new(13.0, 400.0)).truncate().shrink().help_when_truncated("A rather long file name.rs"),
            ),
        )
    }
    let mut rt = Runtime::new(S { w: 60.0 }, view, (400.0, 100.0), Env::new(true, 1.0));
    rt.set_native_help(true);
    rt.frame(0.0);
    assert_eq!(rt.help_regions().len(), 1, "cut short, the cell offers its whole text");
    assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("cell")).unwrap().help.as_deref(), Some("A rather long file name.rs"));
    rt.update(|s, _| s.w = 380.0);
    rt.frame(0.0);
    assert!(rt.help_regions().is_empty(), "the whole text shows, so no tooltip");
    assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("cell")).unwrap().help, None);
}

#[cfg(target_os = "macos")]
#[test]
fn a_faded_text_that_runs_past_its_box_reads_as_cut() {
    struct S {
        w: f32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(400.0, 100.0).child(
            row().size(s.w, 20.0).child(
                crate::text("A rather long file name.rs").key("cell").font(TextStyle::new(13.0, 400.0)).truncate_fade().shrink().help_when_truncated("A rather long file name.rs"),
            ),
        )
    }
    let mut rt = Runtime::new(S { w: 60.0 }, view, (400.0, 100.0), Env::new(true, 2.0));
    rt.set_native_help(true);
    rt.frame(0.0);
    let i = rt.nodes.iter().position(|n| n.key.as_deref() == Some("cell")).unwrap();
    assert!(rt.nodes[i].text_cut(2.0) && rt.nodes[i].elided.is_none(), "a fade cuts nothing out of the layout");
    assert_eq!(rt.help_regions().len(), 1, "faded out, the cell offers its whole text");
    assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("cell")).unwrap().help.as_deref(), Some("A rather long file name.rs"));
    rt.update(|s, _| s.w = 380.0);
    rt.frame(0.0);
    assert!(rt.help_regions().is_empty(), "the whole text shows, so no tooltip");
    assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("cell")).unwrap().help, None);
}

/// Two lists keyed alike, side by side, each in its own scope.
fn twins(_: &(), _: &mut Cx<()>) -> El {
    let list = |scope: &str| {
        col().key(scope.to_owned()).w(100.0).h(200.0).child(
            col().key("viewport").w_full().h_full().scroll(crate::ScrollSpec { stick_bottom: false }).child(
                col().key("rows").w_full().growth(crate::GrowthSpec::default()).virtual_rows(1000, 20.0,
                    |_: &(), i| std::borrow::Cow::Owned(format!("r{i}")),
                    |_: &(), _| el().w_full().h(20.0)),
            ),
        )
    };
    row().child(list("a")).child(list("b"))
}

#[test]
fn scroll_into_view_in_reveals_only_inside_its_scope() {
    let mut rt = Runtime::new((), twins, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let offset = |rt: &Runtime<()>, scope: &str| {
        let id = rt.path_ids[&format!("#0/{scope}/viewport")];
        rt.scrolls[&id].offset
    };
    rt.update(|_, tx| tx.scroll_into_view_in("b", "r500"));
    rt.frame(0.0);
    assert_eq!(offset(&rt, "a"), 0.0, "the other list keeps still");
    assert!(offset(&rt, "b") > 9000.0, "{}", offset(&rt, "b"));
    rt.update(|_, tx| tx.scroll_into_view_in("a", "r900"));
    rt.frame(0.0);
    assert!(offset(&rt, "a") > 17_000.0 && offset(&rt, "b") < 10_100.0, "{} {}", offset(&rt, "a"), offset(&rt, "b"));
}

#[test]
fn key_handler_removals_fade_by_default_and_unmount_at_once_with_tx_instant() {
    struct S {
        shown: bool,
        instant: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().size(200.0, 100.0).focusable(true).on_key(|s: &mut S, ev, tx| {
            if ev.key == Key::Escape {
                s.shown = false;
                if s.instant {
                    tx.instant();
                }
                return true;
            }
            false
        });
        if s.shown {
            root = root.child(el().key("banner").size(80.0, 24.0));
        }
        root
    }

    // Default key-handler commit: the removed element fades out over its default opacity transition.
    let mut rt = Runtime::new(S { shown: true, instant: false }, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    assert!(rt.frame(1.0 / 60.0), "default removal animates the exit fade");
    assert!(
        rt.elements().iter().any(|e| e.key.as_deref() == Some("banner")),
        "banner stays mounted in the scene while fading out"
    );

    // With `tx.instant()` in the key handler, the removed element unmounts in the same frame.
    let mut rt = Runtime::new(S { shown: true, instant: true }, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    assert!(!rt.frame(1.0 / 60.0), "tx.instant() unmounts with no exit fade");
    assert!(
        !rt.elements().iter().any(|e| e.key.as_deref() == Some("banner")),
        "banner is gone in the commit frame"
    );
}
