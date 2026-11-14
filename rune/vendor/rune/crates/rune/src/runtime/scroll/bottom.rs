//! Where a scroll viewport stands against its bottom: `El::on_at_bottom` reports changes, and
//! `Tx::scroll_to_bottom` glides there (a chat's "jump to latest").
use super::*;

/// The glide's spring (`motion::GLIDE`) retargets each frame, so content growing under it
/// extends the trip.
/// A glide starts at most this many viewports above the bottom (a longer trip cuts first).
pub(super) const GLIDE_REACH: f64 = 2.0;
/// Within this many points of the bottom a viewport counts as showing it.
const AT_BOTTOM: f64 = 1.0;
/// Slack starts closing once its stack has stopped growing for this long (seconds).
const SLACK_WAIT: f64 = 0.12;

/// Room a pinned viewport gains at its bottom (a composer shrinking under it), held under its
/// growth stack's last row instead of carrying the rows down. Rows growing in fill it first;
/// once the stack has stopped growing for `SLACK_WAIT`, the rest closes on the growth spring.
/// Without it, a send dips the rows: the composer gives its room back faster than the new
/// row's capped growth takes it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Slack {
    /// `h` is the slack in points, stepped toward 0 once closing.
    track: GrowTrack,
    closing: bool,
    stepped: bool,
    acc: f32,
    /// The viewport's presented top and bottom last frame.
    edges: Option<[f32; 2]>,
    /// When a row of the stack last grew.
    grew_at: f64,
    /// The window size at the last commit: a resize lands the slack.
    pub(super) window: (f32, f32),
    /// `Tx::follow_edge`: until the viewport's edges reach their layout, the rows ride them
    /// and the room they free adds no slack.
    pub(super) follow: bool,
}

impl Slack {
    pub(super) fn height(&self) -> f32 { self.track.h }

    pub(super) fn moving(&self) -> bool { self.track.h > 0.0 || self.stepped }

    /// No slack, in a window of this size.
    pub(super) fn landed(window: (f32, f32)) -> Slack { Slack { window, ..Slack::default() } }
}

/// Steps a glide toward the current bottom, or the offset `Tx::scroll_to` asked for while the
/// content reaches it. Returns whether it is still moving.
pub(super) fn glide(st: &mut ScrollState, dt: f32) -> bool {
    let ScrollMode::Glide { v, to: aim } = st.mode else { return false };
    let to = aim.offset().map_or(st.max, |to| to.min(st.max));
    // Content shrinking past the glide's end lands it there, rather than its speed carrying
    // it past and springing back.
    if st.offset >= st.max && to >= st.max {
        st.offset = st.max;
        st.mode = ScrollMode::Rest;
        return false;
    }
    let mut glide = Some(v);
    let moving = scroll_offset::glide_axis(st.glide_spring(), &mut st.offset, &mut glide, to, dt);
    st.mode = match glide {
        Some(v) => ScrollMode::Glide { v, to: aim },
        None => ScrollMode::Rest,
    };
    moving
}

impl<S: 'static> Runtime<S> {
    /// The scroll viewport `key` names (`Node::named`), or the nearest one containing it.
    fn scroll_viewport(&self, key: &str) -> Option<usize> {
        let mut i = self.nodes.iter().enumerate()
            .position(|(i, n)| n.named(key) && !self.subtree_exiting(i))?;
        loop {
            if self.nodes[i].scroll.is_some() && self.nodes[i].scroll_axes.y() { return Some(i); }
            i = self.nodes[i].parent?;
        }
    }

    pub(super) fn scroll_to_bottom(&mut self, requests: Vec<(String, bool)>) {
        // This frame's commit may have changed the content: measure the extent first.
        self.step_scroll(0.0);
        let mut targets = Vec::new();
        for (key, animated) in requests {
            let Some(i) = self.scroll_viewport(&key) else { continue };
            let (id, sticky, view_h) = (self.nodes[i].id, self.nodes[i].scroll.is_some_and(|s| s.stick_bottom), f64::from(self.nodes[i].local.h));
            let Some(st) = self.scrolls.get_mut(&id) else { continue };
            targets.push((id, animated, sticky, view_h, st.offset, st.glide()));
            // Stand at the bottom first so a virtual list mounts and measures its tail rows,
            // which can differ from the estimate: the trip below then ends at the true bottom.
            st.offset = st.max;
            st.pin();
        }
        for _ in 0..3 {
            if !self.virtual_window_changed() { break; }
            self.commit(&Tx::default());
            self.step_scroll(0.0);
            for (id, ..) in &targets {
                if let Some(st) = self.scrolls.get_mut(id) { st.offset = st.max; }
            }
        }
        for (id, animated, sticky, view_h, start, glide) in targets {
            let Some(st) = self.scrolls.get_mut(&id) else { continue };
            // A settled frame (a window showing or uncovering) lands at the end.
            if animated && !self.reduced_motion && !self.settled && st.max - start > 0.5 {
                st.offset = start.max(st.max - GLIDE_REACH * view_h);
                st.mode = ScrollMode::Glide { v: glide.unwrap_or(0.0), to: GlideTo::Bottom };
                st.spring = None;
            } else {
                st.offset = st.max;
                if sticky { st.pin() } else { st.mode = ScrollMode::Rest }
            }
        }
        if self.virtual_window_changed() {
            self.commit(&Tx::default());
            self.step_scroll(0.0);
        }
    }

    /// `Tx::scroll_page`: each viewport moves by its requested pages.
    pub(super) fn scroll_pages(&mut self, requests: Vec<(String, i32)>) {
        // This frame's commit may have changed the content: measure the extent first.
        self.step_scroll(0.0);
        for (key, pages) in requests {
            let Some(i) = self.scroll_viewport(&key) else { continue };
            self.page_to(i, |offset, _, page| offset + page * f64::from(pages));
        }
        if self.virtual_window_changed() {
            self.commit(&Tx::default());
            self.step_scroll(0.0);
        }
    }

    /// The offset to draw stuck-to-bottom viewport `i` at, whose presented bottom edge is
    /// `bottom`: layout's offset keeps the content's target bottom on the viewport's target
    /// bottom, but while either is still animating (the viewport resizing under a banner)
    /// that would jump the rows and slide them back. This keeps the presented edges together.
    ///
    /// Rows that sat on the bottom hold it as the content crosses between overflowing and
    /// fitting: while they still end on the bottom (`justify_end`), or until the content's top
    /// would carry them above it, after which they ride the content's top. The offset goes
    /// negative while fitting rows sit low in a box that has not shrunk yet.
    pub(super) fn stuck_offset(&self, i: usize, bottom: f32) -> Option<f64> {
        let n = &self.nodes[i];
        let st = self.scrolls.get(&n.id).filter(|st| st.pinned())?;
        if !n.scroll.is_some_and(|s| s.stick_bottom) { return None; }
        let c = self.scroll_content(i)?;
        let lag = self.stuck_lag(i, c, bottom)?;
        // How far the rows end above the bottom in the layout (negative past it), beside the
        // stack's own bottom padding and border.
        let content_bottom = self.scroll_children(i)
            .map(|k| {
                let pad = self.stacks.get(&self.nodes[k].id).map_or(0.0, |s| (s.pad_bottom - self.nodes[k].inset[3]).max(0.0));
                self.nodes[k].target[3] - pad
            })
            .reduce(f32::max)?;
        let short = f64::from(n.target[3] - content_bottom);
        // Stuck on a shrinking stack's end: the rows' last fraction collapses under it (see `step_scroll`).
        let end = st.laid.filter(|&end| st.offset <= end + 1e-6).unwrap_or(f64::INFINITY);
        if st.held && (short <= 0.5 || lag - short >= -0.5) {
            Some(self.scroll_overflow(i).min(end) + lag)
        } else if st.max > 0.0 {
            Some((st.max.min(end) + lag).max(0.0))
        } else {
            // Content that fits the viewport's layout can still reach past its presented bottom
            // while the viewport grows toward it (a composer shrinking after a send): follow it
            // from there, not from when the layout overflows, which landed the gap in one frame.
            let at = self.scroll_overflow(i).min(end) + lag;
            (at > 0.0).then_some(at)
        }
    }

    /// The offset that draws viewport `i`'s content bottom on its presented bottom `bottom`.
    pub(super) fn bottom_offset(&self, i: usize, bottom: f32) -> Option<f64> {
        let c = self.scroll_content(i)?;
        Some(self.scroll_overflow(i) + self.stuck_lag(i, c, bottom)?)
    }

    /// Scroll viewport `i`'s present direct children.
    pub(super) fn scroll_children(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        (i + 1..self.nodes.len())
            .take_while(move |&j| self.nodes[j].parent >= Some(i))
            .filter(move |&j| self.nodes[j].parent == Some(i) && !self.is_exiting(self.nodes[j].id) && self.scene.contains(self.nodes[j].id))
    }

    /// Scroll viewport `i`'s growth-stack child, or the child that supplies its content end.
    pub(super) fn scroll_content(&self, i: usize) -> Option<usize> {
        let view = self.nodes[i].target;
        self.scroll_stack(i).or_else(|| {
            self.scroll_children(i).max_by(|&a, &b| {
                let end = |c: usize| {
                    let n = &self.nodes[c];
                    f64::from(n.target[1] - view[1]) + self.stack_extent(c).unwrap_or(f64::from(n.target[3] - n.target[1]))
                };
                end(a).total_cmp(&end(b))
            })
        })
    }

    /// Scroll viewport `i`'s growth-stack child, if any.
    pub(super) fn scroll_stack(&self, i: usize) -> Option<usize> {
        self.scroll_children(i).find(|&c| self.nodes[c].growth.is_some())
    }

    /// How much further content `c` of viewport `i` has to travel to its layout than the
    /// viewport's presented bottom `bottom` has.
    fn stuck_lag(&self, i: usize, c: usize, bottom: f32) -> Option<f64> {
        let top = self.scene.presented(self.nodes[c].id)?.rect.edges()[1];
        Some(f64::from((top - self.nodes[c].target[1]) - (bottom - self.nodes[i].target[3])))
    }

    /// Steps stick-to-bottom viewport `i`'s `Slack` after its offset. Returns whether any is left.
    pub(super) fn step_slack(&mut self, i: usize, dt: f32) -> bool {
        let id = self.nodes[i].id;
        if self.scrolls.get(&id).is_some_and(|st| !st.pinned() && !st.slack.moving()) {
            if let Some(st) = self.scrolls.get_mut(&id) { st.slack.edges = None; }
            return false;
        }
        let Some(c) = self.scroll_content(i) else { return false };
        let stack = self.nodes[c].growth.as_deref().copied().and_then(|spec| Some((spec, self.stacks.get_mut(&self.nodes[c].id)?)));
        let view = self.scene.presented(id).map(|p| p.rect.edges());
        let (Some((spec, stack)), Some(view), false) = (stack, view, self.reduced_motion) else {
            if let Some(st) = self.scrolls.get_mut(&id) { st.slack = Slack::landed(st.slack.window); }
            return false;
        };
        let grown = std::mem::take(&mut stack.grown);
        let growing = stack.growing;
        let pad = (stack.pad_bottom - self.nodes[c].inset[3]).max(0.0);
        let short = self.nodes[i].target[3] - (self.nodes[c].target[3] - pad);
        let top_c = self.scene.presented(self.nodes[c].id).map_or(view[1], |p| p.rect.edges()[1]);
        let limit = top_c + (self.nodes[c].target[3] - self.nodes[c].target[1] - pad);
        let now = self.t;
        let target = self.nodes[i].target;
        let Some(st) = self.scrolls.get_mut(&id) else { return false };
        let pinned = st.pinned();
        let s = &mut st.slack;
        let h0 = s.track.h;
        let [top, bottom] = [view[1], view[3]];
        if growing {
            s.grew_at = now;
            s.closing = false;
            s.track.v = 0.0;
        }
        if pinned {
            // Growth fills the slack first. Room the bottom edge gains while the rows sit on it
            // joins it, unless the whole viewport moved down with it.
            let mut h = s.track.h - grown;
            if let Some([top0, bottom0]) = s.edges && st.held && !s.follow {
                let (b0, b1) = if short > 0.5 { (bottom0.min(limit), bottom.min(limit)) } else { (bottom0, bottom) };
                h += (b1 - b0) - (top - top0).max(0.0);
            }
            s.track.h = h.max(0.0);
        }
        s.edges = Some([top, bottom]);
        if s.follow && (top - target[1]).abs() < 0.01 && (bottom - target[3]).abs() < 0.01 { s.follow = false; }
        if s.track.h <= 1.0 / 128.0 {
            s.track = GrowTrack::default();
            s.closing = false;
        } else if !s.closing && !growing && now - s.grew_at >= SLACK_WAIT {
            s.closing = true;
            s.acc = 0.0;
            s.track.age = 0.0;
            s.track.v = 0.0;
        }
        if s.closing {
            s.acc += dt;
            while s.acc >= SUBSTEP {
                s.track.step(spec.spring, &spec.edge, false);
                s.acc -= SUBSTEP;
            }
            if !s.track.moving() { s.track = GrowTrack::default(); s.closing = false; }
        }
        s.stepped = (s.track.h - h0).abs() > 1e-5;
        s.moving()
    }

    /// Queues `on_at_bottom` and `on_far_from_bottom` for viewports whose state changed this
    /// frame.
    pub(super) fn report_at_bottom(&mut self) {
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            let Some(h) = n.handlers.get().filter(|h| h.on_at_bottom.is_some() || h.on_far_from_bottom.is_some()) else { continue };
            if self.subtree_exiting(i) { continue; }
            let Some(st) = self.scrolls.get_mut(&n.id) else { continue };
            if h.on_at_bottom.is_some() {
                let at = st.offset >= st.max - AT_BOTTOM;
                if st.at_bottom != Some(at) {
                    st.at_bottom = Some(at);
                    self.queue.push(Action::AtBottom(n.path.clone(), at));
                }
            }
            if let Some((min, _)) = &h.on_far_from_bottom {
                // Rows collapsing measure from the end they head for, glide or not.
                let end = st.laid.map_or(st.max, |end| end.min(st.max));
                let far = end - st.offset > f64::from(min.max(n.local.h));
                if st.far_from_bottom != Some(far) {
                    st.far_from_bottom = Some(far);
                    self.queue.push(Action::FarFromBottom(n.path.clone(), far));
                }
            }
        }
    }

    /// Whether the scroll viewport keyed `key` (or the one containing it) shows its bottom,
    /// as of the last frame.
    pub fn scroll_at_bottom(&self, key: &str) -> Option<bool> {
        let i = self.scroll_viewport(key)?;
        self.scrolls.get(&self.nodes[i].id).map(|st| st.offset >= st.max - AT_BOTTOM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::el::{col, el};

    // One state type for every test here: each type monomorphises a whole `Runtime<S>`.
    #[derive(Default)]
    struct Chat { rows: usize, reports: Vec<bool>, instant: bool, reduced: bool, virtualized: bool, loose: bool, tall_tail: bool, composer: Option<crate::TextEdit>, sends: bool, track_drawn: bool, drawn: Vec<f32>, far: Vec<bool>, dock: Option<f32> }

    fn view(s: &Chat, cx: &mut Cx<Chat>) -> El {
        cx.reduced_motion(s.reduced);
        let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
        if s.virtualized { stack = stack.virtualize(60.0); }
        for i in 0..s.rows {
            let h = if s.tall_tail && i + 10 >= s.rows { 150.0 } else { 40.0 + (i % 3) as f32 * 10.0 };
            let mut row = el().key(format!("r{i}")).w_full().h(h);
            if i == 0 && s.track_drawn {
                row = row.on_drawn_layout(|s: &mut Chat, r| s.drawn.push(r[1]));
            }
            stack = stack.child(row);
        }
        let root = col().w_full().h_full()
            .child(col().key("scroll").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: !s.loose })
                .on_at_bottom(|s: &mut Chat, at| s.reports.push(at))
                .on_far_from_bottom(400.0, |s: &mut Chat, far| s.far.push(far))
                .child(stack))
            .child(el().key("jump").w_full().h(40.0).on_press_tx(|s: &mut Chat, tx| {
                if s.instant { tx.scroll_to_bottom_instant("stack") } else { tx.scroll_to_bottom("scroll") }
            }));
        // A dock under the list that grows and shrinks with no row coming in.
        let root = match s.dock { Some(h) => root.child(el().key("dock").w_full().h(h)), None => root };
        // A multi-line composer below the list, in its own small scroll area.
        let Some(edit) = &s.composer else { return root };
        let field = crate::text_input(edit, crate::TextStyle::new(14.0, 400.0)).key("composer").w_full()
            .on_edit(|s: &mut Chat| s.composer.as_mut().unwrap());
        // A chat's composer: sending clears it, so it shrinks on its spring as the row grows in.
        if s.sends {
            return root.child(field.animate_edit_layout().on_submit(|s: &mut Chat, _| {
                s.composer.as_mut().unwrap().clear();
                s.rows += 1;
            }));
        }
        root.child(col().key("composer-scroll").w_full().h(60.0).scroll(ScrollSpec { stick_bottom: false }).child(field))
    }

    fn chat(state: Chat) -> Runtime<Chat> {
        let mut rt = Runtime::new(state, view, (200.0, 340.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 120.0);
        rt
    }

    fn press(rt: &mut Runtime<Chat>, key: &str) {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    }

    fn bottom_gap(rt: &Runtime<Chat>) -> f32 {
        let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&*format!("r{}", rt.state.rows - 1))).unwrap();
        300.0 - (last.rect[1] + last.rect[3])
    }

    #[test]
    fn at_bottom_reports_once_per_change_and_glides_back() {
        let mut rt = chat(Chat { rows: 40, ..Default::default() });
        assert_eq!(rt.state.reports, [true], "the first layout reports its state");
        assert_eq!(rt.scroll_at_bottom("scroll"), Some(true));
        for _ in 0..5 {
            rt.wheel(100.0, 100.0, 300.0);
            rt.frame(1.0 / 120.0);
        }
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.reports, [true, false], "scrolling further up does not report again");
        let commits = rt.stats.commits;
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        assert_eq!(rt.stats.commits, commits, "no per-frame callbacks or commits at rest");
        assert!(bottom_gap(&rt) < -1000.0, "reading far up: {}", bottom_gap(&rt));

        press(&mut rt, "jump");
        rt.frame(1.0 / 120.0);
        let start = bottom_gap(&rt);
        assert!((-600.0..0.0).contains(&start), "a long trip cuts to two viewports from the end: {start}");
        let mut previous = start;
        let mut frames = 0;
        while rt.state.reports.len() < 3 && frames < 240 {
            rt.frame(1.0 / 120.0);
            let gap = bottom_gap(&rt);
            assert!(gap >= previous - 0.01, "the glide only moves toward the bottom: {previous} -> {gap}");
            previous = gap;
            frames += 1;
        }
        assert!(frames > 10, "the glide is animated ({frames} frames)");
        assert_eq!(rt.state.reports, [true, false, true]);
        for _ in 0..60 { rt.frame(1.0 / 120.0); }
        assert!(bottom_gap(&rt).abs() < 0.05, "{}", bottom_gap(&rt));
        assert!(!rt.animating);
        rt.state.rows += 1;
        rt.invalidate();
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        assert!(bottom_gap(&rt).abs() < 0.05, "a sticky viewport follows the end again after landing");
        assert_eq!(rt.state.reports, [true, false, true]);
        assert!(!rt.animating);
    }

    /// Far from the bottom means more than the viewport's height (300 here) and at least
    /// `min` (400) above it; a glide back reports near before it lands.
    #[test]
    fn far_from_bottom_reports_past_its_threshold_only() {
        let mut rt = chat(Chat { rows: 40, ..Default::default() });
        assert_eq!(rt.state.far, [false], "the first layout reports its state");
        rt.update(|_, tx| tx.scroll_to_y("scroll", 1e6));
        let max = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("stack")).unwrap().rect[3] - 300.0;
        rt.update(|_, tx| tx.scroll_to_y("scroll", max - 350.0));
        for _ in 0..3 { rt.frame(1.0 / 120.0); }
        assert_eq!(rt.state.far, [false], "350 above the bottom is within 400");
        rt.update(|_, tx| tx.scroll_to_y("scroll", max - 450.0));
        for _ in 0..3 { rt.frame(1.0 / 120.0); }
        assert_eq!(rt.state.far, [false, true]);
        let commits = rt.stats.commits;
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        assert_eq!(rt.stats.commits, commits, "no per-frame callbacks at rest");
        press(&mut rt, "jump");
        let mut near_at = None;
        for f in 0..240 {
            rt.frame(1.0 / 120.0);
            if near_at.is_none() && rt.state.far.len() == 3 { near_at = Some((f, bottom_gap(&rt))); }
        }
        let (_, gap) = near_at.expect("the glide reports near");
        assert!((-400.0..0.0).contains(&gap), "near once within 400 of the end, before landing: {gap}");
        assert_eq!(rt.state.far, [false, true, false]);
    }

    /// A scroll to the bottom asked for while the window is covered (or hidden) waits for the
    /// frame that uncovers it, which settles: that frame shows the bottom, with no glide after.
    /// (Settling already pins a sticky viewport; this one isn't.)
    #[test]
    fn a_scroll_asked_while_covered_lands_when_it_uncovers() {
        let mut rt = chat(Chat { rows: 40, loose: true, ..Default::default() });
        for _ in 0..5 {
            rt.wheel(100.0, 100.0, 300.0);
            rt.frame(1.0 / 120.0);
        }
        for _ in 0..60 { rt.frame(1.0 / 120.0); }
        assert!(bottom_gap(&rt) < -1000.0, "reading far up: {}", bottom_gap(&rt));
        rt.set_live_visible(false);
        rt.update(|_, tx| tx.scroll_to_bottom("scroll"));
        rt.hidden_wake();
        assert!(bottom_gap(&rt) < -1000.0, "nothing moves while covered");
        rt.set_live_visible(true);
        rt.settle_all();
        rt.frame(1.0 / 120.0);
        assert!(bottom_gap(&rt).abs() < 0.05, "the uncovering frame shows the bottom: {}", bottom_gap(&rt));
        assert!(!rt.animating, "with no glide left to play");
    }

    #[test]
    fn an_update_that_changes_nothing_still_scrolls() {
        let mut rt = chat(Chat { rows: 40, instant: true, ..Default::default() });
        for _ in 0..5 {
            rt.wheel(100.0, 100.0, 300.0);
            rt.frame(1.0 / 120.0);
        }
        for _ in 0..60 { rt.frame(1.0 / 120.0); }
        assert_eq!(rt.scroll_at_bottom("scroll"), Some(false));
        assert!(rt.update_if(|_, tx| { tx.scroll_to_bottom_instant("stack"); false }), "a scroll takes a frame");
        assert!(rt.stale());
        for _ in 0..3 { rt.frame(1.0 / 120.0); }
        assert_eq!(rt.scroll_at_bottom("scroll"), Some(true));
    }

    #[test]
    fn wheel_interrupts_a_glide() {
        let mut rt = chat(Chat { rows: 40, ..Default::default() });
        rt.wheel(100.0, 100.0, 400.0);
        rt.frame(1.0 / 120.0);
        press(&mut rt, "jump");
        for _ in 0..4 { rt.frame(1.0 / 120.0); }
        rt.wheel(100.0, 100.0, 50.0);
        rt.frame(1.0 / 120.0);
        let held = bottom_gap(&rt);
        for _ in 0..60 { rt.frame(1.0 / 120.0); }
        assert_eq!(bottom_gap(&rt), held, "the hand takes over from the glide");
        assert_eq!(rt.scroll_at_bottom("r3"), Some(false), "a row finds its viewport");
    }

    #[test]
    fn instant_and_reduced_motion_land_in_the_same_frame() {
        for state in [Chat { rows: 40, instant: true, ..Default::default() }, Chat { rows: 40, reduced: true, ..Default::default() }] {
            let mut rt = chat(state);
            rt.wheel(100.0, 100.0, 800.0);
            rt.frame(1.0 / 120.0);
            press(&mut rt, "jump");
            rt.frame(1.0 / 120.0);
            assert!(bottom_gap(&rt).abs() < 0.05, "{}", bottom_gap(&rt));
            rt.frame(1.0 / 120.0);
            assert_eq!(rt.state.reports, [true, false, true]);
        }
    }

    /// A hidden host's poll that scrolls changes no state, yet the last frame no longer shows
    /// the window: a launcher's cached reveal must refresh. A poll that only schedules a wake
    /// leaves it fresh.
    #[test]
    fn a_hidden_poll_that_scrolls_leaves_the_last_frame_out_of_date() {
        let mut rt = chat(Chat { rows: 40, ..Default::default() });
        rt.wheel(100.0, 100.0, 800.0);
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        let up = bottom_gap(&rt);
        assert!(up < -100.0, "{up}");
        rt.poll = Some(|s: &mut Chat, _, tx: &mut Tx| {
            tx.wake_after(std::time::Duration::from_secs(1));
            if std::mem::take(&mut s.instant) { tx.scroll_to_bottom_instant("stack"); }
            false
        });
        assert!(!rt.poll_hidden(), "a wake alone draws nothing new");
        assert!(!rt.needs_commit());
        rt.state.instant = true;
        assert!(rt.poll_hidden(), "a scroll asked for while hidden schedules a hidden refresh");
        assert!(rt.needs_commit(), "and the cached frame isn't fresh");
        assert_eq!(bottom_gap(&rt), up, "nothing scrolled while hidden");
        rt.frame(0.0);
        assert!(bottom_gap(&rt).abs() < 0.05, "{}", bottom_gap(&rt));
        assert!(!rt.needs_commit());
    }

    #[test]
    fn glides_to_the_exact_bottom_of_a_virtual_list() {
        let mut rt = chat(Chat { rows: 600, virtualized: true, ..Default::default() });
        for _ in 0..40 {
            rt.wheel(100.0, 100.0, 2000.0);
            rt.frame(1.0 / 120.0);
        }
        assert!(rt.nodes.iter().all(|n| n.key.as_deref() != Some("r599")), "the tail is unmounted while reading the top");
        press(&mut rt, "jump");
        for _ in 0..240 { rt.frame(1.0 / 120.0); }
        assert!(bottom_gap(&rt).abs() < 0.05, "{}", bottom_gap(&rt));
        assert_eq!(rt.state.reports.last(), Some(&true));
        assert!(!rt.animating);
    }

    fn max(rt: &Runtime<Chat>) -> f64 {
        let i = rt.scroll_viewport("scroll").unwrap();
        rt.scrolls[&rt.nodes[i].id].max
    }

    /// Reads far up a virtual list whose unmounted tail rows are much taller than the estimate.
    fn far_from_a_tall_tail(state: Chat) -> Runtime<Chat> {
        let mut rt = chat(Chat { rows: 600, virtualized: true, tall_tail: true, ..state });
        for _ in 0..40 {
            rt.wheel(100.0, 100.0, 2000.0);
            rt.frame(1.0 / 120.0);
        }
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        assert!(rt.nodes.iter().all(|n| n.key.as_deref() != Some("r599")), "the tail is unmounted");
        rt
    }

    #[test]
    fn instant_lands_on_an_unmeasured_virtual_tail() {
        for loose in [true, false] {
            let mut rt = far_from_a_tall_tail(Chat { instant: true, loose, ..Default::default() });
            press(&mut rt, "jump");
            rt.frame(1.0 / 120.0);
            assert!(bottom_gap(&rt).abs() < 0.05, "loose {loose}: {}", bottom_gap(&rt));
            for _ in 0..10 { rt.frame(1.0 / 120.0); }
            assert!(bottom_gap(&rt).abs() < 0.05, "loose {loose}: stays at the bottom: {}", bottom_gap(&rt));
        }
    }

    #[test]
    fn a_glide_measures_the_virtual_tail_before_it_starts() {
        for loose in [true, false] {
            let mut rt = far_from_a_tall_tail(Chat { loose, ..Default::default() });
            press(&mut rt, "jump");
            rt.frame(1.0 / 120.0);
            let trip = max(&rt);
            assert!(bottom_gap(&rt) >= -2.0 * 300.0 - 0.5, "loose {loose}: starts two viewports from the true bottom: {}", bottom_gap(&rt));
            let mut frames = 0;
            while rt.animating && frames < 240 {
                rt.frame(1.0 / 120.0);
                assert_eq!(max(&rt), trip, "loose {loose}: the bottom holds still mid-glide (frame {frames})");
                frames += 1;
            }
            assert!(frames > 10, "loose {loose}: animated ({frames} frames)");
            assert!(bottom_gap(&rt).abs() < 0.05, "loose {loose}: {}", bottom_gap(&rt));
        }
    }

    fn scroll_state<'a>(rt: &'a Runtime<Chat>, key: &str) -> &'a ScrollState {
        let i = rt.scroll_viewport(key).unwrap();
        &rt.scrolls[&rt.nodes[i].id]
    }

    fn page_height(rt: &Runtime<Chat>, key: &str) -> f64 {
        f64::from(rt.nodes[rt.scroll_viewport(key).unwrap()].local.h * 0.9)
    }

    fn key(rt: &mut Runtime<Chat>, key: crate::Key) {
        rt.key(KeyEvent { key, mods: Default::default() });
        rt.frame(1.0 / 120.0);
    }

    fn with_composer(text: &str) -> Runtime<Chat> {
        let mut edit = crate::TextEdit::new(text);
        edit.focused = true;
        chat(Chat { rows: 40, composer: Some(edit), ..Default::default() })
    }

    #[test]
    fn page_keys_from_a_short_composer_page_the_message_list_beside_it() {
        let mut rt = with_composer("hello");
        assert_eq!(scroll_state(&rt, "composer-scroll").max, 0.0, "the draft fits its scroll area");
        let (bottom, page) = (max(&rt), page_height(&rt, "scroll"));
        assert!(bottom > 3.0 * page);
        assert_eq!(scroll_state(&rt, "scroll").offset, bottom);
        key(&mut rt, crate::Key::PageUp);
        assert_eq!(scroll_state(&rt, "scroll").offset, bottom - page, "the list pages, not the composer");
        assert!(!scroll_state(&rt, "scroll").pinned());
        // The pointer over the composer doesn't claim the keys either.
        let (x, y) = rt.find("composer").unwrap();
        rt.pointer_move(x, y);
        key(&mut rt, crate::Key::PageUp);
        assert_eq!(scroll_state(&rt, "scroll").offset, bottom - 2.0 * page);
        key(&mut rt, crate::Key::PageDown);
        key(&mut rt, crate::Key::PageDown);
        assert_eq!(scroll_state(&rt, "scroll").offset, bottom);
        assert!(scroll_state(&rt, "scroll").pinned(), "back on the end, the list follows it again");
        let composer = rt.state.composer.as_ref().unwrap();
        assert!(composer.focused);
        assert_eq!(composer.text, "hello");
    }

    #[test]
    fn an_overflowing_composer_keeps_the_page_keys_even_at_its_edge() {
        let mut rt = with_composer(&vec!["line"; 12].join("\n"));
        let list = max(&rt);
        let composer = scroll_state(&rt, "composer-scroll").max;
        assert!(composer > 0.0, "the draft overflows: {composer}");
        for _ in 0..10 { key(&mut rt, crate::Key::PageUp); }
        assert_eq!(scroll_state(&rt, "composer-scroll").offset, 0.0, "the composer paged to its top");
        assert_eq!(scroll_state(&rt, "scroll").offset, list, "and the list never moved, as an NSTextView keeps them");
    }

    #[test]
    fn tx_scroll_page_pages_a_viewport_and_repins_its_end() {
        let mut rt = chat(Chat { rows: 40, ..Default::default() });
        let (bottom, page) = (max(&rt), page_height(&rt, "scroll"));
        assert!(rt.update_if(|_, tx| { tx.scroll_page("scroll", -2); false }), "a page takes a frame");
        rt.frame(1.0 / 120.0);
        assert_eq!(scroll_state(&rt, "scroll").offset, bottom - 2.0 * page);
        assert_eq!(rt.scroll_at_bottom("scroll"), Some(false));
        let row = rt.elements().into_iter().find(|e| e.rect[1] > 0.0 && e.rect[1] < 200.0 && e.key.as_deref().is_some_and(|k| k.starts_with('r'))).unwrap().key.unwrap();
        rt.update_if(move |_, tx| { tx.scroll_page(row, 1); false });
        rt.frame(1.0 / 120.0);
        assert_eq!(scroll_state(&rt, "scroll").offset, bottom - page, "a row's key pages the viewport it's in");
        rt.update_if(|_, tx| { tx.scroll_page("scroll", 5); false });
        rt.frame(1.0 / 120.0);
        assert_eq!(scroll_state(&rt, "scroll").offset, bottom, "clamped at the end");
        assert!(scroll_state(&rt, "scroll").pinned());
        rt.state.rows += 5;
        rt.invalidate();
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        assert!(bottom_gap(&rt).abs() < 0.05, "a stick_bottom list follows new rows again: {}", bottom_gap(&rt));
        assert!(!rt.animating);
    }

    #[test]
    fn tx_scroll_page_finds_the_list_of_a_key_inside_a_virtual_row() {
        let mut rt = chat(Chat { rows: 600, virtualized: true, ..Default::default() });
        let (bottom, page) = (max(&rt), page_height(&rt, "scroll"));
        assert!(rt.nodes.iter().all(|n| n.key.as_deref() != Some("r0")), "the head is unmounted");
        rt.update_if(|_, tx| { tx.scroll_page("r599", -3); false });
        rt.frame(1.0 / 120.0);
        // Rows mounting on the way measure against their estimate, which moves the extent a little.
        let st = scroll_state(&rt, "scroll");
        let pages = (st.max - st.offset) / page;
        assert!((2.5..3.5).contains(&pages), "three pages above the end: {pages} (bottom was {bottom})");
        assert!(!scroll_state(&rt, "scroll").pinned());
        let shown = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with('r')) && e.rect[1] + e.rect[3] > 0.0 && e.rect[1] < 300.0).count();
        assert!(shown >= 5, "the window mounted the rows it pages to ({shown})");
    }

    /// The rects of this frame's keyed elements.
    fn rects(rt: &Runtime<Chat>) -> std::collections::HashMap<String, [f32; 4]> {
        rt.elements().into_iter().filter_map(|e| Some((e.key?, e.rect))).collect()
    }

    /// How often `ys` turns around (moves under 0.01 pt don't count).
    fn reversals(ys: &[f32]) -> usize {
        let (mut dir, mut turns) = (0.0, 0);
        for w in ys.windows(2) {
            let d = w[1] - w[0];
            if d.abs() < 0.01 { continue; }
            if dir != 0.0 && d.signum() != dir { turns += 1; }
            dir = d.signum();
        }
        turns
    }

    /// The gap between the last row's bottom and the list viewport's.
    fn slack_gap(rt: &Runtime<Chat>) -> f32 {
        let r = rects(rt);
        let (view, last) = (r["scroll"], r[&format!("r{}", rt.state.rows - 1)]);
        view[1] + view[3] - (last[1] + last[3])
    }

    fn growing(rt: &Runtime<Chat>) -> bool {
        rt.grows.values().any(|g| g.target > g.h + 1.0 / 128.0)
    }

    fn sending(lines: usize) -> Runtime<Chat> {
        let mut edit = crate::TextEdit::new(&vec!["line"; lines].join("\n"));
        edit.focused = true;
        chat(Chat { rows: 40, composer: Some(edit), sends: true, ..Default::default() })
    }

    fn enter(rt: &mut Runtime<Chat>) {
        rt.key(KeyEvent { key: crate::Key::Enter, mods: Default::default() });
    }

    struct Sent { rt: Runtime<Chat>, ys: Vec<Vec<f32>>, shrank: f32, grew_until: usize, frames: usize }

    /// Sends a draft of `lines` lines from a composer under a pinned growth stack, and records
    /// the rows on screen at 120 fps until everything rests.
    fn send(lines: usize) -> Sent {
        let mut rt = sending(lines);
        let before = rects(&rt);
        let view = before["scroll"];
        let shown: Vec<String> = (0..40).map(|i| format!("r{i}"))
            .filter(|k| before[k][1] + before[k][3] > view[1] && before[k][1] < view[1] + view[3]).collect();
        enter(&mut rt);
        let mut ys: Vec<Vec<f32>> = shown.iter().map(|k| vec![before[k][1]]).collect();
        let (mut grew_until, mut frames) = (0, 0);
        while frames < 600 && (frames < 2 || rt.animating) {
            rt.frame(1.0 / 120.0);
            frames += 1;
            if growing(&rt) { grew_until = frames; }
            let r = rects(&rt);
            for (k, y) in shown.iter().zip(&mut ys) { y.push(r[k][1]); }
            assert_eq!(rt.scroll_at_bottom("scroll"), Some(true), "the list reports its bottom through the slack");
        }
        assert_eq!(rt.state.rows, 41, "the draft was sent");
        let shrank = before["composer"][3] - rects(&rt)["composer"][3];
        Sent { rt, ys, shrank, grew_until, frames }
    }

    /// Sending shrinks the composer under a pinned list while the new row grows in on its
    /// slower-starting spring. The freed room waits under the last row, so rows on screen only
    /// rise; nothing dips and comes back.
    #[test]
    fn a_send_only_moves_the_rows_up() {
        let Sent { rt, ys, shrank, .. } = send(3);
        assert!(shrank > 30.0, "the composer gave back its room: {shrank}");
        for y in &ys {
            assert!(y.windows(2).all(|w| w[1] <= w[0] + 0.01), "a row moved down: {y:?}");
            assert!(y[0] - y[y.len() - 1] > 5.0, "the row rose by the new row less the freed room: {y:?}");
        }
        assert!(slack_gap(&rt).abs() < 0.05, "the last row ends on the bottom: {}", slack_gap(&rt));
        assert_eq!(rt.state.reports, [true]);
    }

    /// Room freed beyond what the new row fills stays under it until the growth has stopped
    /// for a moment, then closes on the growth spring.
    #[test]
    fn room_the_new_row_does_not_fill_closes_after_its_growth() {
        let Sent { rt, ys, shrank, grew_until, frames } = send(7);
        assert!(shrank > 80.0, "{shrank}");
        let mut first_down: Option<usize> = None;
        for y in &ys {
            assert!(reversals(y) <= 1, "{y:?}");
            if let Some(k) = y.windows(2).position(|w| w[1] > w[0] + 0.01) {
                first_down = Some(first_down.map_or(k + 1, |f| f.min(k + 1)));
            }
        }
        let first_down = first_down.expect("the slack closed by moving the rows down");
        assert!(first_down >= grew_until + 14, "the slack waits 120 ms after growth: down at {first_down}, grew until {grew_until}");
        assert!(frames < 240, "and closes in time ({frames} frames)");
        assert!(slack_gap(&rt).abs() < 0.05, "{}", slack_gap(&rt));
    }

    /// The gap between the last row and the list's bottom each frame as the dock under it
    /// changes to `h` on a tween, following its edge or not.
    fn dock_gaps(rt: &mut Runtime<Chat>, h: f32, follow: bool) -> Vec<f32> {
        rt.update(|s, tx| {
            s.dock = Some(h);
            tx.motion_curve(rune_motion::Curve::tween(0.3, rune_motion::Easing::EASE_OUT));
            if follow { tx.follow_edge(); }
        });
        let mut gaps = vec![slack_gap(rt)];
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
            gaps.push(slack_gap(rt));
        }
        gaps
    }

    /// With `Tx::follow_edge`, a pinned list rides a dock's top edge both ways on the dock's
    /// curve, frame by frame. Without it the room a shrinking dock frees waits under the last
    /// row as slack, as a send's does, and closes late.
    #[test]
    fn follow_edge_carries_pinned_rows_with_a_dock() {
        let mut rt = chat(Chat { rows: 40, dock: Some(160.0), ..Default::default() });
        for (h, way) in [(40.0, "shrinking"), (160.0, "growing"), (40.0, "shrinking again")] {
            let gaps = dock_gaps(&mut rt, h, true);
            let worst = gaps.iter().map(|g| g.abs()).fold(0.0, f32::max);
            assert!(worst < 0.05, "{way}: the rows leave the edge by {worst}: {gaps:?}");
        }
        assert!(rects(&rt)["dock"][3] < 41.0, "the dock moved");
        rt.update(|s, tx| { s.dock = Some(160.0); tx.settle(); });
        rt.frame(1.0 / 120.0);
        let held = dock_gaps(&mut rt, 40.0, false).into_iter().fold(0.0, f32::max);
        assert!(held > 30.0, "without it the freed room waits as slack: {held}");
    }

    /// A hand on the list during the slack moves the rows from where they are drawn.
    #[test]
    fn scrolling_up_through_slack_does_not_jump() {
        let mut rt = sending(7);
        enter(&mut rt);
        while growing(&rt) || rt.state.rows < 41 { rt.frame(1.0 / 120.0); }
        rt.frame(1.0 / 120.0);
        let gap = slack_gap(&rt);
        assert!(gap > 10.0, "slack is held under the last row: {gap}");
        let y = rects(&rt)["r30"][1];
        rt.wheel(100.0, 100.0, 30.0);
        rt.frame(1.0 / 120.0);
        let moved = rects(&rt)["r30"][1] - y;
        assert!((moved - 30.0).abs() < 1.0, "the wheel moves the rows 30 pt from where they were: {moved}");
        assert_eq!(rt.scroll_at_bottom("scroll"), Some(false));
        for _ in 0..240 { rt.frame(1.0 / 120.0); }
        assert!(!rt.animating);
        assert!((slack_gap(&rt) + 30.0).abs() < 0.05, "the slack closed under the reader: {}", slack_gap(&rt));
    }

    /// A window growing taller opens no slack: the rows stay on the bottom.
    #[test]
    fn a_taller_window_keeps_the_rows_on_the_bottom() {
        let mut rt = sending(7);
        enter(&mut rt);
        while growing(&rt) || rt.state.rows < 41 { rt.frame(1.0 / 120.0); }
        assert!(slack_gap(&rt) > 10.0);
        rt.set_viewport((200.0, 400.0), 2.0);
        rt.frame(1.0 / 120.0);
        assert!(slack_gap(&rt).abs() < 0.05, "a resize lands the slack: {}", slack_gap(&rt));
        for k in 1..8 {
            rt.set_viewport((200.0, 400.0 + 10.0 * k as f32), 2.0);
            rt.frame(1.0 / 120.0);
            assert!(slack_gap(&rt).abs() < 0.05, "the rows follow the bottom: {}", slack_gap(&rt));
        }
    }

    /// An instant change (a zoom, say) lands the slack with it, not on a glide a moment later.
    #[test]
    fn an_instant_change_lands_the_slack_at_once() {
        let mut rt = sending(7);
        let y = rects(&rt)["r35"][1];
        rt.update(|s, tx| { tx.instant(); s.composer.as_mut().unwrap().clear() });
        rt.frame(1.0 / 120.0);
        let landed = rects(&rt)["r35"][1];
        assert!(landed - y > 80.0, "the rows came down with the change: {y} -> {landed}");
        assert!(slack_gap(&rt).abs() < 0.05, "{}", slack_gap(&rt));
        for _ in 0..60 { rt.frame(1.0 / 120.0); }
        assert_eq!(rects(&rt)["r35"][1], landed, "and stayed there");
    }

    /// A composer shrinking with nothing growing in lets the rows down after it, once.
    #[test]
    fn a_composer_shrinking_alone_lets_the_rows_down_once() {
        let mut rt = sending(7);
        let mut ys = vec![rects(&rt)["r35"][1]];
        rt.update(|s, _| s.composer.as_mut().unwrap().clear());
        for _ in 0..240 {
            rt.frame(1.0 / 120.0);
            ys.push(rects(&rt)["r35"][1]);
        }
        assert!(ys.windows(2).all(|w| w[1] >= w[0] - 0.01), "{ys:?}");
        assert!(ys[ys.len() - 1] - ys[0] > 80.0, "{ys:?}");
        assert!(slack_gap(&rt).abs() < 0.05, "{}", slack_gap(&rt));
        assert!(!rt.animating);
    }

    /// A row removed at the bottom closes and the rows above come down once, never rising.
    #[test]
    fn a_row_removed_at_the_bottom_lets_the_rows_down_once() {
        let mut rt = chat(Chat { rows: 40, ..Default::default() });
        rt.state.rows = 39;
        rt.invalidate();
        let mut ys = Vec::new();
        for _ in 0..240 {
            rt.frame(1.0 / 120.0);
            ys.push(rects(&rt)["r35"][1]);
        }
        assert!(ys.windows(2).all(|w| w[1] >= w[0] - 0.01), "{ys:?}");
        assert!(ys[ys.len() - 1] - ys[0] > 30.0, "{ys:?}");
        assert!(bottom_gap(&rt).abs() < 0.05, "{}", bottom_gap(&rt));
        assert!(!rt.animating);
    }

    /// An app's own `REFLOW` override on send slows the new row with the composer, so rows
    /// neither overshoot upward on a short draft nor dip before rising on a tall one.
    #[test]
    fn a_send_on_reflow_motion_never_dips() {
        let mut rt = sending(3);
        let before = rects(&rt)["r38"][1];
        rt.update(|s, tx| {
            tx.motion(motion::REFLOW);
            s.composer.as_mut().unwrap().clear();
            s.rows += 1;
        });
        let mut ys = vec![before];
        while ys.len() < 400 && (ys.len() < 3 || rt.animating) {
            rt.frame(1.0 / 120.0);
            ys.push(rects(&rt)["r38"][1]);
        }
        assert!(ys.windows(2).all(|w| w[1] <= w[0] + 0.01), "3-line reflow send moved down: {ys:?}");
        assert!(slack_gap(&rt).abs() < 0.05, "{}", slack_gap(&rt));

        let mut rt = sending(7);
        let before = rects(&rt)["r38"][1];
        rt.update(|s, tx| {
            tx.motion(motion::REFLOW);
            s.composer.as_mut().unwrap().clear();
            s.rows += 1;
        });
        let mut ys = vec![before];
        while ys.len() < 400 && (ys.len() < 3 || rt.animating) {
            rt.frame(1.0 / 120.0);
            ys.push(rects(&rt)["r38"][1]);
        }
        assert!(reversals(&ys) <= 1, "7-line reflow send reversed more than once: {ys:?}");
        assert!(slack_gap(&rt).abs() < 0.05, "{}", slack_gap(&rt));
    }

    /// Split frames (`catch_up` before the commit, `dt` after) pair the full frame's growth with
    /// the full frame's edge step, keep `ElementInfo::moving` on while slack closes, and deliver
    /// the resting `on_drawn_layout` rect once slack reaches zero.
    #[test]
    fn split_frames_and_drawn_layout_follow_slack_to_rest() {
        let mut edit = crate::TextEdit::new(&vec!["line"; 7].join("\n"));
        edit.focused = true;
        let mut rt = chat(Chat { rows: 6, composer: Some(edit), sends: true, track_drawn: true, ..Default::default() });
        rt.state.drawn.clear();
        let y0 = rects(&rt)["r0"][1];
        enter(&mut rt);
        let mut ys = vec![y0];
        let mut frames = 0;
        while frames < 360 && (frames < 2 || rt.animating) {
            rt.frame_split(0.5 / 120.0, 0.5 / 120.0);
            frames += 1;
            let r0 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r0")).unwrap();
            if rt.animating && (r0.rect[1] - *ys.last().unwrap()).abs() > 0.01 {
                assert!(r0.moving, "r0 moved at frame {frames} with moving=false");
            }
            ys.push(r0.rect[1]);
        }
        assert!(reversals(&ys) <= 1, "{ys:?}");
        assert!(slack_gap(&rt).abs() < 0.05, "{}", slack_gap(&rt));
        assert!((*rt.state.drawn.last().unwrap() - *ys.last().unwrap()).abs() < 0.05, "drawn_layout settled on final y");
    }

    /// Top-aligned rows that already fit, or cross from overflowing into fitting as the composer
    /// shrinks, never get pushed above the top of the viewport (`r0.y > 0` from negative offset).
    #[test]
    fn top_aligned_rows_do_not_rise_above_the_top_when_the_composer_shrinks() {
        for rows in [2, 5] {
            let mut edit = crate::TextEdit::new(&vec!["line"; 7].join("\n"));
            edit.focused = true;
            let mut rt = chat(Chat { rows, composer: Some(edit), sends: true, ..Default::default() });
            let mut ys = vec![rects(&rt)["r0"][1]];
            rt.update(|s, _| s.composer.as_mut().unwrap().clear());
            for _ in 0..240 {
                rt.frame(1.0 / 120.0);
                let y = rects(&rt)["r0"][1];
                assert!(y <= 0.05, "rows={rows}: r0 had a gap above it at the top of the viewport: {y}");
                ys.push(y);
            }
            assert!(ys.windows(2).all(|w| w[1] >= w[0] - 0.01), "rows={rows}: r0 moved up: {ys:?}");
            assert!((*ys.last().unwrap()).abs() < 0.05, "rows={rows}: r0 rests at the top: {:?}", ys.last());
        }
    }

    /// Rows that fit the list's layout but grow past its bottom while the composer shrinks
    /// under it start following from the first point past it, with no kick.
    #[test]
    fn rows_crossing_into_overflow_as_the_composer_shrinks_start_smoothly() {
        for lines in [2, 3] {
            let mut edit = crate::TextEdit::new(&vec!["line"; lines].join("\n"));
            edit.focused = true;
            let mut rt = chat(Chat { rows: 5, composer: Some(edit), sends: true, ..Default::default() });
            let mut ys = vec![rects(&rt)["r0"][1]];
            enter(&mut rt);
            for _ in 0..240 {
                rt.frame(1.0 / 120.0);
                ys.push(rects(&rt)["r0"][1]);
            }
            assert!(*ys.last().unwrap() < -5.0, "lines={lines}: the rows overflowed: {ys:?}");
            let steps: Vec<f32> = ys.windows(2).map(|w| w[1] - w[0]).collect();
            for w in steps.windows(2) {
                assert!((w[1] - w[0]).abs() < 1.5, "lines={lines}: r0's speed kicks {} pt a frame: {ys:?}", w[1] - w[0]);
            }
        }
    }
}
