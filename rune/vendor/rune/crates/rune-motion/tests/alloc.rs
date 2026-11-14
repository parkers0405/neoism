//! The steady-state frame allocates nothing: proven with a counting global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use rune_motion::prelude::*;
use rune_motion::{NodeSpec, Prop, PropClass};

struct Counting;
// Per thread: tests run in parallel and must not count each other's allocations.
thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

fn bump() {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

const X: Prop<f32> = Prop::custom(64, PropClass::Spatial);

fn allocs() -> usize {
    ALLOCS.with(|c| c.get())
}

#[test]
fn store_tick_allocates_nothing_in_steady_state() {
    let n = 5000;
    let mut store = AnimationStore::with_capacity(2 * n as usize, 3);
    for i in 0..n {
        let id = NodeId(i);
        store.set(id, X, 0.0);
        store.set(id, prop::FRAME, Rect::new(0.0, 0.0, 10.0, 10.0));
        // A spread of bounces and durations so tracks settle at different times.
        let s = Spring::new(0.3 + (i % 7) as f32 * 0.2, (i % 5) as f32 * 0.1);
        store.animate(id, X, 100.0 + i as f32, s);
        store.animate(id, prop::FRAME, Rect::new(5.0, 3.0, 20.0, 10.0), s.delay((i % 3) as f32 * 0.05));
        if i % 4 == 0 {
            store.animate(id, X, -50.0, Curve::tween(0.4, Easing::EASE_IN_OUT));
        }
    }
    // Warm up: the first frames grow the event and touched buffers to the working-set size.
    let mut t = 0.0;
    for _ in 0..4 {
        t += 1.0 / 120.0;
        store.tick_at(t);
    }
    let before = allocs();
    let mut ticks = 0;
    while store.active_len() > 0 && ticks < 2000 {
        t += 1.0 / 120.0;
        store.tick_at(t);
        ticks += 1;
    }
    let after = allocs();
    assert!(ticks > 100, "the animations should run for a while ({ticks})");
    assert_eq!(after - before, 0, "{} allocations over {ticks} ticks", after - before);
}

#[test]
fn scene_frame_allocates_nothing_while_animating() {
    let mut scene = Scene::new();
    for i in 0..500u64 {
        scene.insert(NodeId(i), NodeSpec::new(Rect::new(0.0, i as f32 * 10.0, 100.0, 10.0)), Mount::Initial);
    }
    scene.frame_at(0.0);
    for i in 0..500u64 {
        scene.set_frame(NodeId(i), Rect::new(10.0, i as f32 * 12.0, 100.0, 12.0), motion::REFLOW);
        scene.animate(NodeId(i), prop::OPACITY, 0.5, motion::SETTLE);
    }
    let mut t = 0.0;
    for _ in 0..3 {
        t += 1.0 / 120.0;
        scene.frame_at(t);
    }
    let before = allocs();
    for _ in 0..40 {
        t += 1.0 / 120.0;
        let out = scene.frame_at(t);
        assert!(out.animating && out.changed.len() == 500);
    }
    assert_eq!(allocs() - before, 0);
}
