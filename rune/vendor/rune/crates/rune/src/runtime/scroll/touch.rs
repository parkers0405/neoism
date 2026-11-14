//! Touch scrolling: a finger pans the scroll viewport under it, rubber-bands past its ends,
//! and flings it on with momentum, on every platform that reports touches (`runtime/input/touch.rs`
//! turns touches into presses, drags and these pans).
use super::*;

/// The viewport a touch holds, or one it flung that is still settling.
pub(super) struct TouchScroll {
    pub(super) id: NodeId,
    pub(super) origin: f64,
    pub(super) dragging: bool,
    /// The pan moves the viewport's horizontal axis (`El::scroll_x`).
    pub(super) horizontal: bool,
}

/// A critically damped edge return, integrated exactly for variable frame times.
fn rebound(offset: f64, velocity: f32, target: f64, dt: f32) -> (f64, f32) {
    let dt = f64::from(dt);
    let displacement = offset - target;
    let b = f64::from(velocity) + 14.0 * displacement;
    let decay = (-14.0 * dt).exp();
    (target + (displacement + b * dt) * decay, ((f64::from(velocity) - 14.0 * b * dt) * decay) as f32)
}

impl<S: 'static> Runtime<S> {
    /// A touch went down: it stops the momentum of the viewports under it and holds the
    /// vertical one until the pan picks its axis (`touch_scroll_pan`). A horizontal one still
    /// settling from an earlier touch (momentum or its edge return) holds where the finger
    /// caught it instead.
    pub(super) fn touch_scroll_start(&mut self, x: f32, y: f32) -> Option<NodeId> {
        let across = self.scroll_at_axis(x, y, true);
        if let Some(st) = across.and_then(|id| self.scrolls.get_mut(&id)) { st.x.v = 0.0; }
        let caught = match &mut self.touch.scroll {
            Some(t) if t.horizontal && Some(t.id) == across => {
                t.dragging = true;
                t.origin = self.scrolls.get(&t.id).map_or(t.origin, |st| st.x.offset);
                true
            }
            _ => false,
        };
        let id = self.scroll_at(x, y)?;
        let st = self.scrolls.get_mut(&id)?;
        st.halt();
        if !caught { self.touch.scroll = Some(TouchScroll { id, origin: st.offset, dragging: true, horizontal: false }); }
        Some(id)
    }

    /// The touch that went down at (x, y) started panning, mostly horizontally or not: a
    /// horizontal pan takes the innermost horizontal viewport there, and any other pan stays
    /// with the vertical one. Returns the viewport and whether it moves horizontally.
    pub(super) fn touch_scroll_pan(&mut self, x: f32, y: f32, horizontal: bool, vertical: Option<NodeId>) -> (Option<NodeId>, bool) {
        if let Some(id) = self.scroll_at_axis(x, y, true).filter(|_| horizontal)
            && let Some(st) = self.scrolls.get(&id)
        {
            self.touch.scroll = Some(TouchScroll { id, origin: st.x.offset, dragging: true, horizontal: true });
            return (Some(id), true);
        }
        // A horizontal viewport the touch caught lets go, and the vertical one takes the pan.
        if self.touch.scroll.as_ref().is_some_and(|t| t.horizontal) {
            if let Some(t) = &mut self.touch.scroll { t.dragging = false; }
            if let Some(id) = vertical && let Some(st) = self.scrolls.get(&id) {
                self.touch.scroll = Some(TouchScroll { id, origin: st.offset, dragging: true, horizontal: false });
            }
        }
        (vertical, false)
    }

    /// The pan moved `d` points along its axis from where the touch went down.
    pub(super) fn touch_scroll_move(&mut self, id: NodeId, d: f32) {
        let Some(touch) = &self.touch.scroll else { return };
        if touch.id != id { return; }
        let (origin, horizontal) = (touch.origin, touch.horizontal);
        let Some(st) = self.scrolls.get_mut(&id) else { return };
        let toward = if horizontal { st.x.toward(f64::from(d)) } else { -f64::from(d) };
        let (offset, max) = st.axis(horizontal);
        let to = origin + toward;
        let edge = to.clamp(0.0, max);
        let over = to - edge;
        *offset = edge + over * 0.55 / (1.0 + over.abs() * 0.55 / 300.0);
        if !horizontal { st.mode = ScrollMode::Rest; }
        st.direct = true;
        self.scroll_cancels_press(id);
    }

    /// The touch lifted from viewport `id` (None when it held none) with `velocity` along its
    /// axis. Whatever it held lets go, a viewport it caught without panning included.
    pub(super) fn touch_scroll_end(&mut self, id: Option<NodeId>, velocity: f32) {
        let Some(touch) = &mut self.touch.scroll else { return };
        touch.dragging = false;
        if Some(touch.id) != id { return; }
        let horizontal = touch.horizontal;
        if let Some(st) = self.scrolls.get_mut(&touch.id) {
            let v = f64::from(velocity.clamp(-8000.0, 8000.0));
            if horizontal { st.x.v = st.x.toward(v) as f32 } else { st.mode = ScrollMode::Momentum { v: -v as f32 } }
        }
    }

    /// Own the touched viewport's panned axis through momentum and its edge return. Other
    /// viewports use the native wheel path, including native clamping and stick-to-bottom.
    pub(super) fn step_touch_scroll(&mut self, id: NodeId, overflow: f64, dt: f32, horizontal: bool) -> Option<bool> {
        let touch = self.touch.scroll.as_ref().filter(|t| t.id == id && t.horizontal == horizontal)?;
        let dragging = touch.dragging;
        let st = self.scrolls.get_mut(&id)?;
        if horizontal { st.x.max = overflow.max(0.0); } else { st.max = overflow.max(0.0); }
        let mut v = if horizontal { st.x.v } else { st.coast() };
        st.scrolling = dragging || v.abs() > 1.0;
        st.by_hand |= st.scrolling;
        // Keep the host on its display link between contact samples. Reporting rest
        // here makes bare editor viewports stop/restart the clock during a live pan.
        if dragging { return Some(true); }
        st.direct = false;
        let (offset, max) = st.axis(horizontal);
        let edge = offset.clamp(0.0, max);
        if *offset != edge {
            (*offset, v) = rebound(*offset, v, edge, dt);
            if (*offset - edge).abs() < 0.1 && v.abs() < 1.0 { *offset = edge; v = 0.0; }
        } else {
            let decay = (-SCROLL_DECAY * dt).exp();
            *offset += f64::from(v * (1.0 - decay) / SCROLL_DECAY);
            v *= decay;
        }
        let moving = v.abs() > 1.0 || *offset < 0.0 || *offset > max;
        if horizontal { st.x.v = v; } else if v != 0.0 { st.mode = ScrollMode::Momentum { v }; } else { st.halt(); }
        if !moving { self.touch.scroll = None; }
        Some(moving)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_touch_pan_moves_the_axis_its_first_direction_picks() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            let tiles = (0..10).fold(crate::row(), |r, i| r.child(crate::el().key(format!("tile{i}")).w(100.0).h(100.0)));
            let rows = (0..20).fold(crate::col().w_full().child(crate::col().key("strip").w_full().h(100.0).scroll_x().child(tiles)),
                |c, i| c.child(crate::el().key(format!("row{i}")).w_full().h(40.0)));
            crate::col().w_full().h_full().child(crate::col().key("page").w_full().h(300.0)
                .scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
        }
        let mut rt = Runtime::new((), view, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        let id = |rt: &Runtime<()>, key: &str| rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
        let (page, strip) = (id(&rt, "page"), id(&rt, "strip"));

        let vertical = rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(vertical, Some(page));
        assert_eq!(rt.touch_scroll_pan(150.0, 50.0, true, vertical), (Some(strip), true));
        rt.touch_scroll_move(strip, -50.0);
        assert_eq!((rt.scrolls[&strip].x.offset, rt.scrolls[&page].offset), (50.0, 0.0));
        rt.touch_scroll_end(Some(strip), 0.0);

        // A vertical pan over the strip passes through to the page.
        let vertical = rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(rt.touch_scroll_pan(150.0, 50.0, false, vertical), (Some(page), false));
        rt.touch_scroll_move(page, -30.0);
        assert_eq!((rt.scrolls[&strip].x.offset, rt.scrolls[&page].offset), (50.0, 30.0));
        rt.touch_scroll_end(Some(page), 0.0);

        // A touch on the strip while it springs back past its left edge holds it there, until a
        // tap lets it go or a sideways pan carries on from where it was caught.
        let held = |rt: &Runtime<()>| rt.touch.scroll.as_ref().map(|t| (t.id, t.horizontal, t.dragging, t.origin));
        rt.scrolls.get_mut(&strip).unwrap().x.offset = -20.0;
        rt.touch.scroll = Some(TouchScroll { id: strip, origin: 0.0, dragging: false, horizontal: true });
        let vertical = rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(held(&rt), Some((strip, true, true, -20.0)), "caught, not handed to the page");
        rt.touch_scroll_end(vertical, 0.0);
        assert_eq!(held(&rt), Some((strip, true, false, -20.0)), "a tap lets it go back");
        let vertical = rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(rt.touch_scroll_pan(150.0, 50.0, true, vertical), (Some(strip), true));
        rt.touch_scroll_move(strip, 10.0);
        assert!(rt.scrolls[&strip].x.offset < -20.0 * 0.55, "still rubber-banding: {}", rt.scrolls[&strip].x.offset);
        rt.touch_scroll_end(Some(strip), 0.0);

        // Caught there, a vertical pan lets the strip go and moves the page.
        rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(rt.touch_scroll_pan(150.0, 50.0, false, vertical), (Some(page), false));
        assert_eq!(held(&rt).map(|h| (h.0, h.1, h.2)), Some((page, false, true)));
    }

    #[test]
    fn edge_return_is_frame_rate_independent() {
        let one = rebound(-50.0, -400.0, 0.0, 0.5);
        let mut many = (-50.0, -400.0);
        for _ in 0..60 { many = rebound(many.0, many.1, 0.0, 0.5 / 60.0); }
        assert!((one.0 - many.0).abs() < 0.001);
        assert!((one.1 - many.1).abs() < 0.01);
        assert!(one.0 < 0.0 && one.0 > -1.0);
    }
}
