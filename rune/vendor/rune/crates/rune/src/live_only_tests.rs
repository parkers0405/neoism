//! `Tx::live_only` on the physical key path native windows use, and debug builds' check that
//! a live-only handler didn't change what the view reads.
use super::*;
use crate::{Cx, El, Env, Key, Live, Mods, PhysicalInput, PhysicalKeyCode, el, text};

struct S {
    /// Read by the view.
    count: u32,
    /// Handlers bump `count` (what the view reads) as well as `pan`.
    misuse: bool,
    pan: Live<[f32; 2]>,
    physical: usize,
}

impl S {
    fn nudge(&mut self, tx: &mut Tx) {
        let [x, y] = self.pan.get();
        self.pan.set([x + 10.0, y]);
        if self.misuse { self.count += 1; }
        tx.live_only();
    }
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    el().size(200.0, 200.0)
        .child(el().key("camera").size(50.0, 50.0).live_offset(&s.pan).on_press(|s: &mut S| s.count += 1))
        .child(text(format!("{}", s.count)).key("count"))
        .child(el().key("recorder").size(40.0, 40.0).focusable(true)
            .on_physical_input(|s: &mut S, e, tx| {
                if !matches!(e, PhysicalInput::Key { pressed: false, .. }) { return false; }
                s.physical += 1;
                s.nudge(tx);
                true
            }))
        .on_key_up(|s: &mut S, _, tx| { s.nudge(tx); true })
}

fn app(misuse: bool) -> Runtime<S> {
    let state = S { count: 0, misuse, pan: Live::new([0.0, 0.0]), physical: 0 };
    let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, e, tx| { if e.key != Key::Right { return false; } s.nudge(tx); true });
    rt.frame(0.0);
    rt
}

fn key(code: PhysicalKeyCode, key: Key, pressed: bool) -> (PhysicalInput, Option<KeyEvent>) {
    (PhysicalInput::Key { key: code, mods: Mods::default(), pressed, repeat: false }, Some(KeyEvent { key, mods: Mods::default() }))
}

fn camera_x(rt: &Runtime<S>) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("camera")).unwrap().rect[0]
}

#[test]
fn physical_keys_and_their_releases_can_be_live_only() {
    let mut rt = app(false);
    rt.check_live_only(false);
    let before = rt.stats.commits;
    let (ev, fallback) = key(PhysicalKeyCode::ArrowRight, Key::Right, true);
    rt.physical_with_fallback(ev, fallback);
    rt.frame(1.0 / 120.0);
    let (ev, fallback) = key(PhysicalKeyCode::ArrowRight, Key::Right, false);
    rt.physical_with_fallback(ev, fallback);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((rt.stats.commits - before, rt.live_only_skips), (0, 2), "on_key, then on_key_up");
    assert_eq!(camera_x(&rt), 20.0, "the frames drew both");

    // A key the app declines goes on to Rune's own handling and commits as before.
    let (ev, fallback) = key(PhysicalKeyCode::Tab, Key::Tab, true);
    rt.physical_with_fallback(ev, fallback);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.stats.commits - before, 1);
}

#[test]
fn a_physical_release_heard_at_rest_can_be_live_only() {
    let mut rt = app(false);
    rt.check_live_only(false);
    let (x, y) = rt.find("recorder").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    let before = rt.stats.commits;
    // No logical key: the focused handler hears it at once, outside a frame.
    let event = PhysicalInput::Key { key: PhysicalKeyCode::ShiftLeft, mods: Mods::default(), pressed: false, repeat: false };
    assert!(rt.physical_with_fallback(event, None), "it still asks for a frame to draw");
    assert!(!rt.dirty);
    rt.frame(1.0 / 120.0);
    assert_eq!((rt.state.physical, rt.stats.commits - before), (1, 0));
    assert_eq!(camera_x(&rt), 10.0);
}

#[test]
fn debug_builds_warn_when_a_live_only_handler_changed_what_the_view_reads() {
    let mut rt = app(true);
    // A quarter second after the last commit, a live-only event is checked: it commits, and
    // the view built a different count.
    rt.frame(0.5);
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.live_only_warnings.len(), 1, "{:?}", rt.live_only_warnings);
    assert!(rt.live_only_warnings[0].contains("on_key"), "{}", rt.live_only_warnings[0]);

    // Used as meant, the check commits and stays quiet; events between checks skip the view.
    let mut rt = app(false);
    rt.frame(0.5);
    let before = rt.stats.commits;
    for _ in 0..10 {
        rt.key_up(KeyEvent { key: Key::Right, mods: Mods::default() });
        rt.frame(1.0 / 120.0);
    }
    assert!(rt.live_only_warnings.is_empty(), "{:?}", rt.live_only_warnings);
    assert_eq!((rt.stats.commits - before, rt.live_checks, rt.live_only_skips), (1, 1, 9));
}

#[test]
fn the_check_ignores_a_change_another_event_made_in_the_same_frame() {
    let mut rt = app(false);
    rt.frame(0.5);
    // The release is checked, and then a press in the same frame changes the count.
    rt.key_up(KeyEvent { key: Key::Right, mods: Mods::default() });
    let (x, y) = rt.find("camera").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!((rt.state.count, rt.live_checks), (1, 1));
    assert!(rt.live_only_warnings.is_empty(), "{:?}", rt.live_only_warnings);
}

#[test]
fn declined_element_key_does_not_hide_an_ancestor_change() {
    fn view(s: &u32, _: &mut Cx<u32>) -> El {
        el().on_key(|s: &mut u32, _, _| { *s += 1; true })
            .child(el().key("field").size(40.0, 40.0).focusable(true)
                .on_key(|_: &mut u32, _, tx| { tx.live_only(); false }))
            .child(text(s.to_string()).key("count"))
    }
    let mut rt = Runtime::new(0, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.check_live_only(false);
    rt.frame(0.0);
    let (x, y) = rt.find("field").unwrap();
    rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
    let before = rt.stats.commits;
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state, 1);
    assert_eq!(rt.stats.commits, before + 1);
}

#[test]
fn live_only_does_not_cancel_poll_update_or_later_event_commits() {
    for source in 0..3 {
        let mut rt = app(false);
        rt.check_live_only(false);
        if source == 0 { rt.poll = Some(|s, _, _| { s.count += 1; true }); }
        if source == 1 { rt.update(|s, _| s.count += 1); }
        rt.key_up(KeyEvent { key: Key::Right, mods: Mods::default() });
        if source == 2 {
            let (x, y) = rt.find("camera").unwrap();
            rt.pointer_down(x, y); rt.pointer_up(x, y);
        }
        let before = rt.stats.commits;
        rt.frame(0.0);
        assert_eq!(rt.state.count, 1);
        assert!(rt.stats.commits > before);
        assert_eq!(rt.nodes.iter().find(|n| n.key.as_deref() == Some("count")).unwrap().text.as_ref().unwrap().content, "1");
    }
}

#[test]
fn live_values_published_by_poll_draw_in_that_frame_after_a_scroll_callback() {
    fn view(pan: &Live<[f32; 2]>, _: &mut Cx<Live<[f32; 2]>>) -> El {
        let follow = pan.clone();
        el().size(200.0, 200.0)
            .child(el().key("scroll").size(100.0, 100.0)
                .scroll(crate::ScrollSpec { stick_bottom: false })
                .on_scroll_offset(move |at| { follow.set([at.offset[1], 0.0]); })
                .child(el().size(100.0, 1000.0)))
            .child(el().key("marker").absolute(0.0, 0.0).size(10.0, 10.0).live_offset(pan))
    }
    let mut rt = Runtime::new(Live::new([0.0, 0.0]), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.wheel(50.0, 50.0, -20.0);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.get(), [20.0, 0.0]);
    assert!(!rt.live_pending(), "the scroll callback's publication was already drawn");
    let commits = rt.stats.commits;
    // A scroll-triggered animation continues through Live values, without rebuilding its view.
    rt.poll = Some(|pan, _, _| { pan.set([pan.get()[0] + 5.0, 0.0]); false });
    for frame in 1..=4 {
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.get()[0], 20.0 + frame as f32 * 5.0);
        let marker = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("marker")).unwrap();
        assert_eq!(marker.rect[0], 20.0 + frame as f32 * 5.0, "poll's position in frame {frame}");
        assert_eq!(rt.stats.commits, commits, "live animation does not rebuild the view");
    }
}

#[test]
fn debug_check_does_not_blame_live_only_for_layout_feedback_before_a_key() {
    let mut rt = app(false);
    rt.frame(0.5);
    // A layout callback changed view state, then a checked event, then another key
    // forces the queue's mid-frame commit rather than the final commit.
    rt.nodes[0].handlers.on_layout = Some(Rc::new(|s, _, _| s.cast::<S>().unwrap().count += 1));
    rt.queue.push(Action::Layout(rt.nodes[0].path.clone(), [0.0; 4]));
    rt.key_up(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.live_only_warnings.is_empty(), "{:?}", rt.live_only_warnings);
}

#[test]
fn retained_exit_pruning_does_not_change_the_view_print() {
    let mut rt = app(false);
    let before = rt.view_print();
    let id = rt.nodes.last().unwrap().id;
    rt.scene.remove(id);
    let exiting = rt.view_print();
    assert!(before.first_difference(&exiting).is_some());
    rt.scene.set_time(10.0); rt.scene.frame(); rt.prune_exited();
    assert_eq!(exiting.first_difference(&rt.view_print()), None);
}

#[test]
fn live_only_keeps_clipboard_scroll_and_wake_requests() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().child(el().key("scroll").size(100.0, 100.0).scroll(crate::ScrollSpec { stick_bottom: false })
            .child(el().size(100.0, 1000.0)))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.check_live_only(false);
    rt.on_key = Some(|_, _, tx| {
        tx.live_only(); tx.copy("copied"); tx.scroll_to_y("scroll", 300.0);
        tx.wake_after(std::time::Duration::from_secs(1)); true
    });
    rt.frame(0.0);
    let before = rt.stats.commits;
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() }); rt.frame(0.0);
    assert_eq!(rt.stats.commits, before);
    assert_eq!(rt.copy_request.as_ref().unwrap().content().text(), "copied");
    let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("scroll")).unwrap().id;
    assert_eq!(rt.scrolls[&id].offset, 300.0);
    assert_eq!(rt.wake_deadline(), Some(1.0));
}

#[test]
fn local_points_follow_scroll_offsets_in_windows_and_overlays() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().child(el().key("scroll").size(200.0, 200.0).scale(1.5).scroll(crate::ScrollSpec { stick_bottom: false })
            .child(el().size(200.0, 1000.0).child(el().key("target").size(50.0, 50.0))))
    }
    for overlay in [false, true] {
        let mut rt = Runtime::new((), view, (400.0, 400.0), Env::new(true, 2.0));
        rt.native_overlay = overlay;
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_to_y("scroll", 100.0)); rt.frame(0.0);
        let i = rt.nodes.iter().position(|n| n.key.as_deref() == Some("target")).unwrap();
        let r = rt.drawn[i];
        let p = rt.pointer_event(i, r[0] + 15.0, r[1] + 30.0);
        assert_eq!((p.x, p.y), (10.0, 20.0));
        assert_eq!((p.window_x, p.window_y), (r[0] + 15.0, r[1] + 30.0));
    }
}

#[test]
fn live_transform_refreshes_hover_paint_without_a_handler() {
    fn view(s: &Live<[f32; 2]>, _: &mut Cx<Live<[f32; 2]>>) -> El {
        el().child(el().key("moving").size(40.0, 40.0).live_offset(s).hover(Color::WHITE))
    }
    let mut rt = Runtime::new(Live::new([100.0, 0.0]), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0); rt.pointer_move(20.0, 20.0); rt.frame(0.0);
    rt.state.set([0.0, 0.0]);
    assert!(rt.frame(0.0), "changed hover paint must schedule its sample frame");
    let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("moving")).unwrap().id;
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.scene.get(id, P_HOVER), Some(1.0));
}

#[test]
fn a_commit_records_why_for_the_frame_log() {
    for (source, want) in [(0, CommitWhy::POLL), (1, CommitWhy::INPUT)] {
        let mut rt = app(false);
        rt.check_live_only(false);
        rt.frame(0.0);
        rt.commit_why = 0;
        rt.key_up(KeyEvent { key: Key::Right, mods: Mods::default() });
        rt.frame(0.0);
        assert_eq!(rt.commit_why, 0, "a live-only key commits nothing");
        if source == 0 { rt.poll = Some(|s, _, _| { s.count += 1; true }); }
        if source == 1 {
            let (x, y) = rt.find("camera").unwrap();
            rt.pointer_down(x, y); rt.pointer_up(x, y);
        }
        rt.frame(0.0);
        assert_eq!(rt.commit_why & want, want, "source {source}: {:?}", CommitWhy::names(rt.commit_why));
    }
}
