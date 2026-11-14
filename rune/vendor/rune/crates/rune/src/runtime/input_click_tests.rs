use super::*;
use crate::{Cx, El, Env, Ime, Key, KeyEvent, Mods, PhysicalInput, Runtime, TextEdit, TextStyle, col, text_input};

struct Form {
    single: TextEdit,
    multi: TextEdit,
    other: TextEdit,
    secret: TextEdit,
    secret_multi: TextEdit,
    pills: TextEdit,
    show_single: bool,
    presses: usize,
    drags: Vec<Phase>,
}

fn style() -> TextStyle {
    TextStyle::new(14.0, 400.0).lh(20.0)
}

fn view(s: &Form, _: &mut Cx<Form>) -> El {
    let st = style();
    let mut root = col().w_full().h_full().padding(8.0).gap(8.0);
    if s.show_single {
        root = root.child(
            text_input(&s.single, st)
                .single_line()
                .key("single")
                .size(280.0, 24.0)
                .on_edit(|s: &mut Form| &mut s.single)
                .on_press(|s: &mut Form| s.presses += 1)
                .on_drag(|s: &mut Form, d, _| s.drags.push(d.phase))
                .on_input_key(|s: &mut Form, ev, _| {
                    if ev.key == Key::Function(2) {
                        s.single.insert("!");
                        return true;
                    }
                    false
                }),
        );
    }
    root.child(
        text_input(&s.multi, st)
            .key("multi")
            .size(280.0, 80.0)
            .on_edit(|s: &mut Form| &mut s.multi),
    )
    .child(
        text_input(&s.other, st)
            .single_line()
            .key("other")
            .size(280.0, 24.0)
            .on_edit(|s: &mut Form| &mut s.other),
    )
    .child(
        text_input(&s.secret, st)
            .single_line()
            .secure()
            .key("secret")
            .size(280.0, 24.0)
            .on_edit(|s: &mut Form| &mut s.secret),
    )
    .child(
        text_input(&s.secret_multi, st)
            .secure()
            .key("secret-multi")
            .size(280.0, 48.0)
            .on_edit(|s: &mut Form| &mut s.secret_multi),
    )
    .child(
        text_input(&s.pills, st)
            .single_line()
            .key("pills")
            .size(280.0, 24.0)
            .on_edit(|s: &mut Form| &mut s.pills),
    )
}

fn runtime() -> Runtime<Form> {
    let mut pills = TextEdit::new("").rich();
    pills.insert_pill("u:1", "@maya");
    pills.insert(" hello");
    pills.set_caret(0);
    let state = Form {
        single: TextEdit::new("make the title smaller"),
        multi: TextEdit::new("first line here\nsecond line there\nthird line end"),
        other: TextEdit::new("other words"),
        secret: TextEdit::new("correct horse battery"),
        secret_multi: TextEdit::new("line one secret\nline two secret"),
        pills,
        show_single: true,
        presses: 0,
        drags: Vec::new(),
    };
    let mut rt = Runtime::new(state, view, (320.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn rect_of(rt: &Runtime<Form>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
}

/// Window point `(x, y)` at `frac` (`0.0..1.0`) across the glyph cluster at `byte` in `key`.
fn point_in_cluster(rt: &Runtime<Form>, key: &str, byte: usize, frac: f32) -> (f32, f32) {
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
    let l = n.text_layout.as_ref().unwrap();
    let s = rt.env.scale;
    let cl = parley::Cluster::from_byte_index(l, byte).unwrap();
    let x0 = cl.visual_offset().unwrap();
    let adv = cl.advance();
    let cur = Cursor::from_byte_index(l, byte, Affinity::Downstream);
    let bb = crate::text::cursor_geometry(&cur, l, 1.0);
    let r = rect_of(rt, key);
    (
        r[0] + n.inset[0] + (x0 + adv * frac) / s,
        r[1] + n.inset[1] + ((bb.y0 + bb.y1) * 0.5) as f32 / s,
    )
}

fn point_at_byte(rt: &Runtime<Form>, key: &str, byte: usize) -> (f32, f32) {
    point_in_cluster(rt, key, byte, 0.25)
}

fn click(rt: &mut Runtime<Form>, (x, y): (f32, f32)) {
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn double_and_triple_click_select_word_and_line_in_single_line_input() {
    let mut rt = runtime();
    let r = rect_of(&rt, "single");
    let (x, y) = (r[0] + 12.0, r[1] + r[3] * 0.5);

    // Unfocused first press focuses the input and places the caret inside "make".
    assert!(!rt.state.single.focused);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.single.focused);
    assert!(!rt.focus.keyboard(), "pointer focus does not turn on keyboard focus ring");
    assert_eq!(rt.state.single.anchor, rt.state.single.focus);
    assert!(rt.state.single.focus > 0 && rt.state.single.focus < 4);

    // Second press within double_click_interval and CLICK_SLOP selects "make" (0..4).
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (0, 4));
    assert_eq!(rt.state.single.selected(), "make");

    // Third press selects the whole single-line text; fourth stays at three.
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (0, rt.state.single.text.len()));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (0, rt.state.single.text.len()));
}

#[test]
fn triple_click_in_multiline_input_selects_hard_line_paragraph() {
    let mut rt = runtime();
    let second = rt.state.multi.text.find("second").unwrap();
    let third = rt.state.multi.text.find("third").unwrap();
    let pt = point_at_byte(&rt, "multi", second + 2);

    click(&mut rt, pt);
    click(&mut rt, pt);
    assert_eq!((rt.state.multi.anchor, rt.state.multi.focus), (second, second + 6));
    assert_eq!(rt.state.multi.selected(), "second");

    click(&mut rt, pt);
    assert_eq!((rt.state.multi.anchor, rt.state.multi.focus), (second, third));
    assert_eq!(rt.state.multi.selected(), "second line there\n");

    // Fourth click stays at three.
    click(&mut rt, pt);
    assert_eq!((rt.state.multi.anchor, rt.state.multi.focus), (second, third));
}

#[test]
fn click_drag_double_drag_and_triple_drag_extend_by_unit_in_both_directions() {
    let mut rt = runtime();
    let p_the = point_at_byte(&rt, "single", 6); // inside "the" (5..8)
    let p_smaller = point_at_byte(&rt, "single", 17); // inside "smaller" (15..22)
    let p_make = point_at_byte(&rt, "single", 1); // inside "make" (0..4)

    // Single-click drag selects by characters.
    rt.pointer_down(p_the.0, p_the.1);
    rt.frame(1.0 / 60.0);
    let start = rt.state.single.anchor;
    rt.pointer_move(p_smaller.0, p_smaller.1);
    rt.pointer_up(p_smaller.0, p_smaller.1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.single.anchor, start);
    assert!(rt.state.single.focus > 15 && rt.state.single.focus < 22);

    // Wait past double_click_interval, then double-click "the" (5..8) and drag right into "smaller" (15..22).
    rt.frame(1.0);
    click(&mut rt, p_the);
    rt.pointer_down(p_the.0, p_the.1);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (5, 8));
    rt.pointer_move(p_smaller.0, p_smaller.1);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (5, 22));
    assert_eq!(rt.state.single.selected(), "the title smaller");

    // Dragging back left into "make" (0..4) flips anchor to the far end of "the" (8) and focus to 0.
    rt.pointer_move(p_make.0, p_make.1);
    rt.pointer_up(p_make.0, p_make.1);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (8, 0));
    assert_eq!(rt.state.single.selected(), "make the");

    // Triple-click on line 2 of multi ("second line there\n") and drag down to line 3, then up to line 1.
    rt.frame(1.0);
    let second = rt.state.multi.text.find("second").unwrap();
    let third = rt.state.multi.text.find("third").unwrap();
    let len = rt.state.multi.text.len();
    let l1 = point_at_byte(&rt, "multi", 2);
    let l2 = point_at_byte(&rt, "multi", second + 2);
    let l3 = point_at_byte(&rt, "multi", third + 2);

    click(&mut rt, l2);
    click(&mut rt, l2);
    rt.pointer_down(l2.0, l2.1);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.multi.anchor, rt.state.multi.focus), (second, third));
    rt.pointer_move(l3.0, l3.1);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.multi.anchor, rt.state.multi.focus), (second, len));
    rt.pointer_move(l1.0, l1.1);
    rt.pointer_up(l1.0, l1.1);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.multi.anchor, rt.state.multi.focus), (third, 0));
}

#[test]
fn shift_click_extends_from_anchor_and_does_not_start_multiclick() {
    let mut rt = runtime();
    let p_make = point_at_byte(&rt, "single", 2);
    let p_title = point_at_byte(&rt, "single", 11);

    click(&mut rt, p_make);
    let anchor = rt.state.single.anchor;
    assert!(anchor > 0 && anchor < 4);

    // Shift+click at the same point right after a click extends (empty at anchor) instead of double-clicking.
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    click(&mut rt, p_make);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (anchor, anchor));

    // Shift+click further right extends from anchor to the clicked byte.
    click(&mut rt, p_title);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    assert_eq!(rt.state.single.anchor, anchor);
    assert!(rt.state.single.focus >= 9 && rt.state.single.focus <= 14);

    // Immediately clicking at p_title without Shift is a single click, not a double-click.
    click(&mut rt, p_title);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus);
}

#[test]
fn secure_inputs_select_all_on_double_and_triple_click_and_stay_pristine() {
    let mut rt = runtime();
    assert!(rt.state.secret.is_pristine());
    assert!(rt.state.secret_multi.is_pristine());

    let r1 = rect_of(&rt, "secret");
    let pt1 = (r1[0] + 12.0, r1[1] + r1[3] * 0.5);
    click(&mut rt, pt1);
    assert!(rt.state.secret.is_pristine());
    assert_eq!(rt.state.secret.anchor, rt.state.secret.focus);
    click(&mut rt, pt1);
    assert_eq!((rt.state.secret.anchor, rt.state.secret.focus), (0, rt.state.secret.text.len()));
    assert!(rt.state.secret.is_pristine(), "double-click selection must not mark a secure input edited");
    click(&mut rt, pt1);
    assert_eq!((rt.state.secret.anchor, rt.state.secret.focus), (0, rt.state.secret.text.len()));
    assert!(rt.state.secret.is_pristine());

    let r2 = rect_of(&rt, "secret-multi");
    let pt2 = (r2[0] + 12.0, r2[1] + 12.0);
    click(&mut rt, pt2);
    assert_eq!(
        (rt.state.secret_multi.anchor, rt.state.secret_multi.focus),
        (rt.state.secret_multi.text.len(), rt.state.secret_multi.text.len()),
    );
    click(&mut rt, pt2);
    assert_eq!((rt.state.secret_multi.anchor, rt.state.secret_multi.focus), (0, rt.state.secret_multi.text.len()));
    assert!(rt.state.secret_multi.is_pristine());
    click(&mut rt, pt2);
    assert_eq!((rt.state.secret_multi.anchor, rt.state.secret_multi.focus), (0, rt.state.secret_multi.text.len()));
    assert!(rt.state.secret_multi.is_pristine());
}

#[test]
fn timing_slop_and_target_changes_reset_multiclick_count_and_preedit_ignores_it() {
    let mut rt = runtime();
    let r = rect_of(&rt, "single");
    let pt = (r[0] + 12.0, r[1] + r[3] * 0.5);

    // 1. Waiting longer than double_click_interval resets count to 1.
    click(&mut rt, pt);
    let slow = rt.double_click_interval + 0.05;
    rt.frame(slow);
    click(&mut rt, pt);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus, "press after double_click_interval is a single click");

    // 2. Moving outside CLICK_SLOP resets count to 1.
    rt.frame(1.0);
    click(&mut rt, pt);
    click(&mut rt, (pt.0 + CLICK_SLOP + 1.0, pt.1));
    assert_eq!(rt.state.single.anchor, rt.state.single.focus, "press outside CLICK_SLOP is a single click");

    // 3. Pressing another input in between resets count to 1.
    rt.frame(1.0);
    let r_other = rect_of(&rt, "other");
    let pt_other = (r_other[0] + 12.0, r_other[1] + r_other[3] * 0.5);
    click(&mut rt, pt);
    click(&mut rt, pt_other);
    click(&mut rt, pt);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus, "pressing another input resets the click count");

    // 4. Active IME composition ignores multi-click and keeps preedit intact.
    rt.frame(1.0);
    rt.ime(Ime::Preedit("かん".into(), Some((0, 6))));
    rt.frame(1.0 / 60.0);
    let before = (rt.state.single.anchor, rt.state.single.focus);
    click(&mut rt, pt);
    click(&mut rt, pt);
    assert!(rt.state.single.preedit().is_some());
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), before);
}

#[test]
fn right_half_of_char_and_pill_trailing_padding_select_the_hit_word_and_pill() {
    let mut rt = runtime();
    // Byte 3 is the 'e' of "make" in "make the title smaller". Its right half places a single-click
    // caret at byte 4, but double-clicking there must still select "make" (0..4), not " " (4..5).
    let right_of_e = point_in_cluster(&rt, "single", 3, 0.8);
    click(&mut rt, right_of_e);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (4, 4));
    click(&mut rt, right_of_e);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), (0, 4));
    assert_eq!(rt.state.single.selected(), "make");

    // In "pills" ("@maya hello" where 0..5 is a pill), find the trailing inline box of "@maya".
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("pills")).unwrap();
    let l = n.text_layout.as_ref().unwrap();
    let s = rt.env.scale;
    let line = l.get(0).unwrap();
    let trail = line
        .items()
        .find_map(|it| match it {
            parley::PositionedLayoutItem::InlineBox(b) if b.id & 1 == 1 => Some(b),
            _ => None,
        })
        .expect("pill trailing inline box");
    let r = rect_of(&rt, "pills");
    let trail_pt = (r[0] + n.inset[0] + (trail.x + trail.width * 0.5) / s, r[1] + r[3] * 0.5);

    // Single click on trailing pill padding puts caret at pill end (5); double-click selects the pill (0..5).
    click(&mut rt, trail_pt);
    assert_eq!((rt.state.pills.anchor, rt.state.pills.focus), (5, 5));
    click(&mut rt, trail_pt);
    assert_eq!((rt.state.pills.anchor, rt.state.pills.focus), (0, 5));
    assert_eq!(rt.state.pills.selected(), "@maya");
}

#[test]
fn input_press_and_drag_selection_do_not_fire_on_press_or_on_drag() {
    let mut rt = runtime();
    let p_make = point_at_byte(&rt, "single", 1);
    let p_title = point_at_byte(&rt, "single", 11);

    // Plain click on an input with .on_press and .on_drag focuses and places the caret without firing either.
    click(&mut rt, p_make);
    assert_eq!(rt.state.presses, 0);
    assert!(rt.state.drags.is_empty());

    // Dragging a selection inside the input does not fire .on_press or .on_drag either.
    rt.frame(1.0);
    rt.pointer_down(p_make.0, p_make.1);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(p_title.0, p_title.1);
    rt.pointer_up(p_title.0, p_title.1);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.single.anchor < rt.state.single.focus);
    assert_eq!(rt.state.presses, 0);
    assert!(rt.state.drags.is_empty());
}

#[test]
fn focus_loss_and_input_removal_clear_active_drag_and_drop_queued_drag_steps() {
    let mut rt = runtime();
    let p_make = point_at_byte(&rt, "single", 1);
    let p_the = point_at_byte(&rt, "single", 6);
    let p_smaller = point_at_byte(&rt, "single", 18);

    // 1. Shift-click (which leaves `last = None`) + drag move queued, then focus changes before the drag runs.
    click(&mut rt, p_make);
    let anchor = rt.state.single.anchor;
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    rt.pointer_down(p_the.0, p_the.1);
    rt.frame(1.0 / 60.0);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    let after_shift = (rt.state.single.anchor, rt.state.single.focus);
    assert_eq!(after_shift.0, anchor);
    assert!(rt.input_click.dragging_path().is_some());

    // Queue a drag move, then lose focus before the frame drains the queue: the queued drag step is dropped.
    rt.pointer_move(p_smaller.0, p_smaller.1);
    rt. set_focus(None, false);
    rt.frame(1.0 / 60.0);
    assert!(rt.input_click.dragging_path().is_none());
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), after_shift);
    rt.pointer_up(p_smaller.0, p_smaller.1);
    rt.frame(1.0 / 60.0);

    // 2. Drag past CLICK_SLOP (clearing `last`), then unmount the input mid-drag and remount it while held.
    rt.frame(1.0);
    rt.pointer_down(p_make.0, p_make.1);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(p_the.0, p_the.1);
    rt.frame(1.0 / 60.0);
    let before_unmount = (rt.state.single.anchor, rt.state.single.focus);
    assert!(before_unmount.0 < before_unmount.1);

    rt.update(|s, tx| {
        s.show_single = false;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.input_click.dragging_path().is_none(), "unmounting the dragged input clears dragging");

    rt.update(|s, tx| {
        s.show_single = true;
        s.single.focused = true;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    rt.pointer_move(p_smaller.0, p_smaller.1);
    rt.pointer_up(p_smaller.0, p_smaller.1);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.single.anchor, rt.state.single.focus), before_unmount);
}

#[test]
fn cancel_pointer_and_window_focus_loss_clear_multiclick_history() {
    let mut rt = runtime();
    let pt = point_at_byte(&rt, "single", 2);

    // 1. cancel_pointer() between two quick clicks prevents double-click word selection.
    click(&mut rt, pt);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus);
    rt.cancel_pointer();
    click(&mut rt, pt);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus);

    // 2. Losing and regaining window focus between two quick clicks prevents double-click word selection.
    rt.frame(1.0);
    rt.window_focus(true);
    rt.frame(1.0 / 60.0);
    click(&mut rt, pt);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus);
    rt.window_focus(false);
    rt.window_focus(true);
    rt.frame(1.0 / 60.0);
    click(&mut rt, pt);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus);
}

#[test]
fn custom_input_on_key_handler_resets_multiclick_history() {
    let mut rt = runtime();
    let pt = point_at_byte(&rt, "single", 2);

    click(&mut rt, pt);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus);

    // Key::Function(2) is consumed by single's custom `.on_key` handler after inserting "!".
    rt.key(KeyEvent { key: Key::Function(2), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.single.text.contains('!'));

    // Clicking again immediately within double_click_interval is a single click, not a word selection.
    click(&mut rt, pt);
    assert_eq!(rt.state.single.anchor, rt.state.single.focus);
}

#[test]
fn presses_land_on_targets_inside_a_scrolled_viewport() {
    fn view(_: &Vec<usize>, _: &mut Cx<Vec<usize>>) -> El {
        let rows = (0..10).map(|i| crate::el().key(format!("row{i}")).w_full().h(50.0).on_press(move |s: &mut Vec<usize>| s.push(i)));
        col().size(200.0, 100.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(col().w_full().children(rows))
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.pointer_move(100.0, 25.0);
    rt.wheel(100.0, 25.0, -200.0);
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
    }
    rt.pointer_move(100.0, 25.0);
    rt.pointer_down(100.0, 25.0);
    rt.pointer_up(100.0, 25.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state, [4]);
}
