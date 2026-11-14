//! Native application and Dock menus. One App owns the process-wide custom menu at a time.
//! Default AppKit/winit items are preserved. Other native platforms return Unsupported;
//! headless scripts use the same item lookup and action dispatch on every platform.
use std::sync::{Arc, Mutex, Weak};
use crate::{App, AppHandle, Shortcut, Tx, app::Shared};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod default_macos;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod key_equivalent;
#[cfg(test)]
mod key_equivalent_tests;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod standard;
#[cfg(target_os = "macos")]
mod standard_macos;
#[cfg(target_os = "macos")]
pub(crate) use default_macos::{ensure as ensure_default_menu, promote as promote_default_menu};
#[cfg(target_os = "macos")]
pub(crate) use standard_macos::{adopt as adopt_standard_view, install as install_standard_menus};
#[cfg(target_os = "macos")]
pub(crate) use key_equivalent::set_key_equivalent;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Location { App, Named(String), Dock }

#[derive(Clone)]
struct Description {
    id: String,
    title: String,
    location: Location,
    equivalent: Option<Shortcut>,
    keys_first: bool,
    enabled: bool,
    checked: bool,
    separator: bool,
}

/// A custom native item. Defaults to the app menu (after its About item, in the order given),
/// enabled and unchecked, with its title as id.
pub struct MenuItem<S> { description: Description, action: fn(&mut S, &mut Tx) }
impl<S> Clone for MenuItem<S> {
    fn clone(&self) -> Self { Self { description: self.description.clone(), action: self.action } }
}
impl<S> MenuItem<S> {
    pub fn new(title: impl Into<String>, action: fn(&mut S, &mut Tx)) -> Self {
        let title = title.into();
        Self { description: Description { id: title.clone(), title, location: Location::App,
            equivalent: None, keys_first: false, enabled: true, checked: false, separator: false }, action }
    }
    /// A separator line, placed with `menu` or `dock` like an item. It has no id or title,
    /// can't be fired, and needs no unique id.
    pub fn separator() -> Self {
        let mut item = Self::new("", |_, _| {});
        item.description.separator = true;
        item
    }
    pub fn id(mut self, id: impl Into<String>) -> Self { self.description.id = id.into(); self }
    /// Place this item in a named top-level menu. Existing native menus are reused. A menu
    /// Rune makes is titled with the app's `Label::Menu(name)`, so the name stays the same in
    /// every language (`rune::set_labels`).
    pub fn menu(mut self, title: impl Into<String>) -> Self { self.description.location = Location::Named(title.into()); self }
    pub fn dock(mut self) -> Self { self.description.location = Location::Dock; self }
    pub fn key_equivalent(mut self, shortcut: Shortcut) -> Self { self.description.equivalent = Some(shortcut); self }
    /// Let the window's own key handling see this item's key equivalent first. AppKit gives a
    /// menu equivalent to the menu before any view, so by default the item takes the chord
    /// even while a field, a modal or `on_key` wants it. With this, the press goes to the key
    /// window like any other key (the focused element, then modals, then `on_key`), and the
    /// item runs only if no handler takes it. Picking the item in the menu always runs it.
    pub fn keys_first(mut self) -> Self { self.description.keys_first = true; self }
    pub fn enabled(mut self, enabled: bool) -> Self { self.description.enabled = enabled; self }
    pub fn checked(mut self, checked: bool) -> Self { self.description.checked = checked; self }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuError {
    Unsupported,
    NotRunning,
    InUse,
    Invalid(String),
    UnknownItem(String),
    AmbiguousTitle(String),
    Disabled(String),
    Backend(String),
}
impl std::fmt::Display for MenuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("native menus are unsupported on this platform"),
            Self::NotRunning => f.write_str("the menu's app is not running"),
            Self::InUse => f.write_str("another Rune app owns the native menu"),
            Self::Invalid(message) => write!(f, "invalid menu: {message}"),
            Self::UnknownItem(item) => write!(f, "unknown menu item: {item}"),
            Self::AmbiguousTitle(item) => write!(f, "ambiguous menu title: {item}; use its id"),
            Self::Disabled(item) => write!(f, "menu item is disabled: {item}"),
            Self::Backend(message) => write!(f, "native menu: {message}"),
        }
    }
}
impl std::error::Error for MenuError {}

struct State<S> {
    items: Vec<MenuItem<S>>,
    revision: u64,
    running: bool,
    headless: bool,
    errors: Vec<MenuError>,
}
pub(crate) struct Hub<S>(Mutex<State<S>>);
impl<S> Default for Hub<S> {
    fn default() -> Self { Self(Mutex::new(State { items: Vec::new(), revision: 0, running: false, headless: false, errors: Vec::new() })) }
}

fn validate<S>(items: &[MenuItem<S>]) -> Result<(), MenuError> {
    let mut ids = std::collections::HashSet::new();
    for item in items {
        let d = &item.description;
        if matches!(&d.location, Location::Named(title) if title.trim().is_empty()) { return Err(MenuError::Invalid("empty menu title".into())); }
        if d.separator { continue; }
        if d.id.trim().is_empty() || d.title.trim().is_empty() { return Err(MenuError::Invalid("empty id or title".into())); }
        if !ids.insert(&d.id) { return Err(MenuError::Invalid(format!("duplicate id {}", d.id))); }
    }
    Ok(())
}

impl<S: 'static> App<S> {
    /// Add a custom native item at startup. Deferred failures (including Unsupported on
    /// Windows/Linux) are available through AppHandle::take_menu_errors().
    pub fn menu_item(self, item: MenuItem<S>) -> Self {
        let mut state = self.shared.menus.0.lock().unwrap();
        state.items.push(item); state.revision += 1;
        drop(state);
        self
    }
}
impl<S: 'static> AppHandle<S> {
    /// Replace all this App's custom items, preserving native defaults. Validates and queues
    /// the new snapshot; enabled/checked changes reach AppKit on the next event-loop drain.
    /// Native installation failures are retained by take_menu_errors(). On Windows/Linux this
    /// returns Unsupported without changing anything (except inside a headless run). On macOS
    /// the menus and standard items Rune made also take the app's current labels
    /// (`rune::set_labels`), so an app calls this again after its language changes.
    pub fn set_menu(&self, items: impl IntoIterator<Item = MenuItem<S>>) -> Result<(), MenuError> {
        let items: Vec<_> = items.into_iter().collect();
        let mut state = self.shared.menus.0.lock().unwrap();
        if !cfg!(target_os = "macos") && !state.headless { return Err(MenuError::Unsupported); }
        if !state.running { return Err(MenuError::NotRunning); }
        validate(&items)?;
        state.items = items; state.revision += 1;
        drop(state);
        self.shared.wake();
        Ok(())
    }
    pub fn take_menu_errors(&self) -> Vec<MenuError> {
        std::mem::take(&mut self.shared.menus.0.lock().unwrap().errors)
    }
}

fn lookup<S>(state: &State<S>, selector: &str) -> Result<usize, MenuError> {
    if !state.running { return Err(MenuError::NotRunning); }
    let items = state.items.iter().enumerate().filter(|(_, i)| !i.description.separator);
    let index = if let Some((index, _)) = items.clone().find(|(_, i)| i.description.id == selector) { index }
    else {
        let mut matches = items.filter(|(_, i)| i.description.title == selector);
        let index = matches.next().ok_or_else(|| MenuError::UnknownItem(selector.into()))?.0;
        if matches.next().is_some() { return Err(MenuError::AmbiguousTitle(selector.into())); }
        index
    };
    if !state.items[index].description.enabled { return Err(MenuError::Disabled(selector.into())); }
    Ok(index)
}

/// The window host, for a key press it maps to `key`: what to do if no handler takes it, when
/// a menu item (a standard ⌘W or ⌘M, or `MenuItem::keys_first`) is handing Rune that key.
pub(crate) fn take_unhandled(key: Option<&crate::KeyEvent>) -> Option<crate::runtime::Unhandled> {
    #[cfg(target_os = "macos")]
    { key.and_then(standard_macos::take_unhandled) }
    #[cfg(not(target_os = "macos"))]
    { let _ = key; None }
}

pub(crate) fn fire<S: 'static>(shared: &Arc<Shared<S>>, selector: &str) -> Result<(), MenuError> {
    enqueue(shared, selector, None)
}
fn enqueue<S: 'static>(shared: &Arc<Shared<S>>, selector: &str, expected: Option<u64>) -> Result<(), MenuError> {
    let state = shared.menus.0.lock().unwrap();
    if expected.is_some_and(|r| r != state.revision) { return Ok(()); }
    let index = lookup(&state, selector)?;
    let id = state.items[index].description.id.clone();
    let revision = state.revision;
    drop(state);
    let weak = Arc::downgrade(shared);
    AppHandle { shared: shared.clone() }.update(move |s, tx| {
        let Some(shared) = weak.upgrade() else { return };
        let state = shared.menus.0.lock().unwrap();
        if !state.running || state.revision != revision { return; }
        let action = lookup(&state, &id).ok().map(|i| state.items[i].action);
        drop(state);
        if let Some(action) = action { action(s, tx); }
    });
    Ok(())
}

pub(crate) struct Guard<S: 'static> {
    shared: Weak<Shared<S>>,
    revision: Option<u64>,
    #[cfg(target_os = "macos")]
    native: Option<macos::Native>,
}
impl<S: 'static> Guard<S> {
    pub(crate) fn attach(shared: &Arc<Shared<S>>, headless: bool) -> Self {
        let mut state = shared.menus.0.lock().unwrap();
        state.running = true; state.headless = headless;
        if let Err(error) = validate(&state.items) {
            state.items.clear(); state.errors.push(error);
        }
        drop(state);
        let mut guard = Self { shared: Arc::downgrade(shared), revision: None,
            #[cfg(target_os = "macos")] native: None };
        guard.drain();
        guard
    }
    pub(crate) fn drain(&mut self) {
        let Some(shared) = self.shared.upgrade() else { return };
        let state = shared.menus.0.lock().unwrap();
        if self.revision == Some(state.revision) { return; }
        let (items, revision, headless) = (state.items.clone(), state.revision, state.headless);
        drop(state);
        let result = validate(&items).and_then(|()| {
            if headless { return Ok(()); }
            #[cfg(target_os = "macos")]
            {
                if items.is_empty() { self.native = None; return Ok(()); }
                if self.native.is_none() { self.native = Some(macos::Native::new()?); }
                let weak = Arc::downgrade(&shared);
                let sink = Arc::new(move |id: &str| {
                    if let Some(shared) = weak.upgrade()
                        && let Err(error) = enqueue(&shared, id, Some(revision)) { eprintln!("rune: {error}"); }
                });
                self.native.as_mut().unwrap().set(&items.iter().map(|i| i.description.clone()).collect::<Vec<_>>(), sink)
            }
            #[cfg(not(target_os = "macos"))]
            { if items.is_empty() { Ok(()) } else { Err(MenuError::Unsupported) } }
        });
        // The app's labels may have changed with its language since the menus were made.
        #[cfg(target_os = "macos")]
        if !headless && let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            default_macos::relabel(&objc2_app_kit::NSApplication::sharedApplication(mtm));
        }
        self.revision = Some(revision);
        if let Err(error) = result {
            eprintln!("rune: {error}");
            shared.menus.0.lock().unwrap().errors.push(error);
        }
    }
}
impl<S: 'static> Drop for Guard<S> {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            let mut state = shared.menus.0.lock().unwrap();
            state.running = false; state.items.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppSpec, el};
    fn app() -> App<Vec<u32>> { App::new(AppSpec::new("menu", (100.0, 100.0), Vec::new, |_, _| el())) }
    fn item(id: &str) -> MenuItem<Vec<u32>> { MenuItem::new("Open Quick Panel", |s: &mut Vec<u32>, _| s.push(1)).id(id) }
    fn drain(app: &App<Vec<u32>>, s: &mut Vec<u32>) {
        for update in app.shared.take().updates { update(s, &mut Tx::default()); }
    }

    #[test]
    fn title_and_id_dispatch_on_the_state_thread_with_reentrant_menu_updates() {
        let app = app().menu_item(item("quick")).menu_item(MenuItem::new("Dock", |s: &mut Vec<u32>, tx| { s.push(2); tx.quit(); }).dock());
        let _guard = Guard::attach(&app.shared, true);
        let handle = app.handle();
        let shared = app.shared.clone();
        std::thread::spawn(move || fire(&shared, "Open Quick Panel").unwrap()).join().unwrap();
        let mut state = Vec::new();
        assert!(state.is_empty());
        drain(&app, &mut state);
        assert_eq!(state, [1]);
        fire(&app.shared, "quick").unwrap();
        drain(&app, &mut state);
        assert_eq!(state, [1, 1]);
        let h = handle.clone();
        handle.update(move |_, _| h.set_menu([item("replacement").checked(true)]).unwrap());
        drain(&app, &mut state);
        assert_eq!(app.shared.menus.0.lock().unwrap().items[0].description.id, "replacement");
    }

    #[test]
    fn replacement_disable_and_shutdown_cancel_stale_actions() {
        let app = app().menu_item(item("quick"));
        let guard = Guard::attach(&app.shared, true);
        let handle = app.handle();
        fire(&app.shared, "quick").unwrap();
        handle.set_menu([item("quick").enabled(false).checked(true)]).unwrap();
        let mut state = Vec::new(); drain(&app, &mut state);
        assert!(state.is_empty());
        assert_eq!(fire(&app.shared, "quick"), Err(MenuError::Disabled("quick".into())));
        assert!(app.shared.menus.0.lock().unwrap().items[0].description.checked);
        handle.set_menu([item("quick")]).unwrap();
        fire(&app.shared, "quick").unwrap();
        drop(guard); drain(&app, &mut state);
        assert!(state.is_empty());
        assert_eq!(fire(&app.shared, "quick"), Err(MenuError::NotRunning));
    }

    #[test]
    fn validation_is_atomic_and_ambiguous_titles_require_ids() {
        let app = app().menu_item(item("first")).menu_item(item("second").menu("Tools"));
        let _guard = Guard::attach(&app.shared, true);
        assert_eq!(fire(&app.shared, "Open Quick Panel"), Err(MenuError::AmbiguousTitle("Open Quick Panel".into())));
        assert!(fire(&app.shared, "first").is_ok());
        assert!(matches!(app.handle().set_menu([item("duplicate"), item("duplicate")]), Err(MenuError::Invalid(_))));
        assert_eq!(app.shared.menus.0.lock().unwrap().items.len(), 2);
        assert!(matches!(fire(&app.shared, "missing"), Err(MenuError::UnknownItem(_))));
        let invalid = self::app().menu_item(item("same")).menu_item(item("same"));
        let _guard = Guard::attach(&invalid.shared, true);
        assert!(matches!(invalid.handle().take_menu_errors().as_slice(), [MenuError::Invalid(_)]));
        assert!(matches!(fire(&invalid.shared, "same"), Err(MenuError::UnknownItem(_))));
    }

    #[test]
    fn separators_need_no_ids_and_never_fire() {
        let file = |title: &str| MenuItem::new(title, |s: &mut Vec<u32>, _| s.push(3)).menu("File");
        let app = app().menu_item(file("New")).menu_item(MenuItem::separator().menu("File"))
            .menu_item(file("Close")).menu_item(MenuItem::separator().menu("File")).menu_item(MenuItem::separator().dock());
        let _guard = Guard::attach(&app.shared, true);
        assert!(app.handle().take_menu_errors().is_empty());
        assert_eq!(fire(&app.shared, ""), Err(MenuError::UnknownItem(String::new())));
        fire(&app.shared, "Close").unwrap();
        let mut state = Vec::new(); drain(&app, &mut state);
        assert_eq!(state, [3]);
        app.handle().set_menu([file("New"), MenuItem::separator().menu("File"), MenuItem::separator().menu("File")]).unwrap();
        assert!(app.shared.menus.0.lock().unwrap().items[1].description.separator);
        assert!(matches!(app.handle().set_menu([MenuItem::separator().menu(" ")]), Err(MenuError::Invalid(_))));
    }

    #[test]
    fn equivalent_and_non_send_state_remain_builder_compatible() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<AppHandle<std::rc::Rc<()>>>();
        let shortcut: Shortcut = "cmd+shift+k".parse().unwrap();
        let item = item("quick").key_equivalent(shortcut.clone());
        assert_eq!(item.description.equivalent, Some(shortcut));
        assert!(!item.description.keys_first && item.keys_first().description.keys_first);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_native_platform_is_a_noop_but_headless_works() {
        let app = app().menu_item(item("quick"));
        let _guard = Guard::attach(&app.shared, false);
        assert_eq!(app.handle().take_menu_errors(), [MenuError::Unsupported]);
        assert_eq!(app.handle().set_menu([item("other")]), Err(MenuError::Unsupported));
        assert_eq!(app.shared.menus.0.lock().unwrap().items[0].description.id, "quick");
    }
}
