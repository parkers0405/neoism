use super::*;
use parley::Layout;
use crate::text::Ink;

fn resize_view(s: &bool, _: &mut Cx<bool>) -> El {
    let para = "Paragraph text long enough to wrap over several lines as the window narrows.";
    col().w_full().children((0..3).filter(|&i| *s || i != 2).map(|i| crate::el::text(format!("{i}. {para}")).w_full().wrap().key(format!("t{i}"))))
}

fn layout_of<'a>(rt: &'a Runtime<bool>, key: &str) -> &'a Rc<Layout<Ink>> {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).and_then(|n| n.text_layout.as_ref()).unwrap()
}

/// A commit lets go of the last commit's text layouts before measuring, so on a resize each
/// wrapped text re-breaks its own layout in place; a text that leaves keeps its layout to draw
/// its exit.
#[test]
fn resizing_rebreaks_kept_texts_in_place_and_leaving_ones_keep_their_layout() {
    let mut rt = Runtime::new(true, resize_view, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.set_viewport((600.0, 900.0), 2.0);
    rt.frame(1.0 / 120.0);
    let kept = Rc::as_ptr(layout_of(&rt, "t0"));
    rt.set_viewport((300.0, 900.0), 2.0);
    rt.frame(2.0 / 120.0);
    assert_eq!(Rc::as_ptr(layout_of(&rt, "t0")), kept, "re-broken in place");
    assert!(layout_of(&rt, "t0").len() > 1);
    let leaving = layout_of(&rt, "t2").clone();
    rt.update(|s, _| *s = false);
    rt.frame(3.0 / 120.0);
    assert!(rt.is_exiting(rt.nodes.iter().find(|n| n.key.as_deref() == Some("t2")).unwrap().id));
    assert!(Rc::ptr_eq(layout_of(&rt, "t2"), &leaving), "the leaving text draws its exit from its own layout");
}

/// Measuring the content's height without committing leaves the drawn nodes' layouts alone.
#[test]
fn measuring_content_height_keeps_the_drawn_layouts() {
    let mut rt = Runtime::new(true, resize_view, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let l = layout_of(&rt, "t0").clone();
    rt.fit_height = true;
    rt.content_height();
    assert!(Rc::ptr_eq(layout_of(&rt, "t0"), &l));
}

fn typed(s: &String, _: &mut Cx<String>) -> El {
    col().w_full().child(crate::el::text(s.clone()).w_full().wrap().key("para")).child(crate::el::text("label").key("label"))
}

/// Typing into a paragraph keeps no stale copy of each edit's layout: the replaced text's
/// layouts go when the commit that replaced it ends.
#[test]
fn typing_keeps_no_layout_of_each_edit() {
    let para = "Paragraph text long enough to wrap over several lines as the window narrows. ".repeat(8);
    let mut rt = Runtime::new(para.clone(), typed, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let cached = rt.text.cached_layout_count();
    for i in 0..200 {
        rt.update(|s, _| s.push(char::from(b'a' + (i % 26) as u8)));
        rt.frame((i + 1) as f64 / 120.0);
    }
    assert!(rt.text.cached_layout_count() <= cached + 2, "{} layouts cached, {cached} at first", rt.text.cached_layout_count());
}

fn field(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
    crate::text_input(s, crate::TextStyle::new(14.0, 400.0)).key("field").w(600.0).on_edit(|s: &mut crate::TextEdit| s)
}

/// The same for a focused text field.
#[test]
fn typing_into_a_field_keeps_no_layout_of_each_edit() {
    let mut edit = crate::TextEdit::new("Paragraph text long enough to wrap over several lines as the window narrows. ".repeat(8));
    edit.focused = true;
    let mut rt = Runtime::new(edit, field, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let cached = rt.text.cached_layout_count();
    for i in 0..200 {
        rt.update(|s, _| s.insert("k"));
        rt.frame((i + 1) as f64 / 120.0);
    }
    assert!(rt.text.cached_layout_count() <= cached + 2, "{} layouts cached, {cached} at first", rt.text.cached_layout_count());
}

fn rows(s: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
    col().children(s.iter().map(|&t| crate::el::text(t)))
}

/// A text replaced by an unrelated one, as a reader showing another note, keeps its layout for
/// when it's shown again.
#[test]
fn a_text_shown_again_after_another_keeps_its_layout() {
    let mut rt = Runtime::new(vec!["The first note's opening paragraph"], rows, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let shaped = rt.text.shaped;
    rt.update(|s, _| s[0] = "Something else entirely, from another note");
    for f in 1..4 { rt.frame(f as f64 / 120.0); }
    rt.update(|s, _| s[0] = "The first note's opening paragraph");
    for f in 4..8 { rt.frame(f as f64 / 120.0); }
    assert_eq!(rt.text.shaped, shaped + 1, "only the other note shaped");
}

/// An edited text's old layout stays while another node shows the same text.
#[test]
fn an_edited_text_shown_elsewhere_keeps_its_layout() {
    let mut rt = Runtime::new(vec!["same words", "same words"], rows, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let shaped = rt.text.shaped;
    rt.update(|s, _| s[0] = "same wordz");
    for f in 1..4 { rt.frame(f as f64 / 120.0); }
    rt.update(|s, _| s[0] = "same words");
    for f in 4..8 { rt.frame(f as f64 / 120.0); }
    assert_eq!(rt.text.shaped, shaped + 1, "only the edit shaped");
}

#[test]
fn an_edit_shares_most_of_its_text_and_a_replacement_does_not() {
    use super::super::text_layout::edited;
    assert!(edited("hello world", "hello world!"));
    assert!(edited("hello world", "hello, world"));
    assert!(edited("12", "13"));
    assert!(!edited("same", "same"));
    assert!(!edited("Show", "Hide"));
    assert!(!edited("The first note", "Another one"));
}
