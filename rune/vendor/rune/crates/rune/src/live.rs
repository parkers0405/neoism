//! Latest-wins property values. Sources outlive views and may be shared by windows.
use std::marker::PhantomData;
use std::sync::{Arc, Weak, Mutex};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use portable_atomic::AtomicU128;
use crate::{Color, Spring, Waker};

/// A linear, render-time track. Keep it in app state and bind it like a `Live` value.
/// Starts on first attachment to each runtime; rebuilds and remounts retain elapsed time.
/// No producer ticks, app wake requests or hidden-window sampling are needed.
pub struct Tween<T: LiveValue> { live: Live<T> }
impl<T: LiveValue> Clone for Tween<T> {
    fn clone(&self) -> Self { Self { live: self.live.clone() } }
}
impl<T: LiveValue> Tween<T> {
    pub fn linear(from: T, to: T, duration: std::time::Duration) -> Self {
        let mut live = Live::new(to);
        Arc::get_mut(&mut live.source).unwrap().linear = Some(Linear {
            from: from.encode().unwrap_or(0), duration: duration.as_secs_f64(),
            starts: Mutex::new(Vec::new()),
        });
        Self { live }
    }
}
impl<T: LiveValue> From<&Tween<T>> for Binding<T> {
    fn from(tween: &Tween<T>) -> Self { Self { live: tween.live.clone(), spring: None, rest_spring: None } }
}

struct Linear {
    from: u128,
    duration: f64,
    // The shared Tween owns each runtime clock's start, even while no element binds it.
    // Weak clock identities also keep independent headless runs deterministic.
    starts: Mutex<Vec<(Weak<()>, f64)>>,
}
impl Linear {
    fn start(&self, clock: &Arc<()>, now: f64) -> f64 {
        let mut starts = self.starts.lock().unwrap();
        starts.retain(|(clock, _)| clock.strong_count() > 0);
        let clock = Arc::downgrade(clock);
        if let Some((_, start)) = starts.iter().find(|(c, _)| c.ptr_eq(&clock)) { return *start; }
        starts.push((clock, now));
        now
    }
    fn sample(&self, to: u128, elapsed: f64) -> (u128, bool) {
        let t = if self.duration == 0.0 { 1.0 } else { (elapsed / self.duration).clamp(0.0, 1.0) };
        if t >= 1.0 { return (to, false); }
        if t == 0.0 { return (self.from, true); }
        let mut bits = 0;
        for lane in 0..4 {
            let a = f32::decode(self.from >> (32 * lane)) as f64;
            let b = f32::decode(to >> (32 * lane)) as f64;
            bits |= ((a * (1.0 - t) + b * t) as f32).encode().unwrap_or(0) << (32 * lane);
        }
        (bits, true)
    }
}

mod sealed { pub trait Sealed {} }

/// Latest-wins text for a fixed layout box. Equal strings do not wake subscribers.
/// Unlike numeric Live publication, text publication takes a short mutex and may allocate.
#[derive(Clone)]
pub struct LiveText { text: Arc<Mutex<Arc<str>>>, pub(crate) source: Arc<Source> }
impl std::fmt::Debug for LiveText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveText").finish_non_exhaustive()
    }
}
impl LiveText {
    pub fn new(text: impl Into<Arc<str>>) -> Self {
        Self { text: Arc::new(Mutex::new(text.into())), source: Source::new(0) }
    }
    pub fn get(&self) -> Arc<str> { self.text.lock().unwrap().clone() }
    pub fn set(&self, text: impl Into<Arc<str>>) -> bool {
        let text = text.into();
        {
            let mut current = self.text.lock().unwrap();
            if *current == text { return false; }
            *current = text;
            self.source.value.fetch_add(1, Ordering::Release);
        }
        self.source.notify();
        true
    }
    pub(crate) fn read(&self, seen: &mut Option<u128>) -> Option<Arc<str>> {
        if *seen == Some(self.source.value.load(Ordering::Acquire)) { return None; }
        let text = self.text.lock().unwrap();
        *seen = Some(self.source.value.load(Ordering::Acquire));
        Some(text.clone())
    }
}

/// The small, atomically published values supported by [`Live`].
pub trait LiveValue: sealed::Sealed + Copy + Send + Sync + 'static {
    /// `None` when any lane is NaN. ±inf clamps to ±`f32::MAX` and -0.0 becomes 0.0, so producers
    /// (audio callbacks) never panic and equal values never wake.
    #[doc(hidden)] fn encode(self) -> Option<u128>;
    #[doc(hidden)] fn decode(bits: u128) -> Self;
}
impl sealed::Sealed for f32 {}
impl LiveValue for f32 {
    fn encode(self) -> Option<u128> {
        if self.is_nan() { return None; }
        let x = if self == 0.0 { 0.0 } else { self.clamp(-f32::MAX, f32::MAX) };
        Some(x.to_bits() as u128)
    }
    fn decode(bits: u128) -> Self { f32::from_bits(bits as u32) }
}
impl sealed::Sealed for [f32; 2] {}
impl LiveValue for [f32; 2] {
    fn encode(self) -> Option<u128> { Some(self[0].encode()? | (self[1].encode()? << 32)) }
    fn decode(bits: u128) -> Self { [f32::decode(bits), f32::decode(bits >> 32)] }
}
impl sealed::Sealed for Color {}
impl sealed::Sealed for [f32; 4] {}
impl LiveValue for [f32; 4] {
    fn encode(self) -> Option<u128> {
        Some(self[0].encode()? | (self[1].encode()? << 32) | (self[2].encode()? << 64) | (self[3].encode()? << 96))
    }
    fn decode(bits: u128) -> Self {
        [f32::decode(bits), f32::decode(bits >> 32), f32::decode(bits >> 64), f32::decode(bits >> 96)]
    }
}
impl LiveValue for Color {
    fn encode(self) -> Option<u128> { [self.r, self.g, self.b, self.a].encode() }
    fn decode(bits: u128) -> Self {
        let [r, g, b, a] = <[f32; 4]>::decode(bits);
        Self { r, g, b, a }
    }
}

/// Cloneable, thread-safe, coherent latest value. Equal bit patterns do not wake a window.
/// NaN is ignored (`set` returns false; `new` starts at zero), ±inf clamps to ±`f32::MAX`. Opacity is clamped by presentation; scale must be nonnegative.
/// Publication is lock-free on supported desktop CPUs. Window notification is coalesced;
/// its platform callback is not a hard-real-time API.
pub struct Live<T: LiveValue> { pub(crate) source: Arc<Source>, _t: PhantomData<T> }
impl<T: LiveValue> Clone for Live<T> {
    fn clone(&self) -> Self { Self { source: self.source.clone(), _t: PhantomData } }
}
impl<T: LiveValue> Live<T> {
    pub fn new(value: T) -> Self {
        // Wasm without the atomics feature has one thread, so portable-atomic's fallback can't tear.
        assert!(
            cfg!(all(target_family = "wasm", not(target_feature = "atomics"))) || AtomicU128::is_lock_free(),
            "Rune Live requires lock-free 128-bit atomics on this CPU"
        );
        Self { source: Source::new(value.encode().unwrap_or(0)), _t: PhantomData }
    }
    /// Returns whether the published bit pattern changed. No allocation, including first use
    /// on a producer thread. Concurrent producers publish whole values, never torn lanes.
    pub fn set(&self, value: T) -> bool {
        let Some(bits) = value.encode() else { return false };
        let s = self.source.seq.load(Ordering::Acquire);
        if s & KIND_MASK == KIND_SET {
            if self.source.value.swap(bits, Ordering::AcqRel) == bits { return false; }
            self.source.notify();
            return true;
        }
        let prev = self.source.value.swap(bits, Ordering::AcqRel);
        if s & KIND_MASK == KIND_SNAP && prev == bits {
            return false;
        }
        self.source.bump_seq(KIND_SET);
        self.source.notify();
        true
    }
    /// Publishes `value` to land immediately on bound elements without running a
    /// [`Self::smooth`] spring, cancelling any running or delayed motion on that property.
    pub fn snap(&self, value: T) -> bool {
        let Some(bits) = value.encode() else { return false };
        let s = self.source.seq.load(Ordering::Acquire);
        let prev = self.source.value.swap(bits, Ordering::AcqRel);
        if s & KIND_MASK == KIND_SNAP && prev == bits { return false; }
        self.source.bump_seq(KIND_SNAP);
        self.source.notify();
        true
    }
    /// Moves smoothed bindings to `active`, holds for `hold` while the window sleeps, then moves
    /// to `rest`. [`Self::get`] and unsmoothed bindings read `rest`.
    pub fn pulse(&self, active: T, rest: T, hold: std::time::Duration) -> bool {
        let (Some(active_bits), Some(rest_bits)) = (active.encode(), rest.encode()) else { return false };
        self.source.value.store(rest_bits, Ordering::Release);
        self.source.active.store(active_bits, Ordering::Release);
        let nanos = hold.as_nanos().min(u64::MAX as u128) as u64;
        self.source.hold_ns.store(nanos, Ordering::Release);
        self.source.bump_seq(KIND_PULSE);
        self.source.notify();
        true
    }
    pub fn get(&self) -> T { T::decode(self.source.value.load(Ordering::Acquire)) }
    /// Smooth this binding with an interruptible rune-motion spring; the source stays 1:1.
    pub fn smooth(&self, spring: Spring) -> Binding<T> {
        Binding { live: self.clone(), spring: Some(spring), rest_spring: None }
    }
}

pub struct Binding<T: LiveValue> {
    live: Live<T>,
    spring: Option<Spring>,
    rest_spring: Option<Spring>,
}
impl<T: LiveValue> Binding<T> {
    /// Use `leave` when returning to a [`Live::pulse`]'s resting value or moving downward.
    pub fn rest(mut self, leave: Spring) -> Self {
        self.rest_spring = Some(leave);
        self
    }
}
impl<T: LiveValue> From<&Live<T>> for Binding<T> {
    fn from(live: &Live<T>) -> Self { Self { live: live.clone(), spring: None, rest_spring: None } }
}

const KIND_MASK: u64 = 0b11;
const KIND_SET: u64 = 0;
const KIND_SNAP: u64 = 1;
const KIND_PULSE: u64 = 2;

#[derive(Clone, Copy)]
pub(crate) enum LatchOp {
    Snap(u128),
    Set(u128),
    Pulse { active: u128, rest: u128, hold: f64 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Property { Opacity, Offset, Scale, Fill, Rect, Blur, Trim, TextColor, Border, Shadow, GradientTop, GradientBottom }
impl Property {
    /// A colour: it moves paint only, never geometry.
    pub(crate) fn is_color(self) -> bool {
        matches!(self, Property::Fill | Property::TextColor | Property::Border | Property::Shadow | Property::GradientTop | Property::GradientBottom)
    }
}
#[derive(Clone)]
pub(crate) struct Bound {
    pub source: Arc<Source>,
    pub property: Property,
    pub spring: Option<Spring>,
    pub rest_spring: Option<Spring>,
    pub seen: Option<(u128, u64)>,
}
impl Bound {
    pub fn new<T: LiveValue>(binding: impl Into<Binding<T>>, property: Property) -> Self {
        let b = binding.into();
        Self { source: b.live.source, property, spring: b.spring, rest_spring: b.rest_spring, seen: None }
    }
    pub fn attach(&self, clock: &Arc<()>, now: f64) {
        if let Some(linear) = &self.source.linear { linear.start(clock, now); }
    }
    pub fn read(&mut self, clock: &Arc<()>, now: f64) -> (Option<LatchOp>, bool) {
        let mut seq = self.source.seq.load(Ordering::Acquire);
        let mut bits = self.source.value.load(Ordering::Acquire);
        let seq2 = self.source.seq.load(Ordering::Acquire);
        if seq2 != seq {
            seq = seq2;
            bits = self.source.value.load(Ordering::Acquire);
        }
        if let Some(linear) = &self.source.linear {
            let (bits, active) = linear.sample(bits, now - linear.start(clock, now));
            if self.seen == Some((bits, seq)) { return (None, active); }
            self.seen = Some((bits, seq));
            return (Some(LatchOp::Snap(bits)), active);
        }
        let initial = self.seen.is_none();
        if self.seen == Some((bits, seq)) { return (None, false); }
        self.seen = Some((bits, seq));
        if initial {
            return (Some(LatchOp::Snap(bits)), false);
        }
        let op = match seq & KIND_MASK {
            KIND_SNAP => LatchOp::Snap(bits),
            KIND_SET => LatchOp::Set(bits),
            _ => LatchOp::Pulse {
                active: self.source.active.load(Ordering::Acquire),
                rest: bits,
                hold: self.source.hold_ns.load(Ordering::Acquire) as f64 * 1e-9,
            },
        };
        (Some(op), false)
    }
}

pub(crate) struct Wake {
    pub pending: AtomicBool,
    visible: AtomicBool,
    callback: Waker,
}
impl Wake {
    pub fn new(callback: Waker) -> Arc<Self> {
        Arc::new(Self { pending: AtomicBool::new(false), visible: AtomicBool::new(true), callback })
    }
    pub fn visible(&self, visible: bool) {
        let was_visible = self.visible.swap(visible, Ordering::AcqRel);
        if visible && !was_visible && self.pending.load(Ordering::Acquire) { (self.callback)(); }
    }
    pub(crate) fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) && self.visible.load(Ordering::Acquire) {
            (self.callback)();
        }
    }
}

pub(crate) struct Subscription {
    pub source_id: u64,
    pub active: AtomicBool,
    wake: Weak<Wake>,
}
struct Listener { subscription: Arc<Subscription>, next: *mut Listener }

pub(crate) struct Source {
    linear: Option<Linear>,
    id: u64,
    value: AtomicU128,
    active: AtomicU128,
    hold_ns: AtomicU64,
    seq: AtomicU64,
    listeners: AtomicPtr<Listener>,
}
impl Source {
    pub(crate) fn new(value: u128) -> Arc<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Arc::new(Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            value: AtomicU128::new(value),
            active: AtomicU128::new(0),
            hold_ns: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            listeners: AtomicPtr::new(std::ptr::null_mut()),
            linear: None,
        })
    }
    fn bump_seq(&self, kind: u64) {
        let mut cur = self.seq.load(Ordering::Relaxed);
        loop {
            let next = (((cur >> 2) + 1) << 2) | kind;
            match self.seq.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return,
                Err(actual) => cur = actual,
            }
        }
    }
    // Append-only immutable list: no reclamation can race a publisher. One cell per
    // source/runtime pair (not per view rebuild); all cells reclaimed with the source.
    pub fn subscribe(&self, wake: &Arc<Wake>) -> Arc<Subscription> {
        let mut head = self.listeners.load(Ordering::Acquire);
        let mut p = head;
        while !p.is_null() {
            // SAFETY: nodes are published with release and only freed by exclusive Drop.
            let node = unsafe { &*p };
            if node.subscription.wake.ptr_eq(&Arc::downgrade(wake)) {
                node.subscription.active.store(true, Ordering::Release);
                return node.subscription.clone();
            }
            p = node.next;
        }
        let subscription = Arc::new(Subscription { source_id: self.id, active: AtomicBool::new(true), wake: Arc::downgrade(wake) });
        let p = Box::into_raw(Box::new(Listener { subscription: subscription.clone(), next: head }));
        loop {
            match self.listeners.compare_exchange_weak(head, p, Ordering::Release, Ordering::Acquire) {
                Ok(_) => return subscription,
                Err(current) => {
                    head = current;
                    // SAFETY: p is unpublished, owned exclusively by this call.
                    unsafe { (*p).next = head; }
                }
            }
        }
    }
    pub(crate) fn notify(&self) {
        let mut p = self.listeners.load(Ordering::Acquire);
        while !p.is_null() {
            // SAFETY: this source is alive through the caller's Arc; list is immutable.
            let node = unsafe { &*p };
            if node.subscription.active.load(Ordering::Acquire)
                && let Some(wake) = node.subscription.wake.upgrade() { wake.notify(); }
            p = node.next;
        }
    }
    pub fn id(&self) -> u64 { self.id }
}
impl Drop for Source {
    fn drop(&mut self) {
        let mut p = *self.listeners.get_mut();
        while !p.is_null() {
            // SAFETY: last source Arc is gone, so no publisher/subscriber can access it.
            let node = unsafe { Box::from_raw(p) };
            p = node.next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn concurrent_producers_never_tear_color_lanes() {
        let live = Live::new(Color::WHITE);
        std::thread::scope(|scope| {
            for writer in 0..4 {
                let live = &live;
                scope.spawn(move || {
                    for i in 0..20_000 {
                        let n = (i * 4 + writer) as f32;
                        live.set(Color { r: n, g: n, b: n, a: n });
                    }
                });
            }
            for _ in 0..80_000 {
                let c = live.get();
                assert_eq!([c.r; 3], [c.g, c.b, c.a]);
            }
        });
    }

    #[test]
    fn subscriptions_coalesce_and_respect_visibility_and_lifetime() {
        let live = Live::new(0.0);
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let wake = Wake::new(Arc::new(move || { c.fetch_add(1, Ordering::Relaxed); }));
        let subscription = live.source.subscribe(&wake);
        assert!(Arc::ptr_eq(&subscription, &live.source.subscribe(&wake)));
        assert!(!live.set(0.0));
        for n in 1..100 { live.set(n as f32); }
        assert_eq!(count.load(Ordering::Relaxed), 1);
        wake.pending.store(false, Ordering::Release);
        wake.visible(false);
        live.set(100.0);
        assert_eq!(count.load(Ordering::Relaxed), 1);
        wake.visible(true);
        assert_eq!(count.load(Ordering::Relaxed), 2);
        wake.pending.store(false, Ordering::Release);
        subscription.active.store(false, Ordering::Release);
        live.set(101.0);
        assert!(!wake.pending.load(Ordering::Acquire));
        subscription.active.store(true, Ordering::Release);
        drop(wake);
        live.set(102.0);
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn non_finite_values_never_panic() {
        let v = Live::new(0.5f32);
        assert!(!v.set(f32::NAN));
        assert_eq!(v.get(), 0.5);
        assert!(v.set(f32::INFINITY));
        assert_eq!(v.get(), f32::MAX);
        assert!(v.set(f32::NEG_INFINITY));
        assert_eq!(v.get(), -f32::MAX);
        assert!(v.set(0.0));
        assert!(!v.set(-0.0), "-0.0 equals 0.0 and must not wake");
        assert_eq!(v.get().to_bits(), 0.0f32.to_bits());
        assert_eq!(Live::new(f32::NAN).get(), 0.0);
        let p = Live::new([1.0f32, 2.0]);
        assert!(!p.set([f32::NAN, 3.0]), "a NaN lane rejects the whole value");
        assert_eq!(p.get(), [1.0, 2.0]);
        let r = Live::new([0.0f32; 4]);
        assert!(r.set([0.0, 0.0, f32::INFINITY, 1.0]));
        assert_eq!(r.get(), [0.0, 0.0, f32::MAX, 1.0]);
        let c = Live::new(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
        assert!(!c.set(Color { r: f32::NAN, g: 0.0, b: 0.0, a: 1.0 }));
        assert_eq!(c.get().r, 1.0);
    }
}
