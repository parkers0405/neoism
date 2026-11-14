//! Count the whole runtime frame, not only the spring evaluator. No GPU/driver allocations.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, text};

struct Counting;
thread_local! { static ALLOCS: Cell<usize> = const { Cell::new(0) }; }
fn bump() { let _ = ALLOCS.try_with(|c| c.set(c.get() + 1)); }
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { bump(); unsafe { System.alloc(l) } }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 { bump(); unsafe { System.alloc_zeroed(l) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { bump(); unsafe { System.realloc(p, l, n) } }
}
#[global_allocator]
static A: Counting = Counting;
fn allocs() -> usize { ALLOCS.with(Cell::get) }

#[test]
fn disabled_trace_hooks_neither_format_nor_allocate() {
    struct Unformatted;
    impl std::fmt::Display for Unformatted {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { panic!("disabled trace formatted an event") }
    }
    fn view(_: &(), _: &mut Cx<()>) -> El { rune::el().trace_name("shell") }
    let mut rt = Runtime::new((), view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    let element = rune::el();
    let before = allocs();
    let _named = element.trace_name("chip:Area").trace_namespace("gpui", "pill");
    assert!(allocs() - before <= 1, "trace labels share the element's cold storage");
    let before = allocs();
    for _ in 0..100 {
        rt.update(|_, tx| {
            assert!(!tx.tracing());
            tx.trace_event(Unformatted);
            tx.trace_channel("phase:recording", "phase", 1.0);
        });
        rt.trace_event(Unformatted);
        rt.trace_channel("shell.width", "position", 100.0);
    }
    assert_eq!(allocs() - before, 0);
}

#[test]
fn static_faded_container_planning_allocates_nothing() {
    use rune::{Color, Live, el};
    fn view(offset: &Live<[f32; 2]>, _: &mut Cx<Live<[f32; 2]>>) -> El {
        el().size(200.0, 200.0).opacity(0.5)
            .child(el().size(100.0, 100.0).fill(Color::WHITE))
            .child(el().size(100.0, 100.0).live_offset(offset).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(Live::new([0.0; 2]), view, (200.0, 200.0), Env { system_dark: true, scale: 2.0 });
    rt.frame(0.0);
    rt.state.set([1.0; 2]);
    rt.frame(0.0);
    let before = allocs();
    for i in 0..60 {
        // Force presentation without rebuilding the static-opacity ancestor.
        rt.state.set([i as f32; 2]);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(allocs() - before, 0, "static faded ancestor scope planning");
}

fn view(body: &String, _: &mut Cx<String>) -> El {
    col().w_full().h(400.0).scroll(ScrollSpec { stick_bottom: true }).child(
        col().w_full().virtualize(80.0).growth(GrowthSpec::default()).child(
            col().key("row").w_full().streaming(true).child(text(body.clone()).key("body").wrap().w_full().reveal(0.18))
        )
    )
}

#[test]
fn streaming_presentation_allocates_nothing() {
    let mut rt = Runtime::new("hello ".repeat(120), view, (400.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state.truncate(360);
    rt.settle_all();
    rt.frame(0.0);
    rt.state.push_str(&"hello ".repeat(60));
    rt.invalidate();
    // Commit/shaping is separately budgeted; warm glyphs and scratch capacity before measuring.
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let before = allocs();
    for _ in 0..30 { assert!(rt.frame(1.0 / 120.0)); }
    let count = allocs() - before;
    assert_eq!(count, 0, "presentation allocations across 30 streaming frames");
}

#[test]
fn live_publication_and_presentation_allocate_nothing() {
    use rune::{Color, Live, el};
    type State = (Live<f32>, Live<[f32; 2]>, Live<Color>);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().size(200.0, 200.0).child(el().size(20.0, 20.0)
            .live_opacity(&s.0).live_offset(&s.1).live_fill(&s.2))
    }
    let state = (Live::new(1.0), Live::new([0.0, 0.0]), Live::new(Color::WHITE));
    let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let producer = (rt.state.0.clone(), rt.state.1.clone(), rt.state.2.clone());
    // Includes the very first publication on a new producer thread, not only warmed sets.
    std::thread::spawn(move || {
        let before = allocs();
        for i in 0..120 {
            producer.0.set(0.3 + i as f32 / 240.0);
            producer.1.set([i as f32, i as f32]);
            producer.2.set(Color { r: i as f32 / 120.0, ..Color::WHITE });
        }
        assert_eq!(allocs() - before, 0, "producer allocations");
    }).join().unwrap();
    rt.frame(0.0);
    let before = allocs();
    for i in 0..120 {
        rt.state.0.set(0.5 + i as f32 / 240.0);
        rt.state.1.set([i as f32, i as f32]);
        rt.state.2.set(Color { r: i as f32 / 120.0, ..Color::WHITE });
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(allocs() - before, 0, "live CPU presentation allocations");
}

#[test]
fn scroll_edge_fades_allocate_nothing_per_frame() {
    use rune::{Color, el};
    fn view(faded: &bool, _: &mut Cx<bool>) -> El {
        let rows = (0..40).map(|i| el().key(format!("row{i}")).w_full().h(20.0).fill(Color::WHITE));
        let scroll = col().w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(col().w_full().children(rows));
        if *faded { scroll.scroll_edge_fade(24.0) } else { scroll }
    }
    let gliding = |faded| {
        let mut rt = Runtime::new(faded, view, (200.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.fling(100.0, 100.0, -1500.0);
        for _ in 0..4 { rt.frame(1.0 / 120.0); }
        let before = allocs();
        for _ in 0..30 { assert!(rt.frame(1.0 / 120.0), "still gliding"); }
        allocs() - before
    };
    assert_eq!(gliding(true), gliding(false), "edge fades add no per-frame allocations while scrolling");
}

#[test]
fn a_cached_ink_probe_allocates_nothing() {
    // Initialize fonts before ink records its cache epoch, and keep other test threads'
    // text systems from invalidating that epoch while we measure cache hits.
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let style = rune::TextStyle::new(12.0, 400.0);
    let first = rune::text::ink("⌘K", style);
    assert!(first.is_some());
    let before = allocs();
    for _ in 0..100 {
        assert_eq!(rune::text::ink("⌘K", style), first);
    }
    assert_eq!(allocs() - before, 0);
}
