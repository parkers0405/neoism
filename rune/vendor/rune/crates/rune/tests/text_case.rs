//! `El::text_case` draws cased glyphs while tools, spans and links keep the authored string.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Link, Runtime, Span, TextCase, TextEdit, TextStyle, col, rich, text, text_input};

struct S { edit: TextEdit }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let red = rune::color::hex("#ff0000").unwrap();
    col().w_full().h_full().gap(10.0)
        .child(text("Straße").key("cased").font(style).text_case(TextCase::Uppercase))
        .child(text("STRASSE").key("typed").font(style))
        .child(rich("ß link", vec![Span { color: Some(red), ..Span::new(3, 7) }]).key("rich").font(style).w(200.0)
            .text_case(TextCase::Uppercase).link(Link::new(3..7, "go")))
        .child(rich("SS LINK", Vec::new()).key("rich-typed").font(style).w(200.0).link(Link::new(3..7, "go-typed")))
        .child(text_input(&s.edit, style).key("field").w(200.0).text_case(TextCase::Uppercase).on_edit(|s: &mut S| &mut s.edit))
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"))
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S { edit: TextEdit::new("straße") }, view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

#[test]
fn cased_text_draws_capitals_and_reports_its_authored_string() {
    let rt = runtime();
    let (cased, typed) = (find(&rt, "cased"), find(&rt, "typed"));
    assert_eq!(cased.text.as_deref(), Some("Straße"), "tools read the authored string");
    assert_eq!(cased.rect[2], typed.rect[2], "ß draws SS: the same width as typed capitals");
    assert_eq!(find(&rt, "field").text.as_deref(), Some("straße"), "fields draw what is typed");
}

#[test]
fn spans_and_links_keep_their_authored_byte_ranges() {
    let rt = runtime();
    let (link, typed) = (find(&rt, "go"), find(&rt, "go-typed"));
    assert_eq!(link.text.as_deref(), Some("link"), "the link reports its authored words");
    assert_eq!(link.rect[0], typed.rect[0], "the link sits over LINK after the expanded SS");
    assert_eq!(link.rect[2], typed.rect[2]);
    assert_eq!(find(&rt, "rich").text.as_deref(), Some("ß link"));
}
