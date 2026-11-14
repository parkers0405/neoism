//! The lib test binary's allocator: the system one, counting this thread's allocations and
//! its live and peak bytes, so a test can assert what a call allocates.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}
fn track(allocs: usize, bytes: isize) {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + allocs));
    let _ = LIVE.try_with(|live| {
        live.set(live.get() + bytes);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
    });
}
// SAFETY: forwards to the system allocator, counting calls and bytes on this thread.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { track(1, l.size() as isize); unsafe { System.alloc(l) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { track(0, -(l.size() as isize)); unsafe { System.dealloc(p, l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        track(1, n as isize - l.size() as isize);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static COUNTING: Counting = Counting;

/// Allocations made on this thread so far.
pub(crate) fn allocs() -> usize { ALLOCS.with(Cell::get) }

/// Runs `f`, returning its result and the most bytes it held allocated at once on this thread
/// beyond what was live when it started.
pub(crate) fn peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let start = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(start));
    let out = f();
    (out, (PEAK.with(Cell::get) - start).max(0) as usize)
}
