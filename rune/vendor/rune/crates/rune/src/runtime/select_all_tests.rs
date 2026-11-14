//! Cmd+A with nothing selected: the last clicked group, else the window's only one.
use super::*;
use crate::{Cx, El, Env, Key, KeyEvent, Mods, TextEdit, TextStyle, col, el, text, text_input};

#[derive(Default)]
struct S { two: bool, field: TextEdit, app_takes: bool, app_saw: u32 }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w(200.0).selectable(true);
    let mut root = col().key("root").size(300.0, 300.0).padding(10.0).focusable(true)
        .on_key(|s: &mut S, ev: &KeyEvent, _: &mut Tx| {
            let hit = s.app_takes && ev.key == Key::Command('a');
            s.app_saw += u32::from(hit);
            hit
        })
        .child(col().key("one").selection_group(true).child(t("alpha one", "a0")).child(t("alpha two", "a1")));
    if s.two {
        root = root.child(col().key("two").selection_group(true).mt(10.0).child(t("beta one", "b0")).child(t("beta two", "b1")));
    }
    root.child(text_input(&s.field, style).key("field").size(200.0, 24.0).mt(10.0).on_edit(|s: &mut S| &mut s.field))
        .child(el().key("blank").size(200.0, 60.0))
}

fn runtime(two: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S { two, ..S::default() }, view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn center(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
    (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5)
}

fn click(rt: &mut Runtime<S>, key: &str) {
    let (x, y) = center(rt, key);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

fn select_all(rt: &mut Runtime<S>) {
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
}

#[test]
fn the_only_group_is_selected() {
    let mut rt = runtime(false);
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha one\nalpha two"));
}

#[test]
fn the_last_clicked_group_is_selected() {
    let mut rt = runtime(true);
    click(&mut rt, "b1");
    click(&mut rt, "blank");
    assert_eq!(rt.selected_text(), None);
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("beta one\nbeta two"));
}

#[test]
fn two_groups_and_no_click_select_nothing() {
    let mut rt = runtime(true);
    select_all(&mut rt);
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn a_group_that_unmounted_falls_back_to_the_only_one() {
    let mut rt = runtime(true);
    click(&mut rt, "b0");
    click(&mut rt, "blank");
    rt.update(|s, _| s.two = false);
    rt.frame(0.0);
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha one\nalpha two"));
}

#[test]
fn a_focused_field_keeps_its_own_select_all() {
    let mut rt = runtime(false);
    click(&mut rt, "field");
    select_all(&mut rt);
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn an_app_that_handles_the_key_keeps_it() {
    let mut rt = runtime(false);
    rt.state.app_takes = true;
    rt.set_focus(Some("root".to_owned()), false);
    select_all(&mut rt);
    assert_eq!(rt.state.app_saw, 1);
    assert_eq!(rt.selected_text(), None);
}
