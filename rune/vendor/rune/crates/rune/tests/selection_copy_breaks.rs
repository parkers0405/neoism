//! `El::copy_breaks` and `El::copy_prefix`: a group's copy leaves the newlines its texts ask for
//! between them, the larger of the two, and puts a text's prefix before it when the copy takes it
//! from its start along with other texts. A copy of one text alone goes without its prefix.
mod support;

use rune::{Cx, El, Env, Runtime, TextStyle, col, text};
use support::{command, drag, glyphs, wait};

fn view(_: &(), _: &mut Cx<()>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().selectable(true);
    col().w_full().h_full().padding(10.0)
        .child(col().key("group").w(400.0).gap(12.0).selection_group(true)
            .child(t("first para", "a").copy_breaks(2, 2))
            .child(t("one", "i0").copy_prefix("- "))
            .child(t("two", "i1").copy_prefix("- "))
            .child(t("plain", "p")))
}

fn runtime() -> (Runtime<()>, rune::clipboard::MemoryClipboard) {
    let mut rt = Runtime::new((), view, (500.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    (rt, clipboard)
}

fn before(g: (f32, f32)) -> (f32, f32) { (g.0 - 3.0, g.1) }
fn after(g: (f32, f32)) -> (f32, f32) { (g.0 + 3.0, g.1) }

#[test]
fn copy_leaves_the_breaks_and_prefixes_texts_ask_for() {
    let (mut rt, clipboard) = runtime();
    let a = glyphs(&rt, "a");
    let p = glyphs(&rt, "p");
    // Glyphs skip spaces: a[5] is the "p" of "para".
    drag(&mut rt, before(a[5]), after(p[1]));
    assert_eq!(rt.selected_text().as_deref(), Some("para\n\n- one\n- two\npl"));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("para\n\n- one\n- two\npl"));
}

#[test]
fn a_text_taken_from_inside_or_alone_goes_without_its_prefix() {
    let (mut rt, _) = runtime();
    let one = glyphs(&rt, "i0");
    let two = glyphs(&rt, "i1");
    // Just inside the text, which starts at the padding's edge.
    drag(&mut rt, (two[0].0 - 1.0, two[0].1), after(two[2]));
    assert_eq!(rt.selected_text().as_deref(), Some("two"));
    wait(&mut rt);
    drag(&mut rt, before(one[1]), after(two[2]));
    assert_eq!(rt.selected_text().as_deref(), Some("ne\n- two"));
}

