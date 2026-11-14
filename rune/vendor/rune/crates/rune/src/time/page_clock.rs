//! The browser's [`Instant`]: `performance.now()` read off the page's current `performance`
//! object on every call. A test harness that swaps in a fake clock after the app has started
//! (Playwright's `clock.install`) then drives Rune's frames too; `web_time` keeps the object it
//! found first. A clock that jumps back (a fake clock starting at 0) continues from the last
//! reading instead, so instants stay monotonic.

use std::cell::RefCell;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::time::Duration;

/// A monotonic reading of the page clock, in the same terms as `std::time::Instant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(Duration);

impl Instant {
    pub fn now() -> Self {
        thread_local!(static CLOCK: RefCell<Monotonic> = const { RefCell::new(Monotonic::new()) });
        let ms = CLOCK.with(|clock| clock.borrow_mut().read(js::performance_now()));
        Instant(Duration::from_secs_f64(ms / 1000.0))
    }

    pub fn duration_since(&self, earlier: Instant) -> Duration { self.saturating_duration_since(earlier) }
    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> { self.0.checked_sub(earlier.0) }
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration { self.0.saturating_sub(earlier.0) }
    pub fn elapsed(&self) -> Duration { Instant::now().saturating_duration_since(*self) }
    pub fn checked_add(&self, duration: Duration) -> Option<Instant> { self.0.checked_add(duration).map(Instant) }
    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> { self.0.checked_sub(duration).map(Instant) }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn to_web_time(self) -> web_time::Instant {
        let now = Instant::now();
        match self.checked_duration_since(now) {
            Some(ahead) => web_time::Instant::now() + ahead,
            None => web_time::Instant::now().checked_sub(now.0 - self.0).unwrap_or_else(web_time::Instant::now),
        }
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, rhs: Duration) -> Instant { self.checked_add(rhs).expect("overflow when adding duration to instant") }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) { *self = *self + rhs; }
}

impl Sub<Duration> for Instant {
    type Output = Instant;
    fn sub(self, rhs: Duration) -> Instant { self.checked_sub(rhs).expect("overflow when subtracting duration from instant") }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, rhs: Duration) { *self = *self - rhs; }
}

impl Sub<Instant> for Instant {
    type Output = Duration;
    fn sub(self, rhs: Instant) -> Duration { self.saturating_duration_since(rhs) }
}

/// Turns a source that may jump back into one that never does: after a jump back, readings
/// continue from the last one and advance as the source does.
#[derive(Debug)]
pub(crate) struct Monotonic {
    last: f64,
    offset: f64,
}

impl Monotonic {
    pub(crate) const fn new() -> Self { Monotonic { last: 0.0, offset: 0.0 } }

    pub(crate) fn read(&mut self, source: f64) -> f64 {
        if !source.is_finite() { return self.last; }
        let t = source + self.offset;
        if t < self.last { self.offset = self.last - source; } else { self.last = t; }
        self.last
    }
}

#[cfg(target_arch = "wasm32")]
mod js {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    #[wasm_bindgen]
    extern "C" {
        type Global;
        #[wasm_bindgen(method, getter, structural)]
        fn performance(this: &Global) -> Performance;
        type Performance;
        #[wasm_bindgen(method, structural)]
        fn now(this: &Performance) -> f64;
    }

    pub(super) fn performance_now() -> f64 { js_sys::global().unchecked_into::<Global>().performance().now() }
}

#[cfg(not(target_arch = "wasm32"))]
mod js {
    pub(super) fn performance_now() -> f64 { 0.0 }
}

#[cfg(test)]
#[path = "page_clock_tests.rs"]
mod tests;
