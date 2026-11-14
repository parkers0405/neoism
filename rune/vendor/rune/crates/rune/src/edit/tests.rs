use super::*;
use super::history::{HISTORY_BYTES, HISTORY_STEPS};

fn k(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

#[test]
fn is_command_matches_any_character_and_both_forms_of_cmd_space() {
    let cmd = Mods { cmd: true, ..Mods::default() };
    let ev = |key, mods| KeyEvent { key, mods };
    assert!(ev(Key::Command(','), cmd).is_command(','));
    assert!(ev(Key::Command('1'), cmd).is_command('1'));
    assert!(ev(Key::Command('k'), cmd).is_command('K'));
    assert!(ev(Key::Command('z'), Mods { shift: true, ..cmd }).is_command('z'));
    assert!(!ev(Key::Command('k'), cmd).is_command('j'));
    assert!(ev(Key::Text(" ".into()), cmd).is_command(' '));
    assert!(ev(Key::Command(' '), cmd).is_command(' '));
    assert!(!ev(Key::Text(" ".into()), Mods::default()).is_command(' '));
    assert!(!ev(Key::Text("k".into()), Mods::default()).is_command('k'));
}

fn undo(e: &mut TextEdit, redo: bool) {
    e.key(&KeyEvent { key: Key::Command('z'), mods: Mods { cmd: true, shift: redo, ..Mods::default() } }, None);
}

#[test]
fn secure_unicode_editing_has_no_history_or_export() {
    let mut e = TextEdit::new("hé🙂");
    e.insert("x");
    e.set_secure(true);
    assert!(e.history.undo.is_empty());
    assert_eq!(e.display().0, "••••");
    for (model, display) in [(0, 0), (1, 3), (3, 6), (7, 9), (8, 12)] {
        assert_eq!(e.display_index(model), display);
        assert_eq!(e.model_index(display), model);
    }
    e.anchor = 1;
    e.focus = 7;
    for command in ['c', 'x', 'z'] {
        assert!(e.key(&k(Key::Command(command)), None).copy.is_none());
        assert_eq!(e.text, "hé🙂x");
    }
    e.paste("界");
    assert_eq!((&*e.text, e.anchor, e.focus), ("h界x", 4, 4));
    e.ime(&Ime::Preedit("composition".into(), None));
    e.ime(&Ime::Commit("commit".into()));
    assert!(e.preedit().is_none());
    assert_eq!(e.text, "h界x");
    e.undo(); e.redo();
    assert_eq!(e.text, "h界x");
    assert!(e.history.undo.is_empty() && e.history.redo.is_empty());
    assert!(!format!("{e:?}").contains("h界x"));
    e.clear();
    assert!(e.is_secure());
}

#[test]
fn secure_revealed_uses_plain_display_and_words_while_keeping_protections() {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let hash = |e: &TextEdit| {
        let mut h = DefaultHasher::new();
        e.hash(&mut h);
        h.finish()
    };
    let mut e = TextEdit::new("hé🙂 world").secure();
    let hidden_hash = hash(&e);
    assert!(!e.is_revealed());
    assert_eq!(e.display().0, "•••••••••");
    // Hidden secure fields treat the masked string as a single word.
    let alt_left = KeyEvent { key: Key::Left, mods: Mods { alt: true, ..Mods::default() } };
    let alt_right = KeyEvent { key: Key::Right, mods: Mods { alt: true, ..Mods::default() } };
    e.set_caret(e.text.len());
    e.key(&alt_left, None);
    assert_eq!(e.focus, 0);
    e.key(&alt_right, None);
    assert_eq!(e.focus, e.text.len());

    e.set_selection(1, 7);
    e.set_revealed(true);
    assert!(e.is_secure() && e.is_revealed());
    assert_ne!(hash(&e), hidden_hash, "Hash for TextEdit must include revealed");
    assert_eq!((e.anchor, e.focus), (1, 7), "toggling revealed preserves selection");
    assert_eq!(e.display(), ("hé🙂 world".into(), None));
    for idx in [0, 1, 3, 7, 8, 13] {
        assert_eq!(e.display_index(idx), idx);
        assert_eq!(e.model_index(idx), idx);
    }
    // Revealed secure fields step over words normally.
    e.set_caret(e.text.len());
    e.key(&alt_left, None);
    assert_eq!(e.focus, 8);
    e.key(&alt_left, None);
    assert_eq!(e.focus, 0);
    e.key(&alt_right, None);
    assert_eq!(e.focus, 7);

    // Copy, cut, undo, redo and IME remain blocked when revealed.
    e.set_selection(1, 7);
    for command in ['c', 'x', 'z'] {
        assert!(e.key(&k(Key::Command(command)), None).copy.is_none());
        assert_eq!(e.text, "hé🙂 world");
    }
    e.key(&k(Key::Text("!".into())), None);
    assert_eq!(e.text, "h! world");
    assert!(!e.has_history());
    undo(&mut e, false);
    e.undo();
    e.redo();
    assert_eq!(e.text, "h! world");
    e.ime(&Ime::Preedit("comp".into(), Some((0, 4))));
    e.ime(&Ime::Commit("commit".into()));
    assert!(e.preedit().is_none());
    assert_eq!(e.text, "h! world");
    let dbg = format!("{e:?}");
    assert!(!dbg.contains("h! world") && !dbg.contains("world") && dbg.contains("[redacted]"));

    // Toggling revealed off again preserves text and selection.
    e.set_selection(2, 8);
    e.set_revealed(false);
    assert_eq!((&*e.text, e.anchor, e.focus), ("h! world", 2, 8));
    assert_eq!(e.display().0, "••••••••");

    // Conceal wipes a revealed field to bullets and clears revealed.
    let mut exiting = TextEdit::new("hunter2").secure().revealed(true);
    exiting.conceal();
    assert!(exiting.is_secure() && !exiting.is_revealed());
    assert_eq!(exiting.text, "•••••••");
    assert_eq!(exiting.display().0, "•••••••");
}

#[test]
fn hidden_multiline_navigation_and_edits_have_no_invisible_interior() {
    let mut e = TextEdit::new("first\nlast").secure();
    e.set_single_line(false);
    for key in [Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End] {
        e.set_caret(2);
        e.key(&k(key.clone()), None);
        assert_eq!(e.range(), (e.text.len(), e.text.len()));
        e.key(&KeyEvent { key, mods: Mods { shift: true, ..Mods::default() } }, None);
        assert_eq!(e.range(), (0, e.text.len()));
    }
    assert!(e.is_pristine(), "navigation keys do not clear pristine");
    e.mark_edited();
    e.set_caret(2);
    e.key(&k(Key::Text("!".into())), None);
    assert_eq!(e.text, "first\nlast!");
    e.set_caret(2);
    e.key(&k(Key::Backspace), None);
    assert_eq!(e.text, "first\nlast");
    e.key(&k(Key::Delete), None);
    assert_eq!(e.text, "first\nlast");
    e.set_caret(2);
    e.paste("\r\nnext");
    assert_eq!(e.text, "first\nlast\nnext");
    e.set_selection(1, 3);
    e.paste("replacement\nlines");
    assert_eq!(e.text, "replacement\nlines");
    e.set_revealed(true);
    e.set_selection(1, 3);
    e.set_revealed(false);
    e.set_revealed(true);
    assert_eq!(e.range(), (1, 3), "toggle without editing preserves revealed selection");
}

#[test]
fn pristine_secure_backspace_at_start_and_ime_do_not_bypass_protection() {
    let mut e = TextEdit::new("fictional-secret").secure();
    e.set_single_line(false);
    e.set_caret(0);
    e.key(&k(Key::Backspace), None);
    assert!(e.text.is_empty());
    assert!(!e.has_history());
    e.replace_text("fictional-secret");
    e.ime(&Ime::Preedit("composition".into(), None));
    assert!(e.preedit().is_none());
    assert!(e.is_pristine());
    e.ime(&Ime::Commit("commit".into()));
    assert_eq!(e.text, "fictional-secret", "secure inputs reject IME commits too");
    assert!(e.is_pristine());
    e.paste("replacement");
    assert_eq!(e.text, "replacement");
    assert!(!e.is_pristine());
    e.undo();
    assert_eq!(e.text, "replacement");
    assert!(!e.has_history());
}

#[test]
fn hidden_multiline_pristine_replace_and_delete() {
    // 1. Typing while pristine replaces the whole value, regardless of caret or navigation.
    let mut e = TextEdit::new("existing-fictional-key\nline-two").secure();
    e.set_single_line(false);
    assert!(e.is_pristine());
    e.set_caret(0);
    for nav in [Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End] {
        e.key(&k(nav), None);
    }
    assert!(e.is_pristine());
    e.key(&k(Key::Text("a".into())), None);
    assert_eq!(e.text, "a");
    assert!(!e.is_pristine());
    // Subsequent edits append and delete normally.
    e.key(&k(Key::Text("b".into())), None);
    assert_eq!(e.text, "ab");
    e.paste("cd");
    assert_eq!(e.text, "abcd");
    e.key(&k(Key::Backspace), None);
    assert_eq!(e.text, "abc");

    // 2. Pasting while pristine replaces the whole value, regardless of selection.
    let mut e = TextEdit::new("existing-fictional-key\nline-two").secure();
    e.set_single_line(false);
    e.set_selection(3, 7);
    e.paste("new-key\nline-2");
    assert_eq!(e.text, "new-key\nline-2");
    assert!(!e.is_pristine());
    e.paste("\nline-3");
    assert_eq!(e.text, "new-key\nline-2\nline-3");

    // 3. Deletions while pristine empty the whole value, then subsequent typing starts fresh.
    for del in [
        k(Key::Backspace),
        k(Key::Delete),
        KeyEvent { key: Key::Backspace, mods: Mods { cmd: true, ..Mods::default() } },
        KeyEvent { key: Key::Backspace, mods: Mods { alt: true, ..Mods::default() } },
        KeyEvent { key: Key::Text("k".into()), mods: Mods { ctrl: true, ..Mods::default() } },
    ] {
        let mut e = TextEdit::new("existing-fictional-key\nline-two").secure();
        e.set_single_line(false);
        e.set_caret(4);
        e.key(&del, None);
        assert_eq!(e.text, "", "deletion {del:?} must empty pristine hidden multiline");
        assert_eq!(e.range(), (0, 0));
        assert!(!e.is_pristine());
        e.key(&k(Key::Text("x".into())), None);
        e.key(&k(Key::Text("y".into())), None);
        assert_eq!(e.text, "xy");
    }

    // 4. Toggling revealed (Show -> Hide) without editing keeps pristine; editing while revealed clears it.
    let mut e = TextEdit::new("existing-fictional-key\nline-two").secure();
    e.set_single_line(false);
    e.set_revealed(true);
    e.set_selection(2, 5);
    e.set_revealed(false);
    assert!(e.is_pristine(), "Show then Hide without editing keeps pristine");
    e.set_caret(e.text.len());
    e.paste("pasted-after-peek");
    assert_eq!(e.text, "pasted-after-peek");

    let mut rev = TextEdit::new("existing-fictional-key").secure().revealed(true);
    rev.set_single_line(false);
    rev.set_caret(rev.text.len());
    rev.key(&k(Key::Text("!".into())), None);
    assert_eq!(rev.text, "existing-fictional-key!", "revealed field edits normally at caret");
    assert!(!rev.is_pristine());
    rev.set_revealed(false);
    rev.key(&k(Key::Text("?".into())), None);
    assert_eq!(rev.text, "existing-fictional-key!?", "edited while revealed stays non-pristine when hidden");

    // 5. App resetting via replace_text re-arms pristine, even for identical text.
    rev.replace_text("reloaded-from-vault");
    assert!(rev.is_pristine());
    rev.key(&k(Key::Text("z".into())), None);
    assert_eq!(rev.text, "z");
    assert!(!rev.is_pristine());
    rev.replace_text("z");
    assert!(rev.is_pristine(), "same-value app replacement re-arms pristine");
    rev.paste("fresh");
    assert_eq!(rev.text, "fresh");
    assert!(!rev.is_pristine());

    // 6. Empty field is never pristine; single-line secure field never applies pristine replacement.
    let mut empty = TextEdit::new("").secure();
    empty.set_single_line(false);
    assert!(!empty.is_pristine());
    empty.key(&k(Key::Text("a".into())), None);
    empty.key(&k(Key::Text("b".into())), None);
    assert_eq!(empty.text, "ab");

    let mut single = TextEdit::new("hunter2").secure();
    single.set_single_line(true);
    single.set_caret(7);
    single.key(&k(Key::Text("!".into())), None);
    assert_eq!(single.text, "hunter2!", "single-line secure field edits at caret");
}

#[test]
fn multiline_secure_edit_inserts_newlines_keeps_pasted_lines_and_masks_to_fixed_length() {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let hash = |e: &TextEdit| {
        let mut h = DefaultHasher::new();
        e.hash(&mut h);
        h.finish()
    };

    let mut e = TextEdit::new("").secure();
    e.set_single_line(false);
    assert!(!e.is_single_line() && e.is_hidden_multiline());
    assert_eq!(e.display().0, "");
    assert_eq!(e.display_index(0), 0);

    e.paste("-----BEGIN FAKE KEY-----\r\nAAAAB3NzaC1yc2E\tline2");
    assert_eq!(e.text, "-----BEGIN FAKE KEY-----\nAAAAB3NzaC1yc2E\tline2");
    assert!(!e.key(&k(Key::Enter), None).submit, "plain Enter in multi-line secure field inserts a newline");
    e.key(&k(Key::Text("-----END FAKE KEY-----".into())), None);
    assert_eq!(
        e.text,
        "-----BEGIN FAKE KEY-----\nAAAAB3NzaC1yc2E\tline2\n-----END FAKE KEY-----"
    );
    assert!(
        e.key(&KeyEvent { key: Key::Enter, mods: Mods { cmd: true, ..Mods::default() } }, None).submit,
        "Cmd+Enter still submits a multi-line secure field"
    );

    let mut short = TextEdit::new("ten-bytes!").secure();
    short.set_single_line(false);
    let long_secret = (0..50)
        .map(|i| format!("FAKEKEYLINE{i:02}AAAAB3NzaC1yc2EAAAADAQABAAABAQDfakebase64payloadblock0123456789ABCDEFGH=="))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(long_secret.len() >= 4096);
    let mut long = TextEdit::new(long_secret).secure();
    long.set_single_line(false);

    assert_eq!(short.display().0, MULTILINE_SECURE_MASK);
    assert_eq!(long.display().0, MULTILINE_SECURE_MASK);
    assert_eq!(hash(&short), hash(&long), "hidden multi-line hash must not leak length or line count");
    for offset in [0, 3, 10, 500, long.text.len()] {
        assert_eq!(long.display_index(offset), MULTILINE_SECURE_MASK.len());
        assert_eq!(long.model_index(offset), long.text.len());
    }
    assert_eq!(long.display_selection_range(5, 5), (0, 0));
    assert_eq!(long.display_selection_range(2, 100), (0, MULTILINE_SECURE_MASK.len()));

    // Concealing a multi-line secure field (hidden or revealed) leaves only the fixed mask.
    let mut exiting_revealed = long.clone().revealed(true);
    exiting_revealed.conceal();
    assert!(exiting_revealed.is_secure() && !exiting_revealed.is_revealed());
    assert_eq!(exiting_revealed.text, MULTILINE_SECURE_MASK);
    assert_eq!(exiting_revealed.display().0, MULTILINE_SECURE_MASK);
}

/// Arrows and backspace treat a ZWJ family, a flag and a skin-toned hand as one character.
#[test]
fn caret_and_backspace_step_over_whole_emoji() {
    let (family, flag, wave) = ("👨\u{200D}👩\u{200D}👧", "🇯🇵", "👋🏽");
    let text = format!("a{family}{flag}{wave}");
    let mut e = TextEdit::new(&text);
    e.key(&k(Key::Home), None);
    let mut stops = vec![e.focus];
    for _ in 0..5 { e.key(&k(Key::Right), None); stops.push(e.focus); }
    let ends = [0, 1, 1 + family.len(), 1 + family.len() + flag.len(), text.len(), text.len()];
    assert_eq!(stops, ends);
    for want in [format!("a{family}{flag}"), format!("a{family}"), "a".into()] {
        e.key(&k(Key::Backspace), None);
        assert_eq!(*e.text, want);
    }
}

#[test]
fn history_groups_typing_and_restores_unicode_selection() {
    let mut e = TextEdit::new("");
    for s in ["h", "é", "🙂"] { e.key(&k(Key::Text(s.into())), None); }
    undo(&mut e, false);
    assert_eq!((&*e.text, e.anchor, e.focus), ("", 0, 0));
    undo(&mut e, true);
    assert_eq!((&*e.text, e.anchor, e.focus), ("hé🙂", 7, 7));
    e.key(&k(Key::Left), None);
    e.key(&KeyEvent { key: Key::Left, mods: Mods { shift: true, ..Mods::default() } }, None);
    assert_eq!((e.anchor, e.focus), (3, 1));
    e.paste("界\r\n日");
    assert_eq!(e.text, "h界\n日🙂");
    undo(&mut e, false);
    assert_eq!((&*e.text, e.anchor, e.focus), ("hé🙂", 3, 1));
    undo(&mut e, true);
    assert_eq!((&*e.text, e.anchor, e.focus), ("h界\n日🙂", 8, 8));
    undo(&mut e, false);
    e.key(&k(Key::Text("a".into())), None);
    undo(&mut e, true);
    assert_eq!(e.text, "ha🙂", "new edit discards the redo branch");
    undo(&mut e, false);
    assert_eq!((&*e.text, e.anchor, e.focus), ("hé🙂", 3, 1));
}

#[test]
fn history_deletion_cut_and_ime_are_atomic() {
    let mut e = TextEdit::new("hé🙂");
    e.key(&k(Key::Backspace), None);
    undo(&mut e, false);
    assert_eq!((&*e.text, e.anchor, e.focus), ("hé🙂", 7, 7));
    e.key(&k(Key::Home), None);
    e.key(&k(Key::Delete), None);
    undo(&mut e, false);
    assert_eq!((&*e.text, e.anchor, e.focus), ("hé🙂", 0, 0));
    e.anchor = 3;
    e.focus = 1;
    let out = e.key(&KeyEvent { key: Key::Command('x'), mods: Mods::default() }, None);
    assert_eq!(out.copy.as_deref(), Some("é"));
    undo(&mut e, false);
    assert_eq!((e.anchor, e.focus), (3, 1));
    e.ime(&Ime::Preedit("に".into(), None));
    e.ime(&Ime::Preedit("日本".into(), None));
    e.ime(&Ime::Commit("日本".into()));
    assert_eq!(e.text, "h日本🙂");
    undo(&mut e, false);
    assert_eq!((&*e.text, e.anchor, e.focus), ("hé🙂", 3, 1));
    assert!(e.preedit().is_none());
    undo(&mut e, true);
    assert_eq!(e.text, "h日本🙂");
    e.ime(&Ime::Preedit("x".into(), None));
    undo(&mut e, false);
    assert_eq!(e.text, "h日本🙂", "undo cancels preedit without undoing committed text");
    assert!(e.preedit().is_none());
}

#[test]
fn history_is_bounded_shared_and_cleared_after_send() {
    let mut e = TextEdit::new("");
    for _ in 0..HISTORY_STEPS + 12 { e.paste("é"); }
    let rendered = e.clone();
    assert!(Arc::ptr_eq(&e.history, &rendered.history));
    assert_eq!(e.history.undo.len(), HISTORY_STEPS);
    for _ in 0..HISTORY_STEPS + 1 { e.undo(); }
    assert_eq!(e.text, "é".repeat(12));
    assert_eq!(rendered.text, "é".repeat(HISTORY_STEPS + 12));
    e.paste(&"a".repeat(HISTORY_BYTES));
    assert!(e.history.bytes <= HISTORY_BYTES);
    assert!(e.history.undo.is_empty(), "one oversized edit drops history rather than retaining unbounded text");
    e.focused = true;
    e.clear();
    e.undo();
    assert!(e.focused && e.text.is_empty());
    e.paste("draft");
    e.text = "external".into();
    e.anchor = e.text.len();
    e.focus = e.anchor;
    e.undo();
    assert_eq!(e.text, "external", "external model replacement cannot restore stale history");
}

fn chord(s: &str, mods: Mods) -> KeyEvent { KeyEvent { key: Key::Text(s.into()), mods } }

#[test]
fn command_chords_type_nothing() {
    let mut e = TextEdit::new("pw").secure();
    e.key(&chord(" ", Mods { cmd: true, shift: true, logo: true, ..Mods::default() }), None);
    e.key(&chord("a", Mods { ctrl: true, ..Mods::default() }), None);
    assert_eq!(e.text, "pw");
    let mut plain = TextEdit::new("");
    plain.key(&chord(" ", Mods { cmd: true, logo: true, ..Mods::default() }), None);
    plain.key(&chord("a", Mods { ctrl: true, cmd: !cfg!(target_os = "macos"), ..Mods::default() }), None);
    assert_eq!(plain.text, "");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn altgr_text_still_types() {
    let mut e = TextEdit::new("");
    e.key(&chord("€", Mods { ctrl: true, cmd: true, alt: true, ..Mods::default() }), None);
    assert_eq!(e.text, "€");
}

#[test]
fn option_text_still_types() {
    let mut e = TextEdit::new("").secure();
    e.key(&chord("é", Mods { alt: true, ..Mods::default() }), None);
    e.key(&chord("ø", Mods { alt: true, shift: true, ..Mods::default() }), None);
    assert_eq!(e.text, "éø");
}

#[test]
fn paste_keeps_single_line_fields_on_one_line() {
    let mut secure = TextEdit::new("").secure();
    let mut single = TextEdit::new("");
    single.set_single_line(true);
    let mut multi = TextEdit::new("");
    for e in [&mut secure, &mut single, &mut multi] { e.paste("a\tb\r\nc\u{2028}d"); }
    assert_eq!(secure.text, "abcd");
    assert_eq!(single.text, "abcd");
    assert_eq!(multi.text, "a\tb\nc\u{2028}d");
    single.ime(&Ime::Commit("x\ny".into()));
    assert_eq!(single.text, "abcdxy");
}

#[test]
fn shift_enter_in_a_single_line_field_submits_without_a_line_break() {
    let shift_enter = KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } };
    let mut single = TextEdit::new("ab");
    single.set_single_line(true);
    let mut secure = TextEdit::new("ab").secure();
    for e in [&mut single, &mut secure] {
        assert!(e.key(&shift_enter, None).submit);
        e.key(&k(Key::Text("\n".into())), None);
        e.key(&k(Key::Text("\r\nc".into())), None);
        assert_eq!(e.text, "abc");
    }
}

#[test]
fn command_chord_reaches_on_key() {
    use crate::{text_input, Cx, El, Env, Runtime, TextStyle};
    struct Form { pw: TextEdit, keys: Vec<Key> }
    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        text_input(&s.pw, TextStyle::new(14.0, 400.0)).single_line().key("pw").size(200.0, 40.0)
            .on_edit(|s: &mut Form| &mut s.pw)
    }
    let mut pw = TextEdit::new("").secure();
    pw.focused = true;
    let mut rt = Runtime::new(Form { pw, keys: vec![] }, view, (200.0, 40.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| { s.keys.push(ev.key.clone()); true });
    rt.frame(0.0);
    rt.key(chord(" ", Mods { cmd: true, shift: true, logo: true, ..Mods::default() })); rt.frame(0.0);
    rt.key(chord("x", Mods::default())); rt.frame(0.0);
    assert_eq!(rt.state.pw.text, "x");
    assert_eq!(rt.state.keys, [Key::Text(" ".into())]);
}

#[cfg(target_os = "macos")]
#[test]
fn control_letters_edit_like_appkit_fields() {
    let ctrl = |c: &str| KeyEvent { key: Key::Text(c.into()), mods: Mods { ctrl: true, ..Mods::default() } };
    let mut e = TextEdit::new("one two\nthree");
    e.set(4, false);
    e.key(&ctrl("a"), None); assert_eq!(e.focus, 0);
    e.key(&ctrl("e"), None); assert_eq!(e.focus, 7);
    e.key(&ctrl("b"), None); e.key(&ctrl("b"), None); assert_eq!(e.focus, 5);
    e.key(&ctrl("f"), None); assert_eq!(e.focus, 6);
    e.key(&ctrl("h"), None); assert_eq!(e.text, "one to\nthree");
    e.key(&ctrl("d"), None); assert_eq!(e.text, "one t\nthree");
    e.set(0, false);
    e.key(&ctrl("k"), None); assert_eq!(e.text, "\nthree");
    e.key(&ctrl("k"), None); assert_eq!(e.text, "three", "at a line's end Ctrl-K joins the next line");
    e.key(&ctrl("n"), None); assert_eq!(e.focus, 5);
    e.key(&ctrl("x"), None); assert_eq!(e.text, "three", "other Control letters never type");
}

#[test]
fn typing_selection_and_submit() {
    let mut e = TextEdit::default();
    e.key(&k(Key::Text("héllo world".into())), None);
    assert_eq!(e.text, "héllo world");
    e.key(&KeyEvent { key: Key::Backspace, mods: Mods { alt: true, ..Mods::default() } }, None);
    assert_eq!(e.text, "héllo ");
    e.key(&k(Key::Left), None);
    e.key(&KeyEvent { key: Key::Left, mods: Mods { shift: true, ..Mods::default() } }, None);
    assert_eq!(e.selected(), "o");
    e.key(&k(Key::Text("0".into())), None);
    assert_eq!(e.text, "héll0 ");
    e.ime(&Ime::Preedit("に".into(), None));
    assert_eq!(e.display().0, "héll0に ");
    e.ime(&Ime::Commit("日".into()));
    assert_eq!(e.text, "héll0日 ");
    assert!(e.key(&k(Key::Enter), None).submit);
    e.key(&KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } }, None);
    assert!(e.text.contains('\n'));
}

#[test]
fn inline_marks_shift_split_inherit_and_respect_boundaries() {
    let mut e = TextEdit::new("hello world");
    // Apply bold to "hello" (0..5).
    e.anchor = 0;
    e.focus = 5;
    e.toggle_mark(Mark::Bold);
    assert!(e.has_mark(&Mark::Bold));
    assert_eq!(e.marks(), &[MarkRange::new(0..5, Mark::Bold)]);

    // Caret at right edge of bold (index 5): inherits Bold on typing!
    e.set(5, false);
    assert!(e.has_mark(&Mark::Bold));
    e.key(&k(Key::Text("!".into())), None);
    assert_eq!(e.text, "hello! world");
    assert_eq!(e.marks(), &[MarkRange::new(0..6, Mark::Bold)]);

    // Caret at index 0 (start of bold run): does NOT inherit Bold.
    e.set(0, false);
    assert!(!e.has_mark(&Mark::Bold));
    e.key(&k(Key::Text(">".into())), None);
    assert_eq!(e.text, ">hello! world");
    assert_eq!(e.marks(), &[MarkRange::new(1..7, Mark::Bold)]);

    // Split Bold by toggling it off on "ll" (3..5).
    e.anchor = 3;
    e.focus = 5;
    assert!(e.has_mark(&Mark::Bold));
    e.toggle_mark(Mark::Bold);
    assert!(!e.has_mark(&Mark::Bold));
    assert_eq!(
        e.marks(),
        &[MarkRange::new(1..3, Mark::Bold), MarkRange::new(5..7, Mark::Bold)]
    );

    // Selecting across the gap (1..7) reports Bold is not active on the whole range,
    // so toggling Bold applies it to the whole selection and merges the runs.
    e.anchor = 1;
    e.focus = 7;
    assert!(!e.has_mark(&Mark::Bold));
    e.toggle_mark(Mark::Bold);
    assert!(e.has_mark(&Mark::Bold));
    assert_eq!(e.marks(), &[MarkRange::new(1..7, Mark::Bold)]);

    // Code and Link marks do NOT inherit at the start of their run (index 8).
    e.anchor = 8;
    e.focus = 13; // "world"
    e.toggle_mark(Mark::Code);
    e.set_link(Some("https://example.com"));
    // Inside "world" (index 10): both Code and Link are active and inherited.
    e.set(10, false);
    assert!(e.has_mark(&Mark::Code));
    assert_eq!(e.active_link(), Some("https://example.com"));
    // At start of link/code run (index 8): neither Code nor Link is inherited.
    e.set(8, false);
    assert!(!e.has_mark(&Mark::Code));
    assert_eq!(e.active_link(), None);
    e.key(&k(Key::Text("?".into())), None);
    assert_eq!(e.text, ">hello! ?world");
    assert_eq!(
        e.marks(),
        &[
            MarkRange::new(1..7, Mark::Bold),
            MarkRange::new(9..14, Mark::Code),
            MarkRange::new(9..14, Mark::Link("https://example.com".into())),
        ]
    );
}

#[test]
fn pending_typing_style_and_command_shortcuts() {
    let mut e = TextEdit::new("").rich();
    // Empty selection: Cmd+B sets pending Bold, Cmd+I sets pending Italic.
    e.key(&KeyEvent { key: Key::Command('b'), mods: Mods { cmd: true, ..Mods::default() } }, None);
    e.key(&KeyEvent { key: Key::Command('i'), mods: Mods { cmd: true, ..Mods::default() } }, None);
    assert!(e.has_mark(&Mark::Bold));
    assert!(e.has_mark(&Mark::Italic));

    e.key(&k(Key::Text("hi".into())), None);
    assert_eq!(e.text, "hi");
    assert_eq!(
        e.marks(),
        &[MarkRange::new(0..2, Mark::Bold), MarkRange::new(0..2, Mark::Italic)]
    );

    // Toggle Bold off on empty selection at index 2, then type " there".
    e.key(&KeyEvent { key: Key::Command('b'), mods: Mods { cmd: true, ..Mods::default() } }, None);
    assert!(!e.has_mark(&Mark::Bold));
    assert!(e.has_mark(&Mark::Italic));
    e.key(&k(Key::Text(" there".into())), None);
    assert_eq!(e.text, "hi there");
    assert_eq!(
        e.marks(),
        &[MarkRange::new(0..2, Mark::Bold), MarkRange::new(0..8, Mark::Italic)]
    );

    // Moving the caret clears pending typing style.
    e.toggle_mark(Mark::Underline);
    assert!(e.has_mark(&Mark::Underline));
    e.key(&k(Key::Left), None);
    assert!(!e.has_mark(&Mark::Underline));
}

#[test]
fn unicode_emoji_cjk_and_combining_marks_across_boundaries() {
    let mut e = TextEdit::new("🦀a\u{0301}日本語");
    // "🦀" is 0..4, "a\u{0301}" is 4..7, "日" is 7..10, "本" is 10..13, "語" is 13..16.
    e.anchor = 0;
    e.focus = 4;
    e.toggle_mark(Mark::Bold);
    e.anchor = 4;
    e.focus = 7;
    e.toggle_mark(Mark::Italic);
    e.anchor = 7;
    e.focus = 13;
    e.toggle_mark(Mark::Strike);
    e.anchor = 10;
    e.focus = 16;
    e.toggle_mark(Mark::Underline);

    // Insert CJK right at boundary between combining mark and CJK (byte 7).
    e.set(7, false);
    e.key(&k(Key::Text("★".into())), None); // 3 bytes, inherits Italic from 4..7
    assert_eq!(e.text, "🦀a\u{0301}★日本語");
    assert_eq!(
        e.marks(),
        &[
            MarkRange::new(0..4, Mark::Bold),
            MarkRange::new(4..10, Mark::Italic),
            MarkRange::new(10..16, Mark::Strike),
            MarkRange::new(13..19, Mark::Underline),
        ]
    );

    // Delete across boundary ("★日", bytes 7..13).
    e.anchor = 7;
    e.focus = 13;
    e.key(&k(Key::Backspace), None);
    assert_eq!(e.text, "🦀a\u{0301}本語");
    assert_eq!(
        e.marks(),
        &[
            MarkRange::new(0..4, Mark::Bold),
            MarkRange::new(4..7, Mark::Italic),
            MarkRange::new(7..10, Mark::Strike),
            MarkRange::new(7..13, Mark::Underline),
        ]
    );

    // Round-trip through HTML preserves exact Unicode mark boundaries.
    let html = e.to_html();
    let rt = TextEdit::from_html(&html);
    assert_eq!(rt.text, e.text);
    assert_eq!(rt.marks(), e.marks());
}

#[test]
fn block_attributes_lists_quotes_code_and_keys() {
    let mut e = TextEdit::new("first").rich();
    e.set_block(Block::Bullet(0));
    assert_eq!(e.current_block(), Block::Bullet(0));

    // Return at end of non-empty list item creates a new Bullet(0) item.
    e.set(5, false);
    assert!(!e.key(&k(Key::Enter), None).submit);
    assert_eq!(e.text, "first\n");
    assert_eq!(e.blocks(), &[Block::Bullet(0), Block::Bullet(0)]);

    // Tab indents the second list item to Bullet(1).
    e.key(&k(Key::Tab), None);
    assert_eq!(e.blocks(), &[Block::Bullet(0), Block::Bullet(1)]);
    e.key(&k(Key::Text("nested".into())), None);

    // Shift+Return inserts a soft break inside the same list item.
    let shift_enter = KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } };
    e.key(&shift_enter, None);
    e.key(&k(Key::Text("cont".into())), None);
    assert_eq!(e.soft_breaks(), &[2]);
    assert_eq!(e.blocks(), &[Block::Bullet(0), Block::Bullet(1), Block::Bullet(1)]);

    // Return at end of "cont" creates a new empty Bullet(1) item on line 3.
    e.key(&k(Key::Enter), None);
    assert_eq!(e.blocks().len(), 4);
    assert_eq!(e.current_block(), Block::Bullet(1));

    // Return on the empty Bullet(1) item ends the list item (converts to Paragraph) without adding a newline.
    let len_before = e.text.len();
    e.key(&k(Key::Enter), None);
    assert_eq!(e.text.len(), len_before);
    assert_eq!(e.current_block(), Block::Paragraph);

    // Backspace at start of a nested list item outdents first, then exits list.
    e.set_block(Block::Numbered(1));
    e.key(&k(Key::Backspace), None);
    assert_eq!(e.current_block(), Block::Numbered(0));
    e.key(&k(Key::Backspace), None);
    assert_eq!(e.current_block(), Block::Paragraph);

    // Inside a Code block: Return inserts a newline, Tab inserts '\t', and inline marks do not apply.
    e.set_block(Block::Code);
    e.toggle_mark(Mark::Bold);
    assert!(!e.has_mark(&Mark::Bold));
    e.key(&k(Key::Tab), None);
    e.key(&k(Key::Text("let x = 1;".into())), None);
    assert!(!e.key(&k(Key::Enter), None).submit);
    e.key(&k(Key::Text("let y = 2;".into())), None);
    assert!(e.text.ends_with("\tlet x = 1;\nlet y = 2;"));
    assert_eq!(e.current_block(), Block::Code);
    assert!(e.marks().is_empty());
}

#[test]
fn undo_redo_and_ime_preserve_marks_blocks_and_spans() {
    let mut e = TextEdit::new("fn main() {}");
    e.anchor = 0;
    e.focus = 2;
    e.toggle_mark(Mark::Bold);
    e.set_block(Block::Quote);

    // Undo block change, then undo mark change, then redo both.
    undo(&mut e, false);
    assert_eq!(e.current_block(), Block::Paragraph);
    assert_eq!(e.marks(), &[MarkRange::new(0..2, Mark::Bold)]);
    undo(&mut e, false);
    assert!(e.marks().is_empty());
    undo(&mut e, true);
    assert_eq!(e.marks(), &[MarkRange::new(0..2, Mark::Bold)]);
    undo(&mut e, true);
    assert_eq!(e.current_block(), Block::Quote);

    // IME preedit with pending typing style and caller-supplied span mapping.
    e.set(3, false); // right before "main"
    e.toggle_mark(Mark::Italic);
    // Suppose caller supplies a syntax span covering "main" (3..7 in model).
    assert_eq!(e.map_range_to_display(3, 7), vec![(3, 7)]);
    e.ime(&Ime::Preedit("日".into(), Some((0, 3))));
    // During preedit ("fn 日main() {}"), "main" in display is shifted by 3 bytes to 6..10!
    assert_eq!(e.display().0, "fn 日main() {}");
    assert_eq!(e.map_range_to_display(3, 7), vec![(6, 10)]);
    // And the preedit itself has the pending Italic mark in rich_display_specs.
    let (spans, _, _, _) = e.rich_display_specs(crate::TextStyle::new(14.0, 400.0), RichStyle::default());
    assert!(spans.iter().any(|s| s.start == 3 && s.end == 6 && s.italic));

    // Commit IME preedit: Italic mark is committed on "日" (3..6).
    e.ime(&Ime::Commit("日".into()));
    assert_eq!(e.text, "fn 日main() {}");
    assert!(e.marks().contains(&MarkRange::new(3..6, Mark::Italic)));

    // Undo restores "fn main() {}" and its original marks.
    undo(&mut e, false);
    assert_eq!(e.text, "fn main() {}");
    assert_eq!(e.marks(), &[MarkRange::new(0..2, Mark::Bold)]);
    assert_eq!(e.map_range_to_display(3, 7), vec![(3, 7)]);
}

#[test]
fn html_import_export_and_paste_safe_subset() {
    let html = concat!(
        "<p>Hello <b>bold</b>, <strong style=\"font-style: italic\">both</strong>, ",
        "<u>under</u>, <del>struck</del>, <code>inline()</code>, and ",
        "<a href=\"https://opencode.ai\">link</a>",
        "<a href=\"javascript:alert(1)\">bad</a></p>",
        "<ul><li>first<ul><li>nested<br>soft</li></ul></li></ul>",
        "<ol><li>one</li><li>two</li></ol>",
        "<blockquote>quoted</blockquote>",
        "<pre><code>fn add(a: i32) -&gt; i32 {\n\ta + 1\n}</code></pre>",
        "<script>evil()</script><style>body{display:none}</style>",
    );
    let e = TextEdit::from_html(html);
    assert!(!e.text.contains("evil"));
    assert!(!e.text.contains("display:none"));
    assert!(e.marks().iter().any(|m| m.mark == Mark::Bold));
    assert!(e.marks().iter().any(|m| m.mark == Mark::Italic));
    assert!(e.marks().iter().any(|m| m.mark == Mark::Underline));
    assert!(e.marks().iter().any(|m| m.mark == Mark::Strike));
    assert!(e.marks().iter().any(|m| m.mark == Mark::Code));
    assert!(e.marks().iter().any(|m| m.mark == Mark::Link("https://opencode.ai".into())));
    assert!(!e.marks().iter().any(|m| matches!(&m.mark, Mark::Link(u) if u.starts_with("javascript:"))));
    assert_eq!(
        e.blocks(),
        &[
            Block::Paragraph,
            Block::Bullet(0),
            Block::Bullet(1),
            Block::Bullet(1),
            Block::Numbered(0),
            Block::Numbered(0),
            Block::Quote,
            Block::Code,
            Block::Code,
            Block::Code,
        ]
    );
    assert_eq!(e.soft_breaks(), &[3]);

    // Round-trip via to_html() -> from_html().
    let exported = e.to_html();
    let round = TextEdit::from_html(&exported);
    assert_eq!(round.text, e.text);
    assert_eq!(round.marks(), e.marks());
    assert_eq!(round.blocks(), e.blocks());
    assert_eq!(round.soft_breaks(), e.soft_breaks());

    // paste_html into an existing editor splices text, marks, and blocks cleanly.
    let mut dest = TextEdit::new("prefix  suffix").rich();
    dest.set(7, false);
    dest.paste_html("<b>rich</b> <i>paste</i>", "rich paste");
    assert_eq!(dest.text, "prefix rich paste suffix");
    assert_eq!(
        dest.marks(),
        &[
            MarkRange::new(7..11, Mark::Bold),
            MarkRange::new(12..17, Mark::Italic),
        ]
    );
}

#[test]
fn spans_on_text_input_hold_through_ime_and_undo() {
    use crate::{Cx, El, Env, Runtime, Span, TextStyle, text_input};
    struct App {
        edit: TextEdit,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        // Highlight the first 3 bytes ("let") with a caller-supplied Span on plain text_input.
        let end = s.edit.text.len().min(3);
        let spans = if end > 0 {
            vec![Span::new(0, end).color(crate::Color::WHITE).bold()]
        } else {
            vec![]
        };
        text_input(&s.edit, TextStyle::new(14.0, 400.0))
            .key("ed")
            .size(240.0, 60.0)
            .spans(spans)
            .on_edit(|s: &mut App| &mut s.edit)
    }
    let mut edit = TextEdit::new("let x = 1;");
    edit.focused = true;
    edit.set(0, false);
    let mut rt = Runtime::new(App { edit }, view, (240.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Start IME preedit at byte 0: display text becomes "あlet x = 1;", so caller's 0..3
    // span on `edit.text` shifts automatically to 3..6 in the rendered TextSpec!
    rt.ime(Ime::Preedit("あ".into(), Some((0, 3))));
    rt.frame(0.0);
    let spec_spans = &rt.nodes.iter().find(|n| n.key.as_deref() == Some("ed")).unwrap().text.as_ref().unwrap().spans;
    assert_eq!(spec_spans.len(), 1);
    assert_eq!((spec_spans[0].start, spec_spans[0].end), (3, 6));

    // Commit IME preedit, then undo back to "let x = 1;": span returns to 0..3.
    rt.ime(Ime::Commit("あ".into()));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Command('z'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "let x = 1;");
    let spec_spans = &rt.nodes.iter().find(|n| n.key.as_deref() == Some("ed")).unwrap().text.as_ref().unwrap().spans;
    assert_eq!(spec_spans.len(), 1);
    assert_eq!((spec_spans[0].start, spec_spans[0].end), (0, 3));
}

#[test]
fn runtime_routes_mark_shortcuts_list_tabs_and_renders_decorations() {
    use crate::{Cx, El, Env, Runtime, TextStyle, col, text_input};
    struct App {
        edit: TextEdit,
        other: TextEdit,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        col()
            .size(320.0, 200.0)
            .child(
                text_input(&s.edit, TextStyle::new(14.0, 400.0))
                    .key("ed")
                    .size(300.0, 140.0)
                    .on_edit(|s: &mut App| &mut s.edit),
            )
            .child(
                text_input(&s.other, TextStyle::new(14.0, 400.0))
                    .key("other")
                    .size(300.0, 40.0)
                    .on_edit(|s: &mut App| &mut s.other),
            )
    }
    let mut edit = TextEdit::from_html(
        "<ul><li>item</li></ul><ol><li><del>struck</del></li></ol><blockquote>quote</blockquote><pre><code>let x = 1;</code></pre>",
    );
    edit.focused = true;
    edit.anchor = 0;
    edit.focus = 4; // "item"
    let mut rt = Runtime::new(
        App { edit, other: TextEdit::new("next") },
        view,
        (320.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let base_instances = rt.instances.len();
    assert!(base_instances > 10);

    // Cmd+B, Cmd+I, Cmd+U toggle marks on the selection through Runtime::key.
    rt.key(KeyEvent { key: Key::Command('b'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.key(KeyEvent { key: Key::Command('i'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.key(KeyEvent { key: Key::Command('u'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
    assert!(rt.state.edit.has_mark(&Mark::Bold));
    assert!(rt.state.edit.has_mark(&Mark::Italic));
    assert!(rt.state.edit.has_mark(&Mark::Underline));

    // Tab inside a list item indents the list item instead of moving focus to "other"!
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.edit.focused);
    assert!(!rt.state.other.focused);
    assert_eq!(rt.state.edit.blocks()[0], Block::Bullet(1));

    // Shift+Tab outdents the list item back to Bullet(0).
    rt.key(KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.edit.blocks()[0], Block::Bullet(0));
}

fn cmd(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } }
}

/// The text under each range of `mark`.
fn marked<'a>(e: &'a TextEdit, mark: &Mark) -> Vec<&'a str> {
    e.marks().iter().filter(|m| m.mark == *mark).map(|m| &e.text[m.start..m.end]).collect()
}

/// What every edit path must leave behind.
fn assert_invariants(e: &TextEdit, ctx: &str) {
    let t = &e.text;
    let lines = 1 + t.bytes().filter(|&b| b == b'\n').count();
    assert_eq!(e.blocks().len(), lines, "{ctx}: one block per line");
    assert!(e.blocks().iter().all(|b| b.list_level().is_none_or(|l| l <= MAX_LIST_LEVEL)), "{ctx}: list levels");
    assert!(e.soft_breaks().windows(2).all(|w| w[0] < w[1]), "{ctx}: soft breaks sorted");
    assert!(e.soft_breaks().iter().all(|&i| i > 0 && (i as usize) < lines), "{ctx}: soft breaks in range");
    assert!(t.is_char_boundary(e.anchor) && t.is_char_boundary(e.focus), "{ctx}: caret on a char boundary");
    let ms = e.marks();
    assert!(ms.windows(2).all(|w| w[0].start <= w[1].start), "{ctx}: marks sorted");
    for m in ms {
        assert!(m.start < m.end && m.end <= t.len(), "{ctx}: mark {m:?} in range");
        assert!(t.is_char_boundary(m.start) && t.is_char_boundary(m.end), "{ctx}: mark {m:?} on char boundaries");
        for line in e.line_of_byte(m.start)..=e.line_of_byte(m.end - 1) {
            assert_ne!(e.blocks()[line], Block::Code, "{ctx}: mark {m:?} reaches code line {line}");
        }
    }
    for (i, a) in ms.iter().enumerate() {
        for b in &ms[i + 1..] {
            if a.mark.same_kind(&b.mark) {
                assert!(b.start >= a.end, "{ctx}: same-kind marks {a:?} {b:?} overlap");
                assert!(!(b.start == a.end && a.mark == b.mark), "{ctx}: equal marks {a:?} {b:?} not merged");
            }
        }
    }
}


include!("rich_tests.rs");
