//! When each traced frame reached the screen, next to when it was meant to.
//!
//! A native `--trace` stamps frames at CPU submission (`t`) and, on macOS, at the display
//! link's target (`target_ms`). Neither shows whether frames reach the screen evenly: two
//! frames can present a fraction of a millisecond apart and leave a gap after them, which
//! reads as a hitch even when every frame's motion is right. On macOS the traced window's
//! layer reports each drawable's Metal `presentedTime`, and the trace writes it as
//! `present_ms`, with `present_dt_ms` from the previous presented frame. Other platforms
//! have no reliable source and write neither.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;
use crate::time::Instant;

/// A window's actual presentation feedback, independent of trace collection. Clones refer
/// to the same window, never to a process-wide counter. Reading does not schedule work.
#[derive(Clone, Debug, Default)]
pub struct PresentationFeedback(Arc<std::sync::Mutex<VecDeque<Instant>>>);

impl PresentationFeedback {
    /// Up to 256 most recent actual presentation timestamps, in chronological order.
    /// Metal callbacks may arrive out of order or in a burst; callback arrival times and
    /// display-link targets are not substituted. Skipped drawables are not included.
    /// The history is non-destructive, so reading it cannot consume trace feedback.
    pub fn timestamps(&self) -> Vec<Instant> {
        self.0.lock().unwrap().iter().copied().collect()
    }

    #[cfg(any(target_os = "macos", test))]
    fn record(&self, at: Instant) {
        let mut times = self.0.lock().unwrap();
        let index = times.partition_point(|t| *t <= at);
        times.insert(index, at);
        if times.len() > 256 { times.pop_front(); }
    }
}

/// A drawable's sequence number and when it reached the screen, or `None` when the system
/// skipped it.
pub(crate) type Presented = (u64, Option<Instant>);

/// Where present times come from. Implementations are filled from another thread.
pub(crate) trait Source: Send + Sync {
    fn take(&self, out: &mut Vec<Presented>);
}

/// A frame whose present time hasn't arrived after this many later frames is written
/// without one. Presentation runs two or three frames behind submission.
const WAIT_FRAMES: u32 = 32;

/// Trace lines held until their frame's present time is known, so each frame's line carries
/// it. Lines stay in submission order.
pub(crate) struct Presents {
    source: Arc<dyn Source>,
    start: Instant,
    pending: VecDeque<Pending>,
    last_present: Option<f64>,
    scratch: Vec<Presented>,
}

struct Pending {
    /// The line without its closing brace.
    line: String,
    seq: Option<u64>,
    /// `Some(None)`: Metal skipped the drawable.
    present: Option<Option<f64>>,
    /// The previous traced frame asked for this one right away, so the gap between their
    /// presents is cadence, not idle time.
    continuous: bool,
    age: u32,
}

/// Milliseconds from `start`, rounded like the trace's `t`, negative before it.
pub(crate) fn ms_since(start: Instant, at: Instant) -> f64 {
    let ms = match at.checked_duration_since(start) {
        Some(d) => d.as_secs_f64() * 1000.0,
        None => -start.duration_since(at).as_secs_f64() * 1000.0,
    };
    r3(ms)
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

impl Presents {
    pub(crate) fn new(source: Arc<dyn Source>, start: Instant) -> Self {
        Self { source, start, pending: VecDeque::new(), last_present: None, scratch: Vec::new() }
    }

    /// Queues a frame's line (without its closing brace) and writes every line now complete.
    /// `seq`: the drawable the frame presented, if the source tracks it.
    pub(crate) fn push(&mut self, line: String, seq: Option<u64>, continuous: bool, out: &mut impl std::io::Write) {
        for p in &mut self.pending { p.age += 1; }
        self.pending.push_back(Pending { line, seq, present: None, continuous, age: 0 });
        self.poll();
        self.write_ready(out, false);
    }

    /// Whether any line still waits for its present time.
    pub(crate) fn waiting(&mut self) -> bool {
        self.poll();
        self.pending.iter().any(|p| p.seq.is_some() && p.present.is_none())
    }

    /// Writes every queued line, with or without its present time.
    pub(crate) fn flush(&mut self, out: &mut impl std::io::Write) {
        self.poll();
        self.write_ready(out, true);
    }

    fn poll(&mut self) {
        self.source.take(&mut self.scratch);
        for (seq, at) in self.scratch.drain(..) {
            if let Some(p) = self.pending.iter_mut().find(|p| p.seq == Some(seq)) {
                p.present = Some(at.map(|at| ms_since(self.start, at)));
            }
        }
    }

    fn write_ready(&mut self, out: &mut impl std::io::Write, all: bool) {
        while let Some(p) = self.pending.front() {
            if !(all || p.seq.is_none() || p.present.is_some() || p.age >= WAIT_FRAMES) { break; }
            let p = self.pending.pop_front().unwrap();
            let line = self.finish(p);
            let _ = writeln!(out, "{line}");
        }
    }

    fn finish(&mut self, p: Pending) -> String {
        let mut line = p.line;
        // An interval spans only frames drawn back to back that both reached the screen.
        if !p.continuous { self.last_present = None; }
        match (p.seq, p.present) {
            (Some(_), Some(Some(at))) => {
                let _ = write!(line, r#","present_ms":{at}"#);
                if let Some(last) = self.last_present {
                    let _ = write!(line, r#","present_dt_ms":{}"#, r3(at - last));
                }
                self.last_present = Some(at);
            }
            (Some(_), Some(None)) => {
                line.push_str(r#","present_ms":null"#);
                self.last_present = None;
            }
            _ => self.last_present = None,
        }
        line.push('}');
        line
    }
}

mod summary;
pub(crate) use summary::Summary;
#[cfg(test)]
use summary::Spread;

/// How long a finished trace waits for its last frames' present times.
pub(crate) const SETTLE: Duration = Duration::from_millis(150);

#[cfg(target_os = "macos")]
pub(crate) mod metal {
    //! Present times from a CAMetalLayer. wgpu acquires and presents the drawable itself and
    //! exposes neither, so the traced window's layer becomes an instance of a subclass of its
    //! own class whose `nextDrawable` adds a Metal presented handler to each drawable it
    //! returns. Only that layer changes, while tracing, reporting encoded frames, or while a
    //! transparent window's shadow needs feedback (`macos_window_shape`), and it gets its
    //! class back when the hook drops.

    use super::{Presented, Source};
    use objc2_metal::MTLDrawable;
    use objc2_video::rc::Retained;
    use objc2_video::runtime::{AnyClass, AnyObject, ClassBuilder, ProtocolObject, Sel};
    use objc2_video::{Message, msg_send, sel};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::ffi::CString;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[link(name = "QuartzCore", kind = "framework")]
    unsafe extern "C" { fn CACurrentMediaTime() -> f64; }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
        fn CFRunLoopWakeUp(run_loop: *mut std::ffi::c_void);
    }

    /// Wakes the main run loop for another turn. Callable from any thread.
    pub(crate) fn wake_main() {
        // SAFETY: CFRunLoopWakeUp is thread-safe; the main run loop lives as long as the process.
        unsafe { CFRunLoopWakeUp(CFRunLoopGetMain()) }
    }

    /// Drawables acquired and presented on a hooked layer.
    #[derive(Default)]
    pub(crate) struct Log {
        next: AtomicU64,
        acquired: AtomicU64,
        /// `(seq, presentedTime)`: host seconds, 0 for a skipped drawable. Kept only while tracing.
        presented: Mutex<Vec<(u64, f64)>>,
        untraced: bool,
        pub(crate) feedback: super::PresentationFeedback,
        /// The newest drawable that reached the screen.
        on_screen: AtomicU64,
        /// Wake the main run loop when a drawable reaches the screen.
        watch: AtomicBool,
    }

    impl Log {
        /// A log without the trace queue. Feedback stays bounded even when nobody reads it.
        pub(crate) fn untraced() -> Self { Log { untraced: true, ..Default::default() } }

        /// The drawable the last `nextDrawable` returned, once.
        pub(crate) fn acquired(&self) -> Option<u64> {
            Some(self.acquired.swap(0, Ordering::AcqRel)).filter(|seq| *seq != 0)
        }

        /// The drawable the last `nextDrawable` returned (0 before the first).
        pub(crate) fn last_acquired(&self) -> u64 { self.next.load(Ordering::Acquire) }

        /// The newest drawable Metal reported on screen (0 before the first).
        pub(crate) fn on_screen(&self) -> u64 { self.on_screen.load(Ordering::Acquire) }

        /// While on, each drawable that reaches the screen wakes the main run loop.
        pub(crate) fn watch(&self, on: bool) { self.watch.store(on, Ordering::Release); }

        fn record_presented(&self, seq: u64, at: f64) {
            let valid = at.is_finite() && at > 0.0;
            static TRACE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("RUNE_TRACE_GPU").is_some());
            if *TRACE { eprintln!("[rune-shown] seq={seq} at={at:.6}"); }
            if !self.untraced { self.presented.lock().unwrap().push((seq, if valid { at } else { 0.0 })); }
            if valid {
                // SAFETY: CACurrentMediaTime has no preconditions. Translate the actual
                // host timestamp, not the time this asynchronous callback arrived.
                self.feedback.record(instant_at(Instant::now(), unsafe { CACurrentMediaTime() }, at));
                self.on_screen.fetch_max(seq, Ordering::AcqRel);
                if self.watch.load(Ordering::Acquire) { wake_main(); }
            }
        }
    }

    impl Source for Log {
        fn take(&self, out: &mut Vec<Presented>) {
            let mut presented = self.presented.lock().unwrap();
            if presented.is_empty() { return; }
            // SAFETY: CACurrentMediaTime has no preconditions.
            let (now, media_now) = (Instant::now(), unsafe { CACurrentMediaTime() });
            out.extend(presented.drain(..).map(|(seq, at)| (seq, (at > 0.0).then(|| instant_at(now, media_now, at)))));
        }
    }

    /// The instant of host time `media` (CACurrentMediaTime seconds), given both clocks now.
    pub(crate) fn instant_at(now: Instant, media_now: f64, media: f64) -> Instant {
        let delta = Duration::from_secs_f64((media - media_now).abs());
        if media >= media_now { now + delta } else { now.checked_sub(delta).unwrap_or(now) }
    }

    /// A hooked layer: its address, original class and log.
    struct Hooked { addr: usize, base: &'static AnyClass, log: Arc<Log> }

    static LAYERS: Mutex<Vec<Hooked>> = Mutex::new(Vec::new());

    unsafe extern "C-unwind" fn next_drawable(this: &AnyObject, _: Sel) -> *mut AnyObject {
        let addr = this as *const AnyObject as usize;
        let hooked = LAYERS.lock().unwrap().iter().find(|h| h.addr == addr).map(|h| (h.base, h.log.clone()));
        let base = match hooked {
            Some((base, _)) => base,
            // The hook dropped while another subclass (key-value observing) sat on top of ours,
            // so the class stayed. Skip every hooked class, or `super` lands back here.
            None => original_class(this.class()),
        };
        // SAFETY: `base` is the layer's original CAMetalLayer class; the result is +0.
        let drawable: *mut AnyObject = unsafe { msg_send![super(this, base), nextDrawable] };
        if let (Some((_, log)), Some(raw)) = (hooked, NonNull::new(drawable)) {
            let seq = log.next.fetch_add(1, Ordering::Relaxed) + 1;
            log.acquired.store(seq, Ordering::Release);
            let handler = block2_metal::RcBlock::new(move |d: NonNull<ProtocolObject<dyn MTLDrawable>>| {
                // SAFETY: Metal passes the presented drawable for the handler's duration.
                let at = unsafe { d.as_ref() }.presentedTime();
                log.record_presented(seq, at);
            });
            // SAFETY: nextDrawable returns a CAMetalDrawable, which conforms to MTLDrawable;
            // Metal copies the block.
            unsafe {
                let drawable = raw.cast::<ProtocolObject<dyn MTLDrawable>>().as_ref();
                drawable.addPresentedHandler(block2_metal::RcBlock::as_ptr(&handler));
            }
        }
        drawable
    }

    const HOOKED_PREFIX: &str = "RunePresentTimed_";

    /// The superclass of the hooked class nearest the root in `class`'s chain.
    fn original_class(class: &'static AnyClass) -> &'static AnyClass {
        let mut hooked = None;
        let mut next = Some(class);
        while let Some(class) = next {
            if class.name().to_bytes().starts_with(HOOKED_PREFIX.as_bytes()) { hooked = Some(class); }
            next = class.superclass();
        }
        hooked.and_then(AnyClass::superclass).expect("next_drawable only runs on a hooked class")
    }

    /// The hooked subclass of `base`, made once per class.
    fn hooked_class(base: &AnyClass) -> Option<&'static AnyClass> {
        static CLASSES: Mutex<Vec<(usize, &'static AnyClass)>> = Mutex::new(Vec::new());
        let mut classes = CLASSES.lock().unwrap();
        let key = base as *const AnyClass as usize;
        if let Some((_, class)) = classes.iter().find(|(k, _)| *k == key) { return Some(class); }
        let name = CString::new(format!("{HOOKED_PREFIX}{}", base.name().to_str().ok()?)).ok()?;
        let class = match ClassBuilder::new(&name, base) {
            Some(mut builder) => {
                // SAFETY: the signature matches -[CAMetalLayer nextDrawable].
                unsafe { builder.add_method(sel!(nextDrawable), next_drawable as unsafe extern "C-unwind" fn(_, _) -> _); }
                builder.register()
            }
            None => AnyClass::get(&name)?,
        };
        classes.push((key, class));
        Some(class)
    }

    /// A surface's layer, reporting present times to a [`Log`] until dropped.
    pub(crate) struct LayerHook {
        layer: Retained<AnyObject>,
        base: &'static AnyClass,
        class: &'static AnyClass,
    }

    impl LayerHook {
        /// `None` when the surface isn't a plain CAMetalLayer on Metal.
        pub(crate) fn attach(surface: &wgpu::Surface<'_>, log: Arc<Log>) -> Option<Self> {
            // SAFETY: the layer is only retained and has its class swapped for a subclass of its
            // class without state; wgpu keeps using it as a CAMetalLayer.
            // Under wgpu's lock until the class is swapped, so wgpu doesn't use the layer in between.
            crate::gpu::with_layer(surface, |layer| Self::hook(layer.retain(), log))?
        }

        fn hook(layer: Retained<AnyObject>, log: Arc<Log>) -> Option<Self> {
            let metal = AnyClass::get(c"CAMetalLayer")?;
            let base = layer.class();
            // SAFETY: isKindOfClass: takes any class.
            let is_metal: bool = unsafe { msg_send![&*layer, isKindOfClass: metal] };
            if !is_metal { return None; }
            let class = hooked_class(base)?;
            LAYERS.lock().unwrap().push(Hooked { addr: Retained::as_ptr(&layer) as usize, base, log });
            // SAFETY: the subclass adds no ivars, so the instance layout is unchanged.
            unsafe { AnyObject::set_class(&layer, class) };
            Some(LayerHook { layer, base, class })
        }
    }

    impl Drop for LayerHook {
        fn drop(&mut self) {
            // Leave a class someone else swapped in since (key-value observing) alone.
            if std::ptr::eq(self.layer.class(), self.class) {
                // SAFETY: restores the original class; layouts are identical.
                unsafe { AnyObject::set_class(&self.layer, self.base) };
            }
            let addr = Retained::as_ptr(&self.layer) as usize;
            LAYERS.lock().unwrap().retain(|h| h.addr != addr);
        }
    }

    #[cfg(test)]
    #[test]
    fn untraced_feedback_uses_presented_time_and_does_not_enable_wakes() {
        let log = Log::untraced();
        for (i, at) in [0.0, -1.0, f64::NAN, f64::INFINITY].into_iter().enumerate() {
            log.record_presented(i as u64 + 1, at);
        }
        assert!(log.feedback.timestamps().is_empty());
        assert_eq!(log.on_screen(), 0);
        // SAFETY: CACurrentMediaTime has no preconditions.
        let host_now = unsafe { CACurrentMediaTime() };
        // Back-to-back callbacks reporting presentations a quarter-second apart.
        log.record_presented(6, host_now - 0.25);
        log.record_presented(5, host_now - 0.5);
        let times = log.feedback.timestamps();
        assert_eq!(times.len(), 2);
        assert!((times[1].duration_since(times[0]).as_secs_f64() - 0.25).abs() < 0.005);
        assert_eq!(log.on_screen(), 6);
        assert!(!log.watch.load(Ordering::Acquire));
        let mut trace = Vec::new();
        log.take(&mut trace);
        assert!(trace.is_empty());
        assert_eq!(log.feedback.timestamps(), times);
    }

    #[cfg(test)]
    #[test]
    fn host_time_maps_onto_instants() {
        let now = Instant::now();
        assert_eq!(instant_at(now, 100.0, 100.008), now + Duration::from_millis(8));
        assert_eq!(instant_at(now, 100.0, 99.992), now - Duration::from_millis(8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn feedback_is_bounded_ordered_window_local_and_non_destructive() {
        let feedback = PresentationFeedback::default();
        let reader = feedback.clone();
        let other_window = PresentationFeedback::default();
        let start = Instant::now();
        let writer = std::thread::spawn(move || {
            // A delayed callback can arrive behind newer presentations.
            for i in (0..300).rev() { feedback.record(start + Duration::from_millis(i)); }
        });
        writer.join().unwrap();
        let expected: Vec<_> = (44..300).map(|i| start + Duration::from_millis(i)).collect();
        assert_eq!(reader.timestamps(), expected);
        assert_eq!(reader.timestamps(), expected);
        assert!(other_window.timestamps().is_empty());
    }

    #[derive(Default)]
    struct Fake(Mutex<Vec<Presented>>);
    impl Source for Fake {
        fn take(&self, out: &mut Vec<Presented>) { out.append(&mut self.0.lock().unwrap()); }
    }

    fn written(out: &[u8]) -> Vec<serde_json::Value> {
        String::from_utf8_lossy(out).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn lines_wait_for_their_present_time_and_keep_their_order() {
        let start = Instant::now();
        let ms = |n: u64| start + Duration::from_micros(n);
        let fake = Arc::new(Fake::default());
        let mut presents = Presents::new(fake.clone(), start);
        let mut out = Vec::new();
        presents.push(r#"{"t":1"#.into(), Some(1), false, &mut out);
        presents.push(r#"{"t":2"#.into(), Some(2), true, &mut out);
        assert!(out.is_empty(), "no line before its present time");
        // Frame 2's present arrives first; frame 1 still holds it back.
        fake.0.lock().unwrap().push((2, Some(ms(33_333))));
        presents.push(r#"{"t":3"#.into(), None, true, &mut out);
        assert!(out.is_empty() && presents.waiting());
        fake.0.lock().unwrap().extend([(1, Some(ms(25_000)))]);
        presents.push(r#"{"t":4"#.into(), Some(3), true, &mut out);
        let lines = written(&out);
        assert_eq!(lines.len(), 3, "frames 1–3 complete in order: {lines:?}");
        assert_eq!(lines[0]["present_ms"], 25.0);
        assert!(lines[0].get("present_dt_ms").is_none(), "the first present has no interval");
        assert_eq!(lines[1]["present_ms"], 33.333);
        assert_eq!(lines[1]["present_dt_ms"], 8.333);
        assert!(lines[2].get("present_ms").is_none(), "a frame without a drawable has no present field");
        fake.0.lock().unwrap().push((3, None));
        presents.flush(&mut out);
        let lines = written(&out);
        assert_eq!(lines[3]["t"], 4);
        assert!(lines[3]["present_ms"].is_null(), "a skipped drawable is null");
    }

    #[test]
    fn idle_gaps_and_lost_presents_do_not_count_as_intervals() {
        let start = Instant::now();
        let fake = Arc::new(Fake::default());
        let mut presents = Presents::new(fake.clone(), start);
        let mut out = Vec::new();
        presents.push(r#"{"t":0"#.into(), Some(1), false, &mut out);
        fake.0.lock().unwrap().push((1, Some(start)));
        // After idle: presented 500 ms later, no interval.
        presents.push(r#"{"t":500"#.into(), Some(2), false, &mut out);
        fake.0.lock().unwrap().push((2, Some(start + Duration::from_millis(500))));
        // Never reported: written without a present after WAIT_FRAMES later frames.
        presents.push(r#"{"t":508"#.into(), Some(3), true, &mut out);
        for k in 0..WAIT_FRAMES { presents.push(format!(r#"{{"t":{}"#, 600 + k), None, true, &mut out); }
        let lines = written(&out);
        assert_eq!(lines.len(), 3 + WAIT_FRAMES as usize);
        assert!(lines[1].get("present_dt_ms").is_none(), "an idle gap is not cadence");
        assert!(lines[2].get("present_ms").is_none(), "a present that never came is left out, not faked");
    }

    /// Frames `(seq, continuous, present ms)` around a gap; `None` present: never reported.
    /// Returns the last line, which presented 508 ms in, right after the frame before it.
    fn after_gap(frames: &[(Option<u64>, bool, Option<Option<u64>>)]) -> serde_json::Value {
        let start = Instant::now();
        let fake = Arc::new(Fake::default());
        let mut presents = Presents::new(fake.clone(), start);
        let mut out = Vec::new();
        presents.push(r#"{"t":0"#.into(), Some(100), true, &mut out);
        fake.0.lock().unwrap().push((100, Some(start)));
        for (k, &(seq, continuous, present)) in frames.iter().enumerate() {
            presents.push(format!(r#"{{"t":{}"#, k + 1), seq, continuous, &mut out);
            if let (Some(seq), Some(at)) = (seq, present) {
                fake.0.lock().unwrap().push((seq, at.map(|ms| start + Duration::from_millis(ms))));
            }
        }
        presents.push(r#"{"t":"last""#.into(), Some(200), true, &mut out);
        fake.0.lock().unwrap().push((200, Some(start + Duration::from_millis(508))));
        presents.flush(&mut out);
        let last = written(&out).pop().unwrap();
        assert_eq!((last["t"].as_str(), last["present_ms"].as_f64()), (Some("last"), Some(508.0)));
        last
    }

    #[test]
    fn a_skipped_first_frame_after_idle_starts_no_interval() {
        let last = after_gap(&[(Some(1), false, Some(None))]);
        assert!(last.get("present_dt_ms").is_none(), "the idle gap is not cadence: {last}");
    }

    #[test]
    fn a_frame_without_a_drawable_breaks_the_interval() {
        let last = after_gap(&[(None, true, None)]);
        assert!(last.get("present_dt_ms").is_none(), "{last}");
    }

    #[test]
    fn a_present_that_never_came_breaks_the_interval() {
        let last = after_gap(&[(Some(1), true, None)]);
        assert!(last.get("present_dt_ms").is_none(), "{last}");
        // The interval still spans two presented frames drawn back to back.
        assert_eq!(after_gap(&[(Some(1), true, Some(Some(500)))])["present_dt_ms"], 8.0);
    }

    const HEADER: &str = r#"{"type":"header","launch":"rune","width":480,"height":120,"hz":120.0,"scale":2,"clock":"wall"}"#;

    #[test]
    fn summary_counts_bunched_and_missed_intervals_and_lag() {
        let mut trace = vec![HEADER.to_owned()];
        // target, present, interval
        let frames: [(f64, Option<f64>, Option<f64>); 7] = [
            (8.0, Some(25.0), None),
            (16.4, Some(33.3), Some(8.3)),
            (24.7, Some(33.8), Some(0.5)), // bunched
            (33.0, Some(34.6), Some(0.8)), // bunched
            (41.4, Some(58.3), Some(23.7)), // missed (> 12.5 ms)
            (49.7, None, None), // skipped
            (58.0, Some(66.7), Some(8.4)),
        ];
        for (k, (target, present, dt)) in frames.into_iter().enumerate() {
            let mut line = serde_json::json!({ "t": k, "n": 1, "target_ms": target });
            line["present_ms"] = present.into();
            if let Some(dt) = dt { line["present_dt_ms"] = dt.into(); }
            trace.push(line.to_string());
        }
        let s = Summary::of_trace(&trace.join("\n")).unwrap();
        assert_eq!((s.frames, s.presented, s.skipped, s.bunched, s.missed), (7, 6, 1, 2, 1));
        let i = s.interval.unwrap();
        assert_eq!((i.p50, i.p95, i.max), (8.3, 23.7, 23.7));
        let lag = s.lag.unwrap();
        assert!((lag.p50 - 9.1).abs() < 1e-9 && (lag.max - 17.0).abs() < 1e-9, "{lag:?}");
        assert!(s.to_string().contains("2 bunched (<2 ms), 1 missed (>12.50 ms)"), "{s}");
    }

    #[test]
    fn traces_without_present_times_have_no_summary() {
        let old = [HEADER, r#"{"t":0,"n":1,"quality":{"cpu_ms":0.2}}"#, r#"{"t":8.33,"n":1,"e":[[1,0,0,10,10,1,0,0]]}"#].join("\n");
        assert_eq!(Summary::of_trace(&old), None);
        // A trace with targets but no present source (another platform) has none either.
        let targets = [HEADER, r#"{"t":0,"n":1,"target_ms":8.0}"#].join("\n");
        assert_eq!(Summary::of_trace(&targets), None);
        assert_eq!(Summary::of_trace(""), None);
    }

    #[test]
    fn spread_uses_nearest_rank() {
        let s = Spread::of((1..=20).map(f64::from).collect()).unwrap();
        assert_eq!((s.p50, s.p95, s.max), (10.0, 19.0, 20.0));
        assert_eq!(Spread::of(vec![4.0]), Some(Spread { p50: 4.0, p95: 4.0, max: 4.0 }));
        assert_eq!(Spread::of(Vec::new()), None);
    }
}
