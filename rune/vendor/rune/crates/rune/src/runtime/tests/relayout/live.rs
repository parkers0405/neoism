use super::*;
use crate::Live;

struct Bindings {
    spring: bool,
    offset: Live<[f32; 2]>,
    rect: Live<[f32; 4]>,
    opacity: Live<f32>,
    scale: Live<f32>,
    blur: Live<f32>,
    fill: Live<Color>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            spring: false,
            offset: Live::new([7.0, 11.0]),
            rect: Live::new([30.0, 40.0, 60.0, 70.0]),
            opacity: Live::new(0.7),
            scale: Live::new(0.8),
            blur: Live::new(2.0),
            fill: Live::new(Color::WHITE),
        }
    }
}

fn view(s: &Bindings, _: &mut Cx<Bindings>) -> El {
    ran();
    let offset = if s.spring { s.offset.smooth(Spring::new(0.4, 0.0)) } else { (&s.offset).into() };
    row().w_full().h_full().justify_end().child(
        col().key("bound").w(100.0).h_full()
            .live_offset(offset)
            .live_opacity(&s.opacity)
            .live_scale(&s.scale)
            .live_blur(&s.blur)
            .live_fill(&s.fill)
            .child(el().key("thumb").absolute(0.0, 0.0).live_rect(&s.rect).fill(Color::WHITE)),
    )
}

#[test]
fn resizing_during_a_live_spring_keeps_its_progress_and_velocity() {
    let mut rt = Runtime::new(Bindings { spring: true, ..Bindings::default() }, view, (700.0, 600.0), Env::new(true, 2.0));
    let mut reference = Runtime::new(Bindings { spring: true, ..Bindings::default() }, view, (700.0, 600.0), Env::new(true, 2.0));
    for r in [&mut rt, &mut reference] {
        r.frame(0.0);
        r.state.offset.set([90.0, 110.0]);
    }
    for i in 0..90 {
        if i == 8 { // Reverse while both springs are still moving.
            rt.state.offset.set([20.0, 30.0]);
            reference.state.offset.set([20.0, 30.0]);
        }
        assert_eq!(resize(&mut rt, 700.0 - i as f32, 2.0), 0);
        reference.frame(1.0 / 120.0);
        assert_eq!(rt.scene.get(node(&rt, "bound").id, prop::OFFSET), reference.scene.get(node(&reference, "bound").id, prop::OFFSET), "frame {i}: resize never relatches or restarts the spring");
    }
}

#[test]
fn a_layout_read_retry_restores_the_moved_live_nodes() {
    fn reading(s: &Bindings, cx: &mut Cx<Bindings>) -> El {
        cx.layout_size("window");
        view(s, cx).key("window")
    }
    let mut rt = Runtime::new(Bindings::default(), reading, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(resize(&mut rt, 500.0, 2.0) > 0, "changed layout reads still run the view");
    rt.state.offset.set([30.0, 50.0]);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.scene.get(node(&rt, "bound").id, prop::OFFSET), Some(rune_motion::Vec2::new(30.0, 50.0)));
    assert_eq!(node(&rt, "bound").vis.fill, Color::WHITE);
}

fn compare(rt: &Runtime<Bindings>, full: &Runtime<Bindings>) {
    assert_eq!(layout(rt), layout(full));
    for key in ["bound", "thumb"] {
        assert_eq!(rt.presented(key), full.presented(key), "drawn {key}");
        let (a, b) = (node(rt, key), node(full, key));
        assert_eq!(a.vis.fill, b.vis.fill, "settled fill is retained");
        assert_eq!(rt.scene.get(a.id, prop::OFFSET), full.scene.get(b.id, prop::OFFSET));
        assert_eq!(rt.scene.get(a.id, prop::SCALE), full.scene.get(b.id, prop::SCALE));
        assert_eq!(rt.scene.get(a.id, prop::BLUR), full.scene.get(b.id, prop::BLUR));
        assert!(a.live.iter().zip(&b.live).all(|(a, b)| a.seen == b.seen));
    }
}

#[test]
fn resize_keeps_live_bindings_without_running_the_view() {
    let mut rt = Runtime::new(Bindings::default(), view, (700.0, 600.0), Env::new(true, 2.0));
    let mut full = Runtime::new(Bindings::default(), view, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    full.frame(0.0);
    for w in [600.0, 420.0, 300.0, 520.0] {
        assert_eq!(resize(&mut rt, w, 2.0), 0, "live bindings allow relayout at {w}");
        // A real resize lands layout. The forced full build must use the same policy rather
        // than treating the geometry change as an animated application-state update.
        full.update(|_, tx| tx.instant());
        assert_eq!(resize(&mut full, w, 2.0), 1);
        compare(&rt, &full);
        for state in [&rt.state, &full.state] {
            state.offset.set([w / 20.0, 15.0]);
            state.rect.set([10.0, w / 10.0, 8.0, 40.0]);
            state.opacity.set(w / 1000.0);
            state.scale.set(w / 700.0);
            state.blur.set(w / 100.0);
            state.fill.set(Color::BLACK);
        }
        assert!(rt.live_pending(), "the retained subscription wakes after resize");
        assert_eq!(views(|| { rt.frame(1.0 / 120.0); }), 0);
        full.frame(1.0 / 120.0);
        compare(&rt, &full);
        assert_eq!(rt.scene.get(node(&rt, "bound").id, prop::OFFSET), Some(rune_motion::Vec2::new(w / 20.0, 15.0)));
        assert_eq!(rt.scene.get(node(&rt, "thumb").id, prop::FRAME), Some(MRect::new(10.0, w / 10.0, 8.0, 40.0)));
    }
}
