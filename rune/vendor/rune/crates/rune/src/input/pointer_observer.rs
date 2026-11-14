//! A small native-pointer mailbox. Global movement outside the canvas wakes the owner only
//! on exit or a followed-display change. No application state or native API runs under its lock.
use std::sync::Mutex;

#[derive(Default)]
struct Sample {
    canvas: [i32; 4], displays: Vec<[i32; 4]>, follow: bool,
    inside: bool, display: Option<usize>, last: Option<(i32, i32)>, pending: Option<(i32, i32)>,
}
pub(crate) struct Observer { sample: Mutex<Sample>, wake: Option<crate::Waker> }
impl Observer {
    pub fn new(wake: Option<crate::Waker>) -> Self { Self { sample: Mutex::default(), wake } }
    pub fn needs_geometry(&self, canvas: [i32; 4], follow: bool) -> bool {
        let s = self.sample.lock().unwrap();
        s.canvas != canvas || s.follow != follow
    }
    pub fn configure(&self, canvas: [i32; 4], displays: Vec<[i32; 4]>, follow: bool) {
        let mut s = self.sample.lock().unwrap();
        if s.canvas != canvas || s.displays != displays || s.follow != follow { s.last = None; }
        s.canvas = canvas; s.displays = displays; s.follow = follow;
    }
    pub fn take(&self) -> Option<(i32, i32)> { self.sample.lock().unwrap().pending.take() }
    pub fn observe(&self, point: (i32, i32)) {
        let wake = {
            let mut s = self.sample.lock().unwrap();
            let contains = |r: &[i32; 4]| point.0 >= r[0] && point.1 >= r[1] && point.0 < r[2] && point.1 < r[3];
            let inside = contains(&s.canvas);
            let display = s.displays.iter().position(contains).or(s.display);
            let changed = s.follow && display != s.display;
            let interested = inside || s.inside || changed;
            s.inside = inside; s.display = display;
            if interested && s.last != Some(point) { s.last = Some(point); s.pending = Some(point); true } else { false }
        };
        if wake && let Some(wake) = &self.wake { wake(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_observation_sleeps_outside_canvas_and_preserves_gap_display() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        let observer = Observer::new(Some(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); })));
        observer.configure([10, 10, 30, 30], vec![[0, 0, 100, 100], [200, 0, 300, 100]], true);
        observer.observe((5, 5)); // Initial display resolution.
        assert_eq!(observer.take(), Some((5, 5)));
        for x in 31..100 { observer.observe((x, 5)); }
        assert!(observer.take().is_none());
        observer.observe((15, 15));
        assert_eq!(observer.take(), Some((15, 15)));
        observer.observe((50, 50)); // One exit.
        assert_eq!(observer.take(), Some((50, 50)));
        observer.observe((150, 50)); // Gap retains the previous display.
        assert!(observer.take().is_none());
        observer.observe((250, 50));
        assert_eq!(observer.take(), Some((250, 50)));
        observer.observe((260, 50));
        assert!(observer.take().is_none());
        assert_eq!(wakes.load(Ordering::Relaxed), 4);
        assert!(observer.needs_geometry([240, 40, 280, 60], true));
        observer.configure([240, 40, 280, 60], vec![[0, 0, 100, 100], [200, 0, 300, 100]], true);
        observer.observe((260, 50));
        assert_eq!(observer.take(), Some((260, 50)), "moving the canvas under a stationary pointer must wake");
    }
}
