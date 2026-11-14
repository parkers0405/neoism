#[test]
fn plain_edit_leaves_rich_keys_alone_and_pastes_plain_text() {
    let shift_enter = KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } };
    let mut e = TextEdit::new("hello");
    e.set_selection(0, 5);
    e.key(&cmd('b'), None);
    e.key(&cmd('i'), None);
    assert!(e.marks().is_empty() && e.is_plain());
    e.set(5, false);
    e.key(&shift_enter, None);
    assert_eq!((e.text.as_str(), e.soft_breaks()), ("hello\n", &[][..]));
    e.paste_html("<b>x</b><ul><li>y</li></ul>", "x\ny");
    assert_eq!(e.text, "hello\nx\ny");
    assert!(e.is_plain(), "a plain field pastes the plain text");

    let mut r = TextEdit::new("hello").rich();
    r.set_selection(0, 5);
    r.key(&cmd('b'), None);
    assert_eq!(marked(&r, &Mark::Bold), ["hello"]);
    r.set(5, false);
    r.key(&shift_enter, None);
    assert_eq!(r.soft_breaks(), &[1]);
    assert!(TextEdit::from_html("<b>a</b>").is_rich() && TextEdit::from_parts("a", vec![], vec![]).is_rich());
}

#[test]
fn plain_field_leaves_format_chords_to_the_app() {
    use crate::{text_input, Cx, El, Env, Runtime, TextStyle};
    struct Form { f: TextEdit, keys: Vec<Key> }
    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        text_input(&s.f, TextStyle::new(14.0, 400.0)).key("f").size(200.0, 40.0).on_edit(|s: &mut Form| &mut s.f)
    }
    let mut f = TextEdit::new("hello");
    f.focused = true;
    f.set_selection(0, 5);
    let mut rt = Runtime::new(Form { f, keys: vec![] }, view, (200.0, 40.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| { s.keys.push(ev.key.clone()); true });
    rt.frame(0.0);
    for c in ['b', 'i', 'u'] { rt.key(cmd(c)); rt.frame(0.0); }
    assert_eq!(rt.state.keys, [Key::Command('b'), Key::Command('i'), Key::Command('u')]);
    assert!(rt.state.f.marks().is_empty());
    // A field with no formatting lays out as plain text: no spans, blocks or soft breaks.
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("f")).unwrap();
    let t = n.text.as_ref().unwrap();
    assert!(t.spans.is_empty() && t.blocks.is_empty() && t.soft_breaks.is_empty() && t.strikes.is_empty());
}

#[test]
fn direct_text_assignment_never_panics() {
    use crate::{text_input, Cx, El, Env, Runtime, TextStyle};
    let fresh = || {
        let mut e = TextEdit::from_html("<ul><li><b>bold</b> <a href=\"https://a.b\">item</a></li></ul><pre><code>a\nb</code></pre><p>x<br>y</p>");
        e.set(e.text.len(), false);
        e
    };
    // The app shrinks `text` under marks, blocks, soft breaks and a caret that sit past its
    // end or inside a character.
    let stale = |e: &mut TextEdit| {
        e.text = "é🦀".into();
        e.anchor = 1;
        e.focus = 3;
    };
    let mut e = fresh();
    stale(&mut e);
    let _ = (e.selected(), e.display(), e.paragraphs(), e.active_block(), e.active_marks(), e.has_mark(&Mark::Bold));
    let _ = (e.to_html(), e.selected_html());
    e.key(&k(Key::Left), None);
    assert_invariants(&e, "after Left");
    for ev in [k(Key::Text("z".into())), k(Key::Backspace), k(Key::Enter), cmd('b'), k(Key::Tab)] {
        let mut e = fresh();
        stale(&mut e);
        e.key(&ev, None);
        assert_invariants(&e, &format!("after {ev:?}"));
    }
    let mut e = fresh();
    stale(&mut e);
    e.ime(&Ime::Preedit("に".into(), None));
    e.ime(&Ime::Commit("日".into()));
    assert_invariants(&e, "after IME");
    let mut e = fresh();
    stale(&mut e);
    e.paste_html("<i>p</i>", "p");
    assert_invariants(&e, "after paste_html");
    let mut e = fresh();
    stale(&mut e);
    e.toggle_mark(Mark::Italic);
    e.set_block(Block::Quote);
    assert_invariants(&e, "after toggle_mark and set_block");

    struct App { e: TextEdit }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        text_input(&s.e, TextStyle::new(14.0, 400.0)).key("e").size(200.0, 120.0).on_edit(|s: &mut App| &mut s.e)
    }
    let mut e = fresh();
    e.focused = true;
    let mut rt = Runtime::new(App { e }, view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| stale(&mut s.e));
    rt.frame(0.0);
    rt.ime(Ime::Preedit("あ".into(), None));
    rt.frame(0.0);
    rt.key(k(Key::Right));
    rt.ime(Ime::Commit("あ".into()));
    rt.frame(0.0);
    assert_invariants(&rt.state.e, "runtime");
}

#[test]
fn count_newlines_matches_a_plain_count() {
    for len in [0, 1, 254, 255, 256, 600, 70_000] {
        let t: Vec<u8> = (0..len).map(|i| if i % 3 == 0 || i > 1000 { b'\n' } else { b'x' }).collect();
        assert_eq!(count_newlines(&t), t.iter().filter(|&&b| b == b'\n').count(), "{len}");
    }
}

#[test]
fn list_levels_stop_at_max_list_level() {
    let mut e = TextEdit::new("a").rich();
    e.set_block(Block::Bullet(0));
    for _ in 0..10 { e.key(&k(Key::Tab), None); }
    assert_eq!(e.blocks(), &[Block::Bullet(MAX_LIST_LEVEL)]);
    e.set_block(Block::Numbered(9));
    assert_eq!(e.blocks(), &[Block::Numbered(MAX_LIST_LEVEL)]);
    assert_eq!(TextEdit::from_parts("a", vec![], vec![Block::Bullet(200)]).blocks(), &[Block::Bullet(MAX_LIST_LEVEL)]);
    let deep = TextEdit::from_html(&format!("{}a", "<ul><li>".repeat(8)));
    assert_eq!(deep.blocks().last(), Some(&Block::Bullet(MAX_LIST_LEVEL)));
}

#[test]
fn paste_html_without_formatting_keeps_the_plain_whitespace() {
    // VS Code's copy: colour spans in a `white-space: pre` div.
    let html = concat!(
        "<meta charset='utf-8'><div style=\"color: #d4d4d4;white-space: pre;\">",
        "<div><span style=\"color: #569cd6;\">fn</span><span>&nbsp;a()&nbsp;{</span></div>",
        "<div><span>&nbsp; &nbsp; b</span></div><div><span>}</span></div></div>",
    );
    let mut e = TextEdit::new("").rich();
    e.paste_html(html, "fn a() {\n    b\n}");
    assert_eq!(e.text, "fn a() {\n    b\n}");
    let mut e = TextEdit::new("").rich();
    e.paste_html("<p>a  <b>b</b></p>", "a  b");
    assert_eq!((e.text.as_str(), marked(&e, &Mark::Bold)), ("a b", vec!["b"]), "formatted HTML still wins");
}

#[test]
fn paste_html_into_a_code_block_keeps_the_plain_whitespace() {
    let mut e = TextEdit::new("").rich();
    e.set_block(Block::Code);
    e.paste_html("<p>fn  a()</p><p>b</p>", "fn  a() {\n\tb\n}");
    assert_eq!(e.text, "fn  a() {\n\tb\n}");
    assert_eq!(e.blocks(), &[Block::Code; 3]);
    assert!(e.marks().is_empty());
}

#[test]
fn soft_breaks_can_be_set_on_a_rebuilt_draft() {
    let mut e = TextEdit::from_parts("one\ntwo\nthree", vec![], vec![Block::Bullet(0); 3]).with_soft_breaks(vec![5, 1, 0, 1]);
    assert!(!e.has_history(), "with_soft_breaks on from_parts adds no undo step");
    assert_eq!(e.soft_breaks(), &[1], "line 0, duplicates and lines past the end are dropped");
    assert_eq!(e.to_html().matches("<li>").count(), 2, "\"two\" continues the first bullet");
    assert_invariants(&e, "with_soft_breaks");
    e.set_soft_breaks(vec![2]);
    assert!(!e.has_history());
    assert_eq!(e.soft_breaks(), &[2]);
    let round = TextEdit::from_parts(e.text.clone(), e.marks().to_vec(), e.blocks().to_vec()).with_soft_breaks(e.soft_breaks().to_vec());
    assert_eq!(round.to_html(), e.to_html());
}

#[test]
fn clear_history_drops_undo_and_redo_while_keeping_formatting_and_selection() {
    let mut e = TextEdit::from_parts("one\ntwo", vec![MarkRange::new(0..3, Mark::Bold)], vec![Block::Bullet(0); 2])
        .with_soft_breaks(vec![1]);
    e.insert("!");
    e.undo();
    assert!(e.has_history());
    e.redo();
    e.set_selection(1, 4);
    e.clear_history();
    assert!(!e.has_history());
    e.undo();
    e.redo();
    assert_eq!((e.text.as_str(), e.anchor, e.focus), ("one\ntwo!", 1, 4));
    assert_eq!(e.marks(), &[MarkRange::new(0..3, Mark::Bold)]);
    assert_eq!(e.blocks(), &[Block::Bullet(0); 2]);
    assert_eq!(e.soft_breaks(), &[1]);
}

#[test]
fn return_on_a_blank_last_line_leaves_a_code_block_in_one_undo_step() {
    let mut e = TextEdit::new("").rich();
    e.set_block(Block::Code);
    e.key(&k(Key::Text("let x = 1;".into())), None);
    e.key(&k(Key::Enter), None);
    assert_eq!((e.text.as_str(), e.blocks()), ("let x = 1;\n", &[Block::Code; 2][..]));
    // Composing on a blank code line and pressing Return to commit stays in the block.
    e.ime(&Ime::Preedit("に".into(), None));
    assert!(!e.key(&k(Key::Enter), None).submit);
    e.ime(&Ime::Commit("日".into()));
    assert_eq!((e.text.as_str(), e.blocks()), ("let x = 1;\n日", &[Block::Code; 2][..]));
    undo(&mut e, false);
    assert_eq!((e.text.as_str(), e.blocks()), ("let x = 1;\n", &[Block::Code; 2][..]));

    let out = e.key(&k(Key::Enter), None);
    assert!(!out.submit);
    assert_eq!((e.text.as_str(), e.blocks()), ("let x = 1;\n", &[Block::Code, Block::Paragraph][..]), "the blank line left the block");
    assert_eq!((e.anchor, e.focus), (11, 11));
    undo(&mut e, false);
    assert_eq!((e.text.as_str(), e.blocks()), ("let x = 1;\n", &[Block::Code; 2][..]), "leaving is one undo step");
    undo(&mut e, true);
    assert_eq!((e.text.as_str(), e.blocks()), ("let x = 1;\n", &[Block::Code, Block::Paragraph][..]), "redo leaves the block again");
    e.key(&k(Key::Text("after".into())), None);
    assert_eq!(e.blocks(), &[Block::Code, Block::Paragraph]);
    assert_invariants(&e, "left code");
    undo(&mut e, false);
    undo(&mut e, false);
    assert_eq!((e.text.as_str(), e.blocks()), ("let x = 1;\n", &[Block::Code; 2][..]));

    // A blank line with more code below it is the code's own.
    let mut e = TextEdit::from_parts("a\n\nb", vec![], vec![Block::Code; 3]);
    e.anchor = 2;
    e.focus = 2;
    e.key(&k(Key::Enter), None);
    assert_eq!((e.text.as_str(), e.blocks()), ("a\n\n\nb", &[Block::Code; 4][..]));
    // So is a non-blank last line, and a blank line under a selection.
    let mut e = TextEdit::from_parts("a\n", vec![], vec![Block::Code; 2]);
    e.anchor = 0;
    e.focus = 2;
    e.key(&k(Key::Enter), None);
    assert_eq!((e.text.as_str(), e.blocks()), ("\n", &[Block::Code; 2][..]));
}

#[test]
fn down_or_right_at_the_end_of_a_trailing_code_block_or_quote_adds_a_paragraph() {
    for (block, key) in [(Block::Code, Key::Down), (Block::Code, Key::Right), (Block::Quote, Key::Down), (Block::Quote, Key::Right)] {
        let mut e = TextEdit::from_parts("x\ny", vec![], vec![block; 2]);
        let ctx = format!("{block:?} {key:?}");
        e.anchor = 1;
        e.focus = 1;
        e.key(&k(key.clone()), None);
        assert_eq!(e.text, "x\ny", "{ctx}: not at the end, the caret just moves");
        e.anchor = 3;
        e.focus = 3;
        e.key(&k(key.clone()), None);
        assert_eq!((e.text.as_str(), e.blocks()), ("x\ny\n", &[block, block, Block::Paragraph][..]), "{ctx}");
        assert_eq!((e.anchor, e.focus), (4, 4), "{ctx}");
        assert_invariants(&e, &ctx);
        e.key(&k(key.clone()), None);
        assert_eq!(e.text, "x\ny\n", "{ctx}: a paragraph at the end stays put");
        undo(&mut e, false);
        assert_eq!((e.text.as_str(), e.blocks()), ("x\ny", &[block; 2][..]), "{ctx}: one undo step");
        undo(&mut e, true);
        assert_eq!((e.text.as_str(), e.blocks()), ("x\ny\n", &[block, block, Block::Paragraph][..]), "{ctx}: redo restores it");
    }
    // Does not fire with a selection at the end, during IME, with modifiers, or in a plain field.
    for key in [Key::Down, Key::Right] {
        let mut e = TextEdit::from_parts("x", vec![], vec![Block::Code]);
        e.set_selection(0, 1);
        e.key(&k(key.clone()), None);
        assert_eq!((e.text.as_str(), e.blocks()), ("x", &[Block::Code][..]), "selection ending at end");

        let mut e = TextEdit::from_parts("x", vec![], vec![Block::Code]);
        e.ime(&Ime::Preedit("に".into(), None));
        e.key(&k(key.clone()), None);
        assert_eq!((e.text.as_str(), e.blocks()), ("x", &[Block::Code][..]), "during IME preedit");

        let mut e = TextEdit::from_parts("x", vec![], vec![Block::Code]);
        e.key(&KeyEvent { key: key.clone(), mods: Mods { shift: true, ..Mods::default() } }, None);
        e.key(&KeyEvent { key: key.clone(), mods: Mods { alt: true, ..Mods::default() } }, None);
        assert_eq!((e.text.as_str(), e.blocks()), ("x", &[Block::Code][..]), "modified arrow");

        let mut plain = TextEdit::new("x");
        plain.key(&k(key), None);
        assert_eq!((plain.text.as_str(), plain.blocks()), ("x", &[Block::Paragraph][..]), "plain field");
    }
}

#[test]
fn a_field_keeps_only_the_marks_and_blocks_it_allows() {
    let mut e = TextEdit::from_parts("ab", vec![MarkRange { start: 0, end: 1, mark: Mark::Underline }], vec![Block::Paragraph])
        .allow_marks([Mark::Bold, Mark::Strike, Mark::Code, Mark::Link(String::new())])
        .allow_blocks([Block::Bullet(0), Block::Quote]);
    assert!(e.marks().is_empty(), "marks it held are dropped");
    assert!(e.allows_mark(&Mark::Link("https://example.com".into())) && !e.allows_mark(&Mark::Italic));
    assert!(e.allows_block(Block::Bullet(3)) && e.allows_block(Block::Paragraph) && !e.allows_block(Block::Code));
    e.anchor = 0;
    e.focus = 2;
    assert_eq!(e.format_chord(&cmd('u')), None, "⌘U is the app's");
    e.key(&cmd('u'), None);
    e.key(&cmd('i'), None);
    e.toggle_mark(Mark::Italic);
    e.set_mark(Mark::Underline);
    assert!(e.marks().is_empty());
    e.key(&cmd('b'), None);
    assert_eq!(marked(&e, &Mark::Bold), ["ab"]);
    e.toggle_block(Block::Code);
    e.set_block(Block::Numbered(0));
    assert_eq!(e.blocks(), &[Block::Paragraph]);
    e.toggle_block(Block::Bullet(0));
    assert_eq!(e.blocks(), &[Block::Bullet(0)]);
    e.set_marks(vec![MarkRange { start: 0, end: 2, mark: Mark::Italic }]);
    assert!(e.marks().is_empty(), "set_marks can't add one either");
    e.set_blocks(vec![Block::Code]);
    assert_eq!(e.blocks(), &[Block::Paragraph], "set_blocks can't add one either");

    // Undo/redo of an edit recorded before allow_marks/allow_blocks was set, or a pending
    // mark armed before allow_marks, cannot smuggle a disallowed mark or block back in.
    let mut pre = TextEdit::new("ab").rich();
    pre.set_selection(0, 2);
    pre.set_mark(Mark::Underline);
    pre.set_block(Block::Code);
    pre.set_caret(2);
    pre.insert("!");
    pre.toggle_mark(Mark::Italic);
    let mut pre = pre.allow_marks([Mark::Bold]).allow_blocks([Block::Quote]);
    assert!(!pre.has_mark(&Mark::Italic), "pending disallowed mark is stripped");
    undo(&mut pre, false);
    assert!(pre.marks().is_empty() && pre.blocks() == &[Block::Paragraph], "undo filters snapshot");
    undo(&mut pre, true);
    assert!(pre.marks().is_empty() && pre.blocks() == &[Block::Paragraph], "redo filters snapshot");

    // Pills are the app's tokens, kept whatever the allow-list says.
    let mut p = TextEdit::new("hi @maya").rich().allow_marks([Mark::Bold]);
    assert!(p.allows_mark(&Mark::Pill(String::new())));
    p.replace_with_pill(3..8, "user:maya", "@maya");
    assert_eq!(p.pills().count(), 1, "replace_with_pill works under an allow-list");
    let p = p.allow_marks(Vec::<Mark>::new());
    assert_eq!(p.pills().count(), 1, "an allow-list set later keeps existing pills");

    let mut e = TextEdit::new("").rich().allow_marks([Mark::Bold]).allow_blocks([Block::Quote]);
    e.paste_html("<ul><li><b>b</b> <i>i</i> <u>u</u></li></ul><blockquote>q</blockquote><pre>c</pre>", "");
    assert_eq!(e.text, "b i u\nq\nc");
    assert_eq!(e.marks().iter().map(|m| &m.mark).collect::<Vec<_>>(), [&Mark::Bold]);
    assert_eq!(e.blocks(), &[Block::Paragraph, Block::Quote, Block::Paragraph]);
    assert_invariants(&e, "filtered paste");
    // HTML whose only formatting the field drops pastes as plain text, keeping its spacing.
    let mut e = TextEdit::new("").rich().allow_marks([Mark::Bold]);
    e.paste_html("<p><i>a  b</i></p>", "a  b");
    assert_eq!(e.text, "a  b");
}

#[test]
fn flatten_spans_matches_the_naive_merge_and_later_spans_win() {
    use crate::text::{flatten_spans, Span};
    use crate::Color;
    fn naive(spans: &[Span]) -> Vec<Span> {
        let mut bounds: Vec<u32> = spans.iter().filter(|s| s.start < s.end).flat_map(|s| [s.start, s.end]).collect();
        bounds.sort_unstable();
        bounds.dedup();
        let mut out: Vec<Span> = Vec::new();
        for w in bounds.windows(2) {
            let mut m: Option<Span> = None;
            for s in spans.iter().filter(|s| s.start < s.end && s.start <= w[0] && w[1] <= s.end) {
                let m = m.get_or_insert_with(|| Span::new(w[0] as usize, w[1] as usize));
                if s.weight.is_some() { m.weight = s.weight; }
                if s.color.is_some() { m.color = s.color; }
                if s.bg.is_some() { m.bg = s.bg; m.bg_pad = s.bg_pad; }
                if s.underline.is_some() { m.underline = s.underline; }
                if s.size.is_some() { m.size = s.size; }
                m.mono |= s.mono;
                m.italic |= s.italic;
            }
            if let Some(m) = m {
                match out.last_mut() {
                    Some(last) if last.end == m.start && Span { start: m.start, end: m.end, ..last.clone() } == m => last.end = m.end,
                    _ => out.push(m),
                }
            }
        }
        out
    }
    let red = Color::WHITE;
    let blue = Color::BLACK;
    assert_eq!(
        flatten_spans(&[Span::new(2, 5).color(blue), Span::new(0, 10).color(red)]),
        [Span::new(0, 10).color(red)],
        "a later span wins even where it starts first"
    );
    assert_eq!(
        flatten_spans(&[Span::new(0, 10).color(red), Span::new(2, 5).color(blue).bold()]),
        [Span::new(0, 2).color(red), Span::new(2, 5).color(blue).bold(), Span::new(5, 10).color(red)]
    );
    let mut seed = 7u64;
    let mut next = |n: u64| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 33) % n };
    for _ in 0..300 {
        let spans: Vec<Span> = (0..next(12)).map(|_| {
            let a = next(40) as usize;
            let mut s = Span::new(a, a + next(15) as usize);
            match next(5) {
                0 => s = s.color(if next(2) == 0 { red } else { blue }),
                1 => s = s.bold(),
                2 => s = s.italic(),
                3 => s = s.bg(red, next(3) as f32),
                _ => s = s.mono().underline(blue),
            }
            s
        }).collect();
        assert_eq!(flatten_spans(&spans), naive(&spans), "{spans:?}");
    }
}

#[test]
fn html_parsing_is_linear_on_hostile_input() {
    let n = 200_000;
    let cases = [
        "<b>".repeat(n) + "x",
        "<script>".to_owned() + &"</script".repeat(n),
        "<p>".to_owned() + &"&".repeat(n * 4),
        "<div><span>".repeat(n / 2) + &"</div>".repeat(n / 2),
        "</b>".repeat(n),
        "<ul><li>".repeat(n / 2) + "x",
        "a<br>".repeat(n),
        "<a href=\"".to_owned() + &"&#x6a;".repeat(n) + "\">x</a>",
    ];
    for html in &cases {
        let t = std::time::Instant::now();
        let e = TextEdit::from_html(html);
        assert_invariants(&e, &html[..20]);
        let _ = e.to_html();
        // Debug builds parse these in well under a second; a quadratic path takes minutes.
        assert!(t.elapsed().as_secs() < 10, "{} took {:?}", &html[..20], t.elapsed());
    }
}

#[test]
fn html_links_keep_only_safe_schemes() {
    for bad in [
        "javascript:alert(1)", "JaVaScRiPt:alert(1)", " javascript:x", "java\tscript:x", "java\nscript:x",
        "java&#x09;script:x", "&#106;avascript:x", "javascript&#58;x", "javascript&colon;x", "java&Tab;script:x",
        "jav&#x0A;ascript:x", "\u{1}javascript:x", "vbscript:x", "data:text/html,x", "file:///etc/passwd",
        "blob:https://a.b/x", "x-app-scheme:do",
    ] {
        let e = TextEdit::from_html(&format!("<a href=\"{bad}\">x</a><a href='{bad}'>y</a>"));
        assert!(e.marks().iter().all(|m| !matches!(m.mark, Mark::Link(_))), "{bad:?} kept: {:?}", e.marks());
        assert_eq!(e.text, "xy");
    }
    for (good, url) in [
        ("https://a.b/c?d=1&amp;e=2", "https://a.b/c?d=1&e=2"),
        ("HTTP://A.B", "HTTP://A.B"),
        ("mailto:a@b.c", "mailto:a@b.c"),
        ("tel:+1555", "tel:+1555"),
        ("/relative/path", "/relative/path"),
        ("#frag", "#frag"),
    ] {
        let e = TextEdit::from_html(&format!("<a href=\"{good}\">x</a>"));
        assert_eq!(e.marks(), &[MarkRange::new(0..1, Mark::Link(url.into()))], "{good}");
    }
    let e = TextEdit::from_html("a&#0;b&#x1;c<pre>d&#0;\te</pre>");
    assert_eq!(e.text, "abc\nd\te", "entities never decode to control characters");
}

#[test]
fn html_from_browsers_notes_slack_and_docs_round_trips() {
    let round_trip = |e: &TextEdit, ctx: &str| {
        let back = TextEdit::from_html(&e.to_html());
        assert_eq!(
            (&back.text, back.marks(), back.blocks(), back.soft_breaks()),
            (&e.text, e.marks(), e.blocks(), e.soft_breaks()),
            "{ctx} round trip"
        );
        assert_invariants(e, ctx);
    };

    let chrome = TextEdit::from_html(concat!(
        "<meta charset='utf-8'><span style=\"color: rgb(0, 0, 0); font-family: Arial; font-weight: 700;\">Hello</span>",
        "<span style=\"color: rgb(0, 0, 0);\"> world</span>",
    ));
    assert_eq!(chrome.text, "Hello world");
    assert_eq!(marked(&chrome, &Mark::Bold), ["Hello"]);
    round_trip(&chrome, "browser");

    let notes = TextEdit::from_html(concat!(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\">\n<html>\n<head>\n",
        "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\">\n",
        "<style type=\"text/css\">\np.p1 {margin: 0.0px 0.0px 0.0px 0.0px}\n</style>\n</head>\n<body>\n",
        "<p class=\"p1\"><b>Bold</b> text</p>\n<p class=\"p1\"><br></p>\n",
        "<ul class=\"ul1\">\n<li class=\"li1\">item one</li>\n<li class=\"li1\"><i>item two</i></li>\n</ul>\n",
        "</body>\n</html>\n",
    ));
    assert_eq!(notes.text, "Bold text\n\nitem one\nitem two");
    assert_eq!(notes.blocks(), &[Block::Paragraph, Block::Paragraph, Block::Bullet(0), Block::Bullet(0)]);
    assert_eq!((marked(&notes, &Mark::Bold), marked(&notes, &Mark::Italic)), (vec!["Bold"], vec!["item two"]));
    round_trip(&notes, "notes");

    let slack = TextEdit::from_html(concat!(
        "<meta charset='utf-8'><b data-stringify-type=\"bold\" style=\"color: rgb(29, 28, 29);\">bold</b>",
        "<span style=\"color: rgb(29, 28, 29);\"> and </span><i data-stringify-type=\"italic\">it</i><span> </span>",
        "<code data-stringify-type=\"code\" class=\"c-mrkdwn__code\">x()</code><br style=\"color: rgb(29, 28, 29);\">",
        "<a target=\"_blank\" class=\"c-link\" data-stringify-link=\"https://a.b/c?d=1&amp;e=2\" ",
        "href=\"https://a.b/c?d=1&amp;e=2\" rel=\"noopener noreferrer\">link</a>",
    ));
    assert_eq!(slack.text, "bold and it x()\nlink");
    assert_eq!(slack.soft_breaks(), &[1]);
    assert_eq!(marked(&slack, &Mark::Code), ["x()"]);
    assert_eq!(marked(&slack, &Mark::Link("https://a.b/c?d=1&e=2".into())), ["link"]);
    round_trip(&slack, "slack");

    let docs = TextEdit::from_html(concat!(
        "<meta charset=\"utf-8\"><b style=\"font-weight:normal;\" id=\"docs-internal-guid-1\">",
        "<p dir=\"ltr\" style=\"line-height:1.38;margin-top:0pt;\"><span style=\"font-size:11pt;font-weight:700;\">Bold</span>",
        "<span style=\"font-size:11pt;font-weight:400;\"> plain</span></p>",
        "<ul style=\"margin-top:0;\"><li dir=\"ltr\" style=\"list-style-type:disc;\"><p dir=\"ltr\" role=\"presentation\">",
        "<span style=\"font-style:italic;\">item</span></p></li></ul></b>",
    ));
    assert_eq!(docs.text, "Bold plain\nitem");
    assert_eq!(docs.blocks(), &[Block::Paragraph, Block::Bullet(0)]);
    assert_eq!((marked(&docs, &Mark::Bold), marked(&docs, &Mark::Italic)), (vec!["Bold"], vec!["item"]));
    round_trip(&docs, "google docs");

    let loose = TextEdit::from_html("<p>A</p>B<p>C<br></p>");
    assert_eq!(loose.text, "A\nB\nC", "loose text gets its own line; a trailing <br> draws nothing");
    round_trip(&loose, "loose");
}

#[test]
fn random_edits_keep_ranges_blocks_and_soft_breaks_consistent() {
    const PIECES: [&str; 10] = ["a", "b ", "é", "🦀", "e\u{301}", "👩\u{200d}👩\u{200d}👧", "日本", "\n", "\t", " "];
    for seed in 0..40u64 {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut next = |n: u64| { state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (state >> 33) % n };
        let mut e = TextEdit::from_html("<p>start <b>bold</b></p><ul><li>item</li></ul>");
        let (mut clip, mut clip_html) = (String::new(), String::new());
        for step in 0..300 {
            let op = next(22);
            let mods = Mods { shift: next(3) == 0, alt: next(4) == 0, ..Mods::default() };
            match op {
                0..=3 => { let p = PIECES[next(PIECES.len() as u64) as usize]; e.key(&k(Key::Text(p.into())), None); }
                4 => { e.key(&KeyEvent { key: Key::Enter, mods }, None); }
                5 => { e.key(&KeyEvent { key: Key::Backspace, mods }, None); }
                6 => { e.key(&KeyEvent { key: Key::Delete, mods }, None); }
                7 => { e.key(&KeyEvent { key: if next(2) == 0 { Key::Left } else { Key::Right }, mods }, None); }
                8 => { e.key(&KeyEvent { key: if next(2) == 0 { Key::Home } else { Key::End }, mods }, None); }
                9 => { e.key(&KeyEvent { key: Key::Tab, mods: Mods { shift: mods.shift, ..Mods::default() } }, None); }
                10 => { e.key(&cmd(['b', 'i', 'u'][next(3) as usize]), None); }
                11 => e.toggle_mark([Mark::Code, Mark::Strike, Mark::Link(format!("https://e.x/{}", next(3)))][next(3) as usize].clone()),
                12 => {
                    let l = next(6) as u8;
                    e.set_block([Block::Paragraph, Block::Bullet(l), Block::Numbered(l), Block::Quote, Block::Code][next(5) as usize]);
                }
                13 => undo(&mut e, next(2) == 0),
                14 => {
                    clip_html = e.selected_html();
                    if let Some(c) = e.key(&cmd(if next(2) == 0 { 'x' } else { 'c' }), None).copy { clip = c; }
                }
                15 => { e.paste(&clip.clone()); }
                16 => { e.paste_html(&clip_html.clone(), &clip.clone()); }
                17 => {
                    let p = PIECES[next(6) as usize];
                    e.ime(&Ime::Preedit(p.into(), None));
                    if next(2) == 0 { e.ime(&Ime::Commit(p.into())); }
                }
                18 => { let a = next(e.text.len() as u64 + 2) as usize; let f = next(e.text.len() as u64 + 2) as usize; e.set_selection(a, f); }
                19 => {
                    // An app writes `text` and the caret directly, then the user types.
                    if next(2) == 0 { let cut = next(e.text.len() as u64 + 1) as usize; let cut = TextEdit::clamp_char_boundary(&e.text, cut); e.text.truncate(cut); }
                    else { e.text.push_str(PIECES[next(PIECES.len() as u64) as usize]); }
                    e.anchor = next(e.text.len() as u64 + 3) as usize;
                    e.focus = e.anchor;
                    e.key(&k(Key::Text("z".into())), None);
                }
                20 => { e.key(&cmd('a'), None); }
                _ => { e.key(&KeyEvent { key: if next(2) == 0 { Key::Up } else { Key::Down }, mods }, None); }
            }
            assert_invariants(&e, &format!("seed {seed} step {step} op {op} text {:?}", e.text));
            let _ = (e.to_html(), e.display(), e.paragraphs());
        }
    }
}

#[test]
fn copy_from_one_rich_field_pastes_marks_and_blocks_into_another() {
    use crate::{Cx, El, Env, Runtime, TextStyle, col, text_input};
    struct App { a: TextEdit, b: TextEdit, plain: TextEdit }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        col().size(320.0, 360.0)
            .child(text_input(&s.a, st).key("a").size(300.0, 140.0).on_edit(|s: &mut App| &mut s.a))
            .child(text_input(&s.b, st).key("b").size(300.0, 140.0).on_edit(|s: &mut App| &mut s.b))
            .child(text_input(&s.plain, st).key("plain").size(300.0, 40.0).on_edit(|s: &mut App| &mut s.plain))
    }
    let mut a = TextEdit::from_html(
        "<p>Hello <b>bold</b> <a href=\"https://example.com\">link</a></p><ul><li>one</li><li><i>two</i></li></ul><blockquote>q</blockquote>",
    );
    a.focused = true;
    a.set_selection(0, a.text.len());
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(App { a, b: TextEdit::new("").rich(), plain: TextEdit::new("") }, view, (320.0, 360.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    rt.key(cmd('c'));
    rt.frame(0.0);
    let copied = m.content().unwrap();
    assert_eq!(copied.text(), rt.state.a.text);
    assert!(copied.html().is_some_and(|h| h.contains("<strong>bold</strong>") && h.contains("<ul>")), "{copied:?}");

    rt.update(|s, _| { s.a.focused = false; s.b.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    let (a, b) = (&rt.state.a, &rt.state.b);
    assert_eq!((&b.text, b.marks(), b.blocks()), (&a.text, a.marks(), a.blocks()));

    // Cut carries the formatting it removes.
    rt.update(|s, _| s.b.set_selection(6, 10));
    rt.key(cmd('x'));
    rt.frame(0.0);
    assert_eq!(rt.state.b.text, format!("Hello  link{}", &rt.state.a.text[15..]));
    assert!(m.content().unwrap().html().is_some_and(|h| h.contains("<strong>bold</strong>")));

    // A plain field pastes the text only and copies no HTML.
    rt.update(|s, _| { s.b.focused = false; s.plain.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    assert_eq!(rt.state.plain.text, "bold");
    assert!(rt.state.plain.is_plain());
    rt.update(|s, _| s.plain.set_selection(0, 4));
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(m.content().unwrap().html(), None);
}

/// `"a @x-agent b"` with the label a pill carrying `agent:x`, the caret after it.
fn with_pill() -> TextEdit {
    let mut e = TextEdit::new("a  b").rich();
    e.set_caret(2);
    e.insert_pill("agent:x", "@x-agent");
    e
}

fn pills_of(e: &TextEdit) -> Vec<(Range<usize>, String)> {
    e.pills().map(|(r, id)| (r, id.to_owned())).collect()
}

#[test]
fn a_pill_replaces_a_typed_query_and_lists_with_its_id() {
    let mut e = TextEdit::new("").rich();
    e.insert("see @ma");
    let at = e.text.find('@').unwrap();
    e.replace_with_pill(at..e.text.len(), "file:src/main.rs", "@main.rs\n");
    assert_eq!(e.text, "see @main.rs ", "line breaks in a label become spaces");
    assert_eq!(pills_of(&e), [(4..13, "file:src/main.rs".to_owned())]);
    assert_eq!(e.range(), (13, 13));
    e.key(&k(Key::Text("x".into())), None);
    assert_eq!(pills_of(&e), [(4..13, "file:src/main.rs".to_owned())], "typing after a pill never extends it");
    e.set_caret(4);
    e.key(&k(Key::Text("y".into())), None);
    assert_eq!((e.text.as_str(), pills_of(&e)), ("see y@main.rs x", vec![(5..14, "file:src/main.rs".to_owned())]));
    let mut secure = TextEdit::new("").secure();
    secure.insert_pill("a", "@a");
    assert_eq!((secure.text.as_str(), secure.pills().count()), ("", 0));
}

#[test]
fn the_caret_steps_over_a_pill_in_one_move() {
    let mut e = with_pill();
    assert_eq!((e.text.as_str(), e.range()), ("a @x-agent b", (10, 10)));
    e.key(&k(Key::Left), None);
    assert_eq!(e.range(), (2, 2));
    e.key(&k(Key::Left), None);
    assert_eq!(e.range(), (1, 1));
    e.key(&k(Key::Right), None);
    e.key(&k(Key::Right), None);
    assert_eq!(e.range(), (10, 10));
    e.set_caret(2);
    e.key(&KeyEvent { key: Key::Right, mods: Mods { alt: true, ..Mods::default() } }, None);
    assert_eq!(e.range(), (10, 10), "a word move that stops inside the label steps over it");
    e.set_caret(2);
    e.key(&KeyEvent { key: Key::Right, mods: Mods { shift: true, ..Mods::default() } }, None);
    assert_eq!((e.anchor, e.focus), (2, 10), "Shift+Right selects the whole pill");
    e.set_caret(5);
    assert_eq!(e.range(), (2, 2), "a click inside goes to the nearer edge");
    e.set_caret(8);
    assert_eq!(e.range(), (10, 10));
    // A caret written straight into a pill moves out before the next edit.
    e.anchor = 6;
    e.focus = 6;
    e.insert("Z");
    assert_eq!((e.text.as_str(), pills_of(&e)), ("a @x-agentZ b", vec![(2..10, "agent:x".to_owned())]));
}

#[test]
fn an_undo_group_is_one_step_with_its_selection_and_nests() {
    let mut e = TextEdit::new("hi @ag").rich();
    e.set_caret(6);
    e.undo_group(|e| {
        e.replace_with_pill(3..6, "agent:1", "@agent");
        e.undo_group(|e| e.insert(" "));
    });
    assert_eq!((e.text.as_str(), e.range(), e.pills().count()), ("hi @agent ", (10, 10), 1));
    undo(&mut e, false);
    assert_eq!((e.text.as_str(), e.range(), e.pills().count()), ("hi @ag", (6, 6), 0));
    undo(&mut e, true);
    assert_eq!((e.text.as_str(), e.range(), pills_of(&e)), ("hi @agent ", (10, 10), vec![(3..9, "agent:1".to_owned())]));
    undo(&mut e, false);
    assert_eq!(e.text, "hi @ag");
    assert!(e.history.undo.is_empty(), "the group was a single step");
}

#[test]
fn undo_groups_keep_typing_coalescing_and_record_nothing_when_empty() {
    let mut e = TextEdit::default();
    e.key(&k(Key::Text("a".into())), None);
    e.key(&k(Key::Text("b".into())), None);
    assert_eq!(e.history.undo.len(), 1, "typing coalesces before a group");
    e.undo_group(|_| {});
    e.undo_group(|e| e.set_caret(0));
    e.undo_group(|e| e.set_selection(0, 2));
    e.undo_group(|e| {
        e.set_caret(2);
        e.insert("x");
        e.key(&k(Key::Backspace), None);
    });
    assert_eq!(e.history.undo.len(), 1, "empty, caret-only and cancelling groups add no step");
    e.key(&k(Key::Text("c".into())), None);
    e.undo_group(|e| { e.insert("X"); e.insert("Y"); });
    e.key(&k(Key::Text("d".into())), None);
    assert_eq!(e.history.undo.len(), 4, "typing run breaks before and after a group");
    undo(&mut e, false);
    assert_eq!(e.text, "abcXY");
    // An empty or caret-only group preserves the redo stack.
    e.undo_group(|e| e.set_caret(0));
    assert_eq!(e.history.redo.len(), 1);
    undo(&mut e, false);
    assert_eq!(e.text, "abc");
}

#[test]
fn undo_group_is_unwind_safe_and_interacts_with_clear_history_paste_and_soft_breaks() {
    let mut e = TextEdit::new("base").rich();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        e.undo_group(|e| {
            e.insert("!");
            panic!("boom");
        });
    }));
    assert!(e.group.0.is_none(), "unwinding clears the active group");
    e.insert("?");
    assert_eq!(e.history.undo.len(), 1, "edits after a caught panic record again");
    e.undo();
    assert_eq!(e.text, "base!");

    // Caught panic in a nested group leaves the outer group intact.
    e.clear_history();
    e.undo_group(|e| {
        e.insert("1");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            e.undo_group(|e| {
                e.insert("2");
                panic!("nested boom");
            });
        }));
        e.insert("3");
    });
    assert_eq!((e.text.as_str(), e.history.undo.len()), ("base!123", 1));
    e.undo();
    assert_eq!(e.text, "base!");

    // Cloning or comparing inside a group does not leak the active group into the clone.
    let mut cloned = None;
    e.undo_group(|e| {
        assert_eq!(*e, e.clone());
        let c = e.clone();
        cloned = Some(c);
    });
    let mut cloned = cloned.unwrap();
    cloned.insert("ok");
    assert_eq!(cloned.history.undo.len(), 1);
    cloned.undo();
    assert_eq!(cloned.text, "base!");

    // clear_history and clear inside a group drop prior edits in the group.
    e.undo_group(|e| {
        e.insert("drop");
        e.clear_history();
    });
    assert!(!e.has_history());
    e.undo_group(|e| {
        e.insert("-keep");
        e.clear_history();
        e.insert("-after");
    });
    assert_eq!(e.history.undo.len(), 1);
    e.undo();
    assert_eq!(e.text, "base!drop-keep");
    e.undo_group(|e| e.clear());
    assert_eq!((e.text.as_str(), e.has_history()), ("", false));

    // Paste, soft breaks and many edits inside one group count as one step against HISTORY_STEPS.
    e.undo_group(|e| {
        e.paste_html("<p><b>one</b></p>", "one");
        e.insert("\ntwo");
        e.set_soft_breaks(vec![1]);
    });
    assert_eq!((e.text.as_str(), e.soft_breaks(), e.history.undo.len()), ("one\ntwo", &[1][..], 1));
    e.undo();
    assert_eq!((e.text.as_str(), e.soft_breaks(), e.marks().len()), ("", &[][..], 0));
    e.redo();
    assert_eq!((e.text.as_str(), e.soft_breaks(), e.marks().len()), ("one\ntwo", &[1][..], 1));
    e.undo_group(|e| {
        for _ in 0..200 {
            e.insert("x");
        }
    });
    assert_eq!(e.history.undo.len(), 2, "200 edits in one group do not evict prior steps");
}

#[test]
fn an_undo_group_leaves_ime_composition_alone() {
    let mut e = TextEdit::new("x").rich();
    e.set_caret(1);
    e.ime(&Ime::Preedit("に".into(), None));
    e.undo_group(|e| e.ime(&Ime::Commit("日本".into())));
    assert_eq!((e.text.as_str(), e.preedit().is_none()), ("x日本", true));
    undo(&mut e, false);
    assert_eq!(e.text, "x");
    e.ime(&Ime::Preedit("a".into(), None));
    e.undo_group(|e| e.undo());
    assert!(e.preedit().is_some(), "undo inside a group does nothing");
}

#[test]
fn backspace_and_delete_take_a_pill_whole_and_undo_restores_its_id() {
    let mut e = with_pill();
    e.key(&k(Key::Backspace), None);
    assert_eq!((e.text.as_str(), e.range(), e.pills().count()), ("a  b", (2, 2), 0));
    undo(&mut e, false);
    assert_eq!((e.text.as_str(), pills_of(&e)), ("a @x-agent b", vec![(2..10, "agent:x".to_owned())]));
    e.set_caret(2);
    e.key(&k(Key::Delete), None);
    assert_eq!((e.text.as_str(), e.pills().count()), ("a  b", 0));
    undo(&mut e, false);
    e.set_caret(11);
    e.key(&k(Key::Backspace), None);
    assert_eq!((e.text.as_str(), pills_of(&e)), ("a @x-agentb", vec![(2..10, "agent:x".to_owned())]));
    e.set_caret(10);
    e.key(&KeyEvent { key: Key::Backspace, mods: Mods { alt: true, ..Mods::default() } }, None);
    assert_eq!((e.text.as_str(), e.pills().count()), ("a b", 0), "a word delete into a pill takes it whole");
    undo(&mut e, false);
    undo(&mut e, true);
    undo(&mut e, false);
    assert_eq!(pills_of(&e), [(2..10, "agent:x".to_owned())]);
}

#[test]
fn a_selection_into_a_pill_covers_it() {
    let mut e = with_pill();
    e.set_selection(0, 5);
    assert_eq!((e.anchor, e.focus), (0, 10));
    e.set_selection(7, 12);
    assert_eq!((e.anchor, e.focus), (2, 12));
    e.set_selection(11, 4);
    assert_eq!((e.anchor, e.focus), (11, 2));
    e.set_selection(0, 5);
    e.insert("z");
    assert_eq!((e.text.as_str(), e.pills().count()), ("z b", 0));
    // Formatting a pill restyles its label and keeps it whole.
    let mut e = with_pill();
    e.set_selection(0, 6);
    e.toggle_mark(Mark::Bold);
    assert_eq!(pills_of(&e), [(2..10, "agent:x".to_owned())]);
    assert!(e.marks().iter().any(|m| m.mark == Mark::Bold && m.range() == (0..10)));
}

#[test]
fn a_composition_never_splits_a_pill() {
    let mut e = with_pill();
    e.anchor = 4;
    e.focus = 4;
    e.ime(&Ime::Preedit("に".into(), None));
    let (shown, pre) = e.display();
    let (pa, _) = pre.unwrap();
    assert!(pa <= 2 || pa >= 10, "preedit at {pa} in {shown:?}");
    assert_eq!(e.display_pills().len(), 1);
    e.ime(&Ime::Commit("日本".into()));
    assert_eq!(pills_of(&e).len(), 1);
    assert_eq!(&e.text[pills_of(&e)[0].0.clone()], "@x-agent");
    assert!(e.text == "a 日本@x-agent b" || e.text == "a @x-agent日本 b", "{}", e.text);
}

#[test]
fn pills_round_trip_through_html_and_outside_html_makes_none() {
    let mut e = with_pill();
    e.set_selection(0, 5);
    e.toggle_mark(Mark::Italic);
    e.set_selection(0, e.text.len());
    let html = e.selected_html();
    assert!(html.contains("<span data-pill=\"agent:x\">"), "{html}");
    assert_eq!(e.selected(), "a @x-agent b", "the plain copy is the label");
    let mut other = TextEdit::new("").rich();
    other.paste_html_with_pills(&html, e.selected());
    assert_eq!((other.text.as_str(), pills_of(&other)), ("a @x-agent b", vec![(2..10, "agent:x".to_owned())]));
    let mut outside = TextEdit::new("").rich();
    outside.paste_html(&html, e.selected());
    assert_eq!((outside.text.as_str(), outside.pills().count()), ("a @x-agent b", 0));
    assert!(outside.marks().iter().any(|m| m.mark == Mark::Italic), "the rest of the formatting stays");
    // A pill split by a mark's edge still serializes as one span.
    let one = TextEdit::from_html("<p><span data-pill=\"f&quot;1\">@f<b>il</b>e</span></p>");
    assert_eq!(pills_of(&one), [(0..5, "f\"1".to_owned())]);
    assert!(one.to_html().contains("<span data-pill=\"f&quot;1\">@f<strong>il</strong>e</span>"), "{}", one.to_html());
    // A pill can't span lines.
    let broken = TextEdit::from_parts("a\nb c", vec![MarkRange::new(0..3, Mark::Pill("x".into())), MarkRange::new(4..5, Mark::Pill("c".into()))], Vec::new());
    assert_eq!(pills_of(&broken), [(4..5, "c".to_owned())]);
}

#[test]
fn a_pill_copied_from_one_rich_field_pastes_whole_into_another() {
    use crate::{ClipboardContent, Cx, El, Env, Runtime, TextStyle, col, text_input};
    struct App { a: TextEdit, b: TextEdit, hooked: TextEdit }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        col().size(320.0, 360.0)
            .child(text_input(&s.a, st).key("a").size(300.0, 100.0).on_edit(|s: &mut App| &mut s.a))
            .child(text_input(&s.b, st).key("b").size(300.0, 100.0).on_edit(|s: &mut App| &mut s.b))
            .child(text_input(&s.hooked, st).key("hooked").size(300.0, 100.0).on_edit(|s: &mut App| &mut s.hooked)
                .on_input_paste(|s: &mut App, content, _| {
                    let Some(html) = content.html() else { return false };
                    s.hooked.paste_html_with_pills(html, content.text());
                    true
                }))
    }
    let mut a = with_pill();
    a.focused = true;
    a.set_selection(0, a.text.len());
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(App { a, b: TextEdit::new("").rich(), hooked: TextEdit::new("").rich() }, view, (320.0, 360.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    rt.key(cmd('c'));
    rt.frame(0.0);
    let copied = m.content().unwrap();
    assert_eq!(copied.text(), "a @x-agent b");
    assert!(copied.html().is_some_and(|h| h.contains("<span data-pill=\"agent:x\">@x-agent</span>")), "{copied:?}");
    let paste_into = |rt: &mut Runtime<App>, which: fn(&mut App) -> &mut TextEdit| {
        rt.update(move |s, _| { s.a.focused = false; s.b.focused = false; s.hooked.focused = false; which(s).focused = true; });
        rt.frame(0.0);
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
    };
    paste_into(&mut rt, |s| &mut s.b);
    assert_eq!(pills_of(&rt.state.b), [(2..10, "agent:x".to_owned())], "its own copy pastes the pill back");

    // The same HTML from outside (another app's copy) pastes plain labels, even with identical plain text, unless a hook keeps them.
    m.set_content(ClipboardContent::new("a @x-agent b").with_html("a <span data-pill=\"agent:x\">@x-agent</span> b"));
    rt.update(|s, _| s.b.set_selection(0, s.b.text.len()));
    paste_into(&mut rt, |s| &mut s.b);
    assert_eq!((rt.state.b.text.as_str(), rt.state.b.pills().count()), ("a @x-agent b", 0));
    m.set_content(ClipboardContent::new("x @x-agent").with_html("x <span data-pill=\"agent:x\">@x-agent</span>"));
    rt.update(|s, _| s.b.set_selection(0, s.b.text.len()));
    paste_into(&mut rt, |s| &mut s.b);
    assert_eq!((rt.state.b.text.as_str(), rt.state.b.pills().count()), ("x @x-agent", 0));
    paste_into(&mut rt, |s| &mut s.hooked);
    assert_eq!(pills_of(&rt.state.hooked), [(2..10, "agent:x".to_owned())]);
}

#[test]
fn pill_edge_cases_words_lines_bidi_and_direct_text_mutation() {
    // Touching pills, punctuation after a pill, and spaces inside a pill label.
    let mut e = TextEdit::new("").rich();
    e.insert_pill("u:1", "@maya");
    e.insert_pill("u:2", "@noah");
    e.insert(", ");
    e.insert_pill("u:3", "@Maya Lin");
    assert_eq!(e.text, "@maya@noah, @Maya Lin");
    let alt_left = KeyEvent { key: Key::Left, mods: Mods { alt: true, ..Mods::default() } };
    let alt_right = KeyEvent { key: Key::Right, mods: Mods { alt: true, ..Mods::default() } };
    let alt_bs = KeyEvent { key: Key::Backspace, mods: Mods { alt: true, ..Mods::default() } };
    e.key(&alt_left, None);
    assert_eq!(e.range(), (12, 12), "steps over @Maya Lin as one word");
    e.key(&alt_left, None);
    assert_eq!(e.range(), (10, 10), "stops between @noah and comma");
    e.key(&alt_left, None);
    assert_eq!(e.range(), (5, 5), "steps over @noah without eating @maya");
    e.key(&alt_left, None);
    assert_eq!(e.range(), (0, 0), "steps over @maya");
    e.key(&alt_right, None);
    assert_eq!(e.range(), (5, 5));
    e.key(&alt_right, None);
    assert_eq!(e.range(), (10, 10));
    e.key(&alt_bs, None);
    assert_eq!((e.text.as_str(), pills_of(&e)), ("@maya, @Maya Lin", vec![(0..5, "u:1".to_owned()), (7..16, "u:3".to_owned())]));

    // Vertical movement and line_edge across wrapped lines ending or starting with pills.
    let mut ts = crate::text::TextSystem::default();
    let style = crate::tok::TextStyle::new(14.0, 400.0);
    let mut w = TextEdit::new("hello world ").rich();
    w.set_caret(w.text.len());
    w.insert_pill("p:1", "@first-pill");
    w.insert(" ");
    w.insert_pill("p:2", "@second-pill");
    w.insert(" tail");
    let chips: Vec<crate::text::PillChip> = w
        .display_pills()
        .into_iter()
        .map(|(start, end, _)| crate::text::PillChip { start, end, icon: None, bg: None, fg: None, bare: None })
        .collect();
    let layout = ts.layout_blocks(&w.text, &[], w.blocks(), &chips, style, 2.0, Some(220.0), parley::Alignment::Start);
    assert!(layout.len() >= 3, "wrapped into {} lines", layout.len());
    // Caret at end of line 0 ("hello world "), press Down onto the pill line: snaps to a pill edge on line 1, never jumps to EOF.
    w.set_caret(11);
    w.key(&k(Key::Down), Some(&layout));
    assert!(w.focus < w.text.len(), "Down onto a pill line stays on that line: {}", w.focus);
    assert!(w.pill_around(w.focus).is_none(), "caret never lands inside a pill: {}", w.focus);
    // Shift+Down across a pill line covers the pill whole.
    w.set_caret(0);
    w.key(&KeyEvent { key: Key::Down, mods: Mods { shift: true, ..Mods::default() } }, Some(&layout));
    assert!(w.pill_around(w.focus).is_none());

    // IME preedit right after a pill does not bleed its underline box into the pill's chip.
    let mut ime_e = with_pill();
    ime_e.set_caret(10);
    ime_e.ime(&Ime::Preedit("に".into(), None));
    let (disp, pre) = ime_e.display();
    let (pa, pb) = pre.unwrap();
    let p_chips: Vec<crate::text::PillChip> = ime_e
        .display_pills()
        .into_iter()
        .map(|(start, end, _)| crate::text::PillChip { start, end, icon: None, bg: None, fg: None, bare: None })
        .collect();
    let ime_layout = ts.layout_blocks(&disp, &[], &[Block::Paragraph], &p_chips, style, 2.0, None, parley::Alignment::Start);
    let (mut decos, mut icons) = (Vec::new(), Vec::new());
    crate::text::pill_chips(&ime_layout, &p_chips, 14.0, 2.0, [0.0, 0.0], Color::BLACK, &mut decos, &mut icons);
    let boxes = crate::text::range_boxes(&ime_layout, pa, pb, false);
    assert_eq!(boxes.len(), 1);
    assert!(boxes[0].0.x0 as f32 >= decos[0].rect[0] + decos[0].rect[2] - 0.5, "preedit box {:?} overlaps chip {:?}", boxes[0].0, decos[0].rect);

    // RTL pill and mixed RTL+LTR pill chips have positive width.
    for (txt, start, end) in [("דוד", 0u32, 6u32), ("שלום@maya", 8, 13), ("@e\u{0301}", 0, 4)] {
        let pc = [crate::text::PillChip { start, end, icon: None, bg: None, fg: None, bare: None }];
        let l = ts.layout_blocks(txt, &[], &[Block::Paragraph], &pc, style, 2.0, None, parley::Alignment::Start);
        let (mut d, mut ic) = (Vec::new(), Vec::new());
        crate::text::pill_chips(&l, &pc, 14.0, 2.0, [0.0, 0.0], Color::BLACK, &mut d, &mut ic);
        assert_eq!(d.len(), 1, "{txt:?}");
        assert!(d[0].rect[2] > 8.0, "chip for {txt:?} has width {}", d[0].rect[2]);
    }

    // Direct text assignment on a field holding pills never panics or exposes invalid pill ranges.
    let mut direct = with_pill();
    direct.text = "é".into();
    assert_eq!(direct.pills().count(), 0);
    assert!(direct.display_pills().is_empty());
    direct.set_caret(99);
    assert_eq!(direct.range(), (2, 2));

    // Mark::Pill at collapsed caret does not arm pending; set_mark replaces an existing pill; plain insert strips a pill.
    let mut m = with_pill();
    m.set_caret(0);
    m.toggle_mark(Mark::Pill("x".into()));
    m.set_mark(Mark::Pill("x".into()));
    assert!(m.pending.is_none());
    m.set_selection(2, 10);
    m.set_mark(Mark::Pill("agent:y".into()));
    assert_eq!(pills_of(&m), [(2..10, "agent:y".to_owned())]);
    m.set_selection(2, 10);
    m.insert("@x-agent");
    assert_eq!((m.text.as_str(), m.pills().count()), ("a @x-agent b", 0));
}
