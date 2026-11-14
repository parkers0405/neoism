//! Hover and press state: which elements the pointer is over, `on_hover_change` and its
//! delays, and the hover and press tracks hover styles and press scales animate.
use super::*;

/// Hover under a pointer that hasn't moved while layout carries content under it: a row
/// expanding and the rows below sliding, a growth stack, a flight, a glide the app asked for.
/// As on macOS, where tracking areas enter and exit as the mouse moves and re-sync once layout
/// settles, hover doesn't light each row that passes: it waits for the motion to settle or the
/// pointer to move. What leaves the tree or turns inert lets go at once, and presses are never
/// held.
#[derive(Default)]
pub(super) struct HoverHold {
    /// The pointer moved, pressed or a hand scrolled while layout moved: hover follows the
    /// pointer until the motion settles.
    pub(super) released: bool,
    /// The last pass held hover: moving layout carried an element whose hover would change.
    pub(super) held: bool,
    /// Scratch for the hover tracks `update_interaction` would change.
    flips: Vec<Flip>,
}

/// Node `i`'s hover track: where it would go, how, whether that changes it, and whether it
/// lands now whatever the hold (a leave from an element turned inert).
#[derive(Clone, Copy)]
struct Flip { i: usize, id: NodeId, on: bool, anim: Anim, changed: bool, now: bool }

/// What `hover_within` would become: the `on_hover_change` elements the pointer is inside,
/// and those of them entering.
type Within = (Vec<String>, Vec<usize>);

/// Viewport `s` moves content under a still pointer on its own: a glide, an app's instant
/// scrolls frame after frame (a glide it steps itself), or slack letting rows down.
fn scroll_carries(s: &ScrollState) -> bool {
    s.glide().is_some() || s.x.glide.is_some() || s.nudged > 0 || s.slack.moving()
}

/// `inner` is `outer` or inside it.
fn holds(outer: &str, inner: &str) -> bool {
    inner == outer || inner.len() > outer.len() && inner.starts_with(outer) && inner.as_bytes()[outer.len()] == b'/'
}

impl<S: 'static> Runtime<S> {
    /// Hover under the pointer at (x, y): what's hovered and what it's inside. Returns whether
    /// moving layout holds hover this pass, for the `update_interaction` that follows.
    pub(super) fn refresh_hover(&mut self, x: f32, y: f32) -> bool {
        if self.hover_hold.released && !self.layout_moving() { self.hover_hold.released = false; }
        let hovered = self.primary_target(x, y);
        let within = self.within_next();
        let held = self.decide_hold(hovered, within.as_ref(), None);
        self.hover_hold.held = held;
        if let Some((inside, _)) = within { self.apply_within(inside, held); }
        self.apply_hovered(hovered, held);
        held
    }

    /// Queue `on_hover_change` for elements the pointer entered or left.
    pub(super) fn refresh_hover_within(&mut self) {
        let Some(within) = self.within_next() else { return };
        let hovered = self.pointer.filter(|_| self.hover_may_hold()).and_then(|(x, y)| self.primary_target(x, y));
        let held = self.decide_hold(hovered, Some(&within), None);
        self.hover_hold.held = held;
        self.apply_within(within.0, held);
    }

    fn apply_hovered(&mut self, mut hovered: Option<usize>, held: bool) {
        // What's hovered stays; one that left the tree or turned inert lets go now.
        if held { hovered = self.hovered.as_deref().and_then(|p| self.hover_live(p)); }
        if hovered.map(|i| self.nodes[i].path.as_str()) == self.hovered.as_deref() { return; }
        self.hovered = hovered.map(|i| self.nodes[i].path.clone());
        if let Some(path) = &self.hovered { self.queue.push(Action::Hover(path.clone())); }
        // A row never takes focus from a text field (a search field above its results).
        let row = hovered.and_then(|i| self.highlight_row(i)).filter(|_| self.focused_input().is_none());
        match row {
            Some(i) if self.focus.path() != Some(&self.nodes[i].path) || !self.focus.hovered() =>
                self.queue.push(Action::HoverFocus(Some(self.nodes[i].path.clone()))),
            None if self.focus.hovered() => self.queue.push(Action::HoverFocus(None)),
            _ => {}
        }
    }

    /// None when no element tracks `on_hover_change` and none is waiting on a delay.
    fn within_next(&self) -> Option<Within> {
        if self.hover_within.is_empty() && self.hover_shown.is_empty() && self.hover_pending.is_empty()
            && !self.hits.iter().any(|(id, _)| self.node_index.get(id).is_some_and(|&i| self.nodes[i].handlers.get().is_some_and(|h| h.on_hover_change.is_some())))
        {
            return None;
        }
        let mut inside: Vec<String> = Vec::new();
        let mut entering: Vec<usize> = Vec::new();
        if let Some((x, y)) = self.pointer {
            for &(id, r) in &self.hits {
                if let Some((i, n)) = self.node_index.get(&id).map(|&i| (i, &self.nodes[i]))
                    && n.handlers.get().is_some_and(|h| h.on_hover_change.is_some()) && self.hit_contains(id, r, x, y) && !inside.contains(&n.path)
                {
                    if !self.hover_within.contains(&n.path) { entering.push(i); }
                    inside.push(n.path.clone());
                }
            }
        }
        // A held press stays inside what holds it until it releases.
        if let Some(captured) = self.press.path() {
            for path in &self.hover_within {
                if !inside.contains(path) && holds(path, captured)
                    && self.nodes.iter().enumerate().any(|(i, n)| n.path == *path && self.allowed(i))
                {
                    inside.push(path.clone());
                }
            }
        }
        Some((inside, entering))
    }

    fn apply_within(&mut self, mut inside: Vec<String>, held: bool) {
        // Held, the pointer stays inside what it was until that leaves the tree or turns inert.
        if held { inside = self.hover_within.iter().filter(|p| self.hover_live(p).is_some()).cloned().collect(); }
        // An element on its way out keeps its delays: only a shown hover hears its leave.
        let delayed = |this: &Self, path: &str| this.nodes.iter().find(|n| n.path == path).map(|n| n.handlers.hover_delay).filter(|d| *d != [0.0; 2]);
        for path in &self.hover_within {
            if !inside.contains(path) && delayed(self, path).is_none() { self.queue.push(Action::HoverChange(path.clone(), false)); }
        }
        for path in &inside {
            if !self.hover_within.contains(path) && delayed(self, path).is_none() { self.queue.push(Action::HoverChange(path.clone(), true)); }
        }
        self.hover_within = inside;
        self.settle_hover_delays();
    }

    /// Whether moving layout holds hover this pass, decided once before any of its three states
    /// change: the pointer may hold, and layout carries an element whose `hovered`,
    /// `hover_within` or hover track would change. `flips` are the track changes when known.
    fn decide_hold(&self, hovered: Option<usize>, within: Option<&Within>, flips: Option<&[Flip]>) -> bool {
        if !self.hover_may_hold() { return false; }
        let carried = |i: usize| self.layout_carries(i);
        let was = |p: &str| self.hover_live(p).is_some_and(carried);
        if hovered.map(|i| self.nodes[i].path.as_str()) != self.hovered.as_deref()
            && (hovered.is_some_and(carried) || self.hovered.as_deref().is_some_and(was))
        {
            return true;
        }
        if let Some((inside, entering)) = within
            && (entering.iter().any(|&i| carried(i)) || self.hover_within.iter().any(|p| !inside.contains(p) && was(p)))
        {
            return true;
        }
        let held = |f: &Flip| f.changed && !f.now && carried(f.i);
        match flips {
            Some(flips) => flips.iter().any(held),
            None => {
                let (captured, alone, shared) = (self.captured(), self.hovered_alone(), self.shared_hovered());
                (0..self.nodes.len()).filter_map(|i| self.hover_flip(i, captured, alone, &shared)).any(|f| held(&f))
            }
        }
    }

    /// Node `n` leaves the tree: if it heard the pointer enter, its `on_hover_change` still
    /// hears the leave, though the handler goes with the node before the action runs. A
    /// delayed element heard the enter only once shown; a pending one just stops waiting.
    pub(super) fn hover_pruned(&mut self, n: &Node) {
        let take = |list: &mut Vec<String>| list.iter().position(|p| *p == n.path).map(|at| list.remove(at)).is_some();
        let inside = take(&mut self.hover_within);
        let shown = take(&mut self.hover_shown);
        self.hover_pending.retain(|p| p.0 != n.path);
        let heard = if n.handlers.hover_delay == [0.0; 2] { inside } else { shown };
        let Some(f) = n.handlers.on_hover_change.clone().filter(|_| heard) else { return };
        self.hover_gone.push((n.path.clone(), f));
        self.queue.push(Action::HoverChange(n.path.clone(), false));
    }

    /// Starts or drops the waits of `hover_delays` elements whose inside changed.
    pub(super) fn settle_hover_delays(&mut self) {
        let mut paths: Vec<String> = self.hover_shown.clone();
        paths.extend(self.hover_pending.iter().map(|p| p.0.clone()));
        paths.extend(self.hover_within.iter().filter(|p| self.node(p).is_some_and(|n| n.handlers.hover_delay != [0.0; 2])).cloned());
        paths.sort();
        paths.dedup();
        let now = self.now_clock();
        for path in paths {
            let Some(delay) = self.node(&path).map(|n| n.handlers.hover_delay) else {
                // Gone or on its way out: a shown hover hears its leave now, a pending enter
                // just stops waiting.
                self.hover_pending.retain(|p| p.0 != path);
                if self.hover_shown.contains(&path) { self.report_hover(path, false); }
                continue;
            };
            let shown = self.hover_shown.contains(&path);
            // A held button, or a press or blur until the pointer next moves, never starts the
            // wait to report an enter.
            let want = self.hover_within.contains(&path)
                && (shown || !self.press.is_held() && !self.hover_paused);
            let pending = self.hover_pending.iter().position(|p| p.0 == path);
            if want == shown {
                if let Some(i) = pending { self.hover_pending.remove(i); }
                continue;
            }
            if pending.is_some_and(|i| self.hover_pending[i].1 == want) { continue; }
            if let Some(i) = pending { self.hover_pending.remove(i); }
            let d = delay[if want { 0 } else { 1 }] as f64;
            if d <= 0.0 {
                self.report_hover(path, want);
            } else {
                self.hover_pending.push((path, want, now + d));
                self.schedule_wakes_from(now, [std::time::Duration::from_secs_f64(d)]);
            }
        }
    }

    pub(super) fn report_hover(&mut self, path: String, inside: bool) {
        self.hover_shown.retain(|p| *p != path);
        if inside { self.hover_shown.push(path.clone()); }
        self.queue.push(Action::HoverChange(path, inside));
    }

    /// Reports the delayed hover changes due by this frame.
    pub(super) fn fire_hover_delays(&mut self) {
        if self.hover_pending.is_empty() { return; }
        let clock = self.clock + 1e-6;
        let (due, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.hover_pending).into_iter().partition(|p| p.2 <= clock);
        self.hover_pending = rest;
        for (path, inside, _) in due { self.report_hover(path, inside); }
    }

    /// The pointer moved or pressed, or a hand scrolled: hover follows the pointer through the
    /// layout moving now, until it settles.
    pub(super) fn release_hover_hold(&mut self) {
        if !self.hover_hold.released && self.layout_moving() { self.hover_hold.released = true; }
        self.hover_hold.held = false;
    }

    /// Hover may wait on moving layout: the pointer is in the window, holds no press, and
    /// hasn't moved since the motion started.
    fn hover_may_hold(&self) -> bool {
        self.pointer.is_some() && !self.press.is_held() && !self.hover_hold.released
    }

    /// Element `i` is carried by moving layout: a frame, offset or flight spring on it or an
    /// ancestor, a growth stack moving its rows, or a viewport gliding where the app sent it,
    /// stepped by the app frame by frame, or letting rows down. A hand's scroll and live
    /// transforms aren't layout motion. Asked only of elements whose hover would change, so a
    /// frame at rest walks nothing.
    fn layout_carries(&self, i: usize) -> bool {
        let store = self.scene.store();
        let mut at = Some(i);
        while let Some(k) = at {
            let id = self.nodes[k].id;
            if store.is_animating(id, prop::FRAME) || store.is_animating(id, prop::OFFSET) || store.is_animating(id, prop::TRAVEL)
                || self.stacks.get(&id).is_some_and(|s| s.moving) || self.grows.get(&id).is_some_and(|g| g.moving())
                || self.scrolls.get(&id).is_some_and(scroll_carries)
            {
                return true;
            }
            at = self.nodes[k].parent;
        }
        false
    }

    /// Any layout moving at all: the running stacks, growths, glides, flights and spring
    /// tracks (started this frame or before), not the tree. Asked only when a hand moves and
    /// while hover follows it.
    fn layout_moving(&self) -> bool {
        let layout = [prop::FRAME.id, prop::OFFSET.id, prop::TRAVEL.id];
        !self.flying.is_empty()
            || self.stacks.values().any(|s| s.moving)
            || self.grows.values().any(|g| g.moving())
            || self.scrolls.values().any(scroll_carries)
            || self.scene.store().active().any(|(_, p)| layout.contains(&p))
    }

    /// While moving layout holds hover, the hovered element, as the cursor and tooltips see
    /// it: what the pointer was over, not what slid under it.
    pub(crate) fn held_hover(&self) -> Option<Option<usize>> {
        (self.hover_hold.held && !self.pointer_captured())
            .then(|| self.hovered.as_deref().and_then(|p| self.hover_live(p)))
    }

    /// The element at `path`, while it's in the tree, not leaving it, and not inert.
    pub(crate) fn hover_live(&self, path: &str) -> Option<usize> {
        (0..self.nodes.len())
            .find(|&i| self.nodes[i].path == path && !self.subtree_exiting(i) && !self.subtree_unmounted(i))
            .filter(|&i| self.allowed(i))
    }

    fn captured(&self) -> Option<&str> {
        self.press.path().or(self.secondary.path())
    }

    /// The innermost `hover_alone` element under the pointer: the elements around it show no
    /// hover.
    fn hovered_alone(&self) -> Option<usize> {
        let (x, y) = self.pointer?;
        (0..self.nodes.len())
            .filter(|&i| self.nodes[i].vis.hover_alone && !self.is_exiting(self.nodes[i].id) && self.allowed(i) && self.node_contains(i, x, y))
            .max_by_key(|&i| self.nodes[i].path.len())
    }

    /// The `hover_group_shared` keys of the elements under the pointer.
    fn shared_hovered(&self) -> Vec<Rc<str>> {
        let Some((x, y)) = self.pointer else { return Vec::new() };
        (0..self.nodes.len())
            .filter_map(|i| {
                let key = self.nodes[i].vis.hover_share.as_ref()?;
                (!self.is_exiting(self.nodes[i].id) && self.allowed(i) && self.node_contains(i, x, y)).then(|| key.clone())
            })
            .collect()
    }

    /// Node `i`'s hover track, if it has one and isn't leaving. `alone`: the `hover_alone`
    /// element under the pointer, which turns off the tracks of the elements around it.
    /// `shared`: the `hover_group_shared` keys the pointer is over.
    fn hover_flip(&self, i: usize, captured: Option<&str>, alone: Option<usize>, shared: &[Rc<str>]) -> Option<Flip> {
        let n = &self.nodes[i];
        if !(n.vis.hover().a > 0.0 || n.vis_flags.hover_group()) || self.is_exiting(n.id) { return None; }
        let inside = self.pointer.is_some_and(|(x, y)| self.node_contains(i, x, y))
            || n.vis.hover_share.as_ref().is_some_and(|k| shared.contains(k))
            || (n.vis.hover_focus && self.focus.keyboard() && self.focus_within(i));
        let capture_inside = if self.secondary.is_held() { inside } else { self.press.inside() };
        let held = captured == Some(n.path.as_str()) && capture_inside;
        let allowed = self.allowed(i);
        // A held press stays hovered while it's inside, though a press scale shrank the drawn
        // rect away from the pointer. A group stays hovered while a descendant holds the press.
        let shadowed = alone.is_some_and(|a| a != i && holds(&n.path, &self.nodes[a].path));
        let on = allowed && (inside || held) && !shadowed && captured.is_none_or(|p| holds(&n.path, p));
        // Like a native sidebar or menu: the highlight lands with the pointer and fades
        // briefly as it leaves.
        let anim = match n.hover_motion() {
            Some((enter, exit)) => if on { enter } else { exit },
            None if on => Anim::INSTANT,
            None => rune_motion::Curve::tween(0.1, rune_motion::Easing::EASE_OUT).into(),
        };
        let changed = self.scene.store().target(n.id, P_HOVER).unwrap_or(0.0) != on as u8 as f32;
        Some(Flip { i, id: n.id, on, anim, changed, now: !on && !allowed })
    }

    /// Retargets the hover, press and press-scale tracks. `held` is the hold `refresh_hover`
    /// just decided; without it (a commit's pass) this decides it.
    pub(super) fn update_interaction(&mut self, held: Option<bool>) -> bool {
        // Hit tests read last frame's drawn rects and clips by index. When a commit moved the
        // indices (a commit's focus pass runs before the frame is built again), they belong to
        // other elements until the build re-evaluates from fresh geometry.
        if !self.nodes.iter().map(|n| n.id).eq(self.built_ids.iter().copied()) {
            return false;
        }
        let mut changed = false;
        let mut flips = std::mem::take(&mut self.hover_hold.flips);
        let captured = self.press.path().or(self.secondary.path());
        let alone = self.hovered_alone();
        let shared = self.shared_hovered();
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            let press_scale = n.handlers.get().and_then(|h| h.press_scale);
            if n.vis.hover().a <= 0.0 && !n.vis_flags.hover_group() && n.vis.press().a <= 0.0 && press_scale.is_none() { continue; }
            if self.is_exiting(n.id) { continue; }
            match self.hover_flip(i, captured, alone, &shared) {
                Some(f) if f.changed => flips.push(f),
                // Default hover animations inherit velocity and have no completion token.
                // An unchanged target preserves its trajectory in the store already; avoid
                // dirtying the scene node for that no-op. Custom animations may carry an
                // explicit velocity, decay or token and must still reach the store.
                Some(f) if n.hover_motion().is_some() => {
                    if !self.scene.store().has(f.id, P_HOVER) { self.scene.set(f.id, P_HOVER, 0.0); }
                    self.scene.animate(f.id, P_HOVER, f.on as u8 as f32, f.anim);
                }
                Some(_) => {}
                None => {}
            }
            if n.vis.press().a <= 0.0 && press_scale.is_none() { continue; }
            let capture_inside = if self.secondary.is_held() {
                self.pointer.is_some_and(|(x, y)| self.node_contains(i, x, y))
            } else {
                self.press.inside()
            };
            let down = self.allowed(i) && captured == Some(n.path.as_str()) && capture_inside;
            let pressed = down && n.vis.press().a > 0.0;
            let id = n.id;
            changed |= self.scene.store().target(id, P_PRESS).unwrap_or(0.0) != pressed as u8 as f32;
            if paint_tracks::ready_from_rest(&mut self.scene, id, P_PRESS, 0.0, pressed as u8 as f32) {
                self.scene.animate(id, P_PRESS, pressed as u8 as f32, if pressed { motion::INSTANT } else { motion::SNAPPY });
            }
            if let Some(k) = press_scale {
                // Down follows the hand; the release springs back with a little life.
                let down = down && !self.reduced_motion;
                if paint_tracks::ready_from_rest(&mut self.scene, id, P_PRESS_SCALE, 1.0, if down { k } else { 1.0 }) {
                    self.scene.animate(id, P_PRESS_SCALE, if down { k } else { 1.0 }, if down { motion::INSTANT } else { motion::POP });
                }
            }
        }
        let held = held.unwrap_or_else(|| {
            let within = self.within_next();
            let hovered = self.pointer.filter(|_| self.hover_may_hold()).and_then(|(x, y)| self.primary_target(x, y));
            self.decide_hold(hovered, within.as_ref(), Some(&flips))
        });
        // A leave from an element turned inert lands now; the rest wait while layout holds hover.
        // An element that drew nothing (a held hover whose row a glide carried out of view)
        // lands it rather than fading where nobody sees.
        let mut unseen: Option<Vec<bool>> = None;
        for f in flips.drain(..) {
            if held && !f.now { continue; }
            if paint_tracks::ready_from_rest(&mut self.scene, f.id, P_HOVER, 0.0, f.on as u8 as f32) {
                if unseen.get_or_insert_with(|| self.drew_nothing()).get(f.i).copied().unwrap_or(false) {
                    self.scene.set(f.id, P_HOVER, f.on as u8 as f32);
                } else {
                    self.scene.animate(f.id, P_HOVER, f.on as u8 as f32, f.anim);
                }
            }
            changed = true;
        }
        self.hover_hold.flips = flips;
        self.hover_hold.held = held;
        changed
    }
}

#[cfg(test)]
#[path = "../hover_tests.rs"]
mod tests;
