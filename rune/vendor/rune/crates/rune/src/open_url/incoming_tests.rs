use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{Glue, Inbox};
use crate::el::el;
use crate::runtime::{Cx, Env, Runtime};
use crate::{App, AppSpec};

type Links = Vec<String>;

/// A get-URL handler that records what was asked of it.
#[derive(Default)]
struct Fake {
    elsewhere: bool,
    refuse: bool,
    installs: Cell<usize>,
    removes: Cell<usize>,
}

impl Glue for Fake {
    fn handled_elsewhere(&self) -> bool { self.elsewhere }
    fn install(&self) -> bool { self.installs.set(self.installs.get() + 1); !self.refuse }
    fn remove(&self) { self.removes.set(self.removes.get() + 1); }
}

fn inbox() -> &'static Inbox { Box::leak(Box::new(Inbox::new())) }

fn app() -> App<Links> {
    let spec = AppSpec::new("links", (100.0, 100.0), Links::new, |_: &Links, _: &mut Cx<Links>| el());
    App::new(spec).on_open_url(|links, url, _| links.push(url.to_owned()))
}

fn plain() -> App<Links> {
    App::new(AppSpec::new("plain", (100.0, 100.0), Links::new, |_: &Links, _: &mut Cx<Links>| el()))
}

/// What a window host does as it starts and when woken: run the queued updates.
fn deliver(app: &App<Links>, rt: &mut Runtime<Links>) -> usize {
    let updates = app.shared.take_updates();
    let n = updates.len();
    for f in updates { rt.update_if(f); }
    n
}

fn runtime() -> Runtime<Links> {
    Runtime::new(Links::new(), |_: &Links, _: &mut Cx<Links>| el(), (100.0, 100.0), Env::default())
}

#[test]
fn without_a_hook_nothing_listens_and_the_launch_handler_goes() {
    let (inbox, glue) = (inbox(), Fake::default());
    inbox.prepare(&glue);
    inbox.receive("app://kept".into());
    assert!(inbox.listen(&plain(), &glue).is_none());
    inbox.settle(&glue);
    assert_eq!((glue.installs.get(), glue.removes.get()), (1, 1), "installed for launch, removed after it");
    let late = app();
    inbox.receive("app://dropped".into());
    let _listening = inbox.listen(&late, &glue).unwrap();
    assert!(late.shared.take_updates().is_empty(), "links from before settling are gone");
}

#[test]
fn links_before_the_first_window_arrive_once_in_order_as_it_starts() {
    let (inbox, glue, app) = (inbox(), Fake::default(), app());
    inbox.prepare(&glue);
    inbox.receive("app://one".into());
    inbox.receive("app://two?x=1".into());
    let _listening = inbox.listen(&app, &glue).unwrap();
    inbox.settle(&glue);
    let mut rt = runtime();
    assert!(rt.state.is_empty(), "nothing runs before the host starts");
    assert_eq!(deliver(&app, &mut rt), 2);
    assert_eq!(rt.state, ["app://one", "app://two?x=1"]);
    assert!(rt.needs_commit(), "the transaction commits and redraws");
    assert_eq!(deliver(&app, &mut rt), 0, "delivered once");
    assert_eq!((glue.installs.get(), glue.removes.get()), (1, 0));
}

#[test]
fn a_later_window_with_the_hook_gets_the_launch_link() {
    let (inbox, glue, first, second) = (inbox(), Fake::default(), plain(), app());
    inbox.prepare(&glue);
    inbox.receive("app://launch".into());
    assert!(inbox.listen(&first, &glue).is_none());
    let _listening = inbox.listen(&second, &glue).unwrap();
    inbox.settle(&glue);
    assert_eq!(second.shared.take_updates().len(), 1);
    assert_eq!(glue.removes.get(), 0);
}

#[test]
fn links_while_running_wake_the_loop_and_arrive() {
    let (inbox, glue, app) = (inbox(), Fake::default(), app());
    let _listening = inbox.listen(&app, &glue).unwrap();
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    app.shared.connect(Arc::new(move || { w.fetch_add(1, Ordering::SeqCst); }));
    let before = wakes.load(Ordering::SeqCst);
    inbox.receive("app://later".into());
    assert_eq!(wakes.load(Ordering::SeqCst), before + 1);
    let mut rt = runtime();
    assert_eq!(deliver(&app, &mut rt), 1);
    assert_eq!(rt.state, ["app://later"]);
}

#[test]
fn when_the_listening_window_goes_the_next_one_takes_over() {
    let (inbox, glue, first, second) = (inbox(), Fake::default(), app(), app());
    let listening = inbox.listen(&first, &glue).unwrap();
    let _second = inbox.listen(&second, &glue).unwrap();
    inbox.receive("app://a".into());
    drop(listening);
    inbox.receive("app://b".into());
    assert_eq!(first.shared.take_updates().len(), 1, "a closed window gets nothing more");
    assert_eq!(second.shared.take_updates().len(), 1);
    assert_eq!(glue.installs.get(), 1, "installed once");
}

#[test]
fn an_apps_own_handler_stays_unless_a_window_has_the_hook() {
    let (inbox, glue) = (inbox(), Fake { elsewhere: true, ..Fake::default() });
    inbox.prepare(&glue);
    inbox.receive("app://theirs".into());
    inbox.settle(&glue);
    assert_eq!((glue.installs.get(), glue.removes.get()), (0, 0));
    let app = app();
    let _listening = inbox.listen(&app, &glue).unwrap();
    assert_eq!(glue.installs.get(), 1);
    assert!(app.shared.take_updates().is_empty());
}

#[test]
fn a_handler_that_cant_install_listens_to_nothing() {
    let (inbox, app) = (inbox(), app());
    assert!(inbox.listen(&app, &Fake { refuse: true, ..Fake::default() }).is_none(), "off the main thread, or another platform");
    inbox.receive("app://dropped".into());
    assert!(app.shared.take_updates().is_empty());
    assert!(inbox.listen(&app, &Fake::default()).is_some());
}

#[cfg(target_os = "macos")]
#[test]
fn the_url_is_the_events_direct_object() {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    let code = |s: &[u8; 4]| u32::from_be_bytes(*s);
    // SAFETY: Foundation constructors; the event is built, never sent.
    autoreleasepool(|_| unsafe {
        let event: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), appleEventWithEventClass: code(b"GURL"),
            eventID: code(b"GURL"), targetDescriptor: std::ptr::null_mut::<AnyObject>(), returnID: -1i16, transactionID: 0i32];
        let event = event.as_ref().unwrap();
        assert_eq!(super::mac::url_of(event), None, "no direct object");
        let url = NSString::from_str("myapp://open?id=1");
        let param: *mut AnyObject = msg_send![class!(NSAppleEventDescriptor), descriptorWithString: &*url];
        let _: () = msg_send![event, setParamDescriptor: param, forKeyword: code(b"----")];
        assert_eq!(super::mac::url_of(event).as_deref(), Some("myapp://open?id=1"));
    });
}
