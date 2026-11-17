//! Exercise Rune's actual responder chain, including cancelled pointer releases. No daemon,
//! native window, disk writes or live demo state are involved.
use super::{Mode, lsp, view};
use crate::app::{App, Focus};
use rune::{Key, KeyEvent, Runtime};

const TEXT: &str = "Hello native editor\nsecond line\n";

fn frame(rt: &mut Runtime<App>) { rt.frame(1.0 / 60.0); }

fn runtime(path: &str) -> Runtime<App> {
    let mut app = App::new();
    app.open_file(path, None);
    app.apply_opened_file(path, TEXT.as_bytes());
    let mut rt = Runtime::new(app, crate::ui::chrome::root, (900.0, 600.0), rune::Env::new(true, 1.0));
    // This is the production AppSpec callback, not a direct Document/editor key call.
    rt.on_key = Some(crate::on_key);
    for _ in 0..4 { frame(&mut rt); }
    rt
}

fn key(rt: &mut Runtime<App>, key: Key) {
    rt.key(KeyEvent { key, mods: Default::default() });
    frame(rt);
}

fn point(rt: &Runtime<App>, col: usize) -> [f32; 2] {
    let [x, y, w, h] = rt.state.active_buffer().unwrap().view_rect.unwrap();
    // Use the mounted editor's geometry, including chrome and Markdown margins.
    for dy in 2..(h.min(100.0) as usize) {
        for dx in 2..w as usize {
            let p = [x + dx as f32, y + dy as f32];
            if view::hit_window(&rt.state, p[0], p[1], false) == Some((0, col)) { return p; }
        }
    }
    panic!("no mounted hit for column {col}");
}

fn click(rt: &mut Runtime<App>, col: usize) {
    let [x, y] = point(rt, col);
    rt.pointer_down(x, y);
    frame(rt);
    rt.pointer_up(x, y);
    frame(rt);
}

fn normal(rt: &Runtime<App>) {
    let b = rt.state.active_buffer().unwrap();
    assert_eq!(rt.state.focus, Focus::Content);
    assert_eq!(b.doc.mode, Mode::Normal);
    assert_eq!(b.doc.visual_anchor, None);
    assert!(!b.doc.vim.visual_linewise);
    assert!(!b.doc.vim.visual_block);
}

#[test]
fn native_click_preserves_normal_and_insert_but_clears_visual() {
    for path in ["/notes/native.md", "/notes/native.rs"] {
        for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
            let mut rt = runtime(path);
            rt.update(|a, _| {
                let b = a.active_buffer_mut().unwrap();
                b.doc.mode = mode;
                if mode == Mode::Visual {
                    b.doc.visual_anchor = Some(b.doc.cursor());
                    b.doc.vim.visual_linewise = true;
                }
            });
            frame(&mut rt);
            click(&mut rt, 4);
            if mode == Mode::Insert { assert_eq!(rt.state.editor_mode(), Some(Mode::Insert)); }
            else { normal(&rt); }
            let b = rt.state.active_buffer().unwrap();
            assert_eq!(b.doc.cursor_col, 4);
            assert_eq!(b.doc.text_for_disk(), TEXT);
            assert!(!b.dirty());
        }
    }
}

#[test]
fn native_escape_exits_insert_even_with_lsp_and_preserves_unsaved_text() {
    for path in ["/notes/native.md", "/notes/native.rs"] {
        let mut rt = runtime(path);
        key(&mut rt, Key::Text("i".into()));
        key(&mut rt, Key::Text("!".into()));
        rt.update(|a, _| {
            let b = a.active_buffer_mut().unwrap();
            b.lsp.completion = Some(lsp::Completion { seq: 1, line: 0, start: 0, items: vec![], selected: 0 });
            b.lsp.hover = Some(lsp::Hover { seq: 2, text: None, anchor: None });
        });
        key(&mut rt, Key::Escape);
        normal(&rt);
        let b = rt.state.active_buffer().unwrap();
        assert!(b.lsp.completion.is_none() && b.lsp.hover.is_none());
        assert_eq!(b.doc.text_for_disk(), format!("!{TEXT}"));
        assert!(b.dirty());
    }
}

#[test]
fn native_escape_cancels_held_drag_without_recreating_visual() {
    for path in ["/notes/native.md", "/notes/native.rs"] {
        for insert in [false, true] {
            let mut rt = runtime(path);
            if insert { key(&mut rt, Key::Text("i".into())); }
            let [x, y] = point(&rt, 1);
            rt.pointer_down(x, y);
            frame(&mut rt);
            let [end_x, end_y] = point(&rt, 12);
            rt.pointer_move(end_x, end_y);
            frame(&mut rt);
            assert_eq!(rt.state.editor_mode(), Some(Mode::Visual));
            // Rune swallows this key and sends Drag::cancelled, not on_key.
            key(&mut rt, Key::Escape);
            normal(&rt);
            rt.pointer_move(end_x + 10.0, end_y + 20.0);
            frame(&mut rt);
            rt.update(|a, tx| { assert!(!a.editor_mouse_tick(tx)); });
            rt.pointer_up(end_x + 10.0, end_y + 20.0);
            frame(&mut rt);
            normal(&rt);
            assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), TEXT);
        }
    }
}

#[test]
fn native_escape_after_release_clears_visual_and_held_insert_click() {
    let mut rt = runtime("/notes/native.md");
    let [start_x, start_y] = point(&rt, 1);
    let [end_x, end_y] = point(&rt, 12);
    rt.pointer_down(start_x, start_y);
    frame(&mut rt);
    rt.pointer_move(end_x, end_y);
    frame(&mut rt);
    rt.pointer_up(end_x, end_y);
    frame(&mut rt);
    assert_eq!(rt.state.editor_mode(), Some(Mode::Visual));
    key(&mut rt, Key::Escape);
    normal(&rt);
    key(&mut rt, Key::Text("v".into()));
    key(&mut rt, Key::Escape);
    normal(&rt);
    key(&mut rt, Key::Text("i".into()));
    let [x, y] = point(&rt, 3);
    rt.pointer_down(x, y);
    frame(&mut rt);
    assert_eq!(rt.state.editor_mode(), Some(Mode::Insert));
    key(&mut rt, Key::Escape);
    normal(&rt);
    rt.pointer_up(x, y);
    frame(&mut rt);
    normal(&rt);
}

#[test]
fn native_text_click_reclaims_keyboard_from_a_rune_field() {
    let mut app = App::new();
    app.open_file("/notes/native.md", None);
    app.apply_opened_file("/notes/native.md", TEXT.as_bytes());
    let mut rt = Runtime::new(app, |a, cx| {
        // A native field models the still-mounted composer in a neighbouring split.
        // All editor events still run through the production root and AppSpec callback.
        crate::ui::chrome::root(a, cx).child(
            rune::text_input(&a.tabs[0].draft, rune::TextStyle::new(14.0, 400.0))
                .key("native-focus-probe").w(100.0).h(24.0).absolute(780.0, 0.0)
                .on_edit(|a: &mut App| &mut a.tabs[0].draft)
                .on_input_key(crate::keys::shortcut))
    }, (900.0, 600.0), rune::Env::new(true, 1.0));
    rt.on_key = Some(crate::on_key);
    for _ in 0..4 { frame(&mut rt); }
    rt.update(|_, tx| tx.focus("native-focus-probe", false));
    frame(&mut rt);
    key(&mut rt, Key::Text("field".into()));
    assert_eq!(rt.state.tabs[0].draft.text, "field");
    click(&mut rt, 3);
    normal(&rt);
    key(&mut rt, Key::Text("i".into()));
    assert_eq!(rt.state.editor_mode(), Some(Mode::Insert));
    key(&mut rt, Key::Escape);
    normal(&rt);
    assert_eq!(rt.state.tabs[0].draft.text, "field");
    assert_eq!(rt.state.active_buffer().unwrap().doc.text_for_disk(), TEXT);
}