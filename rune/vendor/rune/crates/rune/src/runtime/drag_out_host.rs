//! The host's side of drag-outs: starting OS drag sessions and reporting how they ended.

use super::*;

impl<S: 'static> Runtime<S> {
    /// A drag-out the pointer move just delivered asked for. The shell must start the OS
    /// session before returning from that move event.
    pub(crate) fn take_drag_out(&mut self) -> Option<crate::drag_out::DragOutStart> {
        self.drag_out_request.take()
    }

    /// Where the host reports how a drag-out ended; delivered to `on_drag_end` next frame.
    pub(crate) fn drag_end_sink(&self) -> crate::drag_out::DragEnds { self.drag_ends.clone() }
    pub(crate) fn drag_ends_pending(&self) -> bool { !self.drag_ends.borrow().is_empty() }

    /// Whether this host can start OS drags. Without, `drag_out` elements act as if they had
    /// none.
    pub(crate) fn set_os_drag(&mut self, supported: bool) { self.os_drag = supported; }

    pub(super) fn drags_out(&self, n: &Node) -> bool { self.os_drag && n.handlers.get().is_some_and(|h| h.drag_out.is_some()) }
}
