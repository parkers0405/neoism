use super::*;


#[test]
fn page_keys_reach_an_input_and_scroll_its_scroll_view_by_most_of_a_viewport() {
    struct State { edit: crate::TextEdit, keys: Vec<crate::Key> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let field = crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0)).key("field").size(200.0, 40.0)
            .on_edit(|s: &mut State| &mut s.edit)
            .on_input_key(|s: &mut State, ev, _| { s.keys.push(ev.key.clone()); false });
        let list = col().key("list").w_full().child(field)
            .children((0..20).map(|i| el().key(format!("r{i}")).size(200.0, 40.0)));
        col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }
    let mut edit = crate::TextEdit::new("hello");
    edit.focused = true;
    let mut rt = Runtime::new(State { edit, keys: Vec::new() }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let top = |rt: &Runtime<State>| rt.presented("r0").unwrap().0[1];
    assert_eq!(top(&rt), 40.0);
    let page = |rt: &mut Runtime<State>, key: crate::Key, t: f64| {
        rt.key(KeyEvent { key, mods: Default::default() });
        rt.frame(t);
    };
    page(&mut rt, crate::Key::PageDown, 0.1);
    assert_eq!(rt.state.keys, [crate::Key::PageDown], "the input's key handler sees it");
    assert_eq!(top(&rt), 40.0 - 180.0, "a page is 90% of the viewport");
    assert_eq!(rt.state.edit.text, "hello");
    page(&mut rt, crate::Key::PageDown, 0.2);
    page(&mut rt, crate::Key::PageDown, 0.3);
    page(&mut rt, crate::Key::PageDown, 0.4);
    assert_eq!(top(&rt), 40.0 - 640.0, "clamped at the bottom");
    page(&mut rt, crate::Key::PageUp, 0.5);
    assert_eq!(top(&rt), 40.0 - 460.0);
    assert_eq!(crate::tooling::key_named("PageDown"), Some(crate::Key::PageDown));
}

#[test]
fn page_keys_keep_paging_the_list_of_a_focused_field_that_scrolled_out() {
    struct State { edit: crate::TextEdit }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let field = crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0)).key("field").size(100.0, 40.0)
            .on_edit(|s: &mut State| &mut s.edit);
        let rows = col().key("rows").w_full().growth(crate::GrowthSpec::default())
            .child(col().key("r0").child(field))
            .children((1..100).map(|i| el().key(format!("r{i}")).size(100.0, 40.0)))
            .virtualize(40.0);
        let other = col().key("other").w_full().children((0..100).map(|i| el().key(format!("o{i}")).size(100.0, 40.0)));
        crate::row().size(200.0, 200.0)
            .child(col().key("left").size(100.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
            .child(col().key("right").size(100.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(other))
    }
    let mut edit = crate::TextEdit::new("hello");
    edit.focused = true;
    let mut rt = Runtime::new(State { edit }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(150.0, 100.0);
    let y = |rt: &Runtime<State>, key: &str| rt.presented(key).map(|p| p.0[1]);
    for n in 1..=8 {
        rt.key(KeyEvent { key: crate::Key::PageDown, mods: Default::default() });
        rt.frame(n as f64 * 0.1);
    }
    assert!(y(&rt, "field").is_none(), "the field's row scrolled out of the window");
    assert_eq!(y(&rt, "r40"), Some(40.0 * 40.0 - 8.0 * 180.0), "every page moved the focused field's list");
    assert_eq!(y(&rt, "o0"), Some(0.0), "the list under the pointer stayed");
    assert!(rt.state.edit.focused);
}

#[test]
fn single_line_input_scrolls_to_the_caret_instead_of_wrapping() {
    fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(s, crate::TextStyle::new(16.0, 400.0)).single_line()
            .key("search").size(100.0, 24.0).on_edit(|s: &mut crate::TextEdit| s)
    }
    let text = "a very long search query that keeps going past the edge";
    let mut edit = crate::TextEdit::new(text);
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (100.0, 24.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("search")).unwrap();
    let path = n.path.clone();
    assert_eq!(n.text_layout.as_ref().unwrap().lines().count(), 1);
    let scroll = rt.input_scroll[&path];
    assert!(scroll > 100.0, "the end of the text is in view: {scroll}");
    let caret_x = rt.ime_area.unwrap()[0];
    assert!((0.0..=100.0).contains(&caret_x), "caret {caret_x} stays inside the field");

    // A click lands on the scrolled text, not on the hidden start.
    rt.queue.push(Action::Focus(path.clone(), 1.0, 12.0));
    rt.frame(0.0);
    assert!(rt.state.focus > 10, "clicked index {}", rt.state.focus);

    rt.queue.push(Action::Blur);
    rt.frame(0.0);
    assert!(!rt.state.focused);
    assert!(!rt.input_scroll.contains_key(&path), "unfocused fields rest at their start");
}

#[test]
fn grapheme_arrow_caret_hit_tests_roundtrip_after_single_line_scroll() {
    fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(s, crate::TextStyle::new(16.0, 400.0)).single_line()
            .key("search").size(100.0, 40.0).on_edit(|s: &mut crate::TextEdit| s)
    }
    let graphemes = ["W", "W", "W", "W", "W", "W", "W", "W", "W", "W", " ",
        "👨\u{200D}👩\u{200D}👧", "🇯🇵", "👋🏽", "e\u{301}", "❤\u{FE0F}", "Z"];
    let mut edit = crate::TextEdit::new(graphemes.concat());
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (100.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: crate::Key::Home, mods: Default::default() });
    rt.frame(0.0);
    let path = rt.nodes.iter().find(|n| n.key.as_deref() == Some("search")).unwrap().path.clone();
    let mut offset = 0;
    for grapheme in graphemes {
        offset += grapheme.len();
        rt.key(KeyEvent { key: crate::Key::Right, mods: Default::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.focus, offset, "arrow must cross the whole {grapheme:?}");
        let [x, y, _, height] = rt.ime_area.unwrap();
        assert!((0.0..=100.0).contains(&x), "caret must remain in the scrolled field");
        rt.queue.push(Action::Focus(path.clone(), x + 0.1, y + height * 0.5));
        rt.frame(0.0);
        assert_eq!(rt.state.focus, offset, "clicking the reported caret after {grapheme:?} must hit that boundary");
    }
    assert!(rt.input_scroll[&path] > 0.0, "the fixture must exercise scrolled caret geometry");
}

#[test]
fn clipboard_paste_routes_to_window_or_field_and_discards_late_reads() {
    #[derive(Default)]
    struct State { field: crate::TextEdit, pasted: String }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 100.0)
            .on_paste(|s: &mut State, text, tx| { s.pasted = text.to_owned(); tx.copy_secret(text, std::time::Duration::from_secs(30)); })
            .child(crate::text_input(&s.field, crate::TextStyle::new(16.0, 400.0)).key("field").size(200.0, 40.0)
                .on_edit(|s: &mut State| &mut s.field))
    }
    let mut rt = Runtime::new(State::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.accepts_paste());
    rt.paste("window fixture"); rt.frame(0.0);
    assert_eq!(rt.state.pasted, "window fixture");
    assert!(rt.events.iter().any(|e| e == "paste window"));
    assert!(!format!("{:?}", rt.copy_request).contains("window fixture"));
    rt.update(|s, _| s.field.focused = true); rt.frame(0.0);
    rt.paste_result(None, crate::ClipboardContent::new("late window reply")); rt.frame(0.0);
    assert!(rt.state.field.text.is_empty());
    assert_eq!(rt.state.pasted, "window fixture");
    let target = rt.paste_target();
    rt.paste_result(target, crate::ClipboardContent::new("field fixture")); rt.frame(0.0);
    assert_eq!(rt.state.field.text, "field fixture");
    assert!(rt.events.iter().any(|e| e == "paste field"));
}

#[test]
fn rich_pastes_reach_paste_hooks_and_plain_fields_keep_pasting_text() {
    use crate::ClipboardContent;
    use crate::clipboard::Forms;
    #[derive(Default)]
    struct State { plain: crate::TextEdit, rich: crate::TextEdit, take: bool, hook: Vec<Option<String>>, changes: u32, window: Vec<ClipboardContent> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = crate::TextStyle::new(16.0, 400.0);
        col().size(200.0, 100.0)
            .on_paste_content(|s: &mut State, content, _| s.window.push(content.clone()))
            .child(crate::text_input(&s.plain, style).key("plain").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.plain))
            .child(crate::text_input(&s.rich, style).key("rich").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.rich)
                .on_input_change(|s: &mut State, _| s.changes += 1)
                .on_input_paste(|s: &mut State, content, _| {
                    s.hook.push(content.html().map(Into::into));
                    if s.take { s.rich.paste("[rich]"); }
                    s.take
                }))
    }
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(State::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let paste = |rt: &mut Runtime<State>| { rt.update(|_, tx| tx.request_paste()); rt.frame(0.0); };
    m.set_content(ClipboardContent::new("Hello").with_html("<b>Hello</b><script>steal()</script>"));

    paste(&mut rt);
    let window = rt.state.window.pop().unwrap();
    assert_eq!((window.text(), window.html()), ("Hello", Some("<b>Hello</b>")), "the window hears every form, sanitised");

    rt.update(|s, _| s.plain.focused = true); rt.frame(0.0);
    assert_eq!(rt.paste_forms(rt.paste_target().as_deref()), Forms::Text);
    paste(&mut rt);
    assert_eq!(rt.state.plain.text, "Hello", "a field without a hook pastes the text, as before");

    rt.update(|s, _| { s.plain.focused = false; s.rich.focused = true; }); rt.frame(0.0);
    assert_eq!(rt.paste_forms(rt.paste_target().as_deref()), Forms::All);
    paste(&mut rt);
    assert_eq!((rt.state.rich.text.as_str(), rt.state.changes), ("Hello", 1), "a declining hook leaves the paste to the editor");
    rt.update(|s, _| { s.take = true; s.rich.text.clear(); s.rich.anchor = 0; s.rich.focus = 0; }); rt.frame(0.0);
    paste(&mut rt);
    assert_eq!((rt.state.rich.text.as_str(), rt.state.changes), ("[rich]", 2), "a hook that takes it edits the field; on_input_change runs");
    assert_eq!(rt.state.hook, vec![Some("<b>Hello</b>".to_owned()); 2]);

    // HTML with no plain text: only rich hooks hear it, never a plain field or text handler.
    m.set_content(ClipboardContent::new("").with_html("<i>only</i>"));
    rt.update(|s, _| s.take = false); rt.frame(0.0);
    paste(&mut rt);
    assert_eq!((rt.state.rich.text.as_str(), rt.state.hook.last().cloned().flatten().as_deref()), ("[rich]", Some("<i>only</i>")));
    assert!(rt.state.window.is_empty(), "a focused field's paste never falls through to the window");
    assert_eq!(rt.paste_forms(None), Forms::All);

    let mut plain_root = Runtime::new(Vec::<String>::new(), |_: &Vec<String>, _: &mut Cx<Vec<String>>| {
        col().size(200.0, 100.0).on_paste(|s: &mut Vec<String>, text, _| s.push(text.to_owned()))
    }, (200.0, 100.0), Env::new(true, 1.0));
    plain_root.use_clipboard(&m);
    plain_root.frame(0.0);
    assert_eq!(plain_root.paste_forms(None), Forms::Text, "a plain root on_paste never requests HTML or RTF");
    plain_root.update(|_, tx| tx.request_paste());
    plain_root.frame(0.0);
    assert!(plain_root.state.is_empty(), "an HTML-only clipboard never triggers a plain root on_paste");
}

#[test]
fn pasted_images_reach_hooks_only() {
    use crate::{ClipboardContent, ClipboardImage};
    use crate::clipboard::Forms;
    #[derive(Default)]
    struct State { plain: crate::TextEdit, rich: crate::TextEdit, hooked: crate::TextEdit, hook: Vec<ClipboardContent>, window: Vec<ClipboardContent>, reads: Vec<ClipboardContent> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = crate::TextStyle::new(16.0, 400.0);
        col().size(200.0, 150.0)
            .on_paste_content(|s: &mut State, content, _| s.window.push(content.clone()))
            .child(crate::text_input(&s.plain, style).key("plain").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.plain))
            .child(crate::text_input(&s.rich, style).key("rich").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.rich))
            .child(crate::text_input(&s.hooked, style).key("hooked").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.hooked)
                .on_input_paste(|s: &mut State, content, _| {
                    s.hook.push(content.clone());
                    let Some(image) = content.image() else { return false };
                    let (w, h) = image.size();
                    s.hooked.insert_pill(format!("image:{}", image.png().len()), &format!("{w}×{h}"));
                    true
                }))
    }
    let rgba: Vec<u8> = (0..12u8).flat_map(|i| [i * 20, 0, 255 - i * 20, 255]).collect();
    let image = ClipboardImage::from_rgba(4, 3, &rgba).unwrap();
    assert_eq!(image.size(), (4, 3));
    assert!(ClipboardImage::from_png(&b"GIF89a"[..]).is_none());
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(State { rich: crate::TextEdit::new("").rich(), hooked: crate::TextEdit::new("").rich(), ..State::default() }, view, (200.0, 150.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let paste = |rt: &mut Runtime<State>| { rt.update(|_, tx| tx.request_paste()); rt.frame(0.0); };
    let focus = |rt: &mut Runtime<State>, which: fn(&mut State) -> &mut crate::TextEdit| {
        rt.update(move |s, _| { s.plain.focused = false; s.rich.focused = false; s.hooked.focused = false; which(s).focused = true; });
        rt.frame(0.0);
    };

    // A screenshot: an image and no text.
    m.set_content(ClipboardContent::new("").with_image(image.clone()));
    paste(&mut rt);
    assert_eq!(rt.state.window.pop().and_then(|c| c.image().cloned()).as_ref(), Some(&image), "the root's on_paste_content gets the image");
    focus(&mut rt, |s| &mut s.plain);
    paste(&mut rt);
    focus(&mut rt, |s| &mut s.rich);
    assert_eq!(rt.paste_forms(rt.paste_target().as_deref()), Forms::Rich, "a rich field with no hook never fetches the image");
    paste(&mut rt);
    assert_eq!((rt.state.plain.text.as_str(), rt.state.rich.text.as_str()), ("", ""), "an image alone pastes nothing into a field without a hook");
    focus(&mut rt, |s| &mut s.hooked);
    assert_eq!(rt.paste_forms(rt.paste_target().as_deref()), Forms::All);
    paste(&mut rt);
    assert_eq!(rt.state.hooked.text, "4×3");
    assert_eq!(rt.state.hooked.pills().map(|(_, id)| id.to_owned()).collect::<Vec<_>>(), [format!("image:{}", image.png().len())]);
    assert!(rt.state.window.is_empty(), "a focused field's paste never reaches the root");

    // Text beside an image: a declining hook leaves the text to the editor.
    m.set_content(ClipboardContent::new(" caption"));
    paste(&mut rt);
    assert_eq!(rt.state.hooked.text, "4×3 caption");
    assert!(rt.state.hook.last().is_some_and(|c| c.image().is_none()));

    m.set_content(ClipboardContent::new("alt").with_image(image.clone()));
    rt.update(|_, tx| tx.read_clipboard(|s: &mut State, content, _| s.reads.push(content.clone())));
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.reads.iter().map(|c| (c.text(), c.image().map(ClipboardImage::size))).collect::<Vec<_>>(), [("alt", Some((4, 3)))]);
}

#[test]
fn copied_images_and_files_round_trip_and_reach_only_hooks() {
    use std::path::PathBuf;
    use crate::{ClipboardContent, ClipboardImage};
    #[derive(Default)]
    struct State { field: crate::TextEdit, rich: crate::TextEdit, reads: Vec<ClipboardContent> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = crate::TextStyle::new(16.0, 400.0);
        col().size(200.0, 100.0)
            .child(crate::text_input(&s.field, style).key("field").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.field))
            .child(crate::text_input(&s.rich, style).key("rich").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.rich))
    }
    let rgba: Vec<u8> = (0..4u8).flat_map(|i| [i * 60, 0, 0, 255]).collect();
    let image = ClipboardImage::from_rgba(2, 2, &rgba).unwrap();
    let files = [PathBuf::from("/fixtures/a.txt"), PathBuf::from("/fixtures/b c.png")];
    let only_files = ClipboardContent::default().with_files(files.clone());
    assert!(only_files.is_rich() && !only_files.is_empty() && !only_files.has_plain());
    assert_eq!(only_files.files(), &files[..]);
    assert_ne!(only_files, ClipboardContent::default(), "files take part in equality");
    assert!(!format!("{only_files:?}").contains("fixtures"));

    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(State { rich: crate::TextEdit::new("").rich(), ..State::default() }, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let copied = ClipboardContent::new("two files").with_image(image.clone()).with_files(files.clone());
    rt.update(|_, tx| tx.copy_rich(copied.clone()));
    assert_eq!(m.content().as_ref(), Some(&copied), "the memory clipboard keeps every form");
    rt.update(|_, tx| tx.read_clipboard(|s: &mut State, content, _| s.reads.push(content.clone())));
    rt.frame(0.0);
    assert_eq!(rt.state.reads.pop().as_ref(), Some(&copied), "a read gets the image and files back");

    rt.update(|_, tx| tx.copy_rich(only_files.clone()));
    for which in [(|s: &mut State| &mut s.field) as fn(&mut State) -> &mut crate::TextEdit, |s| &mut s.rich] {
        rt.update(move |s, _| { s.field.focused = false; s.rich.focused = false; which(s).focused = true; });
        rt.frame(0.0);
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
    }
    assert_eq!((rt.state.field.text.as_str(), rt.state.rich.text.as_str()), ("", ""), "files alone paste nothing into a field without a hook");
}

#[test]
fn a_paste_of_files_alone_keeps_the_selection_of_a_field_without_a_hook() {
    use crate::ClipboardContent;
    struct State { field: crate::TextEdit }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 100.0).child(crate::text_input(&s.field, crate::TextStyle::new(16.0, 400.0)).key("field").size(200.0, 40.0)
            .on_edit(|s: &mut State| &mut s.field))
    }
    for rich in [false, true] {
        let mut field = crate::TextEdit::new("keep me");
        if rich { field = field.rich(); }
        field.focused = true;
        field.set_selection(0, 7);
        let m = crate::clipboard::memory();
        let mut rt = Runtime::new(State { field }, view, (200.0, 100.0), Env::new(true, 1.0));
        rt.use_clipboard(&m);
        rt.frame(0.0);
        rt.update(|_, tx| tx.copy_rich(ClipboardContent::default().with_files([std::path::PathBuf::from("/fixtures/a.txt")])));
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
        rt.frame(0.0);
        assert_eq!((rt.state.field.text.as_str(), rt.state.field.selected()), ("keep me", "keep me"), "rich {rich}");
    }
}

#[test]
fn read_clipboard_answers_once_even_when_empty_and_pastes_nowhere() {
    use crate::ClipboardContent;
    #[derive(Default)]
    struct State { field: crate::TextEdit, reads: Vec<ClipboardContent> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 100.0).child(crate::text_input(&s.field, crate::TextStyle::new(16.0, 400.0)).key("field").size(200.0, 40.0)
            .on_edit(|s: &mut State| &mut s.field))
    }
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(State::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    rt.update(|s, _| s.field.focused = true); rt.frame(0.0);
    let read = |rt: &mut Runtime<State>| {
        rt.update(|_, tx| tx.read_clipboard(|s: &mut State, content, _| s.reads.push(content.clone())));
        rt.frame(0.0);
        rt.frame(0.0);
    };
    read(&mut rt);
    assert_eq!(rt.state.reads, [ClipboardContent::default()], "an empty clipboard still answers");
    m.set_content(ClipboardContent::new("a").with_html("<a href='javascript:x'>a</a>").with_rtf("{\\rtf1 a}"));
    read(&mut rt);
    let got = rt.state.reads.pop().unwrap();
    assert_eq!((got.text(), got.html(), got.rtf()), ("a", Some("<a>a</a>"), Some("{\\rtf1 a}")));
    assert!(rt.state.field.text.is_empty() && rt.state.reads.len() == 1);
    assert!(rt.clipboard_reads.is_empty() && rt.read_requests.is_empty());
}

#[test]
fn clipboard_requests_follow_handler_order_and_are_consumed_once() {
    use crate::edit::{Key, KeyEvent, Mods, TextEdit};
    fn view(edit: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        col()
            .child(el().key("copy").size(40.0, 28.0).on_press_tx(|_: &mut TextEdit, tx| tx.copy("response")))
            .child(crate::el::text_input(edit, crate::tok::TextStyle::new(14.0, 400.0)).key("input").on_edit(|e: &mut TextEdit| e))
    }
    let mut edit = TextEdit::new("héllo 👋");
    edit.anchor = 0;
    let mut rt = Runtime::new(edit, view, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let button = rt.nodes.iter().find(|n| n.key.as_deref() == Some("copy")).unwrap().path.clone();
    let input = rt.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
    rt.set_focus(Some(input), true);
    let copy_key = KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } };
    rt.queue.push(Action::Press(button.clone(), crate::PointerButton::Primary));
    rt.queue.push(Action::Key { ev: copy_key.clone(), repeat: false, stage: false });
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.copy_request.take().as_deref(), Some("héllo 👋"));
    rt.frame(1.0 / 120.0);
    assert!(rt.copy_request.is_none());
    rt.queue.push(Action::Key { ev: copy_key, repeat: false, stage: false });
    rt.queue.push(Action::Press(button, crate::PointerButton::Primary));
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.copy_request.take().as_deref(), Some("response"));
}
