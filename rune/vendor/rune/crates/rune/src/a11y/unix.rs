//! AT-SPI for a surface: AccessKit's Unix adapter registers the app on the accessibility bus
//! once the desktop turns it on, with each surface as one of its windows. It needs no window
//! handle, so layer-shell overlays have a tree too.

use accesskit::Rect;

use super::{Actions, Activation, Deactivation};

pub(super) struct Adapter(accesskit_unix::Adapter);

impl Adapter {
    pub(super) fn new(activation: Activation, actions: Actions, deactivation: Deactivation) -> Self {
        Adapter(accesskit_unix::Adapter::new(activation, actions, deactivation))
    }

    pub(super) fn update(&mut self, update: accesskit::TreeUpdate) {
        self.0.update_if_active(|| update);
    }

    pub(super) fn focus(&mut self, focused: bool) {
        self.0.update_window_focus_state(focused);
    }

    pub(super) fn bounds(&mut self, outer: [f64; 4], inner: [f64; 4]) {
        let rect = |r: [f64; 4]| Rect { x0: r[0], y0: r[1], x1: r[0] + r[2], y1: r[1] + r[3] };
        self.0.set_root_window_bounds(rect(outer), rect(inner));
    }
}
