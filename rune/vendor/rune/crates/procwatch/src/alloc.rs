//! A counting global allocator: live and peak heap bytes, the largest single request, and the
//! bytes held in big blocks, plus net bytes per thread tag. A few relaxed atomics per call.
//!
//! ```ignore
//! #[cfg(debug_assertions)]
//! #[global_allocator]
//! static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();
//! ```
//!
//! Tags attribute heap to threads: `procwatch::alloc::tag_thread("audio")` at a thread's start
//! counts what that thread allocates minus what it frees. A producer whose buffer is freed
//! elsewhere still shows; that's the point.
//!
//! [`thread_allocations`] counts this thread's allocations, so a test can assert a hot path
//! allocates nothing while other tests run in parallel. [`thread_allocated_bytes`] counts the
//! bytes they asked for, for a per-frame allocation figure.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicI64, AtomicPtr, AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};

/// A block at least this big counts as "big": a buffer worth naming in a warning.
pub const BIG: usize = 64 << 20;
/// Thread tags there's room for; more fall into tag 0, "untagged".
pub const TAGS: usize = 16;

static INSTALLED: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
static BIG_LIVE: AtomicI64 = AtomicI64::new(0);
static BIG_BLOCKS: AtomicI64 = AtomicI64::new(0);
static TAG_NET: [AtomicI64; TAGS] = [const { AtomicI64::new(0) }; TAGS];
static TAG_NAMES: [AtomicPtr<&'static str>; TAGS] = [const { AtomicPtr::new(std::ptr::null_mut()) }; TAGS];

thread_local! {
    static TAG: Cell<u8> = const { Cell::new(0) };
    static THREAD_ALLOCS: Cell<u64> = const { Cell::new(0) };
    static THREAD_BYTES: Cell<u64> = const { Cell::new(0) };
}

fn counted(size: usize) {
    ALLOCS.fetch_add(1, Relaxed);
    let _ = THREAD_ALLOCS.try_with(|c| c.set(c.get() + 1));
    let _ = THREAD_BYTES.try_with(|c| c.set(c.get().wrapping_add(size as u64)));
}

/// Allocations (and reallocations) made on this thread so far; 0 unless [`Counting`] is the
/// global allocator.
pub fn thread_allocations() -> u64 {
    THREAD_ALLOCS.try_with(Cell::get).unwrap_or(0)
}

/// Bytes this thread has asked for so far: every successful alloc, alloc_zeroed and realloc
/// (a realloc counts its whole new size), frees not subtracted. Traffic, not live heap.
/// `None` until [`Counting`] has served a request, so a missing allocator never reads as zero.
pub fn thread_allocated_bytes() -> Option<u64> {
    if INSTALLED.load(Relaxed) == 0 {
        return None;
    }
    THREAD_BYTES.try_with(Cell::get).ok()
}

pub struct Counting<A = System> {
    inner: A,
}

impl Counting<System> {
    pub const fn system() -> Self {
        Counting { inner: System }
    }
}

impl<A> Counting<A> {
    pub const fn new(inner: A) -> Self {
        Counting { inner }
    }
}

fn grew(size: usize) {
    let live = LIVE.fetch_add(size as i64, Relaxed) + size as i64;
    PEAK.fetch_max(live, Relaxed);
    LARGEST.fetch_max(size, Relaxed);
    if size >= BIG {
        BIG_LIVE.fetch_add(size as i64, Relaxed);
        BIG_BLOCKS.fetch_add(1, Relaxed);
    }
    // `try_with`: a thread's last frees run after its locals are gone.
    let tag = TAG.try_with(Cell::get).unwrap_or(0) as usize;
    TAG_NET[tag].fetch_add(size as i64, Relaxed);
}

fn shrank(size: usize) {
    LIVE.fetch_sub(size as i64, Relaxed);
    if size >= BIG {
        BIG_LIVE.fetch_sub(size as i64, Relaxed);
        BIG_BLOCKS.fetch_sub(1, Relaxed);
    }
    let tag = TAG.try_with(Cell::get).unwrap_or(0) as usize;
    TAG_NET[tag].fetch_sub(size as i64, Relaxed);
}

// SAFETY: every call goes straight to `inner`; the counters never touch the heap.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Counting<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        INSTALLED.store(1, Relaxed);
        let p = unsafe { self.inner.alloc(layout) };
        if !p.is_null() {
            counted(layout.size());
            grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        INSTALLED.store(1, Relaxed);
        let p = unsafe { self.inner.alloc_zeroed(layout) };
        if !p.is_null() {
            counted(layout.size());
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.inner.dealloc(ptr, layout) };
        shrank(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { self.inner.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            counted(new_size);
            shrank(layout.size());
            grew(new_size);
        }
        p
    }
}

/// Counts this thread's heap under `name` from now on (net bytes: allocated minus freed here).
pub fn tag_thread(name: &'static str) {
    let index = (1..TAGS)
        .find(|&i| {
            let seen = TAG_NAMES[i].load(Acquire);
            if !seen.is_null() {
                // SAFETY: only ever set from a leaked `&'static str` box below.
                return unsafe { *seen } == name;
            }
            let fresh = Box::into_raw(Box::new(name));
            match TAG_NAMES[i].compare_exchange(std::ptr::null_mut(), fresh, Release, Acquire) {
                Ok(_) => true,
                Err(other) => {
                    // SAFETY: `fresh` was never shared.
                    drop(unsafe { Box::from_raw(fresh) });
                    unsafe { *other == name }
                }
            }
        })
        .unwrap_or(0);
    TAG.with(|t| t.set(index as u8));
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heap {
    pub live: i64,
    pub peak: i64,
    pub allocations: u64,
    pub largest: usize,
    /// Bytes, and blocks, held in allocations of at least [`BIG`].
    pub big_live: i64,
    pub big_blocks: i64,
    /// Net bytes per thread tag, biggest first; "untagged" is everything else.
    pub tags: Vec<(&'static str, i64)>,
}

/// The counters, or `None` when [`Counting`] isn't the global allocator.
pub fn heap() -> Option<Heap> {
    if INSTALLED.load(Relaxed) == 0 {
        return None;
    }
    let mut tags: Vec<(&'static str, i64)> = (0..TAGS)
        .filter_map(|i| {
            let name = match i {
                0 => "untagged",
                _ => {
                    let p = TAG_NAMES[i].load(Acquire);
                    if p.is_null() {
                        return None;
                    }
                    // SAFETY: as in `tag_thread`.
                    unsafe { *p }
                }
            };
            Some((name, TAG_NET[i].load(Relaxed)))
        })
        .collect();
    tags.sort_by_key(|(_, bytes)| -bytes);
    Some(Heap {
        live: LIVE.load(Relaxed),
        peak: PEAK.load(Relaxed),
        allocations: ALLOCS.load(Relaxed),
        largest: LARGEST.load(Relaxed),
        big_live: BIG_LIVE.load(Relaxed),
        big_blocks: BIG_BLOCKS.load(Relaxed),
        tags,
    })
}
