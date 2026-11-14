//! `on_gpu`: when Rune makes a GPU device for a window or overlay, and when a frame first
//! presents on it, for an app that guards its launches against a GPU driver that crashes.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// What `on_gpu`'s hook hears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuEvent {
    /// Rune is about to make a GPU device: for the first window or overlay that draws, or for
    /// one that draws after the last device went with its surfaces.
    Starting,
    /// The first frame since the last `Starting` drew: a window's or overlay's present, or the
    /// pass `warm_gpu` runs on the device it makes.
    Drew,
}

type Hook = Box<dyn Fn(GpuEvent) + Send + Sync>;

static HOOK: OnceLock<Hook> = OnceLock::new();
/// A device was made and nothing has presented on it yet.
static WAITING: AtomicBool = AtomicBool::new(false);

/// Calls `f` on the thread that makes a GPU device (`GpuEvent::Starting`) and on the thread that
/// presents its first frame (`GpuEvent::Drew`). A tray app whose windows and overlays wait
/// hidden makes no device until one shows, so a launch that never draws hears nothing. Set it
/// once, before the app runs; later calls do nothing. Headless renders and `Gpu::headless`
/// don't report.
pub fn on_gpu(f: impl Fn(GpuEvent) + Send + Sync + 'static) {
    let _ = HOOK.set(Box::new(f));
}

pub(crate) fn starting() {
    WAITING.store(true, Ordering::Release);
    if let Some(hook) = HOOK.get() { hook(GpuEvent::Starting); }
}

/// After every present; reports only the first after a `starting`.
pub(crate) fn presented() {
    if WAITING.load(Ordering::Relaxed) && WAITING.swap(false, Ordering::AcqRel) && let Some(hook) = HOOK.get() {
        hook(GpuEvent::Drew);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// One `Drew` per `Starting`, however many frames present.
    #[test]
    fn the_first_present_after_a_device_starts_reports_drew() {
        static HEARD: Mutex<Vec<GpuEvent>> = Mutex::new(Vec::new());
        on_gpu(|event| HEARD.lock().unwrap().push(event));
        presented();
        starting();
        presented();
        presented();
        starting();
        presented();
        // No other unit test makes a surface's device or presents a frame.
        use GpuEvent::*;
        assert_eq!(*HEARD.lock().unwrap(), [Starting, Drew, Starting, Drew]);
    }
}
