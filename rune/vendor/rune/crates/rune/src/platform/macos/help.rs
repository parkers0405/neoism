//! `El::help` as AppKit's own tooltips. Each help element's visible rect is a tooltip rect on
//! winit's (flipped, point-sized) content view, so AppKit owns the delay, look, placement and
//! hiding, as for SwiftUI's `.help`. Rects are snapped to device pixels and synced one by one:
//! only a region that moved or changed text is removed and added again, so scrolling a list
//! doesn't restart a still toolbar button's tooltip. A tooltip's owner is its text: AppKit
//! shows the owner's `description` when it doesn't answer
//! `view:stringForToolTip:point:userData:`, and an `NSString` describes itself.
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

struct Region {
    rect: [f32; 4],
    text: String,
    tag: isize,
    /// AppKit doesn't retain tooltip owners: each lives here until its rect is removed.
    _owner: Retained<NSString>,
}

#[derive(Default)]
pub(crate) struct NativeHelp {
    regions: Vec<Region>,
    view: Option<Retained<NSView>>,
}

/// `r` (window points, x0 y0 x1 y1) on the device pixel grid at `scale`.
fn snap(r: [f32; 4], scale: f32) -> [f32; 4] {
    r.map(|v| (v * scale).round() / scale)
}

/// Which registered regions `have` stay (true) and which wanted regions `want` they already
/// cover (true); the rest of `have` is removed and the rest of `want` added.
fn diff(have: &[([f32; 4], &str)], want: &[([f32; 4], &str)]) -> (Vec<bool>, Vec<bool>) {
    let mut covered = vec![false; want.len()];
    let stays = have.iter().map(|h| {
        let k = want.iter().enumerate().position(|(k, w)| !covered[k] && w == h);
        if let Some(k) = k { covered[k] = true; }
        k.is_some()
    }).collect();
    (stays, covered)
}

impl NativeHelp {
    /// Registers `regions` (window points, x0 y0 x1 y1) with `window`'s content view, adding and
    /// removing only the ones that changed since the last sync. Main thread.
    pub(crate) fn sync(&mut self, window: &winit::window::Window, regions: &[([f32; 4], String)]) {
        let scale = window.scale_factor() as f32;
        let want: Vec<([f32; 4], &str)> = regions.iter()
            .map(|(r, t)| (snap(*r, scale), t.as_str()))
            .filter(|(r, _)| r[2] > r[0] && r[3] > r[1])
            .collect();
        let have: Vec<([f32; 4], &str)> = self.regions.iter().map(|g| (g.rect, g.text.as_str())).collect();
        let (stays, covered) = diff(&have, &want);
        if stays.iter().all(|&s| s) && covered.iter().all(|&c| c) { return; }
        let Some(view) = crate::platform::macos::content_view(window) else { return };
        let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.regions).into_iter().zip(stays).partition(|(_, s)| *s);
        for (g, _) in gone {
            // SAFETY: an AppKit call on the main thread, before `g` drops the owner its tag names.
            unsafe { let _: () = msg_send![&view, removeToolTip: g.tag]; }
        }
        self.regions = kept.into_iter().map(|(g, _)| g).collect();
        for (k, (r, text)) in want.iter().enumerate() {
            if covered[k] { continue; }
            let owner = NSString::from_str(text);
            let rect = NSRect::new(NSPoint::new(r[0] as f64, r[1] as f64), NSSize::new((r[2] - r[0]) as f64, (r[3] - r[1]) as f64));
            // SAFETY: an AppKit call on the main thread; `regions` keeps the owner alive until
            // its tag is removed.
            let tag: isize = unsafe { msg_send![&view, addToolTipRect: rect, owner: &*owner, userData: std::ptr::null_mut::<std::ffi::c_void>()] };
            self.regions.push(Region { rect: *r, text: (*text).to_owned(), tag, _owner: owner });
        }
        self.view = Some(view);
    }

    fn clear(&mut self) {
        if let Some(view) = self.view.take() {
            // SAFETY: an AppKit call on the main thread, before the owners it names drop.
            unsafe { let _: () = msg_send![&view, removeAllToolTips]; }
        }
        self.regions.clear();
    }
}

impl Drop for NativeHelp {
    fn drop(&mut self) { self.clear(); }
}

#[cfg(test)]
mod tests {
    use super::{diff, snap};

    #[test]
    fn only_changed_regions_are_replaced() {
        let toolbar = ([10.0, 4.0, 30.0, 24.0], "Copy");
        let row = |y: f32| ([0.0, y, 200.0, y + 20.0], "Open");
        // A list scrolled under a still toolbar button: the button's tooltip stays registered.
        let (stays, covered) = diff(&[toolbar, row(40.0)], &[toolbar, row(30.0)]);
        assert_eq!((stays, covered), (vec![true, false], vec![true, false]));
        // Two identical regions each keep one registration.
        let (stays, covered) = diff(&[toolbar, toolbar], &[toolbar]);
        assert_eq!((stays, covered), (vec![true, false], vec![true]));
        assert_eq!(snap([10.3, 4.74, 30.0, 24.2], 2.0), [10.5, 4.5, 30.0, 24.0], "rects snap to device pixels");
    }
}
