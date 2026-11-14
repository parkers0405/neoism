//! `Runtime::motion_elements` agrees with `Runtime::elements` on what a motion watcher reads,
//! frame by frame, through a wheel scroll and a fade.
use rune::{Cx, El, Env, MotionElement, Runtime, ScrollSpec, col, el, text};

struct S {
    words: bool,
}

fn secret(_: &S, _: &mut Cx<S>) -> El {
    col().child(text("hunter2").key("plain")).child(text("hunter2").key("secret").secure())
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let rows = (0..30).fold(col().w_full(), |c, i| c.child(el().key(format!("row{i}")).w_full().h(40.0).child(text(format!("row {i}")))));
    col().w_full().h_full()
        .child(col().key("scroll").w(300.0).h(400.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
        .child(if s.words { text("a much longer line of words").key("words") } else { el().key("words").w(10.0).h(10.0).fill(rune::Color::BLACK) })
}

fn agree(rt: &Runtime<S>, out: &mut Vec<MotionElement>) {
    rt.motion_elements(out);
    let els = rt.elements();
    assert_eq!(out.len(), els.len());
    for (m, e) in out.iter().zip(&els) {
        assert_eq!(rt.element_path(m.id), Some((e.path.as_str(), e.key.as_deref())));
        assert_eq!((m.rect, m.visible, m.opacity, m.scale), (e.rect, e.visible, e.opacity, e.scale), "{}", e.path);
        assert_eq!(m.parent.map(|p| els[p as usize].path.as_str()), e.parent.map(|p| els[p].path.as_str()), "{}", e.path);
        let flags = [
            (MotionElement::EXITING, e.exiting),
            (MotionElement::SCROLLING, e.scrolling),
            (MotionElement::GROWING, e.in_growing_row || e.revealing),
            (MotionElement::LIVE, e.live_intent != 0),
            (MotionElement::VIRTUAL, e.virtual_extent),
            (MotionElement::TEXT, e.role == "text"),
            (MotionElement::LOOP, e.loop_phase.is_some()),
            (MotionElement::PAINTS, e.fill.a > 0.01 || e.text.as_deref().is_some_and(|t| !t.is_empty())),
        ];
        for (flag, want) in flags {
            assert_eq!(m.is(flag), want, "{} flag {flag}", e.path);
        }
        assert_eq!(m.text_hash != 0, e.text.as_deref().is_some_and(|t| !t.is_empty()), "{}", e.path);
    }
}

#[test]
fn motion_elements_match_elements_through_a_scroll() {
    let mut rt = Runtime::new(S { words: false }, view, (600.0, 900.0), Env::new(true, 2.0));
    let mut out = Vec::new();
    rt.frame(0.0);
    agree(&rt, &mut out);
    rt.wheel(100.0, 200.0, -120.0);
    let mut scrolled = false;
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
        agree(&rt, &mut out);
        scrolled |= out.iter().any(|m| m.is(MotionElement::SCROLLING));
    }
    assert!(scrolled, "the rows were marked scrolling while the wheel moved them");
    let row = |rt: &Runtime<S>, out: &[MotionElement], k: &str| *out.iter().find(|m| rt.element_path(m.id).and_then(|p| p.1) == Some(k)).unwrap();
    let before = row(&rt, &out, "words");
    rt.update(|s, _| s.words = true);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
        agree(&rt, &mut out);
    }
    let after = row(&rt, &out, "words");
    assert_ne!(before.text_hash, after.text_hash);
    assert!(out.iter().filter(|m| m.is(MotionElement::TEXT)).all(|m| m.text_hash != 0));
}

#[test]
fn a_glide_marks_the_content_it_carries_scrolling() {
    let mut rt = Runtime::new(S { words: false }, view, (600.0, 900.0), Env::new(true, 2.0));
    let mut out = Vec::new();
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 400.0));
    let y = |rt: &Runtime<S>| rt.elements().iter().find(|e| e.key.as_deref() == Some("row3")).map(|e| e.rect[1]).unwrap();
    let (mut moved, mut last) = (0, y(&rt));
    for f in 0..120 {
        rt.frame(1.0 / 60.0);
        agree(&rt, &mut out);
        let now = y(&rt);
        if (now - last).abs() > 0.01 {
            moved += 1;
            let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row3")).unwrap();
            // The frame it lands may close the last fraction of a point after the glide ends.
            assert!(row.scrolling || (now - last).abs() < 0.5, "frame {f}: a row the glide carries {:.2} pt is scrolling", now - last);
        }
        last = now;
    }
    assert!(moved > 5, "it glided");
    assert!(rt.elements().iter().all(|e| !e.scrolling), "and stops scrolling when it lands");
}

#[test]
fn secure_text_hashes_to_zero() {
    let mut rt = Runtime::new(S { words: false }, secret, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let mut out = Vec::new();
    rt.motion_elements(&mut out);
    let hash = |k: &str| out.iter().find(|m| rt.element_path(m.id).and_then(|p| p.1) == Some(k)).unwrap().text_hash;
    assert_ne!(hash("plain"), 0);
    assert_eq!(hash("secret"), 0);
}
