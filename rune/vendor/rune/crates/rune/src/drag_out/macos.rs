//! `NSDraggingSource` for [`super::begin`]. The session starts inside winit's `mouseDragged:`
//! (which dispatches `CursorMoved` synchronously), so `NSApp.currentEvent` is the drag event.
//! Pasteboard types follow what Finder and System Settings' privacy lists accept for a file.
use super::{DragImage, DragItem, DragOperation, DragOutStart, DragOutcome, path_text};
use std::cell::{Cell, RefCell};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability, rc::Retained, sel,
    runtime::{AnyObject, NSObjectProtocol, ProtocolObject}};
use objc2_app_kit::{NSApplication, NSDragOperation, NSDraggingContext, NSDraggingFormation, NSDraggingItem,
    NSDraggingSession, NSDraggingSource, NSEvent, NSEventModifierFlags, NSEventType, NSImage,
    NSFilenamesPboardType, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeURL,
    NSPasteboardType, NSView, NSWindow};
use objc2_foundation::{MainThreadMarker, NSArray, NSCopying, NSData, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL};

type Done = Box<dyn FnOnce(DragOutcome)>;
type Muted = Vec<(Retained<NSWindow>, Retained<NSArray<NSPasteboardType>>)>;

struct Ivars {
    done: RefCell<Option<Done>>,
    mask: Cell<NSDragOperation>,
    /// The app's own windows are offered the drag too (`DragItem::within_app`).
    within_app: Cell<bool>,
    /// The app's windows that stop taking drops while this session runs, with the types each
    /// had registered.
    muted: RefCell<Muted>,
}

/// Runs the end callback, which must not unwind into AppKit.
fn report(done: Done, outcome: DragOutcome) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(outcome))).is_err() {
        eprintln!("rune: drag end callback panicked");
    }
}

impl Drop for Ivars {
    /// A session AppKit never ended (say, one started after the button was already up) still
    /// reports once, when the next drag replaces its source.
    fn drop(&mut self) {
        unmute(std::mem::take(self.muted.get_mut()));
        if let Some(done) = self.done.get_mut().take() { report(done, DragOutcome::Cancelled); }
    }
}

declare_class!(
    struct Source;
    unsafe impl ClassType for Source {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "RuneDragOutSource";
    }
    impl DeclaredClass for Source { type Ivars = Ivars; }
    unsafe impl NSObjectProtocol for Source {}
    unsafe impl NSDraggingSource for Source {
        #[method(draggingSession:sourceOperationMaskForDraggingContext:)]
        fn mask(&self, _session: &NSDraggingSession, context: NSDraggingContext) -> NSDragOperation {
            mask_in(self.ivars().mask.get(), context, self.ivars().within_app.get())
        }
        #[method(draggingSession:endedAtPoint:operation:)]
        fn ended(&self, _session: &NSDraggingSession, _point: NSPoint, operation: NSDragOperation) {
            unmute(std::mem::take(&mut *self.ivars().muted.borrow_mut()));
            let done = self.ivars().done.borrow_mut().take();
            if let Some(done) = done { report(done, outcome(operation)); }
        }
        #[method(ignoreModifierKeysForDraggingSession:)]
        fn ignore_modifiers(&self, _session: &NSDraggingSession) -> bool { single(self.ivars().mask.get()) }
    }
);

thread_local! {
    // AppKit's session doesn't document retaining its source; one drag runs at a time, so the
    // newest source is kept until the next drag replaces it.
    static LIVE: RefCell<Option<Retained<Source>>> = const { RefCell::new(None) };
}

fn outcome(operation: NSDragOperation) -> DragOutcome {
    if operation.contains(NSDragOperation::Copy) { DragOutcome::Copied }
    else if operation.intersects(NSDragOperation::Move | NSDragOperation::Delete | NSDragOperation::Generic) { DragOutcome::Moved }
    else if operation.contains(NSDragOperation::Link) { DragOutcome::Linked }
    else { DragOutcome::Cancelled }
}

/// The operations the source offers: what the item allows, ignoring modifier keys.
fn mask(item: &DragItem) -> NSDragOperation {
    [(DragOperation::Copy, NSDragOperation::Copy), (DragOperation::Move, NSDragOperation::Move), (DragOperation::Link, NSDragOperation::Link)]
        .into_iter().filter(|&(op, _)| item.offers(op)).fold(NSDragOperation::None, |m, (_, ns)| m | ns)
}

/// The app's own windows are offered nothing unless the item lets them take it: Rune windows
/// take no drops from their own drags by default.
fn mask_in(mask: NSDragOperation, context: NSDraggingContext, within_app: bool) -> NSDragOperation {
    if context == NSDraggingContext::WithinApplication && !within_app { NSDragOperation::None } else { mask }
}

/// Modifier keys pick among several offered operations (Cmd+Option for a link in Finder); with
/// one on offer they'd only turn the drop into a refusal.
fn single(mask: NSDragOperation) -> bool { mask.bits().count_ones() <= 1 }

/// winit's windows accept any file drag and answer Copy whatever the source offers, ignoring an
/// empty mask, so a drag let go over the app would end Copied without sliding back (and hand
/// the app its own file). The app's drop-taking windows stop taking drops for the session.
fn mute(mtm: MainThreadMarker) -> Muted {
    let winit_types = || NSArray::from_id_slice(&[unsafe { NSFilenamesPboardType }.copy()]);
    // SAFETY: window and delegate calls on the main thread. NSWindow answers
    // `registeredDraggedTypes` like NSView, though only NSView declares it; without it, the
    // types are winit's.
    unsafe {
        NSApplication::sharedApplication(mtm).windows().into_iter()
            .filter(|w| w.delegate().is_some_and(|d| d.respondsToSelector(sel!(draggingEntered:))))
            .filter_map(|w| {
                let types: Retained<NSArray<NSPasteboardType>> = if w.respondsToSelector(sel!(registeredDraggedTypes)) {
                    msg_send_id![&*w, registeredDraggedTypes]
                } else { winit_types() };
                if types.is_empty() { return None; }
                w.unregisterDraggedTypes();
                Some((w, types))
            })
            .collect()
    }
}

/// Gives the windows back exactly the drag types they had.
fn unmute(windows: Muted) {
    // `muted` is only filled on the main thread, and its owner, a main-thread-only source,
    // drops there.
    for (w, types) in windows { w.registerForDraggedTypes(&types); }
}

pub(super) fn begin(window: &winit::window::Window, start: DragOutStart, picture: Option<DragImage>, done: Done) {
    // The previous source drops first (reporting if it never ended, and giving back the drag
    // types it took), so this session's muting isn't undone. Outside the borrow: it reports.
    let previous = LIVE.with(|live| live.take());
    drop(previous);
    let Some(mtm) = MainThreadMarker::new() else { return done(DragOutcome::Cancelled) };
    let Some(view) = crate::platform::macos::content_view(window) else { return done(DragOutcome::Cancelled) };
    let Some(event) = drag_event(mtm, &view) else { return done(DragOutcome::Cancelled) };
    // SAFETY: AppKit calls on the main thread with live objects; the source outlives the
    // session through LIVE.
    unsafe {
        let (image, frame) = image_and_frame(&start, picture.as_ref());
        let items = pasteboard_items(&start);
        if items.is_empty() { return done(DragOutcome::Cancelled); }
        let dragging: Vec<Retained<NSDraggingItem>> = items.iter().enumerate().map(|(i, item)| {
            let d = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), ProtocolObject::from_ref(&**item));
            // Extra files trail a little behind the first, as Finder stacks them.
            let offset = (i.min(4) as f64) * 3.0;
            let f = NSRect::new(NSPoint::new(frame.origin.x + offset, frame.origin.y + offset), frame.size);
            d.setDraggingFrame_contents(f, image.as_deref().map(|i| &**i as &AnyObject));
            d
        }).collect();
        // A drag the app's windows take leaves their drop registration alone.
        let within_app = start.item.drops_within_app();
        let muted = if within_app { Muted::new() } else { mute(mtm) };
        let source = mtm.alloc::<Source>().set_ivars(Ivars { done: RefCell::new(Some(done)), mask: Cell::new(mask(&start.item)), within_app: Cell::new(within_app), muted: RefCell::new(muted) });
        let source: Retained<Source> = msg_send_id![super(source), init];
        LIVE.with(|live| *live.borrow_mut() = Some(source.clone()));
        let session = view.beginDraggingSessionWithItems_event_source(&NSArray::from_vec(dragging), &event, ProtocolObject::from_ref(&*source));
        // A drop nowhere slides the image back over the element.
        session.setAnimatesToStartingPositionsOnCancelOrFail(true);
        session.setDraggingFormation(NSDraggingFormation::None);
    }
}

/// The mouse event the session starts from: the event being dispatched when it's a left-button
/// press or drag, else one synthesised at the pointer (an overlay's polled pointer runs outside
/// event dispatch).
fn drag_event(mtm: MainThreadMarker, view: &NSView) -> Option<Retained<NSEvent>> {
    let current = NSApplication::sharedApplication(mtm).currentEvent();
    // SAFETY: reading event and window properties on the main thread.
    unsafe {
        if let Some(event) = &current {
            if matches!(event.r#type(), NSEventType::LeftMouseDragged | NSEventType::LeftMouseDown) { return current; }
        }
        // With the button already up, AppKit would never end the session.
        if NSEvent::pressedMouseButtons() & 1 == 0 { return None; }
        let window = view.window()?;
        let at = window.mouseLocationOutsideOfEventStream();
        let time = current.as_ref().map_or(0.0, |e| e.timestamp());
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDragged, at, NSEventModifierFlags::empty(), time, window.windowNumber(), None, 0, 1, 1.0)
    }
}

/// The drag image and where it starts, in the view's (flipped, top-left) points.
fn image_and_frame(start: &DragOutStart, picture: Option<&DragImage>) -> (Option<Retained<NSImage>>, NSRect) {
    // SAFETY: NSImage and NSWorkspace calls on the main thread (`begin`'s only caller context).
    unsafe {
        let [x, y, w, h] = start.rect;
        let image = picture.and_then(|p| {
            let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(p.png_data()))?;
            let natural = image.size();
            let [fx, fy, fw, fh] = p.frame(start.bounds, start.press, (natural.width as f32, natural.height as f32));
            // The frame's size scales the picture; setting it on the image too keeps a
            // high-resolution snapshot at its point size.
            image.setSize(NSSize::new(fw as f64, fh as f64));
            Some((image, NSRect::new(NSPoint::new(fx as f64, fy as f64), NSSize::new(fw as f64, fh as f64))))
        });
        if let Some((image, frame)) = image { return (Some(image), frame); }
        // No picture: the first file's Finder icon, else the element's outline only.
        let icon = start.item.paths().first().map(|path| {
            let workspace: Retained<AnyObject> = msg_send_id![objc2::class!(NSWorkspace), sharedWorkspace];
            let icon: Retained<NSImage> = msg_send_id![&workspace, iconForFile: &*NSString::from_str(&path_text(path))];
            icon.setSize(NSSize::new(64.0, 64.0));
            icon
        });
        match icon {
            Some(icon) => (Some(icon), NSRect::new(NSPoint::new(start.press.0 as f64 - 32.0, start.press.1 as f64 - 32.0), NSSize::new(64.0, 64.0))),
            None => (None, NSRect::new(NSPoint::new(x as f64, y as f64), NSSize::new(w.max(1.0) as f64, h.max(1.0) as f64))),
        }
    }
}

/// One pasteboard item per file, the PNG and text riding on the first (or alone without files).
fn pasteboard_items(start: &DragOutStart) -> Vec<Retained<NSPasteboardItem>> {
    // SAFETY: Foundation and AppKit pasteboard calls with live objects; AppKit's type
    // constants are immutable statics.
    unsafe {
        let item = &start.item;
        let mut out = Vec::new();
        for path in item.paths() {
            let text = path_text(path);
            let ns_path = NSString::from_str(&text);
            let url = NSURL::fileURLWithPath(&ns_path);
            let Some(url) = url.absoluteString() else { continue };
            let p = NSPasteboardItem::new();
            p.setString_forType(&url, NSPasteboardTypeFileURL);
            p.setString_forType(&url, NSPasteboardTypeURL);
            if item.plain_text().is_none() { p.setString_forType(&ns_path, NSPasteboardTypeString); }
            out.push(p);
        }
        if item.png_data().is_some() || item.plain_text().is_some() {
            let p = match out.first() { Some(p) => p.clone(), None => { out.push(NSPasteboardItem::new()); out[0].clone() } };
            if let Some(png) = item.png_data() { p.setData_forType(&NSData::with_bytes(png), NSPasteboardTypePNG); }
            if let Some(text) = item.plain_text() { p.setString_forType(&NSString::from_str(text), NSPasteboardTypeString); }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_offers_what_the_item_allows() {
        assert_eq!(mask(&DragItem::new()), NSDragOperation::Copy);
        assert_eq!(mask(&DragItem::new().allow_move()), NSDragOperation::Copy | NSDragOperation::Move);
        assert_eq!(mask(&DragItem::new().operation(DragOperation::Move)), NSDragOperation::Move);
        assert_eq!(mask(&DragItem::new().operation(DragOperation::Link).allow(DragOperation::Copy)), NSDragOperation::Link | NSDragOperation::Copy);
    }

    #[test]
    fn nothing_is_offered_to_the_apps_own_windows() {
        let m = NSDragOperation::Copy | NSDragOperation::Link;
        assert_eq!(mask_in(m, NSDraggingContext::OutsideApplication, false), m);
        assert_eq!(mask_in(m, NSDraggingContext::WithinApplication, false), NSDragOperation::None);
        // `DragItem::within_app` offers the app's own windows the same operations.
        assert_eq!(mask_in(m, NSDraggingContext::WithinApplication, true), m);
        assert_eq!(mask_in(m, NSDraggingContext::OutsideApplication, true), m);
    }

    #[test]
    fn modifier_keys_only_count_when_several_operations_are_offered() {
        assert!(single(mask(&DragItem::new())));
        assert!(!single(mask(&DragItem::new().allow(DragOperation::Link))));
        assert!(!single(mask(&DragItem::new().allow_move())));
    }

    #[test]
    fn operations_map_to_outcomes() {
        assert_eq!(outcome(NSDragOperation::Copy), DragOutcome::Copied);
        assert_eq!(outcome(NSDragOperation::Move), DragOutcome::Moved);
        assert_eq!(outcome(NSDragOperation::Delete), DragOutcome::Moved);
        assert_eq!(outcome(NSDragOperation::Generic), DragOutcome::Moved);
        assert_eq!(outcome(NSDragOperation::Link), DragOutcome::Linked);
        assert_eq!(outcome(NSDragOperation::None), DragOutcome::Cancelled);
    }

    fn ivars(log: &std::rc::Rc<RefCell<Vec<DragOutcome>>>) -> Ivars {
        let log = log.clone();
        Ivars { done: RefCell::new(Some(Box::new(move |o| log.borrow_mut().push(o)))), mask: Cell::new(NSDragOperation::Copy), within_app: Cell::new(false), muted: RefCell::default() }
    }

    #[test]
    fn a_session_that_never_ended_reports_cancelled_once_on_drop() {
        let log = std::rc::Rc::default();
        drop(ivars(&log));
        assert_eq!(*log.borrow(), [DragOutcome::Cancelled]);
    }

    #[test]
    fn an_ended_session_does_not_report_again_on_drop() {
        let log = std::rc::Rc::default();
        let ivars = ivars(&log);
        let done = ivars.done.borrow_mut().take().unwrap();
        report(done, DragOutcome::Copied);
        drop(ivars);
        assert_eq!(*log.borrow(), [DragOutcome::Copied]);
    }

    #[test]
    fn a_panicking_end_callback_does_not_unwind() {
        report(Box::new(|_| panic!("boom")), DragOutcome::Cancelled);
    }
}
