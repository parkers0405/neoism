//! Offline native-input regressions against the real file, transcript and tree views.
//! No setup(), daemon, filesystem fixture, or OS clipboard is involved. Markdown files
//! currently share the code editor; rendered Markdown lives in the agent transcript.
use crate::{app::App, editor::Mode};
use rune::{Cx, El, Env, Runtime, TouchPhase};

#[derive(Clone, Copy, Debug)]
enum Surface { Code, MarkdownFile, Prose, FencedCode }

#[test]
fn finger_tab_close_requires_a_tap_not_a_drag_or_cancel() {
    for (workspace, gesture) in [false, true].into_iter().flat_map(|workspace| {
        ["tap", "close-drag", "tab-drag", "cancel"].map(|gesture| (workspace, gesture))
    }) {
        let mut app = App::new();
        if workspace { app.new_workspace(); } else { app.open_tab(crate::tabs::TabKind::Home); }
        let tab = app.active_tab().id;
        let workspace_id = app.workspace_id.clone();
        let tab_key = if workspace { format!("workspace-tab:{workspace_id}") } else { format!("pane-tab:{tab}") };
        let close_key = if workspace { format!("workspace-close:{}", app.workspace_active) } else { format!("pane-close:{tab}") };
        let mut rt = Runtime::new(app, crate::ui::chrome::root, (1000.0, 700.0), Env::new(true, 2.0));
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        let (x, y) = rt.find(&tab_key).unwrap();
        rt.pointer_move(x, y);
        rt.frame(1.0 / 120.0);
        let (x, y) = rt.find(if gesture == "tab-drag" { &tab_key } else { &close_key }).unwrap();
        rt.pointer_leave();
        rt.frame(1.0 / 120.0);
        rt.touch(1, TouchPhase::Started, x, y, 1.0);
        rt.frame(1.0 / 120.0);
        if gesture.ends_with("drag") {
            rt.touch(1, TouchPhase::Moved, x - 60.0, y, 1.02);
            rt.frame(1.0 / 120.0);
            if gesture == "tab-drag" { assert!(rt.state.tab_drag.is_some(), "tab body must still drag"); }
            // Returning to the X must not re-arm a close after the finger has dragged.
            rt.touch(1, TouchPhase::Moved, x, y, 1.04);
            rt.frame(1.0 / 120.0);
        }
        let phase = if gesture == "cancel" { TouchPhase::Cancelled } else { TouchPhase::Ended };
        rt.touch(1, phase, x + 1.0, y + 1.0, 1.05);
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        if workspace {
            assert_eq!(rt.state.workspaces.iter().any(|w| w.id == workspace_id), gesture != "tap", "workspace {gesture}");
        } else {
            assert_eq!(rt.state.tabs.iter().any(|t| t.id == tab), gesture != "tap", "buffer {gesture}");
        }
        assert!(rt.state.tab_drag.is_none(), "no gesture may leave a tab drag active after release");
        assert!(rt.state.modal.is_none(), "a drag must not request closing an unsaved tab");
    }
}

#[test]
fn a_finger_pan_keeps_the_frame_clock_running_until_release() {
    for terminal in [TouchPhase::Ended, TouchPhase::Cancelled] {
        let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| {
            rune::col().w_full().h_full().scroll(rune::ScrollSpec { stick_bottom: false })
                .child(rune::col().w_full().h(4000.0))
        }, (900.0, 674.0), Env::new(true, 2.0));
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        rt.touch(1, TouchPhase::Started, 400.0, 500.0, 1.0);
        rt.touch(1, TouchPhase::Moved, 400.0, 400.0, 1.02);
        for _ in 0..90 {
            assert!(rt.frame(1.0 / 120.0), "an active finger pan must not stop and restart the display link between input samples");
        }
        rt.touch(1, terminal, 400.0, 400.0, 2.0);
        let mut rested = false;
        for _ in 0..240 { rested |= !rt.frame(1.0 / 120.0); }
        assert!(rested, "releasing without velocity must let the frame clock rest");
    }
}

impl Surface {
    fn file(self) -> bool { matches!(self, Self::Code | Self::MarkdownFile) }
}

struct Rig {
    rt: Runtime<App>,
    clipboard: rune::clipboard::MemoryClipboard,
    surface: Surface,
}

impl Rig {
    fn new(surface: Surface) -> Self {
        let mut app = App::new();
        app.window = [900.0, 674.0];
        assert!(app.daemon.is_none());
        if surface.file() {
            let path = if matches!(surface, Surface::Code) { "touch.rs" } else { "touch.md" };
            let source = (0..180).map(|i| {
                if matches!(surface, Surface::Code) { format!("let touch_row_{i} = \"selectable words\";\n") }
                else { format!("Markdown row {i}: **selectable words** and `code`.\n") }
            }).collect::<String>();
            app.open_file(path, None);
            app.apply_opened_file(path, source.as_bytes());
        } else {
            let source = if matches!(surface, Surface::Prose) {
                (0..80).map(|i| format!("Touch paragraph {i} has selectable words for a native finger swipe.\n\n")).collect::<String>()
            } else {
                format!("```rust\n{}\n```", (0..100).map(|i| format!("let touch_row_{i} = \"selectable words\";\n")).collect::<String>())
            };
            app.load_fixture("touch", "Touch", &serde_json::json!([{
                "id": "touch-message", "role": "Assistant", "kind": "Assistant", "text": source
            }]).to_string());
            assert_eq!(app.sessions["touch"].messages.len(), 1, "fixture must deserialize");
        }
        let view: fn(&App, &mut Cx<App>) -> El = if surface.file() {
            |a, _| crate::editor::view(a)
        } else {
            |a, cx| crate::agent::timeline::pane(a, Some("touch"), cx)
        };
        let mut rt = Runtime::new(app, view, (900.0, 674.0), Env::new(true, 1.0));
        let clipboard = rune::clipboard::memory();
        clipboard.set_text("untouched sentinel");
        rt.use_clipboard(&clipboard);
        for _ in 0..90 { rt.frame(1.0 / 120.0); }
        let mut rig = Self { rt, clipboard, surface };
        rig.position(400.0);
        assert!(rig.offset() > 300.0, "fixture must start away from either edge");
        rig
    }

    fn key(&self) -> String {
        if self.surface.file() {
            let a = &self.rt.state;
            crate::editor::view::scroll_key(&a.workspace_id, &a.active_buffer().unwrap().path)
        } else { "conversation:touch".into() }
    }

    fn position(&mut self, y: f32) {
        let key = self.key();
        self.rt.update(move |_, tx| tx.scroll_to_y(key, y));
        for _ in 0..4 { self.rt.frame(1.0 / 120.0); }
    }

    fn offset(&self) -> f32 {
        if self.surface.file() { self.rt.state.active_buffer().unwrap().scroll.get().0 }
        else { self.rt.state.sessions["touch"].scroll.get().0 }
    }

    fn point(&self) -> (f32, f32) {
        // Text, not gutter, scrollbar, code-frame Copy button or floating composer.
        let elements = self.rt.elements();
        let e = elements.iter().find(|e| {
            e.text.as_ref().is_some_and(|s| s.contains("selectable words"))
                && e.rect[1] < 330.0 && e.rect[1] + e.rect[3] > 220.0
        }).expect("fixture must expose real text in the swipe region");
        let top = e.rect[1].max(200.0);
        let bottom = (e.rect[1] + e.rect[3]).min(350.0);
        (e.rect[0] + 55.0, (top + bottom) / 2.0)
    }

    fn clean(&self) {
        assert!(self.rt.selected_text().is_none(), "{:?}: finger left Rune text selection: {:?}", self.surface, self.rt.selected_text());
        if self.surface.file() {
            assert_ne!(self.rt.state.active_buffer().unwrap().doc.mode, Mode::Visual, "{:?}: finger entered Visual mode", self.surface);
        } else {
            assert!(!self.rt.state.selection.copy.dragging(), "pan must cancel selection lifecycle");
            assert!(self.rt.state.selection.copy.selected.is_none());
            assert!(self.rt.state.copied_selection.is_none(), "pan must not auto-copy");
        }
        assert_eq!(self.clipboard.text().as_deref().map(|s| s.as_str()), Some("untouched sentinel"));
    }

    fn swipe(&mut self, dy: f32, held: bool) {
        let (x, y) = self.point();
        let before = self.offset();
        self.rt.touch(1, TouchPhase::Started, x, y, 1.0);
        self.rt.frame(1.0 / 120.0);
        self.clean(); // pointer_down must not create a visible range or copy.
        self.rt.touch(1, TouchPhase::Moved, x + 1.0, y + dy.signum() * 3.0, 1.01);
        self.rt.frame(1.0 / 120.0);
        self.clean(); // still below Rune's eight-point pending threshold.
        assert!((self.offset() - before).abs() < 1.0);
        for i in 1..=5 {
            self.rt.touch(1, TouchPhase::Moved, x, y + dy * i as f32 / 5.0, 1.01 + i as f64 * 0.01);
            self.rt.frame(1.0 / 120.0);
        }
        self.rt.touch(1, TouchPhase::Ended, x, y + dy, if held { 1.5 } else { 1.065 });
        self.rt.frame(1.0 / 120.0);
        let lifted = self.offset();
        // Check selection first so a capture regression explains why scrolling failed.
        self.clean();
        assert!((lifted - before) * -dy.signum() > 70.0,
            "{:?}: finger dy={dy} must scroll opposite direction: {before} -> {lifted}", self.surface);
        for _ in 0..24 { self.rt.frame(1.0 / 120.0); }
        self.clean();
        if held { assert!((self.offset() - lifted).abs() < 1.0, "held release must not fling"); }
        else { assert!((self.offset() - lifted) * -dy.signum() > 5.0, "quick swipe must coast"); }
    }
}

#[test] fn code_swipes_scroll_not_select() { Rig::new(Surface::Code).swipe(-100.0, false); }
#[test] fn markdown_file_swipes_scroll_not_select() { Rig::new(Surface::MarkdownFile).swipe(-100.0, false); }
#[test] fn agent_prose_swipes_scroll_not_select() { Rig::new(Surface::Prose).swipe(-100.0, false); }
#[test] fn agent_fenced_code_swipes_scroll_not_select() { Rig::new(Surface::FencedCode).swipe(-100.0, false); }

#[test] fn code_downward_held_swipe() { Rig::new(Surface::Code).swipe(100.0, true); }
#[test] fn markdown_downward_held_swipe() { Rig::new(Surface::MarkdownFile).swipe(100.0, true); }
#[test] fn prose_downward_held_swipe() { Rig::new(Surface::Prose).swipe(100.0, true); }
#[test] fn fenced_code_downward_held_swipe() { Rig::new(Surface::FencedCode).swipe(100.0, true); }

#[test]
fn holding_then_dragging_text_selects_instead_of_scrolling() {
    for surface in [Surface::Code, Surface::MarkdownFile, Surface::Prose, Surface::FencedCode] {
        let mut rig = Rig::new(surface);
        let (x, y) = rig.point();
        let before = rig.offset();
        rig.rt.touch(1, TouchPhase::Started, x, y, 1.0);
        rig.rt.frame(1.0 / 120.0);
        rig.clean();
        rig.rt.touch(1, TouchPhase::Moved, x + 2.0, y + 1.0, 1.2);
        rig.rt.frame(1.0 / 120.0);
        rig.clean();
        rig.rt.touch(1, TouchPhase::Moved, x + 110.0, y + 30.0, 1.65);
        rig.rt.frame(1.0 / 120.0);
        if surface.file() {
            assert_eq!(rig.rt.state.active_buffer().unwrap().doc.mode, Mode::Visual);
        } else {
            assert!(rig.rt.selected_text().is_some(), "{surface:?}: held finger drag must highlight text");
        }
        rig.rt.touch(1, TouchPhase::Ended, x + 110.0, y + 30.0, 1.7);
        rig.rt.frame(1.0 / 120.0);
        assert!(!rig.rt.touching());
        assert!(!rig.rt.pressing());
        assert!((rig.offset() - before).abs() < 1.0, "{surface:?}: selecting inside the viewport must not pan");
        if !surface.file() { assert!(rig.rt.selected_text().is_some(), "highlight must survive lifting the finger"); }
    }
}

fn tap(surface: Surface) {
    tap_with_jitter(surface, 2.0, 2.0);
}

fn tap_with_jitter(surface: Surface, dx: f32, dy: f32) {
        let mut rig = Rig::new(surface);
        let (x, y) = rig.point();
        let before = rig.offset();
        rig.rt.touch(1, TouchPhase::Started, x, y, 1.0);
        rig.rt.frame(1.0 / 120.0);
        rig.clean();
        rig.rt.touch(1, TouchPhase::Moved, x + dx, y + dy, 1.02);
        rig.rt.frame(1.0 / 120.0);
        rig.clean();
        rig.rt.touch(1, TouchPhase::Ended, x + dx, y + dy, 1.05);
        rig.rt.frame(1.0 / 120.0);
        rig.clean();
        assert!((rig.offset() - before).abs() < 1.0);
        assert!(!rig.rt.touching());
        assert!(!rig.rt.pressing());
}

#[test] fn code_tap() { tap(Surface::Code); }
#[test] fn markdown_tap() { tap(Surface::MarkdownFile); }
#[test] fn prose_tap() { tap(Surface::Prose); }
#[test] fn fenced_code_tap() { tap(Surface::FencedCode); }

#[test]
fn sub_threshold_tap_jitter_does_not_become_a_selection_on_release() {
    // Nearly eight points can cross a glyph/row boundary, but is still a tap.
    // Exercise both axes and directions against actual text, not an empty target.
    for surface in [Surface::Code, Surface::MarkdownFile, Surface::Prose, Surface::FencedCode] {
        for (dx, dy) in [(7.5, 0.0), (-7.5, 0.0), (0.0, 7.5), (0.0, -7.5)] {
            tap_with_jitter(surface, dx, dy);
        }
    }
}

fn cancelled_pan(surface: Surface) {
    let mut rig = Rig::new(surface);
    let (x, y) = rig.point();
    rig.rt.touch(1, TouchPhase::Started, x, y, 1.0);
    rig.rt.frame(1.0 / 120.0);
    rig.rt.touch(1, TouchPhase::Moved, x, y - 60.0, 1.03);
    rig.rt.frame(1.0 / 120.0);
    rig.rt.touch(1, TouchPhase::Cancelled, x, y - 60.0, 1.04);
    for _ in 0..3 { rig.rt.frame(1.0 / 120.0); }
    rig.clean();
    assert!(!rig.rt.touching());
    let stopped = rig.offset();
    for _ in 0..24 { rig.rt.frame(1.0 / 120.0); }
    assert!((rig.offset() - stopped).abs() < 1.0, "system cancellation must not fling");
}

#[test] fn code_cancelled_pan() { cancelled_pan(Surface::Code); }
#[test] fn markdown_cancelled_pan() { cancelled_pan(Surface::MarkdownFile); }
#[test] fn prose_cancelled_pan() { cancelled_pan(Surface::Prose); }
#[test] fn fenced_code_cancelled_pan() { cancelled_pan(Surface::FencedCode); }

#[test]
fn repeated_editor_swipes_do_not_move_the_cursor_or_start_multi_click_selection() {
    for surface in [Surface::Code, Surface::MarkdownFile] {
        let mut rig = Rig::new(surface);
        let (x, y) = rig.point();
        let cursor = rig.rt.state.active_buffer().unwrap().doc.cursor();
        let before = rig.offset();
        for n in 0..3 {
            let t = 1.0 + n as f64 * 0.1;
            rig.rt.touch(1, TouchPhase::Started, x, y, t);
            rig.rt.frame(1.0 / 120.0);
            rig.clean();
            for i in 1..=4 {
                rig.rt.touch(1, TouchPhase::Moved, x, y - 20.0 * i as f32, t + i as f64 * 0.01);
                rig.rt.frame(1.0 / 120.0);
            }
            rig.rt.touch(1, TouchPhase::Ended, x, y - 80.0, t + 0.045);
            rig.rt.frame(1.0 / 120.0);
            rig.clean();
            assert_eq!(rig.rt.state.active_buffer().unwrap().doc.cursor(), cursor);
        }
        assert!(rig.offset() > before + 200.0);
    }
}

#[test]
fn mouse_drags_still_select_actual_text() {
    for surface in [Surface::Code, Surface::MarkdownFile, Surface::Prose, Surface::FencedCode] {
        let mut rig = Rig::new(surface);
        let (x, y) = rig.point();
        let before = rig.offset();
        rig.rt.pointer_move(x, y);
        rig.rt.pointer_down(x, y);
        rig.rt.frame(1.0 / 120.0);
        rig.rt.pointer_move(x + 110.0, y + 30.0);
        rig.rt.frame(1.0 / 120.0);
        rig.rt.pointer_up(x + 110.0, y + 30.0);
        rig.rt.frame(1.0 / 120.0);
        if surface.file() {
            assert_eq!(rig.rt.state.active_buffer().unwrap().doc.mode, Mode::Visual);
        } else {
            assert!(rig.rt.selected_text().is_some(), "{surface:?}: mouse must select");
            assert!(rig.rt.state.copied_selection.is_some(), "mouse release still auto-copies");
            assert_ne!(rig.clipboard.text().as_deref().map(|s| s.as_str()), Some("untouched sentinel"));
        }
        assert!((rig.offset() - before).abs() < 1.0, "mouse selection inside viewport must not scroll");
    }
}

// These fixtures exercise tree::view's actual row handlers, not a substitute on_drag.
// No daemon exists, so no test can touch disk. The daemon sender and Tree's pending
// fields are private: move intent is verified through the existing reply path below,
// not by inventing a production-only request recorder or exposing private state.
struct TreeRig {
    rt: Runtime<App>,
    rows: Vec<crate::tree::Row>,
    open: std::collections::HashSet<String>,
    selected: usize,
    tabs: usize,
}

impl TreeRig {
    fn new() -> Self {
        let mut app = App::new();
        app.window = [320.0, 674.0];
        app.sidebar = Some(crate::app::Sidebar::Files);
        app.focus = crate::app::Focus::Sidebar;
        let mut entries = vec![
            serde_json::json!({"name": "a-source", "is_dir": true}),
            serde_json::json!({"name": "b-target", "is_dir": true}),
            serde_json::json!({"name": "c-other", "is_dir": true}),
        ];
        entries.extend((0..80).map(|i| serde_json::json!({"name": format!("folder-{i:03}"), "is_dir": true})));
        entries.extend((0..80).map(|i| serde_json::json!({"name": format!("file-{i:03}.rs"), "is_dir": false})));
        app.apply_dir_listing("", &serde_json::Value::Array(entries));
        app.apply_dir_listing("a-source", &serde_json::json!([
            {"name": "nested", "is_dir": true},
            {"name": "sibling.rs", "is_dir": false},
            {"name": "source.rs", "is_dir": false}
        ]));
        app.apply_dir_listing("a-source/nested", &serde_json::json!([]));
        app.apply_dir_listing("b-target", &serde_json::json!([]));
        app.tree.open.insert("a-source".into());
        // A gesture must not silently change the keyboard row even when it starts elsewhere.
        app.tree.selected = 2;
        assert!(app.daemon.is_none());
        assert!(app.buffers.is_empty());
        let rows = app.tree.rows();
        let open = app.tree.open.clone();
        let selected = app.tree.selected;
        let tabs = app.tabs.len();
        let mut rt = Runtime::new(app, |a: &App, _: &mut Cx<App>| crate::tree::view(a),
            (320.0, 674.0), Env::new(true, 1.0));
        for _ in 0..90 { rt.frame(1.0 / 120.0); }
        Self { rt, rows, open, selected, tabs }
    }

    fn frame(&mut self) { self.rt.frame(1.0 / 120.0); }

    fn wait_for_hold(&mut self) {
        // Advance the native runtime's clock; never sleep or depend on wall-clock load.
        for _ in 0..66 { self.frame(); }
    }

    fn offset(&self) -> f32 { self.rt.state.tree_follow.scroll.get().0 }

    fn position(&mut self, y: f32) {
        self.rt.update(move |_, tx| tx.scroll_to_y(crate::tree::view::SCROLL_KEY, y));
        for _ in 0..4 { self.frame(); }
        assert!((self.offset() - y).abs() < 1.0, "tree fixture must reach requested offset");
    }

    fn row_point(&self, path: &str) -> (f32, f32) {
        let key = format!("tree:{path}");
        let elements = self.rt.elements();
        let rect = elements.iter().find(|e| e.key.as_deref() == Some(key.as_str()))
            .unwrap_or_else(|| panic!("tree fixture missing actual row {path}")).rect;
        let view = self.rt.state.rects[crate::tree::view::SCROLL_KEY];
        let y = rect[1] + rect[3] * 0.5;
        assert!(y > view[1] && y < view[1] + view[3], "{path} must be visible");
        (rect[0] + 100.0, y)
    }

    fn touch(&mut self, phase: TouchPhase, (x, y): (f32, f32), time: f64) {
        self.rt.touch(1, phase, x, y, time);
        self.frame();
    }

    fn unchanged(&self) {
        let app = &self.rt.state;
        assert!(app.daemon.is_none(), "fixture must stay offline");
        assert_eq!(app.tree.rows(), self.rows, "gesture must not create/move/delete a tree entry");
        assert_eq!(app.tree.open, self.open, "gesture must not toggle or spring-open a folder");
        assert_eq!(app.tree.selected, self.selected, "scrolling must not activate a row");
        assert!(app.buffers.is_empty(), "gesture must not open a file");
        assert_eq!(app.tabs.len(), self.tabs, "gesture must not open a tab");
        assert!(app.context.is_none() && app.prompt.is_none(), "gesture must not open a rename/delete/create command");
        assert!(self.rt.selected_text().is_none());
    }

    fn no_drag(&self) {
        assert!(self.rt.state.tree_dnd.is_none(), "ordinary touch must not even initialize tree drag");
        self.unchanged();
    }

    fn released(&self) {
        assert!(self.rt.state.tree_dnd.is_none(), "release/cancel must clear tree drag");
        assert!(!self.rt.touching());
        assert!(!self.rt.pressing(), "release/cancel must clear native press capture");
    }

    fn start_held_drag(&mut self, source: &str, target: (f32, f32)) {
        let start = self.row_point(source);
        let before = self.offset();
        self.touch(TouchPhase::Started, start, 1.0);
        self.no_drag(); // Custom Down must be deferred, not just the first Move.
        self.touch(TouchPhase::Moved, (start.0 + 2.0, start.1 + 1.0), 1.1);
        self.no_drag(); // Ordinary finger jitter is still pending.
        self.wait_for_hold();
        self.touch(TouchPhase::Moved, start, 1.65);
        self.touch(TouchPhase::Moved, target, 1.7);
        assert!(self.rt.state.tree_dnd.is_some(), "holding then dragging must use existing tree drag lifecycle");
        assert!(self.rt.state.tree_drag_row(false, source).0, "held source must be live/lifted");
        assert!((self.offset() - before).abs() < 1.0, "a held tree drag is not a pan");
        self.unchanged(); // No move or activation before release.
    }

    fn assert_completed_move(&mut self, folder: &str, name: &str, is_dir: bool) {
        self.released();
        assert!(self.rt.state.tree.open.contains(folder), "valid drop must open its destination");
        assert!(!self.rt.state.tree_drag_row(false, "a-source/source.rs").0);
        assert!(self.rt.state.buffers.is_empty(), "drop release must not also activate its source");
        assert_eq!(self.rt.state.tabs.len(), self.tabs);
        // tree_move_path records select_after even with no daemon. An unrelated listing
        // must not fulfill it; the exact destination listing must select the moved file.
        let folder = folder.to_owned();
        let name = name.to_owned();
        let expected = format!("{folder}/{name}");
        self.rt.update(move |a, _| {
            a.apply_dir_listing("c-other", &serde_json::json!([]));
            assert_eq!(a.tree.selected, 2, "unrelated listing cannot fulfill pending drop destination");
            a.apply_dir_listing(&folder, &serde_json::json!([{"name": name, "is_dir": is_dir}]));
        });
        self.frame();
        let selected = &self.rt.state.tree.rows()[self.rt.state.tree.selected].path;
        assert_eq!(selected, &expected, "drop must record exact pending move destination");
    }
}

fn tree_swipe(dy: f32) {
    let mut rig = TreeRig::new();
    rig.position(400.0);
    let before = rig.offset();
    // At this offset this point is on a real closed folder row, not blank tree space.
    let (x, y) = (100.0, 300.0);
    rig.touch(TouchPhase::Started, (x, y), 1.0);
    rig.no_drag();
    rig.touch(TouchPhase::Moved, (x + 2.0, y + 2.0), 1.01);
    rig.no_drag();
    assert!((rig.offset() - before).abs() < 1.0);
    for i in 1..=5 {
        rig.touch(TouchPhase::Moved, (x, y + dy * i as f32 / 5.0), 1.01 + i as f64 * 0.01);
        rig.no_drag();
    }
    rig.touch(TouchPhase::Ended, (x, y + dy), 1.065);
    rig.no_drag();
    rig.released();
    let lifted = rig.offset();
    assert!((lifted - before) * -dy.signum() > 70.0, "tree follows finger in opposite scroll direction");
    for _ in 0..24 { rig.frame(); }
    rig.no_drag();
    assert!((rig.offset() - lifted) * -dy.signum() > 5.0, "quick tree swipe must fling");
}

#[test] fn tree_swipe_up_scrolls_without_drag_or_activation() { tree_swipe(-100.0); }
#[test] fn tree_swipe_down_scrolls_without_drag_or_activation() { tree_swipe(100.0); }

#[test]
fn tree_file_row_swipe_does_not_open_file() {
    let mut rig = TreeRig::new();
    let start = rig.row_point("a-source/source.rs");
    rig.touch(TouchPhase::Started, start, 1.0);
    rig.no_drag();
    rig.touch(TouchPhase::Moved, (start.0, start.1 - 70.0), 1.03);
    rig.no_drag();
    rig.touch(TouchPhase::Ended, (start.0, start.1 - 70.0), 1.04);
    rig.no_drag();
    rig.released();
    assert!(rig.offset() > 60.0);
}

#[test]
fn tree_hold_then_drag_moves_only_on_valid_folder_drop() {
    let mut rig = TreeRig::new();
    let target = rig.row_point("b-target");
    rig.start_held_drag("a-source/source.rs", target);
    assert!(rig.rt.state.tree_drag_row(false, "b-target").1.is_some(), "valid folder must be highlighted");
    assert!(rig.rt.state.tree_drag_row(false, "c-other").1.is_none());
    rig.touch(TouchPhase::Ended, target, 1.72);
    rig.assert_completed_move("b-target", "source.rs", false);
}

#[test]
fn tree_hold_then_drag_moves_a_folder_to_another_folder() {
    let mut rig = TreeRig::new();
    let target = rig.row_point("b-target");
    rig.start_held_drag("a-source/nested", target);
    assert!(rig.rt.state.tree_drag_row(false, "b-target").1.is_some());
    rig.touch(TouchPhase::Ended, target, 1.72);
    rig.assert_completed_move("b-target", "nested", true);
}

#[test]
fn tree_held_drag_rejects_own_row_sibling_and_outside_panel() {
    for target_path in [Some("a-source/source.rs"), Some("a-source/sibling.rs"), None] {
        let mut rig = TreeRig::new();
        let target = target_path.map_or((400.0, 200.0), |path| rig.row_point(path));
        // A self-drop still needs travel to become a live drag before returning home.
        let away = rig.row_point("b-target");
        rig.start_held_drag("a-source/source.rs", away);
        rig.touch(TouchPhase::Moved, target, 1.71);
        assert!(rig.rt.state.tree_drag_row(false, "b-target").1.is_none());
        assert!(!rig.rt.state.tree_drag_root(false));
        rig.touch(TouchPhase::Ended, target, 1.73);
        rig.released();
        rig.no_drag();
    }
}

#[test]
fn tree_held_folder_cannot_drop_into_its_descendant() {
    let mut rig = TreeRig::new();
    let target = rig.row_point("a-source/nested");
    rig.start_held_drag("a-source", target);
    assert!(rig.rt.state.tree_drag_row(false, "a-source/nested").1.is_none());
    rig.touch(TouchPhase::Ended, target, 1.72);
    rig.released();
    rig.no_drag();
}

#[test]
fn tree_mouse_drag_still_moves_to_folder() {
    let mut rig = TreeRig::new();
    let (x, y) = rig.row_point("a-source/source.rs");
    let (tx, ty) = rig.row_point("b-target");
    rig.rt.pointer_move(x, y);
    rig.rt.pointer_down(x, y);
    rig.frame();
    assert!(rig.rt.state.tree_dnd.is_some(), "mouse retains immediate drag Down");
    rig.rt.pointer_move(tx, ty);
    rig.frame();
    assert!(rig.rt.state.tree_drag_row(false, "a-source/source.rs").0);
    assert!(rig.rt.state.tree_drag_row(false, "b-target").1.is_some());
    rig.unchanged();
    rig.rt.pointer_up(tx, ty);
    rig.frame();
    rig.assert_completed_move("b-target", "source.rs", false);
}

#[test]
fn tree_tap_file_activates_existing_row() {
    let mut rig = TreeRig::new();
    let point = rig.row_point("a-source/source.rs");
    rig.touch(TouchPhase::Started, point, 1.0);
    rig.no_drag();
    rig.touch(TouchPhase::Ended, point, 1.05);
    rig.released();
    let app = &rig.rt.state;
    assert_eq!(app.tree.rows()[app.tree.selected].path, "a-source/source.rs");
    assert!(app.buffers.contains_key("a-source/source.rs"));
    assert_eq!(app.active_buffer().unwrap().path, "a-source/source.rs");
    assert_eq!(app.tree.open, rig.open);
    assert!(app.context.is_none() && app.prompt.is_none());
    assert!(rig.offset().abs() < 1.0);
}

#[test]
fn tree_tap_folder_toggles_existing_row() {
    let mut rig = TreeRig::new();
    let point = rig.row_point("b-target");
    rig.touch(TouchPhase::Started, point, 1.0);
    rig.no_drag();
    rig.touch(TouchPhase::Ended, point, 1.05);
    rig.released();
    assert!(rig.rt.state.tree.open.contains("b-target"));
    assert_eq!(rig.rt.state.tree.rows()[rig.rt.state.tree.selected].path, "b-target");
    assert!(rig.rt.state.buffers.is_empty());
    assert_eq!(rig.rt.state.tabs.len(), rig.tabs);
}

#[test]
fn tree_cancel_pending_or_stationary_hold_does_nothing() {
    for held in [false, true] {
        let mut rig = TreeRig::new();
        let point = rig.row_point("a-source/source.rs");
        rig.touch(TouchPhase::Started, point, 1.0);
        rig.no_drag();
        if held {
            rig.wait_for_hold();
            rig.touch(TouchPhase::Moved, point, 1.55);
        }
        rig.touch(TouchPhase::Cancelled, point, if held { 1.6 } else { 1.1 });
        rig.released();
        rig.no_drag();
        assert!(rig.offset().abs() < 1.0);
    }
}

#[test]
fn tree_cancel_held_drag_over_valid_folder_does_not_move() {
    let mut rig = TreeRig::new();
    let target = rig.row_point("b-target");
    rig.start_held_drag("a-source/source.rs", target);
    assert!(rig.rt.state.tree_drag_row(false, "b-target").1.is_some());
    rig.touch(TouchPhase::Cancelled, target, 1.72);
    rig.released();
    rig.no_drag();
}

#[test]
fn tree_cancel_swipe_stops_without_activation_or_drag() {
    let mut rig = TreeRig::new();
    rig.position(400.0);
    rig.touch(TouchPhase::Started, (100.0, 300.0), 1.0);
    rig.no_drag();
    rig.touch(TouchPhase::Moved, (100.0, 220.0), 1.03);
    rig.no_drag();
    rig.touch(TouchPhase::Cancelled, (100.0, 220.0), 1.04);
    rig.released();
    rig.no_drag();
    let stopped = rig.offset();
    assert!(stopped > 470.0, "cancellation must retain the completed pan, not reset it");
    for _ in 0..24 { rig.frame(); }
    rig.no_drag();
    assert!((rig.offset() - stopped).abs() < 1.0, "cancelled swipe must not fling");
}

#[test]
fn tree_returning_finger_catches_fling_without_initializing_drag() {
    let mut rig = TreeRig::new();
    rig.position(400.0);
    rig.touch(TouchPhase::Started, (100.0, 300.0), 1.0);
    rig.touch(TouchPhase::Moved, (100.0, 200.0), 1.05);
    rig.touch(TouchPhase::Ended, (100.0, 200.0), 1.055);
    rig.released();
    rig.no_drag();
    let lifted = rig.offset();
    for _ in 0..4 { rig.frame(); }
    assert!(rig.offset() > lifted + 5.0, "fixture must actually be flinging before catch");
    rig.touch(TouchPhase::Started, (100.0, 300.0), 1.1);
    rig.no_drag();
    let caught = rig.offset();
    for _ in 0..8 { rig.frame(); }
    rig.no_drag();
    assert!((rig.offset() - caught).abs() < 1.0, "new finger must stop old momentum immediately");
    // A fresh reverse gesture still pans; catching momentum must not inherit a drag.
    rig.touch(TouchPhase::Moved, (100.0, 350.0), 1.2);
    rig.no_drag();
    assert!(rig.offset() < caught - 40.0);
    rig.touch(TouchPhase::Cancelled, (100.0, 350.0), 1.21);
    rig.released();
    rig.no_drag();
}