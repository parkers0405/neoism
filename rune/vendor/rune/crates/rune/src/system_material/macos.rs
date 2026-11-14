//! Native material views behind a window's Metal layer, moved in the drawable's transaction.
//!
//! winit's content view is layer-backed and wgpu draws into a CAMetalLayer that is a sublayer
//! of the view's layer. Material views live in a flipped host view added under that layer,
//! which never takes clicks, so winit's view still receives every event. With
//! `presentsWithTransaction` on the Metal layer, wgpu presents the drawable into the current
//! Core Animation transaction; the shell opens one around the present and sets every view's
//! frame, corner radius and alpha inside it, so the WindowServer composites the material, the
//! shadow around it and the content on top from the same commit.

use super::{assign, MaterialDraw, Mode, PresentFlag, SystemMaterial, MAX_NATIVE_MATERIALS};
use rune_motion::NodeId;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_video::rc::Retained;
use objc2_video::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2_video::{msg_send, sel};
use std::{cell::Cell, rc::Rc};

/// The host view: flipped, so frames come straight from Rune's top-left points, and never hit,
/// so clicks fall through to winit's view behind it in the hit-test order.
fn host_class() -> Option<&'static AnyClass> {
    const NAME: &std::ffi::CStr = c"RuneMaterialHost";
    if let Some(class) = AnyClass::get(NAME) { return Some(class); }
    let mut builder = ClassBuilder::new(NAME, AnyClass::get(c"NSView")?)?;
    unsafe extern "C-unwind" fn flipped(_: &AnyObject, _: Sel) -> Bool { Bool::YES }
    unsafe extern "C-unwind" fn hit_test(_: &AnyObject, _: Sel, _: CGPoint) -> *mut AnyObject { std::ptr::null_mut() }
    // SAFETY: the signatures match -[NSView isFlipped] and -[NSView hitTest:].
    unsafe {
        builder.add_method(sel!(isFlipped), flipped as unsafe extern "C-unwind" fn(_, _) -> _);
        builder.add_method(sel!(hitTest:), hit_test as unsafe extern "C-unwind" fn(_, _, _) -> _);
    }
    Some(builder.register())
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } }
}

struct Slot {
    view: Retained<AnyObject>,
    material: SystemMaterial,
    /// `NSGlassEffectView`, which rounds itself; the effect views round their layer.
    glass: bool,
    /// The element it showed last, which keeps it while it stays mounted.
    owner: Option<NodeId>,
    /// Frame, corner radius and curve last set, and the alpha (`None` once hidden).
    shape: Option<([f64; 4], f64, bool)>,
    shown: Option<f64>,
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    static kCACornerCurveContinuous: &'static AnyObject;
    static kCACornerCurveCircular: &'static AnyObject;
}

/// One window's material views.
pub(crate) struct Materials {
    mode: Mode,
    /// Native views are on: elements skip their fill, and in synced modes the Metal layer
    /// presents with transactions.
    active: bool,
    view: Option<Retained<AnyObject>>,
    host: Option<Retained<AnyObject>>,
    metal: Option<Retained<AnyObject>>,
    flag: Rc<Cell<PresentFlag>>,
    live_end: Option<Retained<AnyObject>>,
    slots: Vec<Slot>,
}

impl Materials {
    pub(crate) fn new() -> Self {
        Materials { mode: Mode::from_env(), active: false, view: None, host: None, metal: None, flag: Rc::default(), live_end: None, slots: Vec::new() }
    }

    /// Whether this frame's elements should leave their interior to native views.
    pub(crate) fn native(&self) -> bool { self.active }

    /// Whether `present` would only present: no material views to move with the drawable, and
    /// the layer presents without transactions, so the present can happen on another thread.
    pub(crate) fn presents_plainly(&self) -> bool {
        !self.active && self.slots.iter().all(|s| s.shown.is_none()) && !self.flag.get().written()
    }

    fn update_flag(&self, f: impl FnOnce(&mut PresentFlag) -> Option<bool>) -> Option<bool> {
        let mut flag = self.flag.get();
        let out = f(&mut flag);
        self.flag.set(flag);
        out
    }

    /// A window frame animation started or stopped: it presents with transactions while it
    /// moves, and native materials keep them on after it lands.
    pub(crate) fn set_moving(&mut self, surface: &wgpu::Surface<'_>, on: bool) {
        if let Some(on) = self.update_flag(|f| f.moving(on)) { crate::platform::macos::present_with_transaction(surface, on); }
    }

    /// The user started or stopped dragging the window's edge: each frame of the drag presents
    /// in the transaction that resizes the window, and `NSWindowDidEndLiveResizeNotification`
    /// turns it off as soon as they let go even if the window is idle afterwards.
    pub(crate) fn set_live(&mut self, window: &winit::window::Window, surface: &wgpu::Surface<'_>, on: bool) {
        if on && self.live_end.is_none()
            && let (Some(win), Some(metal)) = (crate::platform::macos::ns_window(window), metal_layer(surface))
        {
            let win = unsafe { &*objc2::rc::Retained::as_ptr(&win).cast::<AnyObject>() };
            self.live_end = watch_live_end(win, &metal, self.flag.clone());
        }
        if let Some(on) = self.update_flag(|f| f.live(on)) { crate::platform::macos::present_with_transaction(surface, on); }
    }

    /// After a build: switches native views on when a transparent window's frame found material
    /// elements, and off when it found none. True when it switched them on, so the caller
    /// drops this frame's drawable, builds again natively and acquires another: wgpu reads
    /// `presentsWithTransaction` when it acquires a drawable, and this one is already out.
    pub(crate) fn update(&mut self, transparent: bool, found: bool, window: &winit::window::Window, surface: &wgpu::Surface<'_>) -> bool {
        if self.active && !found {
            // `present` hides the views in this frame's transaction; they stay for the next activation.
            self.active = false;
            if let Some(on) = self.update_flag(|f| f.materials(false)) { crate::platform::macos::present_with_transaction(surface, on); }
            return false;
        }
        if self.active || !found || !transparent || self.mode == Mode::Off { return false; }
        let Some(view) = crate::platform::macos::content_view(window) else { return false };
        // SAFETY: winit's content view outlives this retain, which only adds a reference.
        let Some(view) = (unsafe { Retained::retain(objc2::rc::Retained::as_ptr(&view).cast::<AnyObject>().cast_mut()) }) else { return false };
        let Some(metal) = metal_layer(surface) else { return false };
        if self.mode != Mode::Unsynced && let Some(on) = self.update_flag(|f| f.materials(true)) {
            crate::platform::macos::present_with_transaction(surface, on);
        }
        self.view = Some(view);
        self.metal = Some(metal);
        self.active = true;
        true
    }

    /// Presents the frame with the material views matching `draws`: in one transaction with
    /// the drawable when synced, or just before it otherwise.
    pub(crate) fn present(&mut self, draws: &[MaterialDraw], scale: f32, present: impl FnOnce()) {
        if !self.active && self.slots.iter().all(|s| s.shown.is_none()) { return present(); }
        let draws = if self.active { draws } else { &[] };
        let transaction = AnyClass::get(c"CATransaction");
        let synced = self.mode != Mode::Unsynced && transaction.is_some();
        if let Some(t) = transaction {
            // SAFETY: CATransaction class methods on the main thread, balanced below.
            unsafe { let _: () = msg_send![t, begin]; let _: () = msg_send![t, setDisableActions: true]; }
        }
        self.sync(draws, scale);
        if !synced && let Some(t) = transaction {
            // SAFETY: closes the transaction opened above.
            unsafe { let _: () = msg_send![t, commit]; }
        }
        present();
        if synced && let Some(t) = transaction {
            // SAFETY: closes the transaction opened above, with the drawable inside it.
            unsafe { let _: () = msg_send![t, commit]; }
        }
    }

    fn sync(&mut self, draws: &[MaterialDraw], scale: f32) {
        let draws = &draws[..draws.len().min(MAX_NATIVE_MATERIALS)];
        if draws.is_empty() && self.slots.iter().all(|s| s.shown.is_none()) { return; }
        let Some(host) = self.host() else { return };
        let mode = self.mode;
        let wanted: Vec<_> = draws.iter().map(|d| (mode.resolve(d.material), d.id)).collect();
        let views: Vec<_> = self.slots.iter().map(|s| (s.material, s.owner)).collect();
        let mut taken = vec![false; self.slots.len()];
        for (draw, (at, &(material, id))) in draws.iter().zip(assign(&views, &wanted).into_iter().zip(&wanted)) {
            let k = match at {
                Some(k) => k,
                None => {
                    let Some(slot) = make_view(material, &host) else { continue };
                    self.slots.push(slot);
                    taken.push(false);
                    self.slots.len() - 1
                }
            };
            taken[k] = true;
            let slot = &mut self.slots[k];
            slot.owner = Some(id);
            let (frame, radius) = draw.points(scale);
            let shape = (frame, radius, draw.continuous);
            let alpha = f64::from(draw.opacity.clamp(0.0, 1.0));
            let view = &*slot.view;
            // SAFETY: NSView and NSGlassEffectView setters and CALayer properties on the main thread.
            unsafe {
                if slot.shown != Some(alpha) { let _: () = msg_send![view, setAlphaValue: alpha]; }
                if slot.shape != Some(shape) {
                    let _: () = msg_send![view, setFrame: rect(frame[0], frame[1], frame[2], frame[3])];
                    if slot.glass {
                        let _: () = msg_send![view, setCornerRadius: radius];
                    } else {
                        let layer: *mut AnyObject = msg_send![view, layer];
                        if let Some(layer) = layer.as_ref() {
                            let _: () = msg_send![layer, setCornerRadius: radius];
                            let curve: &AnyObject = if draw.continuous { kCACornerCurveContinuous } else { kCACornerCurveCircular };
                            let _: () = msg_send![layer, setCornerCurve: curve];
                        }
                    }
                    let _: () = msg_send![view, layoutSubtreeIfNeeded];
                    let _: () = msg_send![view, displayIfNeeded];
                }
            }
            slot.shape = Some(shape);
            slot.shown = Some(alpha);
        }
        for (slot, _) in self.slots.iter_mut().zip(taken).filter(|(s, taken)| !taken && s.shown.is_some()) {
            // SAFETY: an NSView setter on the main thread. Zero alpha rather than hidden keeps
            // the view's backdrop layers, as at creation.
            unsafe { let _: () = msg_send![&*slot.view, setAlphaValue: 0.0f64]; }
            slot.shown = None;
            slot.owner = None;
        }
        // At most one spare view per material: unmounting a few elements needn't keep their views.
        let mut k = self.slots.len();
        while k > 0 {
            k -= 1;
            let spare = |s: &Slot| s.shown.is_none();
            if spare(&self.slots[k]) && self.slots[..k].iter().any(|s| spare(s) && s.material == self.slots[k].material) {
                // SAFETY: removes a view this struct added to the host.
                unsafe { let _: () = msg_send![&*self.slots.remove(k).view, removeFromSuperview]; }
            }
        }
    }

    /// The host view, added under the Metal layer on first use and kept there.
    fn host(&mut self) -> Option<Retained<AnyObject>> {
        let view = self.view.clone()?;
        if self.host.is_none() {
            // SAFETY: AppKit calls on the main thread; `host` is retained here and added to
            // winit's content view below every other subview.
            unsafe {
                let bounds: CGRect = msg_send![&*view, bounds];
                let alloc: *mut AnyObject = msg_send![host_class()?, alloc];
                let host: *mut AnyObject = msg_send![alloc, initWithFrame: bounds];
                let host = Retained::from_raw(host)?;
                // Width and height sizable.
                let _: () = msg_send![&*host, setAutoresizingMask: 18usize];
                let _: () = msg_send![&*host, setWantsLayer: true];
                let _: () = msg_send![&*view, addSubview: &*host, positioned: -1isize, relativeTo: std::ptr::null::<AnyObject>()];
                self.host = Some(host);
            }
        }
        let host = self.host.clone()?;
        // SAFETY: CALayer reads and an insert within the content view's own layer.
        unsafe {
            let root: *mut AnyObject = msg_send![&*view, layer];
            let host_layer: *mut AnyObject = msg_send![&*host, layer];
            if let (Some(root), Some(host_layer), Some(metal)) = (root.as_ref(), host_layer.as_ref(), self.metal.as_deref()) {
                let layers: *mut AnyObject = msg_send![root, sublayers];
                if let Some(layers) = layers.as_ref() {
                    let at = |layer: &AnyObject| -> usize { msg_send![layers, indexOfObjectIdenticalTo: layer] };
                    if at(metal) == usize::MAX {
                        let _: () = msg_send![root, insertSublayer: metal, above: host_layer];
                    }
                    // AppKit re-sorts the sublayers to its subviews' order whenever it updates
                    // the layer tree, such as when a hidden window is shown, which would leave
                    // the drawn frame under the material until the next frame. Siblings
                    // composite by zPosition first.
                    let _: () = msg_send![metal, setZPosition: 1.0f64];
                }
            }
        }
        Some(host)
    }
}

impl Drop for Materials {
    fn drop(&mut self) {
        if let Some(token) = &self.live_end {
            unwatch_live_end(token);
        }
        if let Some(host) = &self.host {
            // SAFETY: removes the host this struct added; drop runs on the main thread with the window.
            unsafe { let _: () = msg_send![&**host, removeFromSuperview]; }
        }
    }
}

/// Turns `presentsWithTransaction` off when AppKit ends the user's live resize: winit sends
/// no event for `windowDidEndLiveResize:`, so an idle window draws no frame after mouse-up.
pub(super) fn watch_live_end(window: &AnyObject, layer: &Retained<AnyObject>, flag: Rc<Cell<PresentFlag>>) -> Option<Retained<AnyObject>> {
    let center = AnyClass::get(c"NSNotificationCenter")?;
    let weak = objc2_video::rc::Weak::from_retained(layer);
    let block = block2_metal::RcBlock::new(move |_: *mut AnyObject| {
        let mut f = flag.get();
        let change = f.live(false);
        flag.set(f);
        if let (Some(on), Some(layer)) = (change, weak.load()) {
            // SAFETY: a live CAMetalLayer on the main thread.
            unsafe { let _: () = msg_send![&*layer, setPresentsWithTransaction: Bool::new(on)]; }
        }
    });
    let name = objc2_foundation_video::NSString::from_str("NSWindowDidEndLiveResizeNotification");
    // SAFETY: main thread; `Materials::drop` removes the returned observer token.
    unsafe {
        let center: Retained<AnyObject> = msg_send![center, defaultCenter];
        msg_send![&*center, addObserverForName: &*name, object: window,
            queue: std::ptr::null::<AnyObject>(), usingBlock: &*block]
    }
}

pub(super) fn unwatch_live_end(token: &AnyObject) {
    let Some(center) = AnyClass::get(c"NSNotificationCenter") else { return };
    // SAFETY: removes an observer `watch_live_end` added on the main thread.
    unsafe {
        let center: Retained<AnyObject> = msg_send![center, defaultCenter];
        let _: () = msg_send![&*center, removeObserver: token];
    }
}

impl Mode {
    /// The material actually shown: `Glass` falls back to the popover effect view before
    /// macOS 26 and in legacy mode.
    fn resolve(self, material: SystemMaterial) -> SystemMaterial {
        match material {
            SystemMaterial::Glass if self == Mode::Legacy || AnyClass::get(c"NSGlassEffectView").is_none() => SystemMaterial::Popover,
            other => other,
        }
    }
}

/// A transparent material view in `host`.
fn make_view(material: SystemMaterial, host: &AnyObject) -> Option<Slot> {
    let glass = material == SystemMaterial::Glass;
    let class = AnyClass::get(if glass { c"NSGlassEffectView" } else { c"NSVisualEffectView" })?;
    // SAFETY: AppKit initializers and setters on the main thread; the view is retained by its slot.
    unsafe {
        let alloc: *mut AnyObject = msg_send![class, alloc];
        let view: *mut AnyObject = msg_send![alloc, initWithFrame: rect(0.0, 0.0, 0.0, 0.0)];
        let view = Retained::from_raw(view)?;
        let _: () = msg_send![&*view, setWantsLayer: true];
        if glass {
            // NSGlassEffectViewStyleRegular.
            let _: () = msg_send![&*view, setStyle: 0isize];
        } else {
            // NSVisualEffectMaterialPopover, behind window, always active.
            let _: () = msg_send![&*view, setMaterial: 6isize];
            let _: () = msg_send![&*view, setBlendingMode: 0isize];
            let _: () = msg_send![&*view, setState: 1isize];
            let layer: *mut AnyObject = msg_send![&*view, layer];
            if let Some(layer) = layer.as_ref() { let _: () = msg_send![layer, setMasksToBounds: true]; }
        }
        // Built visible at zero alpha: AppKit skips a hidden glass view's backdrop layers until
        // its first visible frame, which then shows unrefracted.
        let _: () = msg_send![&*view, setAlphaValue: 0.0f64];
        let _: () = msg_send![host, addSubview: &*view];
        let _: () = msg_send![&*view, displayIfNeeded];
        Some(Slot { view, material, glass, owner: None, shape: None, shown: None })
    }
}

/// The surface's CAMetalLayer, when it is one on Metal.
fn metal_layer(surface: &wgpu::Surface<'_>) -> Option<Retained<AnyObject>> {
    crate::gpu::with_layer(surface, |layer| objc2_video::Message::retain(layer))
}
