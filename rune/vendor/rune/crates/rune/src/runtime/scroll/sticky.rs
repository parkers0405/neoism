//! `El::sticky_top`, `El::sticky_bottom` and `El::sticky_left`: elements held to an edge of
//! their scroll viewport while their parent is in view (CSS `position: sticky`), and
//! `Tx::keep_on_screen`, which holds an element where it shows across a commit.
//!
//! The shift is worked out in the paint pass from the offsets that frame applies, so a stuck
//! header never trails a scroll. Layout is untouched: only drawing, hit testing and what tools
//! read move. A sticky element paints after its siblings (`update_layer_paint_order`).
use super::*;
use crate::el::StickyEdge;

/// No viewport band and no parent box: nothing to stick within.
pub(super) const UNBOUNDED: [f32; 8] = [f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY];

/// Points of shift over which `El::stuck_shadow` fades in.
pub(super) const SHADOW_RAMP: f32 = 8.0;

/// One `Tx::keep_on_screen` anchor captured before a commit.
pub(super) struct Keep {
    pub path: String,
    pub viewport: String,
    pub was: f32,
    /// When the element lives inside a virtual row: `(virtual_list_path, row_key, y_in_row)`.
    pub virtual_row: Option<(String, String, f32)>,
    /// The element, or the header it's in, was stuck to the top: it stays there for the commit.
    pub hold: Option<Hold>,
}

/// A header stuck to the top when a `Tx::keep_on_screen` commit started. Until the motion
/// settles it sticks `inset` points below its viewport's top whether or not it's still sticky,
/// bounded by the bottom of the growth row it's drawn in rather than its parent's new layout, so
/// the rows under it fold up to it before the scroll moves, on the same frame.
pub(super) struct Hold {
    pub path: String,
    pub inset: f32,
}

/// One `Tx::scroll_into_view` or `Tx::scroll_center` request that follows its target while the
/// viewport's subtree moves.
pub(super) struct Reveal {
    pub scope: Option<String>,
    pub key: String,
    pub viewport: String,
    pub center: bool,
    /// Whether this reveal lands at the bottom of a `stick_bottom` viewport.
    pub at_end: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Align, Live, Side, col, el, row};

    #[derive(Default)]
    struct FollowState {
        height: f32,
        removed: bool,
        hidden: bool,
        streaming: bool,
    }

    fn follow_view(s: &FollowState, _: &mut Cx<FollowState>) -> El {
        if s.hidden { return el(); }
        let mut stack = col().key("stack").w_full().growth(GrowthSpec::default())
            .child(el().key("lead").w_full().h(400.0))
            .child(el().key("grow").w_full().h(s.height).streaming(s.streaming));
        if !s.removed { stack = stack.child(el().key("tail").w_full().h(35.0)); }
        col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
    }

    fn following(keep: bool) -> Runtime<FollowState> {
        let mut rt = Runtime::new(FollowState { height: 37.0, ..Default::default() }, follow_view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.update(move |s, tx| {
            s.height = 130.0;
            if keep { tx.keep_on_screen("tail"); } else { tx.scroll_into_view("tail"); }
        });
        rt.frame(1.0 / 120.0);
        rt
    }

    fn scroll(rt: &Runtime<FollowState>) -> &ScrollState {
        &rt.scrolls[&rt.node("scroll").unwrap().id]
    }

    #[test]
    fn completed_reveal_and_keep_repin_the_end_and_go_idle() {
        for keep in [false, true] {
            let mut rt = following(keep);
            if !keep { assert!(!scroll(&rt).pinned()); }
            for _ in 0..240 { rt.frame(1.0 / 120.0); }
            assert!(rt.keeps.is_empty() && rt.scroll_reveals.is_empty());
            assert!(!rt.animating);
            assert!(scroll(&rt).pinned(), "the completed hold returns the end to stick_bottom");
            rt.update(|s, _| { s.height = 230.0; s.streaming = true; });
            for _ in 0..60 {
                rt.frame(1.0 / 120.0);
                assert!(scroll(&rt).pinned());
                assert!((scroll(&rt).offset - scroll(&rt).max).abs() < 0.05);
            }
        }
    }

    #[test]
    fn all_scroll_input_and_later_destinations_cancel_a_reveal() {
        for input in 0..5 {
            let mut rt = following(false);
            assert!(!rt.scroll_reveals.is_empty());
            match input {
                0 => rt.wheel_xy(100.0, 100.0, 0.0, 50.0),
                1 => { let i = rt.node_index[&rt.node("scroll").unwrap().id]; rt.page_to(i, |_, _, _| 0.0); }
                2 => rt.wheel(100.0, 100.0, 50.0),
                3 => rt.fling(100.0, 100.0, 100.0),
                _ => rt.update(|_, tx| { tx.scroll_into_view("tail"); tx.scroll_to_y("scroll", 0.0); }),
            }
            if input != 4 { assert!(rt.scroll_reveals.is_empty(), "input {input} cancels at arrival, even before wheel dispatch"); }
            rt.frame(1.0 / 120.0);
            assert!(rt.scroll_reveals.is_empty(), "input {input} cancels the reveal");
            let offset = scroll(&rt).offset;
            if input != 3 {
                for _ in 0..60 { rt.frame(1.0 / 120.0); }
                assert!((scroll(&rt).offset - offset).abs() < 0.05, "no reveal fights input {input}");
            }
        }
    }

    #[test]
    fn a_missing_reveal_target_or_viewport_is_pruned_during_other_growth() {
        for hidden in [false, true] {
            let mut rt = following(false);
            rt.update(move |s, _| { s.removed = true; s.hidden = hidden; s.height = 300.0; });
            rt.frame(1.0 / 120.0);
            assert!(rt.scroll_reveals.is_empty(), "missing targets must not wait for unrelated growth");
            for _ in 0..240 { rt.frame(1.0 / 120.0); }
            assert!(!rt.animating);
        }
    }

    #[test]
    fn instant_only_overrides_scroll_requests_in_that_commit() {
        let mut rt = Runtime::new(FollowState { height: 37.0, ..Default::default() }, follow_view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 0.0));
        rt.frame(1.0 / 120.0);
        assert!(scroll(&rt).glide_to().is_some());
        rt.update(|_, tx| tx.instant());
        rt.frame(1.0 / 120.0);
        assert!(scroll(&rt).glide_to().is_some(), "an unrelated instant commit leaves an existing glide alone");
        rt.update(|_, tx| { tx.instant(); tx.scroll_to("scroll", 0.0, 100.0); });
        rt.frame(1.0 / 120.0);
        assert!(scroll(&rt).glide_to().is_none());
        assert!((scroll(&rt).offset - 100.0).abs() < 0.05);
        rt.update(|_, tx| { tx.instant(); tx.scroll_to_bottom("scroll"); });
        rt.frame(1.0 / 120.0);
        assert!(scroll(&rt).glide().is_none());
        assert!((scroll(&rt).offset - scroll(&rt).max).abs() < 0.05);
    }

    #[test]
    fn revealing_an_already_visible_row_stops_a_running_glide() {
        let mut rt = Runtime::new(FollowState { height: 37.0, ..Default::default() }, follow_view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 0.0));
        rt.frame(0.0);
        assert!(scroll(&rt).glide_to().is_some());
        rt.update(|_, tx| tx.scroll_into_view("tail"));
        rt.frame(1.0 / 120.0);
        assert!(scroll(&rt).glide().is_none() && scroll(&rt).glide_to().is_none());
        assert!((scroll(&rt).offset - scroll(&rt).max).abs() < 0.05);
        assert!(!rt.animating);
    }

    #[test]
    fn revealing_another_viewport_does_not_move_a_virtual_reading_anchor() {
        fn view(_: &[f32; 2], _: &mut Cx<[f32; 2]>) -> El {
            row().size(600.0, 200.0).children((0..2).map(|list| {
                col().key(format!("scroll{list}")).size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
                    .child(col().key(format!("stack{list}")).w_full().growth(GrowthSpec::default())
                        .virtual_rows(12, 50.0,
                            move |_: &[f32; 2], i| format!("r{list}-{i}").into(),
                            move |s: &[f32; 2], i| el().w_full().h(if i == 1 { s[list] } else { 50.0 })))
            }))
        }
        let mut rt = Runtime::new([50.0; 2], view, (600.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_to_y("scroll1", 150.0));
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        let y = |rt: &Runtime<[f32; 2]>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r1-3")).unwrap().rect[1];
        let was = y(&rt);
        for height in [100.0, 30.0] {
            rt.update(move |s, tx| { s[1] = height; tx.scroll_into_view("r0-4"); });
            for _ in 0..60 {
                rt.frame(1.0 / 120.0);
                assert!((y(&rt) - was).abs() < 0.05, "the reader's row moved after height {height}: {} vs {was}", y(&rt));
            }
        }
    }

    #[test]
    fn sticky_left_holds_a_header_to_the_viewport_while_content_scrolls_sideways() {
        struct S {
            pressed: usize,
            offset: Live<[f32; 2]>,
        }
        fn view(s: &S, _: &mut Cx<S>) -> El {
            col().size(300.0, 200.0)
                .child(col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).scroll_xy()
                    .child(col().key("content").min_w(1000.0)
                        .child(row().key("header").w(300.0).h(30.0).justify_end().items_center().fill(Color::WHITE).sticky_left(0.0)
                            .live_offset(&s.offset)
                            .child(el().key("action").size(40.0, 20.0).focusable(true).help("Run action").on_press(|s: &mut S| s.pressed += 1)))
                        .child(el().w_full().h(600.0))))
                .child(el().key("pop").size(50.0, 20.0).anchored("action", Side::Bottom, Align::End, 0.0))
        }
        let mut rt = Runtime::new(S { pressed: 0, offset: Live::new([0.0, 0.0]) }, view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.wheel_xy(100.0, 100.0, -250.0, 0.0);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let els = rt.elements();
        let h = els.iter().find(|e| e.key.as_deref() == Some("header")).unwrap();
        assert_eq!(h.rect[0], 0.0);
        assert_eq!(h.stuck, 250.0);

        // A control at the trailing end of the shifted header is clickable, focusable and anchors
        // overlays and tooltips at its shifted viewport coordinates.
        let action = els.iter().find(|e| e.key.as_deref() == Some("action")).unwrap();
        assert_eq!(action.rect, [260.0, 5.0, 40.0, 20.0]);
        let pop = els.iter().find(|e| e.key.as_deref() == Some("pop")).unwrap();
        assert_eq!(pop.rect[0], 242.0, "anchored overlay follows sticky_left shift (kept 8pt inside viewport)");

        rt.pointer_move(280.0, 15.0);
        rt.frame(0.0);
        rt.frame(rt.help_delay);
        assert!(rt.help_tooltip().is_some(), "tooltip shows on shifted sticky_left child");
        rt.pointer_down(280.0, 15.0);
        rt.pointer_up(280.0, 15.0);
        rt.frame(0.0);
        assert_eq!(rt.state.pressed, 1, "shifted header action takes press after sideways scroll");

        // `live_offset` on a sticky_left element translates from the stuck position rather than
        // being clamped away by the sticky edge.
        rt.state.offset.set([-12.0, 4.0]);
        rt.frame(0.0);
        let h = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("header")).unwrap();
        assert_eq!((h.rect[0], h.rect[1], h.stuck), (-12.0, 4.0, 250.0));
    }

    #[test]
    fn sticky_left_inside_nested_scrollers_moves_vertically_and_stops_at_parent_right() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            col().key("vscroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
                .child(col().w_full()
                    .child(el().w_full().h(50.0))
                    .child(col().key("hscroll").w(300.0).h(100.0).scroll(ScrollSpec { stick_bottom: false }).scroll_x()
                        .child(row().key("band").w(500.0).h(40.0).pr(20.0)
                            .child(el().key("pin").w(100.0).h(40.0).sticky_left(10.0))))
                    .child(el().w_full().h(400.0)))
        }
        let mut rt = Runtime::new((), view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        // Scroll horizontally inside `hscroll` and vertically in `vscroll`.
        rt.wheel_xy(150.0, 80.0, -120.0, 0.0);
        rt.wheel(150.0, 80.0, -30.0);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let pin = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("pin")).unwrap();
        assert_eq!(pin.rect[0], 10.0, "held at inset from horizontal viewport's left");
        assert_eq!(pin.rect[1], 20.0, "scrolled vertically with its outer row");
        assert_eq!(pin.stuck, 130.0);
    }

    #[test]
    fn cached_section_with_sticky_descendant_stays_live_on_scroll() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).scroll_xy()
                .child(col().key("cached").w_full().cache()
                    .child(el().key("header").w_full().h(30.0).fill(Color::WHITE).sticky_top(0.0))
                    .child(el().key("left").w(300.0).h(30.0).fill(Color::WHITE).sticky_left(0.0))
                    .child(el().w_full().h(600.0)))
        }
        let mut rt = Runtime::new((), view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.wheel_xy(100.0, 100.0, -80.0, -100.0);
        rt.frame(0.0);
        let cached = rt.nodes.iter().position(|n| n.key.as_deref() == Some("cached")).unwrap();
        assert!(rt.cache_live_scratch[cached], "sticky contents must not reuse a frozen raster");
        assert!(!rt.node_caches.contains_key(&rt.nodes[cached].id));
        let h = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("header")).unwrap();
        assert_eq!(h.rect[1], 0.0);
        assert_eq!(h.stuck, 100.0);
    }
}

/// How far a sticky box with edges `top`, `bottom`, `left` and `right` moves from where
/// scrolling put it, along the edge's axis. `frame` holds its viewport's top and bottom, then
/// its parent's content box top and bottom, then the same four for the horizontal axis.
pub(super) fn shift(edge: StickyEdge, inset: f32, [top, bottom, left, right]: [f32; 4], frame: [f32; 8]) -> f32 {
    let [band_top, band_bottom, box_top, box_bottom, band_left, _, _, box_right] = frame;
    let d = match edge {
        StickyEdge::Left => (band_left + inset - left).min(box_right - right).max(0.0),
        StickyEdge::Top => (band_top + inset - top).min(box_bottom - bottom).max(0.0),
        StickyEdge::Bottom => (band_bottom - inset - bottom).max(box_top - top).min(0.0),
    };
    if d.is_finite() { d } else { 0.0 }
}

/// Target rect of node `j` after its scroll viewports' offsets and any sticky shift on `j` or
/// its ancestors, for `El::anchored`.
pub(super) fn anchored_target(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, j: usize, viewport: (f32, f32)) -> [f32; 4] {
    let mut t = nodes[j].target;
    for [ox, oy] in scroll_offsets(scrolls, nodes, j) {
        t[0] -= ox;
        t[2] -= ox;
        t[1] -= oy;
        t[3] -= oy;
    }
    let [mut dx, mut dy] = target_shift(nodes, scrolls, j);
    if let Some(anc) = std::iter::successors(Some(j), |&p| nodes[p].parent).find(|&p| nodes[p].above.is_some())
        && let Some(a) = &nodes[anc].above
        && let Some(k) = nodes[..anc].iter().position(|n| n.key.as_ref() == Some(&a.key))
    {
        let at = anchored_target(nodes, scrolls, k, viewport);
        let (w, h) = (nodes[anc].target[2] - nodes[anc].target[0], nodes[anc].target[3] - nodes[anc].target[1]);
        let (ax, ay) = super::anchor_position(a, at, (w, h), viewport);
        dx += ax - nodes[anc].target[0];
        dy += ay - nodes[anc].target[1];
    }
    t[0] += dx;
    t[2] += dx;
    t[1] += dy;
    t[3] += dy;
    t
}

/// Total sticky shift `[dx, dy]` on node `j` and its ancestors from `nodes`'s laid-out rects and
/// `scrolls`.
fn target_shift(nodes: &[Node], scrolls: &HashMap<NodeId, ScrollState>, j: usize) -> [f32; 2] {
    let mut total = [0.0; 2];
    for a in std::iter::successors(Some(j), |&p| nodes[p].parent.filter(|_| nodes[p].above.is_none())) {
        let Some((edge, inset, tail)) = nodes[a].sticky.as_ref().and_then(|s| Some((s.edge?, s.inset, s.tail))) else { continue };
        let Some(p) = nodes[a].parent else { continue };
        if edge == StickyEdge::Left {
            let Some(s) = std::iter::successors(Some(p), |&u| nodes[u].parent)
                .find(|&u| nodes[u].scroll.is_some() && nodes[u].scroll_axes.x()) else { continue };
            let ox: f32 = std::iter::successors(Some(p), |&u| nodes[u].parent)
                .take_while(|&u| u != s)
                .chain(std::iter::once(s))
                .filter_map(|u| scrolls.get(&nodes[u].id).map(|st| st.x.shift() as f32))
                .sum();
            let st_x = nodes[s].target[0];
            let band_w = nodes[s].target[2] - st_x;
            let (box_left, box_right) = if p == s {
                (f32::NEG_INFINITY, f32::INFINITY)
            } else {
                (nodes[p].target[0] - st_x - ox + nodes[p].inset[0], nodes[p].target[2] - st_x - ox - nodes[p].inset[2])
            };
            let left = nodes[a].target[0] - st_x - ox;
            let right = nodes[a].target[2] - st_x - ox;
            let frame = [f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, 0.0, band_w, box_left, box_right];
            total[0] += shift(edge, inset, [0.0, 0.0, left, right], frame);
        } else {
            let Some(s) = std::iter::successors(Some(p), |&u| nodes[u].parent)
                .find(|&u| nodes[u].scroll.is_some() && nodes[u].scroll_axes.y()) else { continue };
            let oy: f32 = std::iter::successors(Some(p), |&u| nodes[u].parent)
                .take_while(|&u| u != s)
                .chain(std::iter::once(s))
                .filter_map(|u| scrolls.get(&nodes[u].id).map(|st| st.offset as f32))
                .sum();
            let st_y = nodes[s].target[1];
            let band_h = nodes[s].target[3] - st_y;
            let (box_top, box_bottom) = if p == s {
                (f32::NEG_INFINITY, f32::INFINITY)
            } else {
                (nodes[p].target[1] - st_y - oy + nodes[p].inset[1], nodes[p].target[3] - st_y - oy - nodes[p].inset[3])
            };
            let top = nodes[a].target[1] - st_y - oy;
            let bottom = nodes[a].target[3] - st_y - oy;
            let frame = [0.0, band_h, box_top, box_bottom, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY];
            total[1] += shift(edge, if tail { inset - (bottom - top) } else { inset }, [top, bottom, 0.0, 0.0], frame);
        }
    }
    total
}

impl<S: 'static> Runtime<S> {
    /// Replay just one inherited fade edge without the nearest viewport's content-aware
    /// fade. Explicit clips inside it and fades outside it still apply. Recursion follows
    /// parents already painted this frame and needs no scratch allocation.
    pub(super) fn sticky_fade(&self, i: usize, viewport: usize, slot: usize, paint: bool) -> (f32, f32) {
        if i == viewport {
            let up = self.nodes[i].parent.map_or(Inh::default(), |p| self.inh_scratch[p]);
            return if paint { (up.paint_fade[slot], up.paint_fade_origin[slot]) }
                else { (up.fade[slot], up.fade_origin[slot]) };
        }
        let n = &self.nodes[i];
        let (width, origin) = self.sticky_fade(n.parent.unwrap(), viewport, slot, paint);
        let mut widths = [0.0; 4];
        let mut origins = NO_CLIP;
        widths[slot] = width;
        origins[slot] = origin;
        let r = self.drawn[i].map(|v| v * self.env.scale);
        let r = [r[0], r[1], r[0] + r[2], r[1] + r[3]];
        if let Some(clip) = n.vis.clip {
            inherit_fade(&mut widths, &mut origins, r, clip.map(|v| v * self.inh_scratch[i].scale * self.env.scale));
        }
        if self.row_mask[i] >= 0.0 {
            inherit_fade(&mut widths, &mut origins, r, [0.0, 0.0, 0.0, self.row_mask[i] * self.env.scale]);
        }
        // An isolation layer already consumed the parent's paint fade. Its descendants
        // must not apply that same fade a second time.
        if paint && self.inh_scratch[i].paint_fade[slot] == 0.0 { return (0.0, NO_CLIP[slot]); }
        (widths[slot], origins[slot])
    }

    /// The nearest vertical scroll viewport around node `i`.
    pub(super) fn viewport_of(&self, i: usize) -> Option<usize> {
        std::iter::successors(self.nodes[i].parent, |&p| self.nodes[p].parent)
            .find(|&p| self.nodes[p].scroll.is_some() && self.nodes[p].scroll_axes.y())
    }

    /// Records where each `Tx::keep_on_screen` element showed on the last frame, before the
    /// commit moves it.
    pub(super) fn capture_keeps(&mut self, tx: &mut Tx) {
        self.anchor_keeps(tx);
        if tx.keep_on_screen.is_empty() { return; }
        let keys = std::mem::take(&mut tx.keep_on_screen);
        // The last frame's rects describe these nodes only until a commit replaces them.
        if !self.built || self.drawn.len() != self.nodes.len() { return; }
        for key in keys {
            let found = (0..self.nodes.len()).find(|&i| {
                let n = &self.nodes[i];
                n.named(&key) && !self.subtree_exiting(i)
            });
            let Some(i) = found else { continue };
            let Some(s) = self.viewport_of(i) else { continue };
            let hold = self.stuck_header(i, s).map(|h| Hold {
                path: self.nodes[h].path.clone(),
                inset: (self.drawn[h][1] - self.drawn[s][1]) / self.drawn_scale[s].max(1e-4),
            });
            self.keeps.retain(|k| k.viewport != self.nodes[s].path);
            self.scroll_reveals.retain(|r| r.viewport != self.nodes[s].path);
            let virtual_row = std::iter::successors(Some(i), |&p| self.nodes[p].parent)
                .take_while(|&p| p != s)
                .find_map(|r| {
                    let c = self.nodes[r].parent?;
                    self.virtuals.contains_key(&self.nodes[c].path).then(|| (
                        self.nodes[c].path.clone(),
                        self.nodes[r].key.clone().unwrap_or_default(),
                        self.nodes[i].target[1] - self.nodes[r].target[1],
                    ))
                });
            self.keeps.push(Keep {
                path: self.nodes[i].path.clone(),
                viewport: self.nodes[s].path.clone(),
                was: self.drawn[i][1] - self.drawn[s][1],
                virtual_row,
                hold,
            });
        }
    }

    /// Node `i`, or the header holding it, if it's drawn stuck to the top of viewport `s`.
    fn stuck_header(&self, i: usize, s: usize) -> Option<usize> {
        if !self.has_sticky || self.stuck.len() != self.nodes.len() { return None; }
        std::iter::successors(Some(i), |&p| self.nodes[p].parent)
            .take_while(|&p| p != s)
            .find(|&h| self.stuck[h] > 0.0 && self.sticky_of(h).is_some_and(|(edge, ..)| edge == StickyEdge::Top))
    }

    /// Whether a `Tx::keep_on_screen` holds a stuck header (`Hold`).
    pub(super) fn holding(&self) -> bool {
        self.keeps.iter().any(|k| k.hold.is_some())
    }

    fn hold_of(&self, i: usize) -> Option<&Hold> {
        if self.keeps.is_empty() { return None; }
        let path = &self.nodes[i].path;
        self.keeps.iter().find_map(|k| k.hold.as_ref().filter(|h| h.path == *path))
    }

    /// Node `i`'s sticky edge, inset and whether the inset counts from its bottom: a `Hold`'s,
    /// else its own.
    pub(super) fn sticky_of(&self, i: usize) -> Option<(StickyEdge, f32, bool)> {
        if let Some(h) = self.hold_of(i) { return Some((StickyEdge::Top, h.inset, false)); }
        self.nodes[i].sticky.as_ref().and_then(|s| Some((s.edge?, s.inset, s.tail)))
    }

    /// `El::stuck_fade`: how much of node `i`'s children show, drawn with its top at `drawn`
    /// (a frame point, `dy` the offset its siblings draw at): whole until the next sticky-top
    /// sibling's top comes within `lead + span` of it, gone at `lead`.
    pub(super) fn stuck_fade(&self, i: usize, drawn: f32, dy: f32) -> f32 {
        let n = &self.nodes[i];
        let Some([lead, span]) = n.sticky.as_ref().and_then(|s| s.fade) else { return 1.0 };
        let next = (i + 1..self.nodes.len())
            .filter(|&j| self.nodes[j].parent == n.parent)
            .find(|&j| self.nodes[j].sticky.as_ref().is_some_and(|s| s.edge == Some(StickyEdge::Top)));
        let Some(p) = next.and_then(|j| self.scene.presented(self.nodes[j].id).map(|p| (j, p))) else { return 1.0 };
        let (j, p) = p;
        let own = self.scene.get(self.nodes[j].id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
        let top = p.rect.y - own.y + dy + self.row_dy[j] - self.row_dy[i];
        ((top - drawn - lead) / span.max(1e-3)).clamp(0.0, 1.0)
    }

    /// The bands and box node `i` sticks within along `edge` (see `shift`), from its ancestors'
    /// drawn places this frame, and the viewport it sticks to.
    pub(super) fn sticky_frame_of(&self, i: usize, edge: StickyEdge) -> ([f32; 8], Option<usize>) {
        let n = &self.nodes[i];
        let mut frame = n.parent.map_or(UNBOUNDED, |pr| self.sticky_frame[pr]);
        let horizontal = edge == StickyEdge::Left;
        let mut ancestor = n.parent;
        let mut viewport = None;
        while let Some(a) = ancestor {
            let nd = &self.nodes[a];
            if nd.scroll.is_some() && if horizontal { nd.scroll_axes.x() } else { nd.scroll_axes.y() } {
                viewport = Some(a);
                break;
            }
            // A section nested inside a growth row must stop at that row's presented
            // bottom, not its descendants' final layout height. Only sticky nodes
            // need this ancestor walk; ordinary painting keeps its existing fast path.
            if !horizontal && self.row_h[a] >= 0.0 {
                frame[2] = frame[2].max(self.sticky_frame[a][2]);
                frame[3] = frame[3].min(self.sticky_frame[a][3]);
            }
            ancestor = nd.parent;
        }
        // A held header's parent may have shrunk in the commit while the growth row it's drawn
        // in folds: it stops at that row's drawn bottom, less what the row lays out below the
        // parent, so it rests where its parent ends once the row settles.
        if !horizontal && self.hold_of(i).is_some() && let Some(p) = n.parent {
            let r = std::iter::successors(Some(p), |&a| self.nodes[a].parent)
                .take_while(|&a| Some(a) != viewport)
                .find(|&a| self.row_h[a] >= 0.0)
                .unwrap_or(p);
            let content_bottom = |a: usize| self.nodes[a].target[3] - self.nodes[a].inset[3];
            frame[2] = f32::NEG_INFINITY;
            frame[3] = self.sticky_frame[r][3] - (content_bottom(r) - content_bottom(p));
        }
        (frame, viewport)
    }

    /// Scrolls each kept element's viewport so the element's top lands where it showed before
    /// the commit, clamped to the content, re-pinning a `stick_bottom` end, and remounting any
    /// virtual rows the jump brings into view.
    pub(super) fn apply_keeps(&mut self) {
        if self.keeps.is_empty() { return; }
        let mut keeps = std::mem::take(&mut self.keeps);
        for pass in 0..4 {
            // Step with the keeps in place: they own their viewports (`ease_to_shrunk_end`).
            self.keeps = keeps;
            self.step_scroll(0.0);
            keeps = std::mem::take(&mut self.keeps);
            let mut moved = false;
            for k in &keeps {
                let Some(s) = self.nodes.iter().position(|n| n.path == k.viewport && n.scroll.is_some() && n.scroll_axes.y()) else { continue };
                let laid = if let Some(i) = self.nodes.iter().position(|n| n.path == k.path) {
                    if self.viewport_of(i) != Some(s) { continue; }
                    if self.scroll_stack(s).and_then(|c| self.stacks.get(&self.nodes[c].id)).is_some_and(|st| st.growing)
                        && let Some(st) = self.scrolls.get_mut(&self.nodes[s].id)
                    {
                        st.unpin();
                    }
                    // Mounted anchors follow drawn geometry after the motion step.
                    continue;
                } else if let Some((v_path, row_key, in_row)) = &k.virtual_row
                    && let Some(v) = self.virtuals.get(v_path)
                    && let Some(idx) = v.index_of(row_key)
                {
                    f64::from(v.top + v.pad_top + v.prefix(idx) + *in_row)
                } else {
                    continue;
                };
                let stick = self.nodes[s].scroll.is_some_and(|sc| sc.stick_bottom);
                let id = self.nodes[s].id;
                let Some(st) = self.scrolls.get_mut(&id) else { continue };
                st.direct = true;
                let to = (laid - f64::from(k.was)).clamp(0.0, st.max);
                if pass > 0 && (to - st.offset).abs() < 0.5 { continue; }
                st.offset = to;
                st.land(stick);
                moved = true;
            }
            if !moved || !self.virtual_window_changed() { break; }
            self.commit(&Tx::default());
        }
        self.keeps = keeps;
    }

    /// Compensate this frame's drawn displacement, in viewport coordinates. Sticky
    /// clamping may absorb part of a correction; the caller permits a bounded repaint.
    pub(super) fn follow_keeps(&mut self) -> bool {
        self.end_takes_over();
        let mut moved = false;
        for k in &self.keeps {
            let Some(i) = self.nodes.iter().enumerate().position(|(i, n)| n.path == k.path && !self.subtree_exiting(i)) else { continue };
            let Some(s) = self.viewport_of(i).filter(|&s| self.nodes[s].path == k.viewport) else { continue };
            let scale = self.drawn_scale[s];
            if scale <= 1e-4 { continue; }
            let delta = (self.drawn[i][1] - self.drawn[s][1] - k.was) / scale;
            if delta.abs() < 1.0 / 64.0 { continue; }
            let stick = self.nodes[s].scroll.is_some_and(|s| s.stick_bottom);
            let Some(st) = self.scrolls.get_mut(&self.nodes[s].id) else { continue };
            let to = (st.offset + f64::from(delta)).clamp(0.0, st.max);
            if (to - st.offset).abs() < 1e-5 { continue; }
            st.offset = to;
            st.land(stick && delta >= 0.0);
            st.direct = true;
            moved = true;
        }
        for r in &self.scroll_reveals {
            let target = (0..self.nodes.len()).find(|&i| self.reveal_names(r, i))
                .map(|i| (i, self.drawn[i][1], self.drawn[i][3]))
                .or_else(|| {
                    let (i, [_, y0, _, y1]) = self.highlight_target_in(&self.nodes, r.scope.as_deref(), &r.key)?;
                    if self.subtree_exiting(i) { return None; }
                    let scale = self.drawn_scale.get(i).copied().unwrap_or(1.0);
                    let dy = (y0 - self.nodes[i].target[1]) * scale;
                    Some((i, self.drawn[i][1] + dy, (y1 - y0).max(0.0) * scale))
                });
            let Some((i, drawn_top, drawn_h)) = target else { continue };
            let Some(s) = self.viewport_of(i).filter(|&s| self.nodes[s].path == r.viewport) else { continue };
            let scale = self.drawn_scale[s];
            if scale <= 1e-4 { continue; }
            let Some(cover) = self.stuck_cover(&r.viewport, &r.key) else { continue };
            let cover = cover / f64::from(scale);
            let rel_top = f64::from((drawn_top - self.drawn[s][1]) / scale) - cover;
            let height = f64::from(drawn_h / scale) + cover;
            let viewport = f64::from(self.drawn[s][3] / scale);
            let margin = f64::from(self.nodes[s].scroll_edges.margin);
            let stick = self.nodes[s].scroll.is_some_and(|s| s.stick_bottom);
            let growing = self.scroll_stack(s).and_then(|c| self.stacks.get(&self.nodes[c].id)).is_some_and(|st| st.growing);
            let Some(st) = self.scrolls.get_mut(&self.nodes[s].id) else { continue };
            let top = st.offset + rel_top;
            let to = reveal::place(r.center, st.offset, top, height, viewport, margin).clamp(0.0, st.max);
            // A `Tx::scroll_center` glide follows its target once the target moves.
            if let ScrollMode::Glide { v, to: aim } = st.mode {
                if aim.offset().is_none_or(|g| (to - g).abs() >= 1.0 / 64.0) {
                    let aim = if let GlideTo::ShrunkEnd(_) = aim { GlideTo::ShrunkEnd(to) } else { GlideTo::Offset(to) };
                    st.mode = ScrollMode::Glide { v: scroll_offset::retarget_v(st.glide_spring(), st.offset, v, to), to: aim };
                }
                continue;
            }
            if (to - st.offset).abs() < 1.0 / 64.0 {
                if growing { st.unpin(); }
                continue;
            }
            st.offset = to;
            st.land(stick && !growing);
            st.direct = true;
            moved = true;
        }
        // Once the content end prevents holding the anchor, let stick-bottom own the
        // viewport again, including growth committed while the collapse is still settling.
        let mut keeps = std::mem::take(&mut self.keeps);
        keeps.retain(|k| {
            let Some(i) = self.nodes.iter().position(|n| n.path == k.path) else { return true };
            let Some(s) = self.viewport_of(i) else { return false };
            !(self.scrolls.get(&self.nodes[s].id).is_some_and(|st| st.pinned())
                && self.drawn[i][1] - self.drawn[s][1] > k.was + 0.5)
        });
        self.keeps = keeps;
        moved
    }

    /// A user or explicit scroll ends the holds and reveals that would pull the view back.
    pub(super) fn drop_scroll_holds(&mut self) {
        self.keeps.clear();
        self.scroll_reveals.clear();
    }

    /// Colour, hover or decorative motion must not extend the anchor's lifetime.
    pub(super) fn finish_keeps(&mut self) {
        if self.keeps.is_empty() && self.scroll_reveals.is_empty() { return; }
        let mut keeps = std::mem::take(&mut self.keeps);
        let mut reveals = std::mem::take(&mut self.scroll_reveals);
        let animating = |viewport: &str| self.nodes.iter().any(|n| {
            let within = n.path == viewport || (n.path.starts_with(viewport)
                && n.path.as_bytes().get(viewport.len()) == Some(&b'/'));
            within && (self.scene.store().is_animating(n.id, prop::FRAME)
                || self.scene.store().is_animating(n.id, prop::TRAVEL)
                || self.grows.get(&n.id).is_some_and(|g| g.moving())
                || self.scrolls.get(&n.id).is_some_and(|st| st.slack.moving()))
        });
        let gliding = |viewport: &str| self.path_ids.get(viewport)
            .and_then(|id| self.scrolls.get(id))
            .is_some_and(|st| st.glide().is_some());
        keeps.retain(|k| animating(&k.viewport));
        // A glide keeps its reveal until it lands; otherwise a reveal lasts while its viewport's
        // subtree moves with its target in it.
        reveals.retain(|r| gliding(&r.viewport) || (animating(&r.viewport) && self.reveal_mounted(r)));
        self.keeps = keeps;
        self.scroll_reveals = reveals;
    }

    /// Whether node `i` is reveal `r`'s target element.
    fn reveal_names(&self, r: &Reveal, i: usize) -> bool {
        let n = &self.nodes[i];
        n.named(&r.key) && !self.subtree_exiting(i)
            && n.parent.is_none_or(|p| super::in_scope(&self.nodes[p].path, r.scope.as_deref()))
    }

    /// Whether reveal `r`'s target element or highlight is mounted in its viewport.
    fn reveal_mounted(&self, r: &Reveal) -> bool {
        let in_viewport = |i: usize| self.viewport_of(i).is_some_and(|s| self.nodes[s].path == r.viewport);
        (0..self.nodes.len()).any(|i| self.reveal_names(r, i) && in_viewport(i))
            || self.highlight_target_in(&self.nodes, r.scope.as_deref(), &r.key)
                .is_some_and(|(i, _)| !self.subtree_exiting(i) && in_viewport(i))
    }

    /// What a reveal of row `key` in viewport `path` must leave to stuck headers: `None` when
    /// the row is, or is inside, a header stuck in view (it shows already), else the height
    /// stuck headers cover at the viewport's top, which the reveal keeps the row clear of.
    pub(super) fn stuck_cover(&self, path: &str, key: &str) -> Option<f64> {
        if !self.has_sticky || self.stuck.len() != self.nodes.len() || self.drawn.len() != self.nodes.len() { return Some(0.0) }
        let Some(s) = self.nodes.iter().position(|n| n.path == path) else { return Some(0.0) };
        let view = self.drawn[s];
        let row = self.nodes.iter().position(|n| n.key.as_deref() == Some(key) || n.path == key);
        let holder = row.and_then(|r| std::iter::successors(Some(r), |&p| self.nodes[p].parent).take_while(|&p| p != s).find(|&p| self.stuck[p] != 0.0 && self.nodes[p].sticky.as_ref().is_none_or(|s| s.edge != Some(StickyEdge::Left))));
        if let (Some(r), Some(_)) = (row, holder) {
            let d = self.drawn[r];
            if d[1] >= view[1] - 0.5 && d[1] + d[3] <= view[1] + view[3] + 0.5 { return None; }
        }
        let cover = (0..self.nodes.len())
            .filter(|&j| self.stuck[j] > 0.0 && self.nodes[j].sticky.as_ref().is_none_or(|s| s.edge != Some(StickyEdge::Left)) && Some(j) != holder && self.viewport_of(j) == Some(s))
            .map(|j| self.drawn[j][1] + self.drawn[j][3] - view[1])
            .fold(0.0f32, f32::max);
        Some(f64::from(cover))
    }
}
