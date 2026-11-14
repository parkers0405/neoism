#[test]
fn a_file_drop_hovers_then_drops_at_its_point() {
    let steps: Vec<_> = parse_script(json!([
        {"drag_files": {"paths": ["a.png"], "key": "zone"}},
        {"t": 0.5, "drop_files": {"paths": ["a.png", "b.png"], "at": [10, 20], "hover": 0.25}},
        {"t": 1.0, "drag_files": "leave"},
    ])).into_iter().map(|e| (e.t, format!("{:?}", e.kind), format!("{:?}", e.pos))).collect();
    let expected = [
        (0.0, r#"Files(Hover(["a.png"]))"#, r#"Key("zone")"#),
        (0.5, r#"Files(Hover(["a.png", "b.png"]))"#, "Xy(10.0, 20.0)"),
        (0.75, r#"Files(Drop(["a.png", "b.png"]))"#, "Xy(10.0, 20.0)"),
        (1.0, "Files(Leave)", "Rel(0.0, 0.0)"),
    ].map(|(t, k, p)| (t, k.to_owned(), p.to_owned()));
    assert_eq!(steps, expected);
}

#[test]
fn a_physical_chord_releases_its_modifiers_after_the_key() {
    use crate::PhysicalInput;
    let kinds: Vec<_> = parse_script(json!([{"physical": "alt+super+space"}, {"t": 0.1, "physical": "f5"}]))
        .into_iter().map(|e| match e.kind { Kind::Physical(event) => event, _ => panic!("physical parsed as another event") }).collect();
    let mods = "alt+super+space".parse::<crate::Shortcut>().unwrap().modifiers();
    let key = |key, mods, pressed| PhysicalInput::Key { key, mods, pressed, repeat: false };
    use crate::PhysicalKeyCode::{F5, Space};
    let expected = [key(Space, mods, true), key(Space, mods, false), PhysicalInput::Modifiers(Mods::default()),
        key(F5, Mods::default(), true), key(F5, Mods::default(), false)];
    assert_eq!(format!("{kinds:?}"), format!("{expected:?}"));
}

#[test]
fn script_events_answer_a_file_dialog() {
    let kinds: Vec<_> = parse_script(json!([{"t": 0.1, "type": "file-dialog", "path": "/exports/a.json"}, {"t": 0.2, "type": "file-dialog-cancel"}]))
        .into_iter().map(|e| {
            let ScriptEvent::FileDialog(public) = e.public().event else { panic!("public event lost the dialog answer") };
            let Kind::FileDialog(path) = e.kind else { panic!("parsed as another event") };
            assert_eq!(public, path);
            path
        }).collect();
    assert_eq!(kinds, [Some("/exports/a.json".into()), None]);
}

#[test]
fn script_events_answer_a_pop_up_menu() {
    let kinds: Vec<_> = parse_script(json!([{"t": 0.2, "type": "menu-cancel"}, {"t": 0.1, "type": "menu-pick", "id": "dark"}]))
        .into_iter().map(|e| {
            let ScriptEvent::PopupMenu(public) = e.public().event else { panic!("public event lost the pop-up answer") };
            let Kind::PopupMenu(picked) = e.kind else { panic!("parsed as another event") };
            assert_eq!(public, picked);
            picked
        }).collect();
    assert_eq!(kinds, [Some("dark".to_owned()), None]);
}

#[test]
fn a_script_paste_can_carry_html_rtf_and_an_image() {
    let png = std::env::temp_dir().join(format!("rune-script-paste-{}.png", std::process::id()));
    std::fs::write(&png, crate::drag_out::encode_png(1, 2, &[9; 8]).unwrap()).unwrap();
    let script = parse_script(json!([
        {"type": "paste", "text": "plain"},
        {"paste": "short"},
        {"paste": {"text": "Hi", "html": "<b>Hi</b><img src=x onerror=y>", "rtf": "{\\rtf1 Hi}"}},
        {"paste": {"png": png.to_str().unwrap()}},
    ]));
    let public: Vec<_> = script.iter().map(|e| format!("{:?}", e.public().event)).collect();
    assert_eq!(public[..2], [r#"Paste("plain")"#, r#"Paste("short")"#]);
    assert!(public[2].starts_with("PasteContent("), "{}", public[2]);
    let mut rt = Runtime::new(Vec::new(), |_: &Vec<crate::ClipboardContent>, _: &mut crate::Cx<Vec<crate::ClipboardContent>>| {
        crate::el().on_paste_content(|s: &mut Vec<crate::ClipboardContent>, content, _| s.push(content.clone()))
    }, (100.0, 100.0), Env::default());
    rt.frame(0.0);
    for event in script {
        let Kind::Paste(content) = event.kind else { panic!("paste parsed as another event") };
        rt.paste_content(content);
    }
    rt.frame(0.0);
    let _ = std::fs::remove_file(&png);
    let texts: Vec<_> = rt.state.iter().map(|c| (c.text(), c.html(), c.rtf(), c.image().map(crate::ClipboardImage::size))).collect();
    assert_eq!(texts, [
        ("plain", None, None, None), ("short", None, None, None),
        ("Hi", Some(r#"<b>Hi</b><img src="x">"#), Some("{\\rtf1 Hi}"), None), ("", None, None, Some((1, 2))),
    ]);
}

#[test]
fn a_script_key_with_a_code_reaches_handlers_with_its_physical_key() {
    let script = parse_script(json!([
        {"type": "key", "name": "&", "cmd": true, "shift": true, "code": "7"},
        {"t": 0.1, "type": "key", "name": "&", "cmd": true, "shift": true},
    ]));
    type Seen = Vec<(Key, Option<crate::PhysicalKeyCode>)>;
    let mut rt = Runtime::new(Seen::new(), |_: &Seen, _: &mut crate::Cx<Seen>| crate::el(), (100.0, 100.0), Env::default());
    rt.on_key = Some(|s, ev, tx| { s.push((ev.key.clone(), tx.physical_key())); true });
    for event in script {
        match event.kind {
            Kind::Key(k, _) => rt.key(k),
            Kind::Physical(input) => rt.physical_input(input),
            kind => assert!(canvas_input(&mut rt, &kind, (0.0, 0.0)), "unexpected {kind:?}"),
        }
        rt.frame(0.0);
    }
    assert_eq!(rt.state, [(Key::Command('&'), Some(crate::PhysicalKeyCode::Digit7)), (Key::Command('&'), None)]);
}

#[test]
fn a_script_key_with_a_code_reaches_handlers_with_its_unmodified_character() {
    let script = parse_script(json!([
        {"type": "key", "name": "å", "alt": true, "code": "a", "base": "a"},
        {"t": 0.1, "type": "key", "name": "k", "cmd": true, "code": "k"},
        {"t": 0.2, "type": "key", "name": "&", "cmd": true, "shift": true, "code": "7"},
        {"t": 0.3, "type": "key", "name": "k", "cmd": true},
        {"t": 0.4, "type": "key-up", "name": "q", "code": "a"},
    ]));
    type Seen = Vec<(Key, Option<char>)>;
    let view = |_: &Seen, _: &mut crate::Cx<Seen>| crate::el()
        .on_key_up(|s: &mut Seen, ev, tx| { s.push((ev.key.clone(), tx.key_base())); true });
    let mut rt = Runtime::new(Seen::new(), view, (100.0, 100.0), Env::default());
    rt.on_key = Some(|s, ev, tx| { s.push((ev.key.clone(), tx.key_base())); true });
    for event in script {
        match event.kind {
            Kind::Key(k, _) => rt.key(k),
            Kind::Physical(input) => rt.physical_input(input),
            kind => assert!(canvas_input(&mut rt, &kind, (0.0, 0.0)), "unexpected {kind:?}"),
        }
        rt.frame(0.0);
    }
    assert_eq!(rt.state, [
        (Key::Text("å".into()), Some('a')),
        (Key::Command('k'), Some('k')),
        (Key::Command('&'), None),
        (Key::Command('k'), None),
        (Key::Text("q".into()), Some('q')),
    ]);
}

#[test]
fn a_script_key_can_be_a_held_keys_repeat() {
    let script = parse_script(json!([
        {"type": "key", "name": "Enter"},
        {"t": 0.1, "type": "key", "name": "Enter", "repeat": true},
        {"t": 0.2, "physical": "f5", "repeat": true},
    ]));
    let mut rt = Runtime::new(Vec::new(), |_: &Vec<bool>, _: &mut crate::Cx<Vec<bool>>| crate::el(), (100.0, 100.0), Env::default());
    rt.on_key = Some(|s, _, tx| { s.push(tx.key_repeat()); true });
    let mut physical = Vec::new();
    for event in script {
        match event.kind {
            Kind::Key(k, false) => rt.key(k),
            Kind::Key(k, true) => rt.repeat_key(k),
            Kind::Physical(crate::PhysicalInput::Key { pressed, repeat, .. }) => physical.push((pressed, repeat)),
            other => panic!("unexpected {other:?}"),
        }
    }
    rt.frame(0.0);
    assert_eq!(rt.state, [false, true]);
    assert_eq!(physical, [(true, true), (false, false)], "the release that ends a hold is not a repeat");
}

#[test]
fn window_panel_headless_focus_steps_preserve_false_and_event_order() {
    let script = parse_script(serde_json::json!([{"focus":true}, {"t":0.1,"focus":false}, {"t":0.2,"focus":false}]));
    let mut rt = Runtime::new(Vec::new(), |_: &Vec<bool>, _: &mut crate::Cx<Vec<bool>>| crate::el(), (100.0, 100.0), Env::default());
    rt.on_focus = Some(|s, focused, _| s.push(focused));
    assert_eq!(script.len(), 3);
    for event in script {
        let Kind::Focus(focused) = event.kind else { panic!("focus parsed as pointer action") };
        rt.window_focus(focused);
    }
    assert_eq!(rt.state, [true, false]);
}

#[test]
fn scripts_drive_wheels_pinches_other_buttons_and_key_releases() {
    use crate::{GesturePhase, PointerButton};
    let script = parse_script(json!([
        {"type": "wheel", "x": 10, "y": 20, "dx": -4, "dy": 6, "cmd": true, "phase": "began"},
        {"t": 0.1, "type": "magnify", "delta": 0.2},
        {"t": 0.2, "type": "drag", "button": "middle", "key": "canvas", "dx": 30, "dur": 0.01},
        {"t": 0.3, "type": "key-up", "name": " "},
    ]));
    let kinds: Vec<_> = script.iter().map(|e| format!("{:?}", e.kind)).collect();
    assert!(matches!(script[0].kind, Kind::Move), "a wheel at a point moves the pointer there: {kinds:?}");
    let Kind::WheelEvent(w) = script[1].kind else { panic!("{kinds:?}") };
    assert_eq!((w.dx, w.dy, w.mods.cmd, w.phase, w.precise), (-4.0, 6.0, true, GesturePhase::Began, true));
    assert!(matches!(script[2].kind, Kind::Magnify(m) if m.delta == 0.2) && matches!(script[2].pos, Pos::Rel(..)));
    assert!(matches!(script[4].kind, Kind::ButtonDown(PointerButton::Middle)), "{kinds:?}");
    assert!(script.iter().any(|e| matches!(e.kind, Kind::ButtonUp(PointerButton::Middle))));
    let last = script.last().unwrap();
    assert!(matches!(&last.kind, Kind::KeyUp(k) if k.key == Key::Text(" ".into())));
    assert!(matches!(last.public().event, ScriptEvent::KeyUp(_)));
    assert!(matches!(script[1].public().event, ScriptEvent::Pointer { action: PointerAction::Wheel(_), .. }));
}
