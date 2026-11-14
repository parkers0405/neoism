//! Native pop-up and context menus. A handler asks with [`Tx::popup_menu`]; once the handler's
//! frame is done the window opens the menu, and the callback runs with the picked item's id, or
//! `None` when the menu is dismissed. The answer goes through the same path as
//! [`Runtime::update`], so it commits on the next frame.
//!
//! - macOS: an `NSMenu` popped up in the window's view, overlays included. Rune keeps drawing
//!   while the menu tracks: frames, display-link pacing and animations keep running.
//! - Windows: a `TrackPopupMenuEx` menu owned by the window, light or dark as the window's
//!   content is. It drops down below an element anchor, as Windows menus do, and Rune keeps
//!   drawing while it tracks: frames, wakes, timers and overlays.
//! - Other platforms: [`popup_menu_supported`] is false and the callback gets `None` at once.
//! - Headless runs: the menu stays open in `--tree` (`popupMenu`) until a script's
//!   `{"type": "menu-pick", "id": "dark"}` or `{"type": "menu-cancel"}` answers it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Value, json};
use winit::window::Window;

use crate::runtime::Waker;
use crate::{PointerPress, Runtime, Shortcut, Tx};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as host;
#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "windows")]
use win32 as host;
#[cfg(target_os = "windows")]
pub(crate) use win32::turns;

/// Platforms without native menus: nothing opens, so every request answers `None` at once.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod host {
    use super::{Arc, Rc, Session, Show, Waker, Window};
    pub(super) const SUPPORTED: bool = false;
    pub(super) type Tracking = ();
    pub(super) fn show(_: &Arc<Window>, _: Show, _: Rc<Session>, _: Option<Waker>) -> bool { false }
    pub(super) fn stop(session: &Session, _: bool) { session.tracking.take(); }
    pub(super) fn scheme(_: bool) {}
    pub(super) fn start(_: Waker) {}
}

/// Whether this platform shows native pop-up menus (macOS and Windows). Where it doesn't,
/// [`Tx::popup_menu`] calls back with `None` straight away, so an app can draw its own menu
/// instead.
pub fn popup_menu_supported() -> bool { host::SUPPORTED }

/// The event loop starts. `turn` runs its idle turn (due timers, overlays), for a platform whose
/// menus hold the event loop in a modal loop of their own while they track.
pub(crate) fn start(turn: Waker) { host::start(turn); }

/// One choice in a [`PopupMenu`]: enabled and unchecked unless set.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupItem {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) enabled: bool,
    pub(crate) checked: bool,
    pub(crate) key_equivalent: Option<Shortcut>,
    pub(crate) symbol: Option<String>,
}

impl PopupItem {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self { id: id.into(), title: title.into(), enabled: true, checked: false, key_equivalent: None, symbol: None }
    }
    pub fn enabled(mut self, enabled: bool) -> Self { self.enabled = enabled; self }
    /// A checkmark beside the title, as a picker shows its current value.
    pub fn checked(mut self, checked: bool) -> Self { self.checked = checked; self }
    /// The shortcut shown beside the title. It is a hint: the menu doesn't register it.
    pub fn key_equivalent(mut self, shortcut: Shortcut) -> Self { self.key_equivalent = Some(shortcut); self }
    /// An SF Symbol shown before the title on macOS 11 and later, e.g. `"moon"`. Windows shows
    /// no symbol.
    pub fn symbol(mut self, name: impl Into<String>) -> Self { self.symbol = Some(name.into()); self }

    pub fn id(&self) -> &str { &self.id }
    pub fn title(&self) -> &str { &self.title }
    pub fn is_enabled(&self) -> bool { self.enabled }
    pub fn is_checked(&self) -> bool { self.checked }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Entry {
    Item(PopupItem),
    Separator,
    /// A titled submenu, and whether it opens.
    Submenu(String, PopupMenu, bool),
}

/// The items of a native pop-up or context menu. Ids must be unique across the menu and its
/// submenus.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PopupMenu {
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: Option<String>,
}

impl PopupMenu {
    pub fn new() -> Self { Self::default() }

    /// A picker's menu: one item per `(id, title)`, with `selected` checked and placed over the
    /// anchor, as a SwiftUI menu-style `Picker` opens.
    pub fn picker<I, T>(options: impl IntoIterator<Item = (I, T)>, selected: &str) -> Self
    where I: Into<String>, T: Into<String> {
        let items = options.into_iter().map(|(id, title)| {
            let id = id.into();
            let checked = id == selected;
            PopupItem::new(id, title).checked(checked)
        });
        Self::new().items(items).selected(selected)
    }

    pub fn item(mut self, item: PopupItem) -> Self { self.entries.push(Entry::Item(item)); self }
    pub fn items(mut self, items: impl IntoIterator<Item = PopupItem>) -> Self {
        self.entries.extend(items.into_iter().map(Entry::Item));
        self
    }
    pub fn separator(mut self) -> Self { self.entries.push(Entry::Separator); self }
    pub fn submenu(mut self, title: impl Into<String>, menu: PopupMenu) -> Self {
        self.entries.push(Entry::Submenu(title.into(), menu, true));
        self
    }
    /// A submenu shown greyed out and closed unless nabled.
    pub fn submenu_enabled(mut self, title: impl Into<String>, menu: PopupMenu, enabled: bool) -> Self {
        self.entries.push(Entry::Submenu(title.into(), menu, enabled));
        self
    }
    /// The top-level item that opens over an element anchor, like a pop-up button's current
    /// value. Without one, a menu anchored to an element drops down below it. Over a point
    /// anchor, this item opens under the point instead of the menu's top-left corner.
    pub fn selected(mut self, id: impl Into<String>) -> Self { self.selected = Some(id.into()); self }

    /// The item with this id, in this menu or a submenu.
    pub fn find(&self, id: &str) -> Option<&PopupItem> {
        self.entries.iter().find_map(|entry| match entry {
            Entry::Item(item) if item.id == id => Some(item),
            Entry::Submenu(_, menu, _) => menu.find(id),
            _ => None,
        })
    }

    /// The selected id when it names a top-level item, the only kind AppKit can position.
    pub(crate) fn positioned(&self) -> Option<&str> {
        let selected = self.selected.as_deref()?;
        self.entries.iter().any(|e| matches!(e, Entry::Item(item) if item.id == selected)).then_some(selected)
    }

    fn validate(&self, ids: &mut std::collections::HashSet<String>) -> Result<(), String> {
        for entry in &self.entries {
            match entry {
                Entry::Item(item) => {
                    if item.id.is_empty() { return Err("an item has an empty id".into()); }
                    if !ids.insert(item.id.clone()) { return Err(format!("duplicate item id {:?}", item.id)); }
                }
                Entry::Submenu(_, menu, _) => menu.validate(ids)?,
                Entry::Separator => {}
            }
        }
        Ok(())
    }

    pub(crate) fn to_json(&self) -> Value {
        let entries: Vec<Value> = self.entries.iter().map(|entry| match entry {
            Entry::Item(item) => {
                let mut v = json!({ "id": item.id, "title": item.title, "enabled": item.enabled, "checked": item.checked });
                if let Some(shortcut) = &item.key_equivalent { v["keyEquivalent"] = shortcut.to_string().into(); }
                if let Some(symbol) = &item.symbol { v["symbol"] = symbol.clone().into(); }
                v
            }
            Entry::Separator => json!({ "separator": true }),
            Entry::Submenu(title, menu, true) => json!({ "submenu": title, "menu": menu.to_json() }),
            Entry::Submenu(title, menu, false) => json!({ "submenu": title, "enabled": false, "menu": menu.to_json() }),
        }).collect();
        let mut v = json!({ "items": entries });
        if let Some(selected) = &self.selected { v["selected"] = selected.clone().into(); }
        v
    }
}

/// Where a menu opens: over an element (by key, or a key path like `row-3/more`), or at a
/// point in window points, such as a context menu's [`PointerPress`].
#[derive(Clone, Debug, PartialEq)]
pub enum PopupAnchor {
    Key(String),
    Point(f32, f32),
}

impl From<&str> for PopupAnchor { fn from(key: &str) -> Self { Self::Key(key.into()) } }
impl From<String> for PopupAnchor { fn from(key: String) -> Self { Self::Key(key) } }
impl From<&String> for PopupAnchor { fn from(key: &String) -> Self { Self::Key(key.clone()) } }
impl From<PointerPress> for PopupAnchor { fn from(p: PointerPress) -> Self { Self::Point(p.x, p.y) } }
impl From<(f32, f32)> for PopupAnchor { fn from((x, y): (f32, f32)) -> Self { Self::Point(x, y) } }

/// Why [`Runtime::resolve_popup`] could not answer a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PopupMenuError {
    NotOpen,
    UnknownItem(String),
    Disabled(String),
}

impl std::fmt::Display for PopupMenuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOpen => f.write_str("no pop-up menu is open"),
            Self::UnknownItem(id) => write!(f, "the pop-up menu has no item {id:?}"),
            Self::Disabled(id) => write!(f, "pop-up menu item {id:?} is disabled"),
        }
    }
}
impl std::error::Error for PopupMenuError {}

pub(crate) type PickFn = Box<dyn FnOnce(crate::states::States<'_>, Option<&str>, &mut Tx)>;

pub(crate) struct Request {
    pub menu: PopupMenu,
    pub anchor: PopupAnchor,
    pub on_pick: PickFn,
    /// A text field's own menu, whose picks the runtime acts on.
    pub field: Option<Box<crate::runtime::field_menu::FieldMenu>>,
}

impl Request {
    pub(crate) fn validate(&self) -> Result<(), String> { self.menu.validate(&mut Default::default()) }
}

/// A `Tx`'s menu request. `Tx` is `Clone`; clones share the request and the runtime takes it once.
#[derive(Clone)]
pub(crate) struct Call(Rc<RefCell<Option<Request>>>);

impl Call {
    pub(crate) fn take(&self) -> Option<Request> { self.0.borrow_mut().take() }
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let menu = self.0.borrow();
        f.debug_tuple("PopupMenuCall").field(&menu.as_ref().map(|r| (&r.menu, &r.anchor))).finish()
    }
}

impl Tx {
    /// Open a native pop-up menu once this handler's frame is done. `on_pick` runs, as an
    /// update would, with the picked item's id after the menu closes, or `None` when it's
    /// dismissed or this platform has no native menu ([`popup_menu_supported`]). Exactly one
    /// call per request. A menu asked for while another is open opens after it closes.
    ///
    /// ```ignore
    /// .on_press_tx(|_: &mut Settings, tx: &mut Tx| {
    ///     let menu = PopupMenu::picker([("system", "System"), ("light", "Light"), ("dark", "Dark")], "system");
    ///     tx.popup_menu(menu, "theme-button", |s: &mut Settings, picked, _| {
    ///         if let Some(id) = picked { s.theme = id.into(); }
    ///     });
    /// })
    /// ```
    pub fn popup_menu<S: 'static>(
        &mut self,
        menu: PopupMenu,
        anchor: impl Into<PopupAnchor>,
        on_pick: impl FnOnce(&mut S, Option<&str>, &mut Tx) + 'static,
    ) {
        let on_pick: PickFn = Box::new(move |s, picked, tx| {
            // None once the playground has dropped the stage that asked.
            if let Some(s) = s.cast::<S>() { on_pick(s, picked, tx); }
        });
        let request = Request { menu, anchor: anchor.into(), on_pick, field: None };
        self.popups.push(Call(Rc::new(RefCell::new(Some(request)))));
    }
}

/// What a host opens: the menu, where its positioned item (or its top-left corner) goes, in
/// window points, and the narrowest it may be. `element` is the anchor element's rect, for a
/// host that places menus its platform's way.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Show {
    pub menu: PopupMenu,
    pub at: (f32, f32),
    pub positioned: Option<String>,
    pub min_width: f32,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub element: Option<[f32; 4]>,
}

impl Show {
    /// Over an element with a selected item, that item covers the element's top-left, as a
    /// pop-up button opens; without one, the menu drops down from the element's bottom-left.
    /// Either way it is at least as wide as the element. At a point, the menu (or its selected
    /// item) opens with its top-left corner there.
    pub(crate) fn new(menu: PopupMenu, anchor: &PopupAnchor, rect: [f32; 4]) -> Self {
        let positioned = menu.positioned().map(str::to_owned);
        let [x, y, w, h] = rect;
        let (at, min_width) = match anchor {
            PopupAnchor::Key(_) if positioned.is_some() => ((x, y), w),
            PopupAnchor::Key(_) => ((x, y + h), w),
            PopupAnchor::Point(..) => ((x, y), 0.0),
        };
        let element = matches!(anchor, PopupAnchor::Key(_)).then_some(rect);
        Self { menu, at, positioned, min_width, element }
    }
}

/// Where one native menu is. The presenter holds no session while it's closed; a handed-over
/// menu is `Opening` until the platform puts it up, `Open` while it tracks, and ends
/// `Resolved` with its answer, or `Cancelled` when its window goes away first. A cancelled
/// menu never opens (or stops tracking), and its answer is dropped.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Phase {
    #[default]
    Opening,
    Open,
    Resolved(Option<String>),
    Cancelled,
}

/// One native menu, from the host handing it over until its answer is read.
#[derive(Default)]
pub(crate) struct Session {
    phase: RefCell<Phase>,
    /// The platform's hold on the menu while it tracks, to stop it.
    pub tracking: RefCell<host::Tracking>,
    /// Where the pointer was over the window as the menu closed, in window points, for a
    /// platform that sends no pointer move until the pointer moves again.
    pub pointer: Cell<Option<(f32, f32)>>,
}

impl Session {
    /// `Opening` → `Open`, as the menu comes up. False when it was cancelled first.
    pub(crate) fn begin(&self) -> bool {
        let mut phase = self.phase.borrow_mut();
        if *phase != Phase::Opening { return false; }
        *phase = Phase::Open;
        true
    }

    /// `Opening` or `Open` → `Resolved`: the menu closed with `picked`. False, and the answer
    /// dropped, when it was cancelled first.
    pub(crate) fn resolve(&self, picked: Option<String>) -> bool {
        let mut phase = self.phase.borrow_mut();
        if !matches!(*phase, Phase::Opening | Phase::Open) { return false; }
        *phase = Phase::Resolved(picked);
        true
    }

    /// Any phase → `Cancelled`. True when the menu was tracking and must be stopped.
    pub(crate) fn cancel(&self) -> bool { self.phase.replace(Phase::Cancelled) == Phase::Open }

    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool { *self.phase.borrow() == Phase::Cancelled }

    /// The answer of a `Resolved` menu, once: the session is `Cancelled` after.
    fn take_answer(&self) -> Option<Option<String>> {
        let mut phase = self.phase.borrow_mut();
        match std::mem::replace(&mut *phase, Phase::Cancelled) {
            Phase::Resolved(picked) => Some(picked),
            other => {
                *phase = other;
                None
            }
        }
    }
}

/// One window's native menu: opens what its runtime asks for, and hands the answer back after
/// `wake` brings the host to its inbox.
#[derive(Default)]
pub(crate) struct Presenter {
    session: Option<Rc<Session>>,
    wake: Option<Waker>,
}

impl Presenter {
    pub(crate) fn new(wake: Waker) -> Self { Self { session: None, wake: Some(wake) } }

    pub(crate) fn connect(&mut self, wake: Waker) { self.wake = Some(wake); }

    /// Runs the callback of a menu that closed since the last event. True when one ran. The
    /// pointer the menu released is back where the menu left it, so a click that doesn't move
    /// it first still lands.
    pub(crate) fn answer<S: 'static>(&mut self, rt: &mut Runtime<S>) -> bool {
        let Some(picked) = self.session.as_ref().and_then(|s| s.take_answer()) else { return false };
        let pointer = self.session.take().and_then(|s| s.pointer.get());
        if let Err(error) = rt.resolve_popup(picked.as_deref()) {
            eprintln!("rune: {error}");
            let _ = rt.resolve_popup(None);
        }
        if let Some((x, y)) = pointer && rt.pointer().is_none() { rt.pointer_move(x, y); }
        true
    }

    /// Opens the runtime's next menu over `window`. Without a shown window or native menus the
    /// callback gets `None` at once. With `paints`, a menu anchored to an element waits while
    /// the runtime has changes to commit, so the next frame places it from the new layout.
    /// True when the window should draw: a callback ran, the pointer was released, or a menu
    /// waits for that frame.
    pub(crate) fn open<S: 'static>(&mut self, rt: &mut Runtime<S>, window: Option<&Arc<Window>>, paints: bool) -> bool {
        let mut redraw = false;
        loop {
            if window.is_some() && paints && rt.popup_waits_for_layout() { return true; }
            let Some(show) = rt.take_popup() else { return redraw };
            redraw = true;
            if let (Some(show), Some(window)) = (show, window) {
                // As dark as what the window draws, which may not be the system's appearance.
                host::scheme(rt.backdrop_dark().unwrap_or(rt.env.system_dark));
                let session = Rc::new(Session::default());
                if host::show(window, show, session.clone(), self.wake.clone()) {
                    self.session = Some(session);
                    return true;
                }
            }
            let _ = rt.resolve_popup(None);
        }
    }

    /// The window is going away (hidden, closed or quitting): the open menu stops tracking and
    /// every request still waiting is answered `None`, while its state is alive. An answer the
    /// cancelled menu gives later is dropped.
    pub(crate) fn cancel<S: 'static>(&mut self, rt: &mut Runtime<S>) {
        self.stop();
        rt.cancel_popups();
    }

    fn stop(&mut self) {
        let Some(session) = self.session.take() else { return };
        let tracking = session.cancel();
        host::stop(&session, tracking);
    }
}

impl Drop for Presenter {
    fn drop(&mut self) { self.stop(); }
}

#[cfg(test)]
mod tests;
