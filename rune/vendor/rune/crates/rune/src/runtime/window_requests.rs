//! What a commit asks of the window besides its pixels: the content height a window that
//! fits its content asks for (`App::fit_content_height`), and the title (`Tx::set_title`).
use super::*;

/// The window's title: `AppSpec`'s until a handler sets another, and whether the window has
/// yet to show the last one set.
#[derive(Debug, Default)]
pub(crate) struct Title { now: Option<String>, changed: bool }

impl Title {
    /// The title the window opens with, before any handler sets one.
    pub(crate) fn start(&mut self, title: &str) {
        if self.now.is_none() { self.now = Some(title.to_owned()); }
    }
    pub(super) fn set(&mut self, title: String) {
        if self.now.as_deref() == Some(title.as_str()) { return; }
        self.now = Some(title);
        self.changed = true;
    }
    pub(crate) fn current(&self) -> Option<&str> { self.now.as_deref() }
    /// A title set since the window last took one, for the window to show.
    pub(crate) fn take_change(&mut self) -> Option<&str> {
        if std::mem::take(&mut self.changed) { self.now.as_deref() } else { None }
    }
}

impl<S: 'static> Runtime<S> {
    /// Asks the window for the content's new height, on this commit's spring. The first
    /// commit, a viewport change and `Tx::instant` jump.
    pub(super) fn fit_window(&mut self, height: f32, tx: &Tx, first: bool) {
        let height = height.ceil_to(self.env.scale).max(1.0);
        if self.fit_asked.unwrap_or(self.viewport.1) == height { return; }
        self.fit_asked = Some(height);
        let jump = first || tx.instant || tx.relayout || self.reduced_motion;
        // Only a macOS window and headless runs move the frame on a spring.
        if !jump && self.window_resting && (cfg!(target_os = "macos") || self.wall_start.is_none()) { self.fit_hold = true; }
        let request = crate::window::frame::FrameRequest {
            size: Some((self.viewport.0, height)), animate: !jump,
            spring: (!jump).then(|| tx.motion.unwrap_or(motion::DEFAULT)), center: false,
        };
        self.window_frame = Some(self.window_frame.unwrap_or_default().merge(request));
    }

    /// The root's natural height at the current width, measured without committing, so a
    /// window that fits its content can open at that height.
    pub fn content_height(&mut self) -> f32 {
        let measured = self.measure_pass(&Tx::default(), false);
        let height = measured.nodes.first().map_or(self.viewport.1, |root| root.local.h);
        self.discard_measured(measured);
        height.ceil_to(self.env.scale).max(1.0)
    }
}
