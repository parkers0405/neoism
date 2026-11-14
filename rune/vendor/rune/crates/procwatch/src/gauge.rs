//! Byte gauges for memory the Rust allocator never sees: pixel buffers, IOSurfaces, GPU
//! textures, model weights mapped by a C library, or a queue you want named in a warning.
//!
//! ```ignore
//! static FRAMES: procwatch::Gauge = procwatch::Gauge::new("video.frames");
//!
//! let lease = FRAMES.track(width * height * 4); // subtracted again when `lease` drops
//! FRAMES.set(cache.bytes());                    // or report a total you measure yourself
//! ```
//!
//! A snapshot subtracts the counted heap and every gauge from the footprint; what's left
//! ([`crate::Snapshot::unaccounted`]) is memory nothing claims, which is where a leaked native
//! handle shows up.
//!
//! Gauges count in builds with debug assertions, or with the `release-gauges` feature;
//! otherwise every call compiles to nothing and [`gauges`] is empty.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, Ordering::Relaxed};

/// Gauges there's room for; more still count but aren't listed.
pub const GAUGES: usize = 64;

const ON: bool = cfg!(any(debug_assertions, feature = "release-gauges"));

static REGISTRY: [AtomicPtr<Gauge>; GAUGES] = [const { AtomicPtr::new(std::ptr::null_mut()) }; GAUGES];

pub struct Gauge {
    name: &'static str,
    bytes: AtomicI64,
    registered: AtomicBool,
}

impl Gauge {
    pub const fn new(name: &'static str) -> Gauge {
        Gauge { name, bytes: AtomicI64::new(0), registered: AtomicBool::new(false) }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Current bytes (0 when gauges are compiled out).
    pub fn get(&self) -> i64 {
        self.bytes.load(Relaxed)
    }

    #[inline]
    pub fn add(&'static self, bytes: usize) {
        if ON {
            self.register();
            self.bytes.fetch_add(bytes as i64, Relaxed);
        }
    }

    #[inline]
    pub fn sub(&'static self, bytes: usize) {
        if ON {
            self.bytes.fetch_sub(bytes as i64, Relaxed);
        }
    }

    /// Replaces the total, for a size you measure rather than count.
    #[inline]
    pub fn set(&'static self, bytes: usize) {
        if ON {
            self.register();
            self.bytes.store(bytes as i64, Relaxed);
        }
    }

    /// Adds `bytes` now and subtracts them when the lease drops.
    #[inline]
    #[must_use = "the bytes are subtracted again when the lease drops"]
    pub fn track(&'static self, bytes: usize) -> Lease {
        self.add(bytes);
        Lease { gauge: self, bytes }
    }

    fn register(&'static self) {
        if self.registered.load(Relaxed) || self.registered.swap(true, Relaxed) {
            return;
        }
        let me = self as *const Gauge as *mut Gauge;
        for slot in &REGISTRY {
            if slot.compare_exchange(std::ptr::null_mut(), me, Relaxed, Relaxed).is_ok() {
                return;
            }
        }
    }
}

/// Bytes counted against a [`Gauge`] until dropped.
pub struct Lease {
    gauge: &'static Gauge,
    bytes: usize,
}

impl Lease {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.gauge.sub(self.bytes);
    }
}

/// Every gauge used so far, biggest first.
pub fn gauges() -> Vec<(&'static str, i64)> {
    let mut out: Vec<(&'static str, i64)> = REGISTRY
        .iter()
        .map(|slot| slot.load(Relaxed))
        .take_while(|p| !p.is_null())
        // SAFETY: only `&'static Gauge`s are ever stored.
        .map(|p| unsafe { &*p })
        .map(|g| (g.name, g.get()))
        .collect();
    out.sort_by_key(|(_, bytes)| -bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    static A: Gauge = Gauge::new("test.a");
    static B: Gauge = Gauge::new("test.b");

    #[test]
    fn leases_add_and_give_back() {
        if !ON {
            A.add(1000);
            assert_eq!((A.get(), gauges().len()), (0, 0), "compiled out");
            return;
        }
        let lease = A.track(1000);
        A.add(24);
        B.set(5);
        let listed = gauges();
        assert_eq!(listed.iter().find(|(n, _)| *n == "test.a").map(|g| g.1), Some(1024));
        assert_eq!(listed.iter().find(|(n, _)| *n == "test.b").map(|g| g.1), Some(5));
        drop(lease);
        A.sub(24);
        assert_eq!(A.get(), 0);
        // Registered once, however often it's used.
        A.add(1);
        A.sub(1);
        assert_eq!(gauges().iter().filter(|(n, _)| *n == "test.a").count(), 1);
    }
}
