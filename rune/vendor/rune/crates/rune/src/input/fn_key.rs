//! The Fn (Globe) key on macOS, for `Key::Modifier`. winit sends no event for it: its
//! flagsChanged handler makes key events only for Shift, Control, Option and Command, and
//! `ModifiersState` has no Fn flag. A local event monitor sees the flagsChanged AppKit sends
//! the key window, and the host feeds it to the runtime as `PhysicalKeyCode::Fn`.

use objc2::rc::Retained;
use objc2::{ClassType, msg_send, msg_send_id};
use objc2_app_kit::{NSEvent, NSView, NSWindow};
use objc2_foundation::NSObject;
use std::{cell::RefCell, rc::Rc};

const FLAGS_CHANGED: usize = 1 << 12;
const KEY_CODE_FN: u16 = 0x3f;
const FLAG_FUNCTION: usize = 1 << 23;

pub(crate) struct FnKey {
    token: Retained<NSObject>,
    /// Fn going down (true) or up, oldest first, since the last `take`.
    changes: Rc<RefCell<Vec<bool>>>,
}

impl FnKey {
    /// Watch Fn in the window showing winit's content view, which is the panel once one
    /// adopts it.
    pub(crate) fn new(window: &winit::window::Window) -> Option<Self> {
        let view: Retained<NSView> = crate::platform::macos::content_view(window)?;
        let changes = Rc::new(RefCell::new(Vec::new()));
        let seen = changes.clone();
        let handler = block2::RcBlock::new(move |event: *mut NSEvent| -> *mut NSEvent {
            // SAFETY: AppKit passes a live flagsChanged event on the main thread.
            unsafe {
                let code: u16 = msg_send![event, keyCode];
                let target: *mut NSWindow = msg_send![event, window];
                if code == KEY_CODE_FN && view.window().is_some_and(|w| Retained::as_ptr(&w).cast_mut() == target) {
                    let flags: usize = msg_send![event, modifierFlags];
                    seen.borrow_mut().push(flags & FLAG_FUNCTION != 0);
                }
            }
            event
        });
        // SAFETY: the handler outlives the monitor, which `Drop` removes.
        let token: Option<Retained<NSObject>> = unsafe {
            msg_send_id![NSEvent::class(), addLocalMonitorForEventsMatchingMask: FLAGS_CHANGED, handler: &*handler]
        };
        Some(Self { token: token?, changes })
    }

    pub(crate) fn take(&self) -> Vec<bool> { std::mem::take(&mut *self.changes.borrow_mut()) }
}

impl Drop for FnKey {
    fn drop(&mut self) {
        // SAFETY: the token came from addLocalMonitorForEventsMatchingMask.
        unsafe { let _: () = msg_send![NSEvent::class(), removeMonitor: &*self.token]; }
    }
}
