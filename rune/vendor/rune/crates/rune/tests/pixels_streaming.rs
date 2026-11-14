//! The producer supplies its buffer; replacing and staging a warm stream allocates nothing.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}
struct Allocator;
fn allocated() {
    let _ = COUNT.try_with(|count| {
        if let Some(n) = count.get() { count.set(Some(n + 1)); }
    });
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocated();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocated();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocated();
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn sixty_hud_updates_allocate_nothing_and_keep_the_same_slot() {
    use rune::{Cx, El, Env, Pixels, Runtime, el};
    fn view(p: &Pixels, _: &mut Cx<Pixels>) -> El { el().size(112.0, 64.0).pixels(p) }
    let pixels = Pixels::new();
    pixels.set(224, 128, vec![0; 224 * 128 * 4]);
    let mut rt = Runtime::new(pixels.clone(), view, (112.0, 64.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let slot = rt.images.place(&pixels, [224.0, 128.0]).unwrap();
    let page = rt.images.pages[0].data.as_ptr();
    let buffers: Vec<_> = (0..60).map(|n| vec![n; 224 * 128 * 4]).collect();
    let mut stable = true;
    COUNT.set(Some(0));
    for buffer in buffers {
        pixels.set(224, 128, buffer);
        stable &= rt.images.place(&pixels, [224.0, 128.0]) == Some(slot);
        rt.images.end_frame();
    }
    let allocations = COUNT.replace(None).unwrap();
    assert_eq!(allocations, 0);
    assert!(stable && !slot.premultiplied, "streaming never selects a mip");
    assert_eq!(rt.images.pages[0].data.as_ptr(), page);
}
