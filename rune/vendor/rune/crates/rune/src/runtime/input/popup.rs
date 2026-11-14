use super::*;
use crate::popup_menu::{PopupAnchor, PopupMenu, PopupMenuError, Show};

impl<S: 'static> Runtime<S> {
    /// The pop-up menu showing, or the next to show, from [`Tx::popup_menu`].
    pub fn popup_menu(&self) -> Option<&PopupMenu> { self.popups.front().map(|r| &r.menu) }

    /// Answers the pop-up menu showing (or next to show) as if the user picked `picked`, or
    /// dismissed it with `None`. Its callback runs now and the next frame commits it, as for
    /// [`Runtime::update`]. For tests and headless drivers; native menus answer themselves.
    pub fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), PopupMenuError> {
        let request = self.popups.front().ok_or(PopupMenuError::NotOpen)?;
        if let Some(id) = picked {
            let item = request.menu.find(id).ok_or_else(|| PopupMenuError::UnknownItem(id.into()))?;
            if !item.enabled { return Err(PopupMenuError::Disabled(id.into())); }
        }
        let Some(request) = self.popups.pop_front() else { return Err(PopupMenuError::NotOpen) };
        self.popup_open = false;
        match picked {
            Some(id) => self.trace_event(format_args!("popup-menu:pick:{id}")),
            None => self.trace_event("popup-menu:cancel"),
        }
        let mut one = self.transaction();
        if let (Some(field), Some(id)) = (request.field, picked) {
            self.field_menu_pick(*field, id, &mut one);
        }
        (request.on_pick)(states!(self), picked, &mut one);
        self.latch_requests(&mut one);
        merge(&mut self.pending, one);
        self.dirty = true;
        Ok(())
    }

    /// The next menu for the shell to open, marked open. `Some(None)` is a request that can't
    /// open (its anchor element is gone, or its ids repeat): answer it with `None`. Nothing while
    /// a menu is open, so menus show one at a time.
    pub(crate) fn take_popup(&mut self) -> Option<Option<Show>> {
        if self.popup_open { return None; }
        let request = self.popups.front()?;
        let rect = match &request.anchor {
            PopupAnchor::Key(key) => self.anchor_rect(key).ok_or_else(|| format!("no element keyed {key:?} to anchor to")),
            &PopupAnchor::Point(x, y) => Ok([x, y, 0.0, 0.0]),
        };
        self.popup_open = true;
        match request.validate().and(rect) {
            Ok(rect) => {
                let show = Show::new(request.menu.clone(), &request.anchor, rect);
                self.trace_event("popup-menu:open");
                // Asked for outside a frame (an update): the next frame drops the hover.
                if self.release_pointer_for_popup() { self.dirty = true; }
                Some(Some(show))
            }
            Err(problem) => {
                eprintln!("rune: pop-up menu: {problem}");
                Some(None)
            }
        }
    }

    /// The next menu anchors to an element and the runtime has changes the next frame commits
    /// (a pick that asked for another menu over what it mounted): open it after that frame.
    pub(crate) fn popup_waits_for_layout(&self) -> bool {
        !self.popup_open && (self.dirty || self.relayout)
            && self.popups.front().is_some_and(|r| matches!(r.anchor, PopupAnchor::Key(_)))
    }

    /// A menu is about to take the mouse: press, capture and hover end now, as if the pointer
    /// left, rather than when it next moves. True when that changed anything.
    pub(crate) fn release_pointer_for_popup(&mut self) -> bool {
        if self.popups.is_empty() || (self.pointer.is_none() && !self.pointer_captured()) {
            return false;
        }
        self.cancel_pointer();
        true
    }

    /// Answers every waiting menu with `None`, as its window goes away. A callback that asks
    /// for another menu is answered too, up to a bound against one that always asks again.
    pub(crate) fn cancel_popups(&mut self) {
        for _ in 0..64 {
            if self.resolve_popup(None).is_err() { return; }
        }
        eprintln!("rune: pop-up menu callbacks kept asking for menus while their window went away; dropping {}", self.popups.len());
        self.popups.clear();
        self.popup_open = false;
    }

    /// A headless run's stand-in for the window's menu: the next menu opens and waits for
    /// [`Runtime::resolve_popup`], and one that can't open answers `None` now.
    pub(crate) fn open_popup_headless(&mut self) {
        while let Some(None) = self.take_popup() {
            let _ = self.resolve_popup(None);
        }
    }

    /// Drawn rect of the element a menu anchors to, matched as [`Runtime::find`] matches.
    fn anchor_rect(&self, key: &str) -> Option<[f32; 4]> {
        let suffix = format!("/{key}");
        let matches = |n: &Node| if key.contains('/') { n.path == key || n.path.ends_with(&suffix) } else { n.key.as_deref() == Some(key) };
        let i = (0..self.nodes.len()).find(|&i| matches(&self.nodes[i]) && !self.subtree_exiting(i))?;
        self.drawn.get(i).copied()
    }

    pub(crate) fn popup_json(&self) -> Option<serde_json::Value> {
        let request = self.popups.front()?;
        let mut v = request.menu.to_json();
        v["open"] = self.popup_open.into();
        v["anchor"] = match &request.anchor {
            PopupAnchor::Key(key) => serde_json::json!({ "key": key }),
            PopupAnchor::Point(x, y) => serde_json::json!({ "x": x, "y": y }),
        };
        v["queued"] = (self.popups.len() - 1).into();
        Some(v)
    }
}
