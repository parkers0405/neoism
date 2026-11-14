//! `El::on_visible`: telling an element when it comes on screen and when it leaves, as a web
//! page's `IntersectionObserver` does, so a demo plays when the reader reaches it and a loop
//! stops when they scroll past. It reads where each frame drew the element, after scroll
//! viewports and a web page's own scroll (`App::web_document`).
use super::*;

/// Whether `rect` (`[x, y, w, h]`) counts as on screen in a window `view` points big: half of
/// it shows, or it covers half the window (an element taller than the window).
pub(crate) fn on_screen(rect: [f32; 4], view: (f32, f32)) -> bool {
    let w = (rect[0] + rect[2]).min(view.0) - rect[0].max(0.0);
    let h = (rect[1] + rect[3]).min(view.1) - rect[1].max(0.0);
    if w <= 0.0 || h <= 0.0 { return false; }
    let need = 0.5 * (rect[2] * rect[3]).min(view.0 * view.1);
    w * h >= need
}

impl<S: 'static> Runtime<S> {
    /// Queues `on_visible` for elements that came on screen or left it since the last frame.
    /// True when it queued any.
    pub(super) fn report_visible(&mut self) -> bool {
        if !self.has_drawn_layout && self.visible_fired.is_empty() { return false; }
        let last = std::mem::take(&mut self.visible_fired);
        let mut seen = HashMap::with_capacity_and_hasher(last.len(), Default::default());
        let mut queued = false;
        // A page draws a band that starts above the window and ends below it; what's on screen is
        // the window inside the band, not the band.
        let (top, view) = self.page_window();
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            if n.handlers.get().is_none_or(|h| h.on_visible.is_none()) || self.subtree_exiting(i) { continue; }
            let Some(&drawn) = self.drawn.get(i) else { continue };
            let on = on_screen([drawn[0], drawn[1] - top, drawn[2], drawn[3]], (self.viewport.0, view));
            seen.insert(n.id, on);
            if last.get(&n.id).copied().unwrap_or(false) != on {
                let path = n.path.clone();
                self.queue.retain(|a| !matches!(a, Action::Visible(p, _) if *p == path));
                self.queue.push(Action::Visible(path, on));
                queued = true;
            }
        }
        self.visible_fired = seen;
        queued
    }
}

#[cfg(test)]
#[path = "visible_tests.rs"]
mod tests;
