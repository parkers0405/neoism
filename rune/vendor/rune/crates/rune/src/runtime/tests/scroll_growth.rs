use super::*;


#[test]
fn growth_rows_grow_and_sticky_scroll_lands_at_bottom() {
    use crate::el::{GrowthSpec, ScrollSpec};
    fn view(n: &usize, _cx: &mut Cx<usize>) -> El {
        let mut stack = col().key("stack").growth(GrowthSpec::default());
        for i in 0..*n {
            stack = stack.child(el().key(format!("r{i}")).h(100.0).w_full());
        }
        col().key("root").child(col().key("scroll").h(300.0).w_full().scroll(ScrollSpec { stick_bottom: true }).child(stack))
    }
    let mut rt = Runtime::new(10usize, view, (200.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // First frame: at the bottom, nothing animating (guarantee 8).
    assert!(!rt.animating);
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r9")).unwrap();
    assert!((last.rect[1] + last.rect[3] - 300.0).abs() < 0.05, "{:?}", last.rect);
    // A new row grows from 0 and the end stays pinned.
    rt.state = 11;
    rt.dirty = true;
    rt.frame(1.0 / 60.0);
    assert!(rt.animating);
    let r10 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap();
    assert!(r10.rect[3] < 100.0 && r10.rect[3] > 0.0);
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.animating);
    let r10 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap();
    assert!((r10.rect[1] + r10.rect[3] - 300.0).abs() < 0.05, "{:?}", r10.rect);
}

#[test]
fn animate_layout_springs_a_growth_row_s_contents() {
    use crate::el::GrowthSpec;
    fn view(s: &(f32, bool), _cx: &mut Cx<(f32, bool)>) -> El {
        let mut trigger = crate::row().key("trigger").h(28.0)
            .child(el().key("label").size(s.0, 16.0).width_leads())
            .child(el().key("chevron").size(14.0, 14.0));
        if s.1 { trigger = trigger.animate_layout(); }
        col().key("stack").w_full().growth(GrowthSpec::default())
            .child(col().key("r0").w_full().child(trigger))
    }
    let chevron_x = |rt: &Runtime<(f32, bool)>| rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some("chevron")).unwrap().rect[0];
    let label_w = |rt: &Runtime<(f32, bool)>| rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some("label")).unwrap().rect[2];
    for animate in [false, true] {
        let mut rt = Runtime::new((53.0, animate), view, (300.0, 100.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert_eq!(chevron_x(&rt), 53.0);
        rt.update(|s, _| s.0 = 88.0);
        rt.frame(1.0 / 120.0);
        let (x, w) = (chevron_x(&rt), label_w(&rt));
        if animate {
            assert!(x > 53.0 && x < 60.0, "springs from where it was: {x}");
            assert!(w > x, "width_leads still leads inside a growth row: {w} vs {x}");
            for _ in 0..120 { rt.frame(1.0 / 120.0); }
            assert_eq!(chevron_x(&rt), 88.0);
        } else {
            assert_eq!(x, 88.0, "a growth row's contents snap by default");
        }
    }
}

#[test]
fn animate_layout_in_a_growth_row_leaves_vertical_to_growth() {
    use crate::el::GrowthSpec;
    // (label width, space above the header, a second row)
    fn view(s: &(f32, f32, bool), _cx: &mut Cx<(f32, f32, bool)>) -> El {
        let head = |k: &str, w: f32| crate::row().key(k).h(28.0).animate_layout()
            .child(el().key(format!("{k}-names")).size(w, 16.0))
            .child(el().key(format!("{k}-chevron")).size(14.0, 14.0));
        let mut stack = col().key("stack").w_full().growth(GrowthSpec::default())
            .child(col().key("r0").w_full().child(el().key("above").w_full().h(s.1)).child(head("h0", s.0)));
        if s.2 { stack = stack.child(col().key("r1").w_full().child(head("h1", 40.0))); }
        stack
    }
    let find = |rt: &Runtime<(f32, f32, bool)>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
    let mut rt = Runtime::new((31.5, 0.0, false), view, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = (92.0, 20.0, true));
    rt.frame(1.0 / 120.0);
    let (chevron, names) = (find(&rt, "h0-chevron").rect, find(&rt, "h0-names").rect);
    assert!(chevron[0] > 31.5 && chevron[0] < 40.0, "x springs: {chevron:?}");
    assert!(names[2] > 31.5 && names[2] < 40.0, "width springs: {names:?}");
    assert_eq!(chevron[1], 20.0, "y lands at once; the row's growth carries it");
    assert_eq!(find(&rt, "h1-names").opacity, 1.0, "a new row's contents ride its growth");
}

#[test]
fn layout_out_of_sight_before_and_after_moves_at_once() {
    fn view(wide: &bool, _cx: &mut Cx<bool>) -> El {
        let rows = (0..40).map(|i| el().key(format!("r{i}")).w(if *wide { 200.0 } else { 100.0 }).h(40.0).fill(Color::WHITE));
        col().w_full().h_full()
            .child(col().key("scroll").w_full().h(100.0)
                .scroll(ScrollSpec { stick_bottom: false }).child(col().children(rows)))
            .child(el().key("crosser").absolute(if *wide { -80.0 } else { 380.0 }, 20.0).size(20.0, 20.0).fill(Color::WHITE))
            .child(col().key("drawer").w_full().h(if *wide { 100.0 } else { 0.0 }).clip_soft(0.0)
                .child(el().key("inside-closing").absolute(10.0, if *wide { 180.0 } else { 20.0 }).size(40.0, 20.0).fill(Color::WHITE)))
    }
    let mut rt = Runtime::new(true, view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.update(|s, _| *s = false);
    rt.frame(1.0 / 120.0);
    let rect = |rt: &Runtime<bool>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
    assert!(rect(&rt, "r0")[2] > 190.0, "a row in sight springs: {}", rect(&rt, "r0")[2]);
    assert_eq!(rect(&rt, "r30")[2], 100.0, "a row out of sight before and after lands at once");
    assert!(rect(&rt, "crosser")[0] < 0.0, "an element sweeping across the window animates: {:?}", rect(&rt, "crosser"));
    assert!(rect(&rt, "inside-closing")[1] > 150.0, "an element entering a still-closing clip animates: {:?}", rect(&rt, "inside-closing"));
}

#[test]
fn animate_layout_lets_a_growth_row_s_new_contents_fade_in() {
    use crate::el::GrowthSpec;
    fn view(s: &(bool, bool), _cx: &mut Cx<(bool, bool)>) -> El {
        let trigger = crate::row().key("trigger").h(28.0).animate_layout()
            .child(el().key(if s.0 { "after" } else { "before" }).size(if s.0 { 88.0 } else { 53.0 }, 16.0));
        let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
        if s.1 { stack = stack.virtualize(40.0); }
        col().w_full().h_full().child(col().key("scroll").w_full().h(100.0).scroll(ScrollSpec { stick_bottom: true })
            .child(stack.child(col().key("r0").w_full().child(col().key("part").child(trigger)))))
    }
    let opacity = |rt: &Runtime<(bool, bool)>, key: &str| rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some(key)).map(|e| e.opacity);
    for virtualized in [false, true] {
        let mut rt = Runtime::new((false, virtualized), view, (300.0, 100.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        rt.update(|s, _| s.0 = true);
        rt.frame(1.0 / 120.0);
        let (before, after) = (opacity(&rt, "before").unwrap_or(0.0), opacity(&rt, "after").unwrap());
        assert!(before > 0.5 && after < 0.5, "virtualized {virtualized}: crossfades, {before} out and {after} in");
    }
}

#[test]
fn explicit_transitions_play_inside_a_growth_row() {
    use crate::el::GrowthSpec;
    fn view(s: &(bool, bool), _cx: &mut Cx<(bool, bool)>) -> El {
        let mut trigger = crate::row().key("trigger").h(28.0).child(el().key("label").size(53.0, 16.0));
        if s.0 {
            trigger = trigger.child(el().key("badge").size(16.0, 16.0)
                .transition(Transition::offset(0.0, 8.0).combined(Transition::opacity())));
        }
        if s.1 { trigger = trigger.animate_layout(); }
        col().key("stack").w_full().growth(GrowthSpec::default())
            .child(col().key("r0").w_full().child(trigger))
    }
    let badge = |rt: &Runtime<(bool, bool)>| rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some("badge")).map(|e| (e.opacity, e.exiting));
    for animate in [false, true] {
        let mut rt = Runtime::new((false, animate), view, (300.0, 100.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        rt.update(|s, _| s.0 = true);
        rt.frame(1.0 / 120.0);
        let (op, _) = badge(&rt).unwrap();
        assert!(op < 0.5, "animate_layout {animate}: it enters on its transition: {op}");
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        rt.update(|s, _| s.0 = false);
        rt.frame(1.0 / 120.0);
        let (op, exiting) = badge(&rt).expect("it plays its exit");
        assert!(exiting && op > 0.5, "animate_layout {animate}: it exits on its transition: {op}");
    }
}

/// A long text in a scroll view walks only the lines its clip shows: the glyphs of lines
/// far below aren't rasterized until scrolled to, while a tall span whose line box sits
/// more than two base ems below the clip still paints the ascenders that reach inside.
#[test]
fn a_long_scrolled_text_paints_only_its_visible_lines() {
    use crate::{text, text_input, Span, TextEdit, TextStyle};
    struct Note { edit: TextEdit }
    fn view(s: &Note, _: &mut Cx<Note>) -> El {
        col().w_full().h_full().child(col().key("viewport").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(text_input(&s.edit, TextStyle::new(14.0, 400.0).lh(20.0)).key("note").w_full().on_edit(|s: &mut Note| &mut s.edit)))
    }
    let mut content = "plain lowercase words only\n".repeat(500);
    content += "QUIZ JOKE VEX";
    let mut rt = Runtime::new(Note { edit: TextEdit::new(content) }, view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let top = rt.text.atlas.glyph_entries();
    let glyphs = rt.stats.glyphs;
    assert!(glyphs > 0 && glyphs < 400, "about ten visible lines painted: {glyphs}");
    rt.wheel(150.0, 100.0, -1e5);
    rt.frame(1.0 / 60.0);
    assert!(rt.text.atlas.glyph_entries() > top, "the capitals on the last line rasterize only once shown");

    // Line 15 starts at y = 75 pt, 25 pt (> 2 * 10 pt) below the 50 pt clip. The 120 pt span keeps
    // the paragraph's 0.5 leading ratio, so its box is 60 pt tall and the glyph overhangs it:
    // the ascender reaches up into the clip and must still paint.
    fn span_view(_: &(), _: &mut Cx<()>) -> El {
        let s = "a\n".repeat(15) + "T";
        col().w_full().h_full().child(col().w_full().h(50.0).scroll(ScrollSpec { stick_bottom: false })
            .child(text(&s).font(TextStyle::new(10.0, 400.0).lh(5.0)).spans(vec![Span::new(30, 31).size(120.0)])))
    }
    let mut rt = Runtime::new((), span_view, (200.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.stats.glyphs, 11, "10 visible 'a' lines plus the overhanging 'T' glyph");
    assert_eq!(rt.text.atlas.glyph_entries(), 2, "'a' and overhanging 'T' both rasterized");
}

#[test]
fn a_window_resize_lands_rewrapped_rows_under_a_pinned_tail() {
    use crate::el::text;
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("viewport").w_full().h_full().scroll(ScrollSpec { stick_bottom: true })
            .child(col().key("stack").w_full().growth(GrowthSpec::default())
                .child(col().key("first").w_full().child(text("Wrap again as the window narrows. ".repeat(40)).wrap().w_full()))
                .child(col().key("last").w_full().child(text("The tail stays pinned. ".repeat(20)).wrap().w_full())))
    }
    let mut rt = Runtime::new((), view, (600.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let gap = |rt: &Runtime<()>| {
        let el = rt.elements();
        let end = |k: &str| el.iter().find(|e| e.key.as_deref() == Some(k)).map(|e| e.rect[1] + e.rect[3]).unwrap();
        end("viewport") - end("last")
    };
    let rest = gap(&rt);
    let first = rt.nodes.iter().find(|n| n.key.as_deref() == Some("first")).unwrap().id;
    let before = rt.grows[&first].h;
    // One step a frame, as a person drags the corner.
    for w in (420..600).rev().step_by(6) {
        rt.set_viewport((w as f32, 300.0), 2.0);
        rt.frame(1.0 / 120.0);
        assert!((gap(&rt) - rest).abs() < 0.5, "at width {w} the tail moved off the bottom: {} vs {rest}", gap(&rt));
        let el = rt.elements();
        let rect = |k: &str| el.iter().find(|e| e.key.as_deref() == Some(k)).unwrap().rect;
        let (a, b) = (rect("first"), rect("last"));
        assert!(a[1] + a[3] <= b[1] + 0.5, "at width {w} the first row runs into the last: {a:?} {b:?}");
    }
    assert!(rt.grows[&first].h > before + 1.0, "the first row rewrapped taller");
    let measured = rt.nodes.iter().find(|n| n.id == first).unwrap().local.h;
    assert_eq!(rt.grows[&first].h, measured, "and its slot landed with it");
}

#[test]
fn rows_all_leaving_a_padded_growth_stack_hold_their_place() {
    use crate::el::GrowthSpec;
    fn view(n: &usize, _cx: &mut Cx<usize>) -> El {
        let stack = col().key("stack").w_full().padding(8.0).growth(GrowthSpec::default())
            .children((0..*n).map(|i| el().key(format!("r{i}")).h(20.0).w_full().fill(Color::WHITE)));
        col().key("root").size(200.0, 200.0).child(stack)
    }
    let tops = |rt: &Runtime<usize>| -> Vec<f32> {
        (0..3).filter_map(|i| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&*format!("r{i}"))).map(|e| e.rect[1])).collect()
    };
    for left in [0usize, 1] {
        let mut rt = Runtime::new(3usize, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(tops(&rt), [8.0, 28.0, 48.0]);
        rt.update(|n, _| *n = left);
        rt.frame(1.0 / 120.0);
        let now = tops(&rt);
        assert_eq!(now[..2], [8.0, 28.0], "{left} left: the leaving rows hold on frame 1: {now:?}");
    }
}

/// `n` rows of 100 pt in a growth stack, in a 300 pt `stick_bottom` viewport.
fn shrinking(n: &usize, _cx: &mut Cx<usize>) -> El {
    use crate::el::{GrowthSpec, ScrollSpec};
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
    for i in 0..*n {
        stack = stack.child(el().key(format!("r{i}")).h(100.0).w_full());
    }
    col().key("root").child(col().key("scroll").h(300.0).w_full().scroll(ScrollSpec { stick_bottom: true }).child(stack))
}

/// Twenty `shrinking` rows, read 200 pt above their end.
fn reading(hz: f64) -> Runtime<usize> {
    let mut rt = Runtime::new(20usize, shrinking, (200.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / hz);
    rt.wheel(100.0, 100.0, 200.0);
    for _ in 0..30 { rt.frame(1.0 / hz); }
    assert_eq!(rt.scrolls.values().next().unwrap().offset, 1500.0, "reading 200 pt above the end");
    rt
}

/// Rows removed under a reader who scrolled up end the content above their offset: the
/// viewport glides to the new end from rest and lands at rest, never past the presented end,
/// instead of picking up the collapse's speed in a frame.
#[test]
fn content_shrinking_under_a_reader_eases_to_its_new_end() {
    let scroll = |rt: &Runtime<usize>| rt.scrolls.values().next().map(|st| (st.offset, st.max)).unwrap();
    // (rows left, frame rate, largest first step and frame-to-frame change, as parts of the peak speed)
    for (left, hz, onset, step) in [(15usize, 120.0, 0.3, 0.4), (5, 120.0, 0.3, 0.4), (15, 60.0, 0.4, 0.55)] {
        let mut rt = reading(hz);
        let (mut last, _) = scroll(&rt);
        rt.update(|n, _| *n = left);
        let mut v = Vec::new();
        for _ in 0..hz as usize {
            rt.frame(1.0 / hz);
            let (offset, max) = scroll(&rt);
            assert!(offset <= max + 1e-6, "{left} left at {hz} Hz: the end never rises into view");
            v.push((offset - last) * hz);
            last = offset;
        }
        let end = left as f64 * 100.0 - 300.0;
        assert!((last - end).abs() < 0.01, "{left} left at {hz} Hz: lands on the new end: {last}");
        let moving: Vec<f64> = v.iter().copied().skip_while(|s| s.abs() < 1e-9).collect();
        let peak = moving.iter().fold(0.0f64, |a, s| a.max(s.abs()));
        let speeds: Vec<i64> = moving.iter().map(|s| s.round() as i64).collect();
        assert!(moving[0].abs() <= onset * peak, "{left} left at {hz} Hz: starts from rest: {speeds:?}");
        assert!(moving.windows(2).all(|w| (w[1] - w[0]).abs() <= step * peak), "{left} left at {hz} Hz: no speed jumps: {speeds:?}");
        let tail = moving.iter().rev().find(|s| s.abs() > 1e-9).unwrap();
        assert!(tail.abs() <= 0.05 * peak, "{left} left at {hz} Hz: lands at rest: {speeds:?}");
    }
}

/// A wheel mid-glide hands the viewport back: no glide starts again until the rows settle.
#[test]
fn a_hand_scroll_mid_shrink_keeps_the_viewport() {
    let mut rt = reading(120.0);
    rt.update(|n, _| *n = 15);
    for _ in 0..6 { rt.frame(1.0 / 120.0); }
    assert!(rt.scrolls.values().next().unwrap().glide().is_some(), "the collapse glides");
    rt.wheel(100.0, 100.0, 10.0);
    for f in 0..120 {
        rt.frame(1.0 / 120.0);
        assert!(rt.scrolls.values().next().unwrap().glide().is_none(), "frame {f}: no glide takes the viewport back from the hand");
    }
}

/// A second removal mid-glide retargets it to the newer end without braking, and rows coming
/// back past the offset end it where it is.
#[test]
fn a_shrink_glide_follows_the_end_as_it_moves() {
    let mut rt = reading(120.0);
    rt.update(|n, _| *n = 15);
    for _ in 0..8 { rt.frame(1.0 / 120.0); }
    let v = rt.scrolls.values().next().unwrap().glide().unwrap();
    assert!(v.abs() > 1.0, "gliding at {v}");
    rt.update(|n, _| *n = 10);
    rt.frame(1.0 / 120.0);
    let st = rt.scrolls.values().next().unwrap();
    assert_eq!(st.glide_to(), Some(700.0), "the glide aims at the newer end");
    assert!(st.glide().unwrap().abs() >= v.abs() * 0.5, "and keeps its speed: {:?} from {v}", st.glide());
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    assert!((rt.scrolls.values().next().unwrap().offset - 700.0).abs() < 0.01, "lands on the newer end");

    let mut rt = reading(120.0);
    rt.update(|n, _| *n = 15);
    for _ in 0..8 { rt.frame(1.0 / 120.0); }
    rt.update(|n, _| *n = 20);
    rt.frame(1.0 / 120.0);
    let st = rt.scrolls.values().next().unwrap();
    assert!(st.glide().is_none(), "rows coming back past the offset end the glide");
    let at = st.offset;
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.scrolls.values().next().unwrap().offset, at, "and the viewport stays where it was");
}

/// A row kept on screen owns its viewport through a collapse: no shrink glide pulls it away.
#[test]
fn a_kept_row_holds_its_viewport_through_a_shrink() {
    let mut rt = reading(120.0);
    let top = |rt: &Runtime<usize>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r15")).unwrap().rect[1];
    let was = top(&rt);
    rt.update(|n, tx| { *n = 17; tx.keep_on_screen("r15"); });
    for f in 0..20 {
        rt.frame(1.0 / 120.0);
        assert!(rt.scrolls.values().next().unwrap().glide().is_none(), "frame {f}: no shrink glide on a kept viewport");
        assert!((top(&rt) - was).abs() < 0.5, "frame {f}: the kept row holds its place");
    }
}

/// Content shrinking past a glide's end lands it there instead of overshooting and springing back.
#[test]
fn a_glide_whose_end_shrinks_past_it_lands_there() {
    let mut rt = reading(120.0);
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 1000.0));
    for _ in 0..6 { rt.frame(1.0 / 120.0); }
    assert!(rt.scrolls.values().next().unwrap().glide().is_some_and(|v| v.abs() > 100.0), "gliding up");
    rt.update(|n, tx| { *n = 12; tx.instant(); });
    let mut low = f64::MAX;
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        low = low.min(rt.scrolls.values().next().unwrap().offset);
    }
    let st = rt.scrolls.values().next().unwrap();
    assert_eq!((st.offset, st.max), (900.0, 900.0));
    assert!(low >= 900.0 - 1e-6, "never dips below the end: {low}");
}

/// The collapse's spring tail can run on for a while under a fraction of a point. A reader
/// the glide landed on the new end sticks to it once the rows are within half a point of it,
/// so rows arriving in that tail grow in under a pinned end rather than below the viewport.
#[test]
fn a_reader_landed_on_a_shrunk_end_follows_rows_arriving_in_its_tail() {
    let mut rt = reading(120.0);
    rt.update(|n, _| *n = 15);
    for f in 0.. {
        rt.frame(1.0 / 120.0);
        let st = rt.scrolls.values().next().unwrap();
        if st.glide().is_none() && st.max - 1200.0 < 0.5 { break; }
        assert!(f < 240, "the glide lands and the rows near the end");
    }
    assert!(rt.stacks.values().next().unwrap().moving, "the rows are still settling");
    assert!(rt.scrolls.values().next().unwrap().pinned(), "the reader sticks to the end");
    rt.update(|n, _| *n = 20);
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    let st = rt.scrolls.values().next().unwrap();
    assert_eq!((st.offset, st.max), (1700.0, 1700.0), "and follows the rows that come back");
}

/// While a collapse glides a reader to the new end, `on_far_from_bottom` measures from the end
/// they're headed to: the presented end, still collapsing, doesn't report them far for a moment.
#[test]
fn a_shrink_glide_never_reports_far_from_the_end_it_heads_for() {
    use crate::el::{GrowthSpec, ScrollSpec};
    fn view(s: &(usize, Vec<bool>), _: &mut Cx<(usize, Vec<bool>)>) -> El {
        let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
        for i in 0..s.0 {
            stack = stack.child(el().key(format!("r{i}")).h(100.0).w_full());
        }
        let scroll = col().key("scroll").h(300.0).w_full().scroll(ScrollSpec { stick_bottom: true })
            .on_far_from_bottom(300.0, |s: &mut (usize, Vec<bool>), far| s.1.push(far));
        col().key("root").child(scroll.child(stack))
    }
    let mut rt = Runtime::new((20usize, Vec::new()), view, (200.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.wheel(100.0, 100.0, 200.0);
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.state.1, [false], "200 above the end is near");
    rt.update(|s, _| s.0 = 15);
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.state.1, [false], "gliding to the new end stays near");
}

/// The glide lands on the new end before the rows' last fraction of a point has collapsed.
/// The reader sticks there without the presented end lifting it: the offset never steps up.
#[test]
fn a_reader_stuck_in_the_collapse_tail_never_steps_up() {
    let top = |rt: &Runtime<usize>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap().rect[1];
    let mut rt = reading(120.0);
    rt.update(|n, _| *n = 15);
    let (mut offset, mut y) = (rt.scrolls.values().next().unwrap().offset, top(&rt));
    let mut stuck_in_tail = false;
    for f in 0..240 {
        rt.frame(1.0 / 120.0);
        let st = rt.scrolls.values().next().unwrap();
        stuck_in_tail |= st.pinned() && st.max > 1200.0;
        assert!(st.offset <= offset, "frame {f}: the offset steps up from {offset} to {}", st.offset);
        let now = top(&rt);
        assert!(now >= y - 1e-3, "frame {f}: the rows step up from {y} to {now}");
        (offset, y) = (st.offset, now);
    }
    assert!(stuck_in_tail, "the reader sticks while the rows still collapse");
    assert_eq!(offset, 1200.0);
}

/// `n` rows of 100 pt and a last row `tall` pt high in a 300 pt `stick_bottom` viewport, which
/// records `on_far_from_bottom`.
fn far_reader(s: &(usize, f32, Vec<bool>), _: &mut Cx<(usize, f32, Vec<bool>)>) -> El {
    use crate::el::{GrowthSpec, ScrollSpec};
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
    for i in 0..s.0 {
        stack = stack.child(el().key(format!("r{i}")).h(100.0).w_full());
    }
    if s.1 > 0.0 { stack = stack.child(el().key("tall").h(s.1).w_full()); }
    let scroll = col().key("scroll").h(300.0).w_full().scroll(ScrollSpec { stick_bottom: true })
        .on_far_from_bottom(300.0, |s: &mut (usize, f32, Vec<bool>), far| s.2.push(far));
    col().key("root").child(scroll.child(stack))
}

/// `far_reader` scrolled `up` from its end.
fn far_reading(rows: usize, tall: f32, up: f32) -> Runtime<(usize, f32, Vec<bool>)> {
    let mut rt = Runtime::new((rows, tall, Vec::new()), far_reader, (200.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.wheel(100.0, 100.0, up);
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.state.2.last(), Some(&true), "{up} above the end is far");
    rt.state.2.clear();
    rt
}

/// A short glide lands on the new end long before a tall row has collapsed: the reader is near
/// the layout's end, and isn't reported far again while the presented end catches up.
#[test]
fn a_landed_glide_over_a_collapsing_tall_row_stays_near() {
    let mut rt = far_reading(10, 3000.0, 2800.0);
    rt.update(|s, _| s.1 = 0.0);
    for _ in 0..360 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.scrolls.values().next().unwrap().offset, 700.0, "the glide lands on the new end");
    assert_eq!(rt.state.2, [false], "a tall row collapsing under a landed glide stays near");
}

/// Rows coming back partway end a shrink glide short of the presented end: the reader is near
/// the regrown layout's end, and isn't reported far while the other rows collapse to it.
#[test]
fn rows_regrowing_under_a_shrink_glide_stay_near() {
    let mut rt = far_reading(20, 0.0, 600.0);
    rt.update(|s, _| s.0 = 5);
    for f in 0.. {
        rt.frame(1.0 / 120.0);
        let st = rt.scrolls.values().next().unwrap();
        assert!(st.glide().is_some() && f < 120, "the glide passes 850");
        if st.offset < 850.0 { break; }
    }
    rt.update(|s, _| s.0 = 12);
    rt.frame(1.0 / 120.0);
    assert!(rt.scrolls.values().next().unwrap().glide().is_none(), "rows coming back past the offset end the glide");
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.state.2, [false], "the regrown end is near, while the rows still collapse to it");
}
