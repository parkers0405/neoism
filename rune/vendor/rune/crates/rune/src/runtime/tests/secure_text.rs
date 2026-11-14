use super::*;


#[test]
fn secure_input_masks_layout_inspector_and_unicode_pointer_indices() {
    fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(s, crate::TextStyle::new(16.0, 400.0)).single_line().secure()
            .key("secret").size(200.0, 40.0).on_edit(|s: &mut crate::TextEdit| s)
    }
    let mut edit = crate::TextEdit::new("hé🙂");
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (200.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.state.is_secure() && rt.has_secure_focus());
    assert!(rt.ime_area.is_none());
    assert!(rt.elements().iter().all(|e| e.text.is_none()));
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    assert_eq!(n.text.as_ref().unwrap().content, "•••");
    let path = n.path.clone();
    let layout = rt.focused_layout().unwrap();
    let caret = Cursor::from_byte_index(&layout, 6, Affinity::Downstream).geometry(&layout, 1.0);
    rt.queue.push(Action::Focus(path, caret.x0 as f32 + 0.1, ((caret.y0 + caret.y1) * 0.5) as f32));
    rt.frame(0.0);
    assert_eq!(rt.state.focus, 3);
    rt.paste("界"); rt.frame(0.0);
    assert_eq!(rt.state.text, "hé界🙂");
    rt.key(KeyEvent { key: crate::Key::Command('a'), mods: Default::default() }); rt.frame(0.0);
    rt.key(KeyEvent { key: crate::Key::Command('c'), mods: Default::default() }); rt.frame(0.0);
    assert!(rt.copy_request.is_none());
    rt.ime(Ime::Commit("ignored".into())); rt.frame(0.0);
    assert_eq!(rt.state.text, "hé界🙂");
    assert!(rt.events.iter().all(|e| !e.contains("hé")));
}

/// The app clears its password as the card leaves; the exiting copy of the field must
/// hold no plaintext from that frame on, yet paint the same dots and no placeholder.
#[test]
fn an_exiting_secure_field_paints_dots_without_its_secret() {
    use crate::{text_input, TextEdit, TextStyle};
    struct Card { pw: TextEdit, open: bool }
    fn view(s: &Card, _: &mut Cx<Card>) -> El {
        let mut root = col().key("root").size(200.0, 100.0);
        if s.open {
            root = root.child(text_input(&s.pw, TextStyle::new(14.0, 400.0)).single_line()
                .placeholder("Enter your password", Color::WHITE).key("pw").size(200.0, 40.0)
                .transition(Transition::blur_replace()).on_edit(|s: &mut Card| &mut s.pw));
        }
        root
    }
    let mut rt = Runtime::new(Card { pw: TextEdit::new("hunter2").secure(), open: true }, view, (200.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let painted = rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count();
    rt.update(|s, _| { s.pw.clear(); s.open = false; });
    assert!(rt.frame(1.0 / 60.0));
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("pw")).expect("exiting field stays drawn");
    assert!(rt.is_exiting(n.id));
    let edit = &n.input.as_ref().unwrap().edit;
    assert_eq!(edit.text, "•".repeat(7), "only the mask remains");
    assert!(!edit.has_history() && !edit.focused);
    assert_eq!(n.text.as_ref().unwrap().content, "•".repeat(7));
    // Exit blur adds balanced layer markers; compare the actual dots, not scope records.
    assert_eq!(rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count(), painted, "the same dots, no placeholder");
    assert!(!format!("{:?}", rt.elements()).contains("hunter2"));

    // A revealed secure field must also conceal to bullets on the very first exit frame,
    // leaving no plaintext in `input.edit`, `text.content`, `text_layout`, or `Debug`.
    let mut rt = Runtime::new(
        Card { pw: TextEdit::new("hunter2").secure().revealed(true), open: true },
        view,
        (200.0, 100.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    let n_open = rt.nodes.iter().find(|n| n.key.as_deref() == Some("pw")).unwrap();
    assert_eq!(n_open.text.as_ref().unwrap().content, "hunter2");
    assert!(!format!("{:?}", n_open.input).contains("hunter2"));
    assert!(!format!("{:?}", n_open.text).contains("hunter2"));
    rt.update(|s, _| { s.pw.clear(); s.open = false; });
    assert!(rt.frame(1.0 / 60.0));
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("pw")).expect("exiting field stays drawn");
    assert!(rt.is_exiting(n.id));
    let edit = &n.input.as_ref().unwrap().edit;
    assert!(!edit.is_revealed(), "conceal resets revealed on exit");
    assert_eq!(edit.text, "•".repeat(7), "only the mask remains");
    assert_eq!(n.text.as_ref().unwrap().content, "•".repeat(7));
    assert_eq!(rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count(), painted, "exiting revealed field paints bullets");
    assert!(!format!("{:?}", rt.elements()).contains("hunter2"));
    assert!(!format!("{:?}", n.input).contains("hunter2"));
    assert!(!format!("{:?}", n.text).contains("hunter2"));
}

#[test]
fn secure_revealed_display_does_not_survive_in_layout_or_raster_caches() {
    use crate::{text_input, TextEdit, TextStyle};
    fn view(s: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        col().key("cache").cache().size(240.0, 100.0).child(
            text_input(s, TextStyle::new(16.0, 400.0)).key("secret")
                .single_line().size(220.0, 40.0).selectable(true)
                .text_case(crate::TextCase::Uppercase)
                .on_edit(|s: &mut TextEdit| s),
        )
    }
    let mut rt = Runtime::new(TextEdit::new("sensitive fixture").secure().revealed(true), view,
        (240.0, 100.0), Env::new(true, 1.0));
    let cached = rt.text.cached_layout_count();
    rt.frame(0.0);
    let i = rt.nodes.iter().position(|n| n.key.as_deref() == Some("secret")).unwrap();
    let n = &rt.nodes[i];
    assert!(!rt.is_selectable(i), "selectable cannot opt secure input into text selection");
    assert_eq!(n.text.as_ref().unwrap().content, "sensitive fixture");
    assert!(n.text.as_ref().unwrap().source.is_none(), "case conversion is ignored on inputs");
    assert_eq!(Rc::strong_count(n.text_layout.as_ref().unwrap()), 1, "only the node owns the shaped run");
    assert_eq!(rt.text.cached_layout_count(), cached, "measurement and paint leave no cached run");
    assert!(rt.node_caches.is_empty(), "an unfocused secret also disables ancestor raster caches");
    assert!(rt.text.atlas.resident_caches.is_empty());
    let path = n.path.clone();
    assert!(!rt.selection_menu(&path, 10.0, 10.0), "no context-menu copy");
    rt.set_focus(Some(path), true);
    rt.frame(0.0);
    let layout = rt.focused_layout().unwrap();
    assert_eq!(Rc::strong_count(&layout), 1, "input hit testing also bypasses the layout cache");
    drop(layout);
    let mut tx = Tx::default();
    // Application key-up hooks may observe their own events, but Rune must not keep
    // typed secrets in the dev/event log when such a hook handles the release.
    let root = rt.nodes.iter().position(|n| n.key.as_deref() == Some("cache")).unwrap();
    rt.nodes[root].focus.key_up = Some(Rc::new(|_, _, _| true));
    rt.events.clear();
    assert!(rt.route_key_up(&KeyEvent { key: crate::Key::Text("private fixture".into()), mods: Default::default() }, &mut tx));
    assert!(rt.events.iter().any(|e| e == "key-up [redacted]"));
    assert!(!format!("{:?}", rt.events).contains("private fixture"));
    rt.update(|s, _| s.set_revealed(false));
    rt.frame(0.0);
    assert_eq!(rt.text.cached_layout_count(), cached);
    rt.update(|s, _| s.clear());
    rt.frame(0.0);
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    assert!(rt.state.text.is_empty() && n.input.as_ref().unwrap().edit.text.is_empty());
    assert!(n.text.as_ref().unwrap().content.is_empty(), "clearing without exit wipes the display");
    assert_eq!(rt.text.cached_layout_count(), cached);
}

#[test]
fn revealed_secure_input_matches_plain_layout_and_keeps_protections() {
    use crate::{text_input, TextEdit, TextStyle};
    struct Form {
        plain: TextEdit,
        secret: TextEdit,
    }
    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        let style = TextStyle::new(16.0, 400.0);
        col().size(240.0, 100.0)
            .child(text_input(&s.plain, style).single_line().key("plain").size(200.0, 40.0).on_edit(|s: &mut Form| &mut s.plain))
            .child(text_input(&s.secret, style).single_line().key("secret").size(200.0, 40.0).on_edit(|s: &mut Form| &mut s.secret))
    }
    let mut plain = TextEdit::new("hé🙂 world");
    plain.set_selection(1, 7);
    let mut secret = TextEdit::new("hé🙂 world").secure().revealed(true);
    secret.set_selection(1, 7);
    secret.focused = true;
    let mut rt = Runtime::new(Form { plain, secret }, view, (240.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    assert!(rt.state.secret.is_secure() && rt.state.secret.is_revealed());
    assert!(rt.has_secure_focus());
    assert!(rt.ime_area.is_none());
    let els = rt.elements();
    let plain_el = els.iter().find(|e| e.key.as_deref() == Some("plain")).unwrap();
    let secret_el = els.iter().find(|e| e.key.as_deref() == Some("secret")).unwrap();
    assert_eq!(plain_el.text.as_deref(), Some("hé🙂 world"));
    assert!(secret_el.text.is_none(), "accessibility text remains None when revealed");

    let np = rt.nodes.iter().find(|n| n.key.as_deref() == Some("plain")).unwrap();
    let ns = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    assert_eq!(ns.text.as_ref().unwrap().content, "hé🙂 world");
    assert!(!format!("{:?}", ns.input).contains("hé🙂"));
    assert!(!format!("{:?}", ns.text).contains("hé🙂"));
    let lp = np.text_layout.as_ref().unwrap().clone();
    let ls = ns.text_layout.as_ref().unwrap().clone();
    assert_eq!(lp.full_width(), ls.full_width(), "revealed layout width matches plain text");
    let secret_caret = secret_el.caret.expect("focused revealed field has caret");
    let secret_sel: Vec<[f32; 4]> = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && i.rect[1] >= secret_el.rect[1] && i.rect[1] < secret_el.rect[1] + secret_el.rect[3])
        .map(|i| [i.rect[0] - secret_el.rect[0], i.rect[1] - secret_el.rect[1], i.rect[2], i.rect[3]])
        .collect();
    assert!(!secret_sel.is_empty());

    // Focus the plain field with the same selection and verify caret and selection rects match.
    rt.update(|s, _| { s.secret.focused = false; s.plain.focused = true; });
    rt.frame(0.0);
    let plain_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("plain")).unwrap();
    let plain_caret = plain_el.caret.expect("focused plain field has caret");
    assert_eq!([plain_caret[0] - plain_el.rect[0], plain_caret[1] - plain_el.rect[1], plain_caret[2], plain_caret[3]],
        [secret_caret[0] - secret_el.rect[0], secret_caret[1] - secret_el.rect[1], secret_caret[2], secret_caret[3]]);
    let plain_sel: Vec<[f32; 4]> = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && i.rect[1] >= plain_el.rect[1] && i.rect[1] < plain_el.rect[1] + plain_el.rect[3])
        .map(|i| [i.rect[0] - plain_el.rect[0], i.rect[1] - plain_el.rect[1], i.rect[2], i.rect[3]])
        .collect();
    assert_eq!(plain_sel, secret_sel);

    // Refocus secret field: copy, cut, undo and IME must do nothing, and toggling revealed re-measures.
    rt.update(|s, _| { s.plain.focused = false; s.secret.focused = true; });
    rt.frame(0.0);
    for ch in ['c', 'x'] {
        rt.key(KeyEvent { key: crate::Key::Command(ch), mods: crate::Mods { cmd: true, ..Default::default() } });
        rt.frame(0.0);
        assert!(rt.copy_request.is_none(), "⌘{} on revealed field must never write to clipboard", ch.to_uppercase());
        assert_eq!(rt.state.secret.text, "hé🙂 world");
    }
    rt.ime(Ime::Preedit("comp".into(), Some((0, 4))));
    rt.ime(Ime::Commit("commit".into()));
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "hé🙂 world");
    rt.key(KeyEvent { key: crate::Key::Text("X".into()), mods: Default::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "hX world");
    rt.key(KeyEvent { key: crate::Key::Command('z'), mods: crate::Mods { cmd: true, ..Default::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "hX world", "no undo on revealed field");

    let revealed_w = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap().text_layout.as_ref().unwrap().full_width();
    rt.update(|s, _| { s.secret.set_selection(1, 4); s.secret.set_revealed(false); });
    rt.frame(0.0);
    assert_eq!(rt.state.secret.range(), (1, 4), "toggling revealed keeps selection");
    let ns = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    assert_eq!(ns.text.as_ref().unwrap().content, "••••••••");
    let hidden_w = ns.text_layout.as_ref().unwrap().full_width();
    assert_ne!(revealed_w, hidden_w, "toggling revealed re-layouts the text");
}

#[test]
fn multiline_secure_input_hides_length_and_lines_wraps_anywhere_and_conceals_on_exit() {
    use crate::{text_input, TextEdit, TextStyle};
    struct State {
        secret: TextEdit,
        open: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().size(240.0, 200.0);
        if s.open {
            root = root.child(
                text_input(&s.secret, TextStyle::new(14.0, 400.0))
                    .secure()
                    .key("secret")
                    .w(180.0)
                    .on_edit(|s: &mut State| &mut s.secret),
            );
        }
        root
    }

    let short_secret = "short-key!";
    let long_secret = (0..50)
        .map(|i| format!("FAKEPEMLINE{i:02}AAAAB3NzaC1yc2EAAAADAQABAAABAQDfakebase64payloadblock0123456789ABCDEFGH=="))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(long_secret.len() >= 4096);

    let mut short_edit = TextEdit::new(short_secret);
    short_edit.focused = true;
    let mut rt_short = Runtime::new(State { secret: short_edit, open: true }, view, (240.0, 200.0), Env::new(true, 1.0));
    rt_short.frame(0.0);

    let mut long_edit = TextEdit::new(long_secret.clone());
    long_edit.focused = true;
    let mut rt_long = Runtime::new(State { secret: long_edit, open: true }, view, (240.0, 200.0), Env::new(true, 1.0));
    rt_long.frame(0.0);

    let el_short = rt_short.elements().into_iter().find(|e| e.key.as_deref() == Some("secret")).unwrap();
    let el_long = rt_long.elements().into_iter().find(|e| e.key.as_deref() == Some("secret")).unwrap();
    assert!(el_short.text.is_none() && el_long.text.is_none());
    assert_eq!(el_short.rect, el_long.rect, "hidden multi-line rect must not leak content length or line count");
    assert_eq!(el_short.caret, el_long.caret, "hidden multi-line caret rect must be identical");

    let n_short = rt_short.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    let n_long = rt_long.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    assert_eq!(n_short.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);
    assert_eq!(n_long.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);
    let l_short = n_short.text_layout.as_ref().unwrap();
    let l_long = n_long.text_layout.as_ref().unwrap();
    assert_eq!(l_short.len(), 1);
    assert_eq!(l_long.len(), 1);
    assert_eq!(l_short.full_width(), l_long.full_width());
    let glyphs_short = rt_short.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count();
    let glyphs_long = rt_long.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count();
    assert_eq!(glyphs_short, 12);
    assert_eq!(glyphs_long, 12);

    // Clicking near the left edge of the hidden mask pins the caret to text.len(), never leaking an interior byte index.
    rt_long.pointer_down(el_long.rect[0] + 4.0, el_long.rect[1] + 4.0);
    rt_long.pointer_up(el_long.rect[0] + 4.0, el_long.rect[1] + 4.0);
    rt_long.frame(0.0);
    assert_eq!(rt_long.state.secret.focus, long_secret.len());

    // Revealing wraps unbroken lines at character boundaries within 180pt width.
    rt_long.update(|s, _| s.secret.set_revealed(true));
    rt_long.frame(0.0);
    let n_rev = rt_long.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    let l_rev = n_rev.text_layout.as_ref().unwrap();
    assert!(l_rev.len() > 100, "50 long unbroken lines must wrap into >100 visual lines");
    assert!(l_rev.full_width() <= 180.5, "wrapped unbroken lines must fit within field width (got {})", l_rev.full_width());

    // Exiting while revealed conceals back to the fixed multi-line mask on the first exit frame.
    rt_long.update(|s, _| s.open = false);
    rt_long.frame(0.0);
    let exiting = rt_long.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).expect("exiting node present");
    let exit_edit = &exiting.input.as_ref().unwrap().edit;
    assert!(!exit_edit.is_revealed());
    assert_eq!(exit_edit.text, crate::edit::MULTILINE_SECURE_MASK);
    assert_eq!(exiting.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);
    assert_eq!(exiting.text_layout.as_ref().unwrap().len(), 1);
    assert!(!format!("{:?}", exiting.input).contains("FAKEPEMLINE"));
    assert!(!format!("{:?}", exiting.text).contains("FAKEPEMLINE"));
}

#[test]
fn secure_multiline_conceal_snaps_geometry_scroll_and_bypasses_caches() {
    fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        col().cache().w(120.0).child(crate::auto_grow(
            crate::text_input(s, crate::TextStyle::new(14.0, 400.0))
                .w_full().key("secret").on_edit(|s: &mut crate::TextEdit| s), 1, 3))
    }
    let mut edit = crate::TextEdit::new("fictional line\n".repeat(60)).secure().revealed(true);
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (180.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.text.cached_layout_count(), 0);
    assert!(rt.node_caches.is_empty());
    let scroll_id = rt.nodes.iter().find(|n| n.scroll.is_some()).unwrap().id;
    assert!(rt.scrolls[&scroll_id].max > 100.0);
    rt.scrolls.get_mut(&scroll_id).unwrap().offset = 300.0;
    rt.update(|s, _| s.set_revealed(false)); // No tx.instant(): conceal must do this itself.
    rt.frame(0.0);
    let st = &rt.scrolls[&scroll_id];
    assert_eq!((st.offset, st.max), (0.0, 0.0));
    assert!(st.glide().is_none());
    let elements = rt.elements();
    for e in &elements {
        assert_eq!(e.rect[3], e.target[3], "concealed height must not spring: {}", e.path);
    }
    let secret = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    assert_eq!(secret.text_layout.as_ref().unwrap().len(), 1);
    assert_eq!(Rc::strong_count(secret.text_layout.as_ref().unwrap()), 1);
    assert_eq!(rt.text.cached_layout_count(), 0);
    assert!(rt.ime_area.is_none());
    let info = elements.iter().find(|e| e.key.as_deref() == Some("secret")).unwrap();
    assert!(info.text.is_none() && info.selected.is_none());

    // Even a narrow hidden field keeps one mask line, including after wrap_anywhere.
    fn narrow(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(s, crate::TextStyle::new(14.0, 400.0)).wrap_anywhere().w(12.0)
    }
    let mut narrow = Runtime::new(rt.state.clone(), narrow, (50.0, 100.0), Env::new(true, 1.0));
    narrow.frame(0.0);
    assert_eq!(narrow.nodes[0].text_layout.as_ref().unwrap().len(), 1);
}

#[test]
fn wrap_anywhere_wraps_unbroken_tokens_keeps_word_breaks_and_lowers_min_content_width() {
    use crate::{rich, row, text, Span};
    const LONG_BASE64: &str = "AAAAB3NzaC1yc2EAAAADAQABAAABAQDfakebase64publickeymaterialwithoutspaces0123456789ABCDEF==";
    const WORDS: &str = "alpha beta gamma delta epsilon zeta eta theta";

    let mut explicit = text("token");
    explicit.style.set_flex_shrink(0.25);
    assert_eq!(explicit.wrap_anywhere().style.flex_shrink(), 0.25);
    assert_eq!(text("token").wrap_anywhere().style.flex_shrink(), 0.0);

    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .size(400.0, 500.0)
            // Default .wrap() vs .wrap_anywhere() in a narrow 160pt box.
            .child(col().w(160.0).child(text(LONG_BASE64).wrap().key("default_wrap")))
            .child(col().w(160.0).child(text(LONG_BASE64).wrap_anywhere().key("anywhere_wrap")))
            // In a flex row, wrap_anywhere lowers min-content width so the flex parent can shrink it.
            .child(row().w(160.0).child(text(LONG_BASE64).wrap_anywhere().shrink().key("row_anywhere")))
            // Normal words with spaces in the same 160pt width.
            .child(col().w(160.0).child(text(WORDS).wrap().key("words_default")))
            .child(col().w(160.0).child(text(WORDS).wrap_anywhere().key("words_anywhere")))
            // Rich text with wrap_anywhere.
            .child(col().w(160.0).child(rich(LONG_BASE64, vec![Span::new(0, 16).bold()]).wrap_anywhere().key("rich_anywhere")))
    }

    let mut rt = Runtime::new((), view, (400.0, 500.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let node = |k: &str| rt.nodes.iter().find(|n| n.key.as_deref() == Some(k)).unwrap();
    let el = |k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).unwrap();

    // Default .wrap() does not break inside an unbroken token and keeps its full min-content width.
    let def_layout = node("default_wrap").text_layout.as_ref().unwrap();
    assert_eq!(def_layout.len(), 1, "default wrap leaves unbroken base64 on 1 line");
    assert!(def_layout.full_width() > 400.0, "default wrap keeps full unbroken token width");

    // .wrap_anywhere() wraps inside the 160pt box and shrinks inside a 160pt flex row with no glyph past the right edge.
    let any_el = el("anywhere_wrap");
    let any_node = node("anywhere_wrap");
    let any_layout = any_node.text_layout.as_ref().unwrap();
    assert!(any_el.rect[2] <= 160.5, "wrap_anywhere fits 160pt box (got {})", any_el.rect[2]);
    assert!(el("row_anywhere").rect[2] <= 160.5, "wrap_anywhere shrinks inside 160pt flex row (got {})", el("row_anywhere").rect[2]);
    assert!(any_layout.len() >= 4, "long base64 wraps across multiple lines (got {})", any_layout.len());
    assert!(any_layout.full_width() <= 160.5, "no line exceeds the 160pt box (got {})", any_layout.full_width());
    for line in any_layout.lines() {
        assert!(
            line.metrics().advance - line.metrics().hanging_advance <= 160.5,
            "line advance {} exceeds 160pt right edge",
            line.metrics().advance
        );
    }

    // Min-content width (max_width = Some(0.0)) shrinks from the full token width to a single character's width.
    let min_def = rt.text.layout_for_spec(node("default_wrap").text.as_ref().unwrap(), 1.0, Some(0.0), parley::Alignment::Start);
    let min_any = rt.text.layout_for_spec(any_node.text.as_ref().unwrap(), 1.0, Some(0.0), parley::Alignment::Start);
    assert!(min_def.full_width() > 400.0);
    assert!(min_any.full_width() < 20.0, "wrap_anywhere min-content width is a single grapheme (got {})", min_any.full_width());

    // Normal words that fit on a line still break at spaces identically to .wrap().
    let w_def = node("words_default").text_layout.as_ref().unwrap();
    let w_any = node("words_anywhere").text_layout.as_ref().unwrap();
    let ranges_def: Vec<_> = w_def.lines().map(|l| l.text_range()).collect();
    let ranges_any: Vec<_> = w_any.lines().map(|l| l.text_range()).collect();
    assert_eq!(ranges_def, ranges_any, "normal words break at spaces identically under wrap_anywhere");

    // Rich text also supports wrap_anywhere.
    let rich_layout = node("rich_anywhere").text_layout.as_ref().unwrap();
    assert!(rich_layout.len() >= 4 && rich_layout.full_width() <= 160.5);
}

#[test]
fn secure_label_masks_reveals_conceals_and_never_leaks_plaintext() {
    use crate::{text, Transition};
    const SINGLE_SECRET: &str = "sk-fictional-token-987654";
    const MULTI_SECRET: &str = "-----BEGIN FAKE KEY-----\nAAAAB3NzaC1yc2EAAAADAQABAAABAQDfakebase64payload0123456789ABCDEFGH==\n-----END FAKE KEY-----";

    #[derive(Clone)]
    struct State {
        revealed: bool,
        open: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().cache().size(240.0, 240.0);
        if s.open {
            root = root
                .child(
                    text(SINGLE_SECRET)
                        .secure()
                        .revealed(s.revealed)
                        .selectable(true)
                        .text_case(crate::TextCase::Uppercase)
                        .transition(Transition::blur_replace())
                        .key("single"),
                )
                .child(
                    text(SINGLE_SECRET)
                        .selectable(true)
                        .secure()
                        .revealed(s.revealed)
                        .truncate()
                        .w(60.0)
                        .key("trunc"),
                )
                .child(
                    col()
                        .w(140.0)
                        .max_h(44.0)
                        .scroll(ScrollSpec { stick_bottom: false })
                        .key("scroll")
                        .child(
                            text(MULTI_SECRET)
                                .wrap_anywhere()
                                .secure()
                                .revealed(s.revealed)
                                .transition(Transition::blur_replace())
                                .key("multi"),
                        ),
                )
                .child(
                    col().w(140.0).child(
                        text(MULTI_SECRET)
                            .secure()
                            .max_lines(2)
                            .revealed(s.revealed)
                            .key("clamped"),
                    ),
                );
        }
        root
    }

    let mut rt = Runtime::new(State { revealed: false, open: true }, view, (240.0, 240.0), Env::new(true, 1.0));
    let cached0 = rt.text.cached_layout_count();
    rt.frame(0.0);

    // 1. All hidden labels show the same fixed mask, hiding length and line count.
    let check_no_leak = |rt: &mut Runtime<State>| {
        let tree = crate::tooling::tree_json(rt, 0.0).to_string();
        let specs: Vec<_> = rt.nodes.iter().map(|n| (&n.text, &n.input)).collect();
        let dbg = format!("{:?} {:?}", rt.elements(), specs);
        for needle in ["sk-fictional", "987654", "BEGIN FAKE KEY", "AAAAB3Nza"] {
            assert!(!tree.contains(needle), "--tree leaked {needle}: {tree}");
            assert!(!dbg.contains(needle), "Debug/elements leaked {needle}");
        }
        assert_eq!(rt.text.cached_layout_count(), cached0, "secure labels must bypass TextSystem layout/shape caches");
        assert!(rt.node_caches.is_empty(), "secure labels must disable ancestor El::cache raster");
        assert!(rt.text.atlas.resident_caches.is_empty());
        for key in ["single", "trunc", "multi", "clamped"] {
            let i = rt.nodes.iter().position(|n| n.key.as_deref() == Some(key)).unwrap();
            let path = rt.nodes[i].path.clone();
            assert_eq!(Rc::strong_count(rt.nodes[i].text_layout.as_ref().unwrap()), 1, "{key} layout owned only by node");
            assert!(!rt.is_selectable(i), "{key} must not be selectable");
            assert!(!rt.selection_menu(&path, 5.0, 5.0), "{key} must not offer Copy menu");
            let info = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
            assert_eq!(info.role, "text");
            assert!(rt.element_is_secure(&info.path), "{key} must report secure state");
            assert!(info.text.is_none(), "{key} must omit ElementInfo::text");
            assert!(info.selected.is_none(), "{key} must omit ElementInfo::selected");
        }
        assert!(tree.contains("\"secure\":true"), "--tree must include secure flag: {tree}");
    };
    check_no_leak(&mut rt);

    let n_single = rt.nodes.iter().find(|n| n.key.as_deref() == Some("single")).unwrap();
    assert_eq!(n_single.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);
    let n_multi = rt.nodes.iter().find(|n| n.key.as_deref() == Some("multi")).unwrap();
    assert_eq!(n_multi.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);
    assert_eq!(n_multi.text_layout.as_ref().unwrap().len(), 1);
    let n_clamped = rt.nodes.iter().find(|n| n.key.as_deref() == Some("clamped")).unwrap();
    assert_eq!(n_clamped.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);
    assert_eq!(n_clamped.text_layout.as_ref().unwrap().len(), 1);

    // Try selecting and copying via pointer + Cmd+C while hidden.
    let single_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("single")).unwrap().rect;
    rt.pointer_down(single_rect[0] + 2.0, single_rect[1] + 4.0);
    rt.pointer_move(single_rect[0] + 60.0, single_rect[1] + 4.0);
    rt.pointer_up(single_rect[0] + 60.0, single_rect[1] + 4.0);
    rt.key(KeyEvent { key: crate::Key::Command('c'), mods: crate::Mods { cmd: true, ..Default::default() } });
    assert!(rt.selected_text().is_none() && rt.copy_request.is_none());

    // 2. Revealed state: lays out plaintext (with wrap_anywhere, max_lines, truncate) without leaking to caches/tree/Debug.
    rt.update(|s, _| s.revealed = true);
    rt.frame(0.0);
    check_no_leak(&mut rt);

    let n_single = rt.nodes.iter().find(|n| n.key.as_deref() == Some("single")).unwrap();
    assert_eq!(n_single.text.as_ref().unwrap().content, SINGLE_SECRET);
    let n_trunc = rt.nodes.iter().find(|n| n.key.as_deref() == Some("trunc")).unwrap();
    assert!(n_trunc.elided.is_some(), "truncate elides overflowing revealed label");
    let n_multi = rt.nodes.iter().find(|n| n.key.as_deref() == Some("multi")).unwrap();
    assert!(n_multi.text_layout.as_ref().unwrap().len() >= 4, "wrap_anywhere wraps revealed multi-line label");
    let n_clamped = rt.nodes.iter().find(|n| n.key.as_deref() == Some("clamped")).unwrap();
    assert_eq!(n_clamped.text_layout.as_ref().unwrap().len(), 2, "max_lines(2) clamps revealed multi-line label");
    assert!(n_clamped.elided.is_some());

    // Try selecting and copying while revealed: still blocked.
    rt.pointer_down(single_rect[0] + 2.0, single_rect[1] + 4.0);
    rt.pointer_move(single_rect[0] + 60.0, single_rect[1] + 4.0);
    rt.pointer_up(single_rect[0] + 60.0, single_rect[1] + 4.0);
    rt.key(KeyEvent { key: crate::Key::Command('c'), mods: crate::Mods { cmd: true, ..Default::default() } });
    assert!(rt.selected_text().is_none() && rt.copy_request.is_none());

    // 3. Concealing (revealed(true) -> revealed(false)) wipes plaintext and snaps geometry and scroll without animating.
    let scroll_id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("scroll")).unwrap().id;
    assert!(rt.scrolls[&scroll_id].max > 10.0);
    rt.scrolls.get_mut(&scroll_id).unwrap().offset = 24.0;
    rt.update(|s, _| s.revealed = false); // No tx.instant(): conceal must snap itself.
    rt.frame(0.0);
    check_no_leak(&mut rt);
    let st = &rt.scrolls[&scroll_id];
    assert_eq!((st.offset, st.max), (0.0, 0.0), "concealing resets ancestor scroll immediately");
    for e in rt.elements() {
        assert_eq!(e.rect[3], e.target[3], "concealed height must not spring: {}", e.path);
    }
    let n_multi = rt.nodes.iter().find(|n| n.key.as_deref() == Some("multi")).unwrap();
    assert_eq!(n_multi.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);

    // 4. Exiting while revealed conceals to the mask on the first exit frame.
    rt.update(|s, tx| {
        s.revealed = true;
        tx.instant();
    });
    rt.frame(0.0);
    rt.update(|s, _| s.open = false);
    assert!(rt.frame(1.0 / 60.0));
    let ex_single = rt.nodes.iter().find(|n| n.key.as_deref() == Some("single")).expect("exiting single label");
    assert!(rt.is_exiting(ex_single.id));
    assert_eq!(ex_single.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);
    assert!(!format!("{:?}", ex_single.text).contains("sk-fictional"));

    if let Some(dir) = std::env::var_os("RUNE_SECURE_LABEL_STILLS") {
        use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer, write_png};
        use crate::{oklch, row, TextStyle};
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        fn card_view(revealed: &bool, _: &mut Cx<bool>) -> El {
            let bg = oklch(0.18, 0.01, 260.0);
            let surface = oklch(0.23, 0.012, 260.0);
            let border = oklch(0.34, 0.015, 260.0);
            let muted = oklch(0.68, 0.015, 260.0);
            let fg = oklch(0.95, 0.008, 260.0);
            let badge = if *revealed { "REVEALED" } else { "HIDDEN" };
            col()
                .size(420.0, 230.0)
                .padding(20.0)
                .gap(14.0)
                .fill(bg)
                .child(
                    row()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .child(text("Fictional Credential Detail").font(TextStyle::new(14.0, 600.0)).color(fg))
                        .child(text(badge).font(TextStyle::new(11.0, 600.0).mono()).color(muted)),
                )
                .child(
                    col()
                        .w_full()
                        .padding(12.0)
                        .gap(6.0)
                        .fill(surface)
                        .radius(8.0)
                        .border(1.0, border)
                        .child(text("API token (single-line secure label)").font(TextStyle::new(11.0, 500.0)).color(muted))
                        .child(
                            text(SINGLE_SECRET)
                                .font(TextStyle::new(13.0, 400.0).mono())
                                .color(fg)
                                .secure()
                                .revealed(*revealed),
                        ),
                )
                .child(
                    col()
                        .w_full()
                        .padding(12.0)
                        .gap(6.0)
                        .fill(surface)
                        .radius(8.0)
                        .border(1.0, border)
                        .child(text("SSH key (multi-line secure label, wrap_anywhere)").font(TextStyle::new(11.0, 500.0)).color(muted))
                        .child(
                            text(MULTI_SECRET)
                                .font(TextStyle::new(12.0, 400.0).mono())
                                .color(fg)
                                .wrap_anywhere()
                                .secure()
                                .revealed(*revealed),
                        ),
                )
        }
        let gpu = Gpu::headless();
        let (w, h) = (840, 460);
        for (name, rev) in [("secure-label-hidden.png", false), ("secure-label-revealed.png", true)] {
            let mut rt = Runtime::new(rev, card_view, (420.0, 230.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            let target = Offscreen::new(&gpu.device, w, h);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.07, 0.08, 0.09, 1.0]);
            target.copy(&mut enc);
            gpu.queue.submit([enc.finish()]);
            let out = target.read(&gpu.device);
            write_png(&dir.join(name), w, h, &out).unwrap();
        }
    }
}

#[test]
fn secure_rich_labels_have_no_links_flights_or_retained_measure_hashes() {
    fn view(revealed: &bool, _: &mut Cx<bool>) -> El {
        col().child(crate::rich("fictional-secret", vec![])
            .link(crate::Link::new(0..9, "fictional-url-secret").label("fictional-link-secret"))
            .secure().revealed(*revealed).key("secret"))
            .child(crate::text("destination").key("target"))
    }
    let mut rt = Runtime::new(true, view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    assert!(n.text.as_ref().unwrap().links.is_empty());
    assert_eq!(rt.layout.measure_hash(n.id.0), Some(Some(0)));
    assert!(rt.element_is_secure(&n.path));
    assert!(!rt.element_is_secure("missing"));
    assert!(!crate::tooling::tree_json(&rt, 0.0).to_string().contains("fictional-"));
    rt.capture_text_flight("secret".into(), "target".into());
    assert!(rt.flights.is_empty(), "a revealed secret must not survive as flying glyphs");
    rt.update(|s, _| *s = false);
    rt.frame(0.0);
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
    assert_eq!(n.text.as_ref().unwrap().content, crate::edit::MULTILINE_SECURE_MASK);
}

#[test]
fn hidden_multiline_pristine_composes_with_runtime_and_app_on_change() {
    use crate::{text_input, Key, KeyEvent, Mods, TextEdit, TextStyle};

    struct App {
        secret: TextEdit,
        saved: String,
        app_pristine: bool,
        revealed: bool,
    }
    impl App {
        fn new(saved: &str) -> Self {
            Self {
                secret: TextEdit::new(saved),
                saved: saved.to_owned(),
                app_pristine: !saved.is_empty(),
                revealed: false,
            }
        }
        /// Simulates an app that also implements the pristine-replace rule in `on_input_change`:
        /// if `TextEdit` had appended to `saved` while pristine, strip `saved`; if `TextEdit`
        /// had deleted a suffix of `saved`, clear it; otherwise keep `secret.text`.
        fn on_secret_change(&mut self) {
            if self.app_pristine && !self.revealed && !self.saved.is_empty() {
                self.app_pristine = false;
                if self.secret.text.len() > self.saved.len()
                    && let Some(inserted) = self.secret.text.strip_prefix(&self.saved)
                {
                    let inserted = inserted.to_owned();
                    self.secret.replace_text(inserted);
                } else if !self.secret.text.is_empty()
                    && self.secret.text.len() == self.saved.len() - 1
                    && self.saved.starts_with(&self.secret.text)
                {
                    self.secret.clear();
                }
            }
        }
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        col().size(240.0, 120.0).child(
            text_input(&s.secret, TextStyle::new(14.0, 400.0))
                .secure()
                .revealed(s.revealed)
                .key("secret")
                .w(200.0)
                .on_edit(|s: &mut App| &mut s.secret)
                .on_input_change(|s: &mut App, _| s.on_secret_change()),
        )
    }

    let focus_by_click = |rt: &mut Runtime<App>| {
        let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("secret")).unwrap().rect;
        rt.pointer_down(r[0] + 6.0, r[1] + 6.0);
        rt.pointer_up(r[0] + 6.0, r[1] + 6.0);
        rt.frame(0.0);
    };

    // Case 1: Typing after clicking and pressing arrows replaces the whole value, then appends.
    let mut rt = Runtime::new(App::new("old-fictional-private-key\nsecond-line"), view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    focus_by_click(&mut rt);
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Home, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Text("x".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "x");
    assert!(!rt.state.secret.is_pristine());
    rt.key(KeyEvent { key: Key::Text("y".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "xy");

    // Keyboard focus does not consume pristine state either.
    let mut tabbed = Runtime::new(App::new("fictional-key"), view, (240.0, 120.0), Env::new(true, 1.0));
    tabbed.frame(0.0);
    tabbed.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    tabbed.frame(0.0);
    assert!(tabbed.elements().iter().any(|e| e.key.as_deref() == Some("secret") && e.focused));
    assert!(tabbed.state.secret.is_pristine());
    tabbed.key(KeyEvent { key: Key::Text("t".into()), mods: Mods::default() });
    tabbed.frame(0.0);
    assert_eq!(tabbed.state.secret.text, "t");

    // Case 2: Pasting after Show -> Hide (without editing) replaces the whole value, then appends.
    let mut rt = Runtime::new(App::new("old-fictional-private-key\nsecond-line"), view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    focus_by_click(&mut rt);
    rt.update(|s, _| s.revealed = true);
    rt.frame(0.0);
    rt.update(|s, _| s.revealed = false);
    rt.frame(0.0);
    assert!(rt.state.secret.is_pristine());
    rt.paste("pasted-key\nline-2");
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "pasted-key\nline-2");
    rt.paste("\nline-3");
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "pasted-key\nline-2\nline-3");

    // Case 3: Backspace while pristine empties the field; subsequent typing starts from "".
    let mut rt = Runtime::new(App::new("old-fictional-private-key\nsecond-line"), view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    focus_by_click(&mut rt);
    rt.key(KeyEvent { key: Key::Backspace, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "");
    assert!(!rt.state.secret.is_pristine());
    rt.key(KeyEvent { key: Key::Text("n".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.secret.text, "n");
}
