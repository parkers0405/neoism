//! Accessibility on the OS side: each window and overlay gets an AccessKit platform adapter
//! (UI Automation on Windows, NSAccessibility on macOS, AT-SPI on Linux). The adapter's
//! handlers run wherever the OS calls them; they only queue what they heard and wake the host,
//! whose thread then activates the runtime's tree (`runtime/a11y/tree.rs`), runs the actions
//! and sends tree updates. Until a client asks, the runtime builds nothing.
#![cfg_attr(not(all(feature = "accessibility", any(target_os = "windows", target_os = "macos", target_os = "linux"))), allow(dead_code))]

use std::sync::{Arc, Mutex};

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};

use crate::runtime::Runtime;

pub(crate) mod dump;
pub(crate) mod roles;

#[cfg(all(feature = "accessibility", target_os = "macos"))]
mod macos;
#[cfg(all(feature = "accessibility", target_os = "linux"))]
mod unix;
#[cfg(all(feature = "accessibility", target_os = "windows"))]
mod windows;

/// What the adapter's handlers heard since the host last looked.
#[derive(Default)]
pub(crate) struct Inbox {
    activate: bool,
    deactivate: bool,
    actions: Vec<ActionRequest>,
}

/// The queue between the adapter's handlers (any thread) and the host's thread.
pub(crate) struct Shared {
    inbox: Mutex<Inbox>,
    wake: crate::Waker,
}

impl Shared {
    fn push(&self, f: impl FnOnce(&mut Inbox)) {
        if let Ok(mut inbox) = self.inbox.lock() { f(&mut inbox); }
        (self.wake)();
    }
}

struct Activation(Arc<Shared>);
impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        // The host's thread builds the tree on the wake; the adapter answers with a
        // placeholder until then.
        self.0.push(|i| i.activate = true);
        None
    }
}

struct Actions(Arc<Shared>);
impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        self.0.push(|i| i.actions.push(request));
    }
}

#[cfg_attr(not(all(feature = "accessibility", target_os = "linux")), allow(dead_code))]
struct Deactivation(Arc<Shared>);
impl DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {
        self.0.push(|i| { i.deactivate = true; i.activate = false; });
    }
}

/// The platform-free half of a surface's adapter: what its handlers heard, and whether the
/// runtime's tree has been sent through it. A runtime outlives its surfaces (a window closed and
/// opened again gets a new adapter), and its tree only diffs against what the adapter it last
/// fed holds, so a new link drops that tree on first touch: nothing is sent through it until
/// its own client asks, and the update then holds the whole tree. Tests drive one in place of
/// an OS adapter.
pub struct AccessibilityLink {
    shared: Arc<Shared>,
    adopted: bool,
}

impl Default for AccessibilityLink {
    fn default() -> Self { Self::with_wake(Arc::new(|| {})) }
}

impl AccessibilityLink {
    /// A link no OS adapter wakes: tests call [`AccessibilityLink::activate`] themselves.
    pub fn new() -> Self { Self::default() }

    pub(crate) fn with_wake(wake: crate::Waker) -> Self {
        AccessibilityLink { shared: Arc::new(Shared { inbox: Mutex::default(), wake }), adopted: false }
    }

    /// What an assistive client's first request does: the next update holds the whole tree.
    pub fn activate(&self) { self.shared.push(|i| i.activate = true); }

    /// Runs what the link heard on `rt`, then returns the tree's changes for the adapter, as a
    /// host's turn does. `name` names the root when the runtime has no window title.
    pub fn service<S: 'static>(&mut self, rt: &mut Runtime<S>, name: &str) -> Option<TreeUpdate> {
        self.receive(rt, name);
        self.update(rt)
    }

    /// The runtime's tree belonged to an earlier adapter: drop it before this link reads or
    /// writes it.
    fn adopt<S: 'static>(&mut self, rt: &mut Runtime<S>) {
        if !std::mem::replace(&mut self.adopted, true) { rt.a11y_deactivate(); }
    }

    fn take(&self) -> Inbox {
        self.shared.inbox.lock().map(|mut i| std::mem::take(&mut *i)).unwrap_or_default()
    }

    fn receive<S: 'static>(&mut self, rt: &mut Runtime<S>, name: &str) -> bool {
        self.adopt(rt);
        let inbox = self.take();
        if inbox.deactivate { rt.a11y_deactivate(); }
        if inbox.activate { rt.a11y_activate(name); }
        if log() && (inbox.activate || inbox.deactivate) { eprintln!("rune: a11y {name:?}: {}", if inbox.activate { "activated" } else { "deactivated" }); }
        let mut queued = false;
        for request in &inbox.actions {
            let ran = rt.a11y_action(request);
            if log() { eprintln!("rune: a11y {name:?}: {:?} on {:?} {}", request.action, request.target_node, if ran { "queued" } else { "refused" }); }
            queued |= ran;
        }
        queued
    }

    fn update<S: 'static>(&mut self, rt: &mut Runtime<S>) -> Option<TreeUpdate> {
        self.adopt(rt);
        let started = log().then(std::time::Instant::now);
        let update = rt.a11y_update()?;
        if let Some(t) = started { eprintln!("rune: a11y update: {} nodes in {:.0} us", update.nodes.len(), t.elapsed().as_secs_f64() * 1e6); }
        Some(update)
    }
}

/// One surface's platform adapter.
pub(crate) struct Adapter {
    link: AccessibilityLink,
    #[cfg(all(feature = "accessibility", target_os = "windows"))]
    inner: windows::Adapter,
    #[cfg(all(feature = "accessibility", target_os = "macos"))]
    inner: macos::Adapter,
    #[cfg(all(feature = "accessibility", target_os = "linux"))]
    inner: unix::Adapter,
}

impl Adapter {
    /// An adapter for `window`, waking its host with `wake`. `None` where Rune has no adapter
    /// (the `accessibility` feature off, the web, iOS).
    #[allow(unused_variables)]
    pub(crate) fn new(window: &winit::window::Window, wake: crate::Waker) -> Option<Self> {
        let link = AccessibilityLink::with_wake(wake);
        #[cfg(all(feature = "accessibility", target_os = "windows"))]
        return windows::Adapter::new(window, Activation(link.shared.clone()), Actions(link.shared.clone())).map(|inner| Adapter { link, inner });
        #[cfg(all(feature = "accessibility", target_os = "macos"))]
        return macos::Adapter::new(window, Activation(link.shared.clone()), Actions(link.shared.clone())).map(|inner| Adapter { link, inner });
        #[cfg(all(feature = "accessibility", target_os = "linux"))]
        return Self::surface(link);
        #[allow(unreachable_code)]
        None
    }

    /// An adapter for a surface with no winit window (a Linux layer surface), on platforms where
    /// the tree needs no window handle.
    #[allow(unused_variables, dead_code)]
    pub(crate) fn windowless(wake: crate::Waker) -> Option<Self> {
        #[cfg(all(feature = "accessibility", target_os = "linux"))]
        return Self::surface(AccessibilityLink::with_wake(wake));
        #[allow(unreachable_code)]
        None
    }

    #[cfg(all(feature = "accessibility", target_os = "linux"))]
    fn surface(link: AccessibilityLink) -> Option<Self> {
        let shared = &link.shared;
        let inner = unix::Adapter::new(Activation(shared.clone()), Actions(shared.clone()), Deactivation(shared.clone()));
        Some(Adapter { link, inner })
    }

    #[allow(unused_variables)]
    fn send(&mut self, update: TreeUpdate) {
        #[cfg(feature = "accessibility")]
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        self.inner.update(update);
    }

    /// The window gained or lost key focus. Windows tracks it from the window's own messages.
    #[allow(unused_variables)]
    pub(crate) fn focus(&mut self, focused: bool) {
        #[cfg(all(feature = "accessibility", any(target_os = "macos", target_os = "linux")))]
        self.inner.focus(focused);
    }

    /// Where the surface is on screen, outer frame and content, in physical pixels: AT-SPI
    /// places the tree from it.
    #[allow(unused_variables)]
    pub(crate) fn bounds(&mut self, outer: [f64; 4], inner: [f64; 4]) {
        #[cfg(all(feature = "accessibility", target_os = "linux"))]
        self.inner.bounds(outer, inner);
    }

    /// Runs what the adapter heard on `rt`, then sends the tree's changes. Returns whether
    /// actions were queued, which need a frame. `name` names the root when the runtime has no
    /// window title.
    pub(crate) fn service<S: 'static>(&mut self, rt: &mut Runtime<S>, name: &str) -> bool {
        let queued = self.receive(rt, name);
        self.sync(rt);
        queued
    }

    /// Runs what the adapter heard on `rt`: activation, deactivation and actions. True when
    /// an action was queued for the next frame. Windows hosts call it from the wake, not from
    /// `about_to_wait`, where a redraw request would wait for the loop's next wake.
    pub(crate) fn receive<S: 'static>(&mut self, rt: &mut Runtime<S>, name: &str) -> bool {
        self.link.receive(rt, name)
    }

    /// Sends the tree's changes since the last update, when a client listens.
    pub(crate) fn sync<S: 'static>(&mut self, rt: &mut Runtime<S>) {
        if let Some(update) = self.link.update(rt) {
            self.send(update);
            #[cfg(all(feature = "accessibility", target_os = "windows"))]
            self.inner.notify(rt.a11y_changes());
        }
    }
}

/// `RUNE_A11Y_LOG=1`: log activations, actions and updates to stderr.
fn log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_A11Y_LOG").is_some_and(|v| v != "0"))
}

