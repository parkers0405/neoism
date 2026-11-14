//! What a web app holds of the page while it runs, and gives back when it ends: the event loop
//! (winit allows one at a time), a canvas the page passed in, the body's style and the
//! document title. An app started while the running one is quitting starts once it has gone,
//! so a route can unmount Rune and mount it again in the same tick.
use std::cell::RefCell;

type Quitting = Box<dyn Fn() -> bool>;
type Start = Box<dyn FnOnce() -> Result<(), String>>;

#[derive(Default)]
struct Page {
    /// The running app's "a quit is on its way".
    live: Option<Quitting>,
    /// The app to start once the running one has gone.
    next: Option<(Quitting, Start)>,
    router: Option<std::rc::Rc<dyn Fn(&str) -> bool>>,
}

thread_local! { static PAGE: RefCell<Page> = RefCell::default(); }

const BUSY: &str = "a Rune app is already running on this page; call its AppHandle::quit before starting another";

/// Starts an app now if the page is free, or once the running app (asked to quit) has gone.
/// `quitting` reports whether the new app was itself asked to quit before it started.
pub(super) fn start(quitting: Quitting, start: Start) -> Result<(), String> {
    let now = PAGE.with_borrow_mut(|page| {
        // The app this one would follow: one already waiting, else the running one.
        let ahead = page.next.as_ref().map(|(quitting, _)| quitting).or(page.live.as_ref()).map(|quitting| quitting());
        match ahead {
            None => Ok(Some(start)),
            Some(true) => { page.next = Some((quitting, start)); Ok(None) }
            Some(false) => Err(BUSY.to_owned()),
        }
    })?;
    now.map_or(Ok(()), |start| start())
}

/// The event loop for `quitting`'s app started.
pub(super) fn occupy(quitting: Quitting, router: Option<Box<dyn Fn(&str) -> bool>>) {
    PAGE.with_borrow_mut(|page| { page.live = Some(quitting); page.router = router.map(Into::into); });
}

/// The running app's event loop is ending. A queued app starts after this turn, once winit
/// has let go of its loop.
pub(super) fn vacate() {
    let next = PAGE.with_borrow_mut(|page| { page.live = None; page.router = None; page.next.take() });
    if let Some((_, start)) = next {
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = start() { super::error(&e, None, false, false); }
        });
    }
}

/// The running app's `App::same_tab_router`, which routed `href` when it returns true.
pub(super) fn route(href: &str) -> bool {
    let router = PAGE.with_borrow(|page| page.router.clone());
    router.is_some_and(|route| route(href))
}

/// What Rune changed on the page, to put back when the app ends.
#[derive(Default)]
pub(super) struct Borrowed {
    canvas: Option<(web_sys::HtmlCanvasElement, String, Vec<(&'static str, Option<String>)>)>,
    body: Option<String>,
    title: Option<String>,
}

/// The attributes Rune and winit set on a canvas.
const CANVAS_ATTRIBUTES: [&str; 4] = ["tabindex", "alt", "width", "height"];

impl Borrowed {
    /// Remembers a canvas the page owns, before Rune styles it.
    pub(super) fn canvas(&mut self, canvas: &web_sys::HtmlCanvasElement) {
        let attributes = CANVAS_ATTRIBUTES.iter().map(|&name| (name, canvas.get_attribute(name))).collect();
        self.canvas = Some((canvas.clone(), canvas.style().css_text(), attributes));
    }

    /// Remembers the body's inline style and the title, before Rune takes over the viewport.
    pub(super) fn viewport(&mut self, document: &web_sys::Document) {
        self.body = document.body().map(|body| body.style().css_text());
        self.title = Some(document.title());
    }

    pub(super) fn give_back(self) {
        if let Some((canvas, css, attributes)) = self.canvas {
            canvas.style().set_css_text(&css);
            for (name, value) in attributes {
                let _ = match value { Some(value) => canvas.set_attribute(name, &value), None => canvas.remove_attribute(name) };
            }
        }
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        if let (Some(css), Some(body)) = (self.body, document.body()) { body.style().set_css_text(&css); }
        if let Some(title) = self.title { document.set_title(&title); }
    }
}
