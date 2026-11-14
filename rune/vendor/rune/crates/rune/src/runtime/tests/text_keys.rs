use super::*;

fn keyed_view(_: &(), _: &mut Cx<()>) -> El {
    col().w_full().children([
        crate::text("Plain words that wrap at the column width").wrap().key("plain"),
        crate::text("cased").text_case(crate::TextCase::Uppercase).key("cased"),
        crate::text("a-long-token-without-spaces").wrap().wrap_anywhere().key("anywhere"),
        crate::text("hunter2").secure().key("secret"),
    ])
}

fn spec<'a>(rt: &'a Runtime<()>, key: &str) -> &'a TextSpec {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).and_then(|n| n.text.as_ref()).unwrap()
}

/// Flatten hashes each text once; measuring and laying it out at every width reuse that key,
/// and it names the same cached layout the unkeyed path does.
#[test]
fn texts_carry_one_shape_key_that_names_their_cached_layout() {
    let mut rt = Runtime::new((), keyed_view, (240.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for key in ["plain", "cased", "anywhere"] {
        let s = spec(&rt, key).clone();
        let (bits, shape) = s.shape.unwrap_or_else(|| panic!("{key} has no shape key"));
        assert_eq!(bits, 2f32.to_bits());
        assert_eq!(shape, crate::text::shape_key(&s.content, &s.spans, s.style, 2.0, s.wrap_anywhere, &s.pills));
        assert_eq!(s.shape_key(1.0), None, "a key at another scale isn't reused");
        let keyed = rt.text.layout_for_spec(&s, 2.0, Some(300.0), parley::Alignment::Start);
        let unkeyed = rt.text.layout_blocks_inner(&s.content, &s.spans, &s.blocks, &s.pills, s.style, 2.0, Some(300.0), parley::Alignment::Start, s.wrap_anywhere, true);
        assert!(Rc::ptr_eq(&keyed, &unkeyed), "{key}");
    }
    assert_eq!(spec(&rt, "cased").content, "CASED");
    assert_eq!(spec(&rt, "secret").shape, None, "secure text keeps no content-derived key");
}

#[test]
fn replacing_a_texts_content_drops_its_key() {
    let mut rt = Runtime::new((), keyed_view, (240.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let mut s = spec(&rt, "plain").clone();
    s.replace_content("Other words".into());
    assert_eq!(s.shape_key(2.0), None);
    let l = rt.text.layout_for_spec(&s, 2.0, None, parley::Alignment::Start);
    assert!(Rc::ptr_eq(&l, &rt.text.layout_rich("Other words", &[], s.style, 2.0, None)));
}
