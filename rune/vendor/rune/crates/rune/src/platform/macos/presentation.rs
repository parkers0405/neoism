//! Native panel presentation: prewarm, sharing, and generation-fenced fades.

use std::{cell::Cell, rc::Rc, time::Duration};
use objc2::msg_send;
use super::{Panel, set_sharing};
use super::presence::Presence;

pub(crate) struct Prewarm {
    warming: Rc<Cell<bool>>,
    presence: Rc<Cell<Presence>>,
    panel: objc2::rc::Retained<objc2_app_kit::NSPanel>,
}
impl Drop for Prewarm {
    fn drop(&mut self) {
        self.warming.set(false);
        if !self.presence.get().ordered() { self.panel.orderOut(None); }
    }
}

impl Panel {
    /// Reveals the panel, fading its alpha in over `fade` (instantly when zero).
    pub fn show(&self, fade: Duration) {
        self.keep_view();
        self.visible.set(true);
        self.refresh_monitors();
        self.pointer.observe(&self.panel, self.wake.as_ref());
        // Excluded before any of it can be seen: before it orders in or its alpha rises.
        let generation = self.update_presence(|p| p.show());
        self.rejoin_spaces();
        if !self.panel.isVisible() {
            // SAFETY: AppKit calls on the main thread.
            unsafe {
                if !fade.is_zero() { self.panel.setAlphaValue(0.0); }
                self.panel.orderFrontRegardless();
            }
        }
        self.fade_to(1.0, fade, generation);
    }

    /// Puts a panel that joins every Space back on all of them before it orders in. After the
    /// app's activation policy changes (its Dock icon shown or hidden), the window server can
    /// tie such a panel to one Space while AppKit still reports the old mask, and setting the
    /// same mask again changes nothing. Clearing it and setting it back makes the window
    /// server apply it again. A panel still shown on another Space orders out first, so `show`
    /// orders it back in on this one.
    fn rejoin_spaces(&self) {
        use objc2_app_kit::NSWindowCollectionBehavior as Behavior;
        // SAFETY: AppKit calls on the main thread.
        unsafe {
            let behavior = self.panel.collectionBehavior();
            if !behavior.contains(Behavior::CanJoinAllSpaces) { return; }
            if self.panel.isVisible() {
                if self.panel.isOnActiveSpace() { return; }
                self.panel.orderOut(None);
            }
            self.panel.setCollectionBehavior(Behavior::Default);
            self.panel.setCollectionBehavior(behavior);
        }
    }

    /// Changes the panel's presence and applies the capture exclusion it now calls for.
    fn update_presence<R>(&self, change: impl FnOnce(&mut Presence) -> R) -> R {
        let mut presence = self.presence.get();
        let result = change(&mut presence);
        self.presence.set(presence);
        set_sharing(&self.panel, presence.excluded());
        result
    }

    /// Animates the panel's alpha to `alpha` over `duration`, replacing any fade in flight.
    /// A fade to zero is a hide: when it finishes with nothing after it, the panel is hidden,
    /// orders out before restoring sharing. Prewarm keeps the surface, not window ordering.
    fn fade_to(&self, alpha: f64, duration: Duration, generation: u64) {
        use objc2::runtime::{AnyClass, AnyObject};
        let hiding = alpha == 0.0;
        let fading = !duration.is_zero();
        self.revealing.set(fading);
        // SAFETY: AppKit calls on the main thread; the completion handler runs there too.
        unsafe {
            if let Some(context) = AnyClass::get("NSAnimationContext") {
                let _: () = msg_send![context, beginGrouping];
                let current: *mut AnyObject = msg_send![context, currentContext];
                let _: () = msg_send![current, setDuration: duration.as_secs_f64()];
                if fading {
                    let (panel, presence, flag) = (self.panel.clone(), self.presence.clone(), self.revealing.clone());
                    let done = block2::RcBlock::new(move || {
                        let mut now = presence.get();
                        if !now.fade_done(generation, hiding) { return; }
                        presence.set(now);
                        flag.set(false);
                        if !now.ordered() { panel.orderOut(None); }
                        if hiding { set_sharing(&panel, now.excluded()); }
                    });
                    let _: () = msg_send![current, setCompletionHandler: &*done];
                }
                let animator: *mut AnyObject = msg_send![&*self.panel, animator];
                let _: () = msg_send![animator, setAlphaValue: alpha];
                let _: () = msg_send![context, endGrouping];
            }
            if duration.is_zero() {
                self.panel.setAlphaValue(alpha);
                if !self.presence.get().ordered() { self.panel.orderOut(None); }
            }
        }
    }

    pub fn prewarm(&self) -> Prewarm {
        debug_assert!(self.options.prewarm);
        unsafe { self.panel.setAlphaValue(0.0); }
        self.panel.setIgnoresMouseEvents(true);
        if !self.panel.isVisible() { unsafe { self.panel.orderFrontRegardless() }; }
        self.warming.set(true);
        Prewarm { warming: self.warming.clone(), presence: self.presence.clone(), panel: self.panel.clone() }
    }

    /// Hides the panel, fading its alpha out over `fade` (instantly when zero). Input stops at once.
    pub fn hide(&self, fade: Duration) {
        self.visible.set(false);
        self.revealing.set(false);
        self.pointer_poll.set(None);
        self.monitors.borrow_mut().take();
        self.pointer.captured.set(false);
        self.pointer.pending.set(None);
        self.pointer.last.set(None);
        self.pointer.inside.set(false);
        self.pointer.clicks.borrow_mut().clear();
        self.pointer.release_since.set(None);
        self.pointer.recovered_release.set(None);
        self.set_cursor(None);
        // Clicks and scrolls go to the window below for the whole fade; `show` restores
        // hit-testing through `pointer.observe`.
        self.panel.setIgnoresMouseEvents(true);
        // Shown until the fade-out finishes, so the exclusion holds while it can still be seen.
        let mut presence = self.presence.get();
        let generation = presence.hide(!fade.is_zero());
        self.presence.set(presence);
        self.fade_to(0.0, fade, generation);
        unsafe { self.panel.resignKeyWindow(); }
        // Already ordered out for an immediate hide; a fade keeps exclusion until completion.
        set_sharing(&self.panel, presence.excluded());
    }

    /// `sharingType` none while shown, or read-only so recorders see the panel again. Hidden,
    /// the panel stays read-only and the request applies at its next show.
    pub fn set_capture_exclusion(&self, exclude: bool) -> crate::OverlaySupport {
        self.update_presence(|p| p.request(exclude));
        self.capture_exclusion()
    }
}
