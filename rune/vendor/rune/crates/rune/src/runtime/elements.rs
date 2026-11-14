//! Element inspection (`Runtime::elements`, `presented`, `presented_pose`) and point picking
//! (`Runtime::element_at`, `contains`, `find`).
use super::*;

/// What tools see of one element.
#[derive(Clone, Debug)]
pub struct ElementInfo {
    pub trace_name: Option<&'static str>,
    pub trace_namespace: Option<(&'static str, &'static str)>,
    pub loop_phase: Option<f64>,
    pub focusable: bool,
    pub focused: bool,
    pub focus_visible: bool,
    pub inert: bool,
    /// Playing its exit transition, or inside an element that is. Lookups by key treat it
    /// as gone; it stays listed so tools can watch the exit.
    pub exiting: bool,
    pub path: String,
    pub key: Option<String>,
    pub role: &'static str,
    pub label: Option<String>,
    /// What a text element shows (none for a secure field). An element of role `"text"` drawn
    /// from several texts, such as rune-kit's `RollingText`, gives its label.
    pub text: Option<String>,
    pub rect: [f32; 4],
    pub target: [f32; 4],
    pub opacity: f32,
    /// Effective uniform scale, including ancestors and the presence transition.
    pub scale: f32,
    /// Additive blur culling margin in logical points, including scaled ancestor sigmas.
    /// Three times this value bounds filter support; it is not the equivalent Gaussian sigma.
    pub blur: f32,
    pub radius: f32,
    pub fill: Color,
    pub text_color: Option<Color>,
    pub hover: f32,
    pub press: f32,
    pub moving: bool,
    pub parent: Option<usize>,
    /// The part of `rect` its ancestors' clips leave visible.
    pub visible: [f32; 4],
    /// Words of this text are still fading in.
    pub revealing: bool,
    /// Inside a growth row whose height is moving: its visible part is being revealed.
    pub in_growing_row: bool,
    /// This node is riding a scroll: a wheel or trackpad, its momentum, or a glide
    /// (`Tx::scroll_to`, `scroll_to_with`, `scroll_to_bottom`).
    pub scrolling: bool,
    /// How far its scroll viewports' offsets carry it this frame, x and y in the window's points
    /// (scaled and turned as `rect` is): `rect` sits this much left and up of where it would
    /// unscrolled. Motion checks add it back, so they judge an element against the content it
    /// scrolls with.
    pub scrolled: [f32; 2],
    /// Invisible estimated extent of a virtual list, not a drawable surface.
    pub virtual_extent: bool,
    /// Property-specific intentional cuts for 1:1 live bindings (never smoothed bindings).
    pub live_intent: u32,
    /// On the focused text field: its caret line in window points (see [`Cx::caret`]).
    pub caret: Option<[f32; 4]>,
    /// Dragging it starts an OS drag ([`El::drag_out`]).
    pub drag_out: bool,
    /// Its tooltip and accessibility help ([`El::help`]).
    pub help: Option<String>,
    /// The part of this [`El::selectable`] text that is selected, as copying would take it.
    pub selected: Option<String>,
    /// Its outline depth ([`El::level`]).
    pub level: Option<u32>,
    /// Whether it is open ([`El::expanded`]); `None` when it can't open.
    pub expanded: Option<bool>,
    /// A selected item of a list, tree, table or group ([`El::focus_selected`]).
    pub item_selected: bool,
    /// How far [`El::sticky_top`], [`El::sticky_bottom`] or [`El::sticky_left`] moved it from
    /// where scrolling put it this frame, in points: down for a stuck header, up for a stuck
    /// footer, right for a stuck left element, 0 when it isn't stuck. `rect` already includes it.
    pub stuck: f32,
}

/// An element [`Runtime::element_at`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    path: String,
    key: Option<String>,
}

/// Options for [`Runtime::element_at_with`] and its siblings. Fields are private, so options can
/// be added without breaking callers; `Pick::default()` is [`Runtime::element_at`]'s behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    transparent: bool,
}

impl Pick {
    pub fn new() -> Self { Self::default() }

    /// Also find elements whose own or inherited opacity is 0, as a design canvas does to
    /// select an invisible layer. Exiting elements are skipped either way, and so are hidden
    /// or unmounted ones.
    pub fn transparent(mut self, on: bool) -> Self {
        self.transparent = on;
        self
    }
}

impl Picked {
    /// Its path in the element tree (as [`ElementInfo::path`]).
    pub fn path(&self) -> &str { &self.path }
    /// Its key ([`El::key`]), if it has one.
    pub fn key(&self) -> Option<&str> { self.key.as_deref() }
}

/// What an element presents beyond its rect and opacity, as of the last frame
/// ([`Runtime::presented_pose`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    rotation: f32,
    scale: f32,
    shadows: Vec<crate::el::Shadow>,
    moving: bool,
}

impl Pose {
    /// The element's own presented `El::rotate`, in radians clockwise. Ancestors' turns are not
    /// included.
    pub fn rotation(&self) -> f32 { self.rotation }
    /// Effective uniform scale, ancestors included (as [`ElementInfo::scale`]).
    pub fn scale(&self) -> f32 { self.scale }
    /// The element's shadows (`El::box_shadow`) at their current sprung geometry and colour.
    pub fn shadows(&self) -> &[crate::el::Shadow] { &self.shadows }
    /// A spring or decorative loop still moves its rect, scale, rotation, icon turn or shadows,
    /// or an ancestor's. An active repeating loop does not settle.
    pub fn moving(&self) -> bool { self.moving }
}

impl<S: 'static> Runtime<S> {
    /// Authored geometry goals, before scroll and spring sampling, in parent space.
    pub(crate) fn trace_goals(&self) -> impl Iterator<Item = (&str, [f32; 7])> {
        self.nodes.iter().map(|n| (n.path.as_str(), [n.local.x, n.local.y, n.local.w, n.local.h, n.vis.offset[0], n.vis.offset[1], n.static_scale]))
    }

    /// Tracks the last tick sampled, by element path and property name: what spent this
    /// frame animating. Tracks of nodes no longer in the tree report their node id.
    pub(crate) fn touched_tracks(&self) -> Vec<(String, &'static str, bool)> {
        let store = self.scene.store();
        let mut out: Vec<_> = store.touched_tracks().map(|(track, node, p)| {
            let path = self.node_index.get(&node).map_or_else(|| format!("node:{}", node.0), |&i| self.nodes[i].path.clone());
            (path, prop_name(p.0), store.is_active(track))
        }).collect();
        // A growth row's feathered edge (`GrowTrack::mask`) paints as it eases out, after its height
        // has landed: it is a track of its own here.
        out.extend(self.grows.iter().filter(|(_, g)| g.mask > 0.01).filter_map(|(id, _)| {
            self.node_index.get(id).map(|&i| (self.nodes[i].path.clone(), "mask", true))
        }));
        out.sort();
        out.dedup();
        out
    }

    /// Elements that draw nothing and hold nothing: no paint of their own, no text, no field,
    /// no focus, no children. Hit and hover zones, spacers. A trace flags them: nobody can see
    /// them come, go or move.
    pub(crate) fn blank_leaves(&self) -> HashSet<&str> {
        let mut parents = vec![false; self.nodes.len()];
        for n in &self.nodes {
            if let Some(p) = n.parent { parents[p] = true; }
        }
        self.nodes.iter().enumerate().filter(|&(i, n)| {
            let v = &n.vis;
            !parents[i] && n.text.is_none() && n.input.is_none() && !n.focus.enabled && n.material.is_none()
                && v.fill.a == 0.0 && (v.border_width() == 0.0 || v.border().a == 0.0 && v.border_bottom.is_none_or(|c| c.a == 0.0))
                && v.shadows.is_empty() && v.surface.is_none() && v.icon.is_none() && v.gradient.is_none() && v.radial.is_none()
                && v.outline.is_none() && v.shape.is_none() && v.pixels.is_none() && v.effect.is_none()
                && v.hover().a == 0.0 && v.press().a == 0.0
        }).map(|(_, n)| n.path.as_str()).collect()
    }

    /// Video elements' playback state by element path, for `--tree`.
    pub(crate) fn videos(&self) -> HashMap<String, crate::video::Snapshot> {
        self.nodes.iter()
            .filter_map(|n| Some((n.path.clone(), n.vis.surface.as_ref()?.0.video()?.snapshot())))
            .collect()
    }

    /// Whether an element path contains secure text, hidden or revealed. Accessibility
    /// adapters should use this alongside [`Self::elements`] to report protected content.
    /// Unknown paths (including synthetic flight/link paths) return false.
    pub fn element_is_secure(&self, path: &str) -> bool {
        self.nodes.iter().find(|n| n.path == path).is_some_and(Node::is_secure)
    }

    /// What a move of `v` points inside node `i` (a scroll viewport's offset) comes to in the
    /// window: scaled and turned as its children draw.
    pub(super) fn content_to_window(&self, i: usize, v: [f32; 2]) -> [f32; 2] {
        let Some(h) = self.inh_scratch.get(i) else { return v };
        let [x, y] = v.map(|c| c * h.scale);
        match h.turn.checked_sub(1).and_then(|k| self.turn_scratch.get(k as usize)) {
            Some(t) => [t.mat[0] * x + t.mat[1] * y, t.mat[2] * x + t.mat[3] * y],
            None => [x, y],
        }
    }

    pub fn elements(&self) -> Vec<ElementInfo> {
        fn intent(n: &Node) -> u32 {
            n.live.iter().filter(|b| b.spring.is_none()).fold(0, |bits, b| bits | match b.property {
                crate::live::Property::Offset => 1,
                crate::live::Property::Scale | crate::live::Property::Rect => 1 | 2,
                crate::live::Property::Opacity => 4,
                _ => 0,
            })
        }
        let store = self.scene.store();
        let animating = |id: NodeId| store.is_animating(id, prop::FRAME) || store.is_animating(id, prop::TRAVEL)
            || store.is_animating(id, prop::PRESENCE) || store.is_animating(id, prop::OFFSET) || store.is_animating(id, prop::SCALE) || store.is_animating(id, P_PRESS_SCALE)
            || store.is_animating(id, P_SCALE) || store.is_animating(id, prop::ROTATION) || paint_pose_springing(store, id)
            || (!self.reduced_motion && self.loops.get(&id).is_some_and(|l| l.active(store.node_time(id))));
        let sel_ranges = self.selection_ranges();
        let mut elements: Vec<_> = self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let p = self.scene.presented(n.id)?;
                let exiting = self.subtree_exiting(i);
                let flight = self.flights.iter().find(|f| f.lands(&n.path, n.key.as_deref()));
                let mut moving = flight.is_some() || animating(n.id) || self.grows.get(&n.id).is_some_and(|g| g.moving());
                let drawn = self.drawn.get(i).copied().unwrap_or([p.rect.x, p.rect.y, p.rect.w, p.rect.h]);
                // A flight carries the destination's laid-out text, which can be narrower than
                // its box. Report the box carried along with it, so the element lands on its own
                // frame instead of jumping out to it when the flight ends.
                let rect = flight.map_or(drawn, |f| {
                    let (d, c) = (f.destination, f.presented);
                    let (zx, zy) = (c.w / d.w.max(1e-3), c.h / d.h.max(1e-3));
                    [c.x + (drawn[0] - d.x) * zx, c.y + (drawn[1] - d.y) * zy, drawn[2] * zx, drawn[3] * zy]
                });
                let s = self.env.scale;
                let clip = if self.launches.flying(n.id) { NO_CLIP } else {
                    flight.map_or_else(|| self.screen_clip_of_parent(n.parent), |f| f.clip.map(|v| v * s))
                };
                let (x0, y0) = (rect[0].max(clip[0] / s), rect[1].max(clip[1] / s));
                let (x1, y1) = ((rect[0] + rect[2]).min(clip[2] / s), (rect[1] + rect[3]).min(clip[3] / s));
                let visible = [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)];
                let revealing = flight.is_some() || self.reveals.get(&n.id).is_some_and(|r| !r.words.is_empty());
                let mut in_growing_row = false;
                let mut scrolling = false;
                let mut scrolled = [0.0; 2];
                let mut live_intent = intent(n);
                // Held at its landing width this frame: its re-layout is the design.
                if self.landing_holds.holds(n.id) {
                    live_intent |= 1 | 2;
                }
                // A group_opacity swap under an instant hover group is a deliberate cut (Beta's
                // status/close swap), for it and what it contains: opacity and transition intent.
                let up = |from: Option<usize>| std::iter::successors(from, |&pi| self.nodes[pi].parent);
                if let Some(gi) = up(Some(i)).find(|&j| self.nodes[j].vis.group_opacity.is_some()) {
                    let group = up(self.nodes[gi].parent)
                        .map(|pi| &self.nodes[pi])
                        .find(|g| g.vis.hover().a > 0.0 || g.vis_flags.hover_group());
                    if group.and_then(|g| g.hover_motion()).is_some_and(|(e, x)| [e, x].iter().any(|a| matches!(a.curve, rune_motion::Curve::Instant))) {
                        live_intent |= 4 | 16;
                    }
                }
                // Offsets are in points inside the scroll viewport; `rect` is in the window, after
                // every scale and turn above. Below a lift (a flight or launch drawn over the
                // window), the scales holding still above it carry them, unturned.
                let mut lifted = None;
                let mut j = i;
                loop {
                    let to_window = |at: Option<usize>, v: [f32; 2]| match lifted {
                        Some(k) => v.map(|c| c * k),
                        None => at.map_or(v, |at| self.content_to_window(at, v)),
                    };
                    if j != i && let Some(st) = self.scrolls.get(&self.nodes[j].id) {
                        let [x, y] = to_window(Some(j), st.drawn);
                        scrolled = [scrolled[0] + x, scrolled[1] + y];
                    }
                    // A launch, and what it carries, leaves under the offsets it launched against
                    // and takes on the scroll's only as it lands.
                    let id = self.nodes[j].id;
                    let parent = self.nodes[j].parent;
                    let fix = self.launches.carried(&self.scene, id);
                    if fix != [0.0; 2] {
                        let k = parent.and_then(|p| self.inh_scratch.get(p)).map_or(1.0, |h| h.still.0);
                        scrolled = [scrolled[0] - fix[0] * k, scrolled[1] - fix[1] * k];
                    }
                    if lifted.is_none() && self.flying.iter().any(|f| f.0 == id) {
                        lifted = Some(parent.and_then(|p| self.inh_scratch.get(p)).map_or(1.0, |h| h.still.0));
                    }
                    let Some(pi) = step_up(&self.nodes, j) else { break };
                    let p = &self.nodes[pi];
                    moving |= animating(p.id)
                        || self.stacks.get(&p.id).is_some_and(|s| s.moving)
                        || self.scrolls.get(&p.id).is_some_and(|s| s.slack.moving() || s.glide().is_some());
                    live_intent |= intent(p);
                    scrolling |= self.scrolls.get(&p.id).is_some_and(ScrollState::moving);
                    in_growing_row |= self.grows.get(&p.id).is_some_and(|g| g.moving());
                    j = pi;
                }
                let secure = n.is_secure();
                Some(ElementInfo {
                    trace_name: n.trace_name,
                    trace_namespace: n.trace_namespace,
                    loop_phase: self.loops.get(&n.id).map(|l| {
                        if self.reduced_motion && !matches!(n.looping.as_ref().map(|s| s.kind), Some(rune_motion::LoopKind::Sequence(_))) { 0.0 }
                        else { l.phase(self.scene.store().node_time(n.id)) }
                    }),
                    focusable: self.can_focus(i),
                    focused: self.focus.path() == Some(&n.path),
                    focus_visible: self.focus.keyboard() && self.focus.path() == Some(&n.path),
                    inert: !self.allowed(i) || exiting,
                    exiting,
                    path: n.path.clone(),
                    key: n.key.clone(),
                    role: n.role,
                    label: n.label.clone(),
                    text: match &n.text {
                        Some(t) => (!secure).then(|| t.source_text().to_owned()),
                        // A text built from several (a rolling label's glyphs) reads as its label.
                        None => (!secure).then(|| n.label.clone()).flatten().filter(|_| n.role == "text"),
                    },
                    rect,
                    target: [n.target[0], n.target[1], n.target[2] - n.target[0], n.target[3] - n.target[1]],
                    opacity: flight.map_or_else(|| self.inh_scratch.get(i).map_or(p.opacity, |h| h.opacity), |f| f.target_alpha),
                    scale: if flight.is_some() { 1.0 } else { self.inh_scratch.get(i).map_or(p.transform.scale.x, |h| h.scale) },
                    blur: if flight.is_some() { 0.0 } else { self.inh_scratch.get(i).map_or(p.blur, |h| h.blur) },
                    radius: p.radius.0[0],
                    fill: self.color(n.id, P_FILL, n.vis.fill),
                    text_color: n.text.as_ref().map(|t| t.color).or(n.vis.icon.map(|(_, c)| c)).map(|c| self.color(n.id, P_TEXT, c)),
                    hover: if (n.focus.highlight.is_some() && self.highlight_visible(i)) || (n.focus.fill.is_some() && self.focus_ring_visible(i)) { 1.0 }
                        else { self.scene.get(n.id, P_HOVER).unwrap_or(0.0) },
                    press: self.scene.get(n.id, P_PRESS).unwrap_or(0.0),
                    moving,
                    parent: n.parent,
                    visible,
                    revealing,
                    in_growing_row,
                    scrolling,
                    scrolled,
                    live_intent,
                    caret: self.caret_rect(&self.nodes, i),
                    drag_out: n.handlers.drag_out.is_some(),
                    help: n.help.as_deref().filter(|_| !n.help_cut || n.text_cut(s)).map(|h| crate::el::help_words(h).into_owned()),
                    selected: n.text.as_ref().filter(|t| t.selectable && !secure).and_then(|t| {
                        let (a, b) = selectable::range_in(&sel_ranges, i)?;
                        t.copy_slice(a as u32, b as u32)
                    }),
                    level: n.a11y.level,
                    expanded: n.a11y.expanded,
                    item_selected: n.focus.selected,
                    stuck: self.stuck.get(i).copied().unwrap_or(0.0),
                    virtual_extent: self.virtuals.contains_key(&n.path) && n.vis.fill.a == 0.0
                        && n.vis.border_width() == 0.0 && n.vis.shadows.is_empty() && n.text.is_none() && n.vis.icon.is_none(),
                })
            })
            .collect();
        for f in &self.flights {
            let r = if self.reduced_motion { f.source } else { f.presented };
            let c = if self.reduced_motion { f.source_clip } else { f.clip };
            let path = format!("flight-source/{}", f.target_key);
            let t = f.destination;
            elements.push(ElementInfo {
                trace_name: None,
                trace_namespace: None,
                loop_phase: None,
                focusable: false, focused: false, focus_visible: false, inert: true, exiting: false,
                key: Some(path.clone()), path, role: "text", label: None, text: None,
                rect: [r.x, r.y, r.w, r.h], target: [t.x, t.y, t.w, t.h],
                opacity: f.source_alpha, scale: 1.0, blur: 0.0, radius: 0.0, fill: Color::TRANSPARENT, text_color: None,
                hover: 0.0, press: 0.0, moving: true, parent: None,
                visible: [r.x.max(c[0]), r.y.max(c[1]), (r.edges()[2].min(c[2]) - r.x.max(c[0])).max(0.0),
                    (r.edges()[3].min(c[3]) - r.y.max(c[1])).max(0.0)],
                revealing: true, in_growing_row: false, scrolling: false, scrolled: [0.0; 2], virtual_extent: false, live_intent: 0, caret: None, drag_out: false,
                help: None, selected: None, level: None, expanded: None, item_selected: false, stuck: 0.0,
            });
        }
        self.link_elements(&mut elements);
        self.highlight_elements(&mut elements);
        elements
    }

    /// Centre of the first element whose key is `key`, or whose path ends with `key` when it
    /// contains a `/` (e.g. `slider/knob`), in points. Elements without keys between them don't
    /// count, so `slider/knob` finds a knob inside unkeyed rows inside the slider. Failing that,
    /// the first line box of the link (`El::link`) or highlight (`El::text_highlight`) keyed
    /// `key`. Exiting elements and their children are gone here; [`Runtime::presented`] still
    /// reads them.
    pub fn find(&self, key: &str) -> Option<(f32, f32)> {
        let suffix = format!("/{key}");
        let matches = |n: &Node| {
            if key.contains('/') {
                n.path == key || n.path.ends_with(&suffix) || keyed_suffix(&n.path, key)
            } else {
                n.key.as_deref() == Some(key)
            }
        };
        let i = (0..self.nodes.len()).find(|&i| matches(&self.nodes[i]) && !self.subtree_exiting(i));
        let Some(i) = i else { return self.find_link(key).or_else(|| self.find_highlight(key)) };
        let r = self.drawn.get(i).copied()?;
        Some((r[0] + r[2] * 0.5, r[1] + r[3] * 0.5))
    }

    /// Drawn rect `[x, y, w, h]` in points and effective opacity (ancestors included) of the
    /// element keyed `key`, exiting or not, as of the last frame.
    pub fn presented(&self, key: &str) -> Option<([f32; 4], f32)> {
        let i = self.nodes.iter().position(|n| n.key.as_deref() == Some(key))?;
        Some((*self.drawn.get(i)?, self.inh_scratch.get(i)?.opacity))
    }

    /// The topmost element drawn at window point (`x`, `y`) in points, as of the last frame: the
    /// point is tested against each element's drawn box the way a press is, through `El::rotate`,
    /// `El::scale`, `El::origin`, offsets and scrolling, and inside its ancestors' clips
    /// (including a turned ancestor's clip), in paint order (so `El::raise`, sticky and flying
    /// elements count). Unlike a press, it also finds elements with no handlers, and it ignores
    /// `El::inert` and modals, so a press can land on a different element than this returns.
    /// Elements that are exiting or fully transparent are skipped (see [`Runtime::element_at_with`] to include transparent ones). `El::hit`, which marks native
    /// overlay input, makes no difference, and overlay panels outside the window are not
    /// considered. An element is not cut by its own clip, only by its ancestors'. This is box
    /// geometry, not pixel/alpha picking: a box with no fill remains pickable (as an invisible
    /// input area does), and rounded corners and soft clip fades do not trim its hit box.
    /// Running handlers can use [`Tx::element_at`] without holding a `Runtime`.
    pub fn element_at(&self, x: f32, y: f32) -> Option<Picked> {
        self.element_at_with(x, y, Pick::default())
    }

    /// [`Runtime::element_at`] with options: [`Pick::transparent`] also finds fully
    /// transparent elements (exiting ones stay skipped).
    pub fn element_at_with(&self, x: f32, y: f32, pick: Pick) -> Option<Picked> {
        self.element_at_within(None, x, y, pick)
    }

    pub(super) fn run_pick_reads(&mut self, tx: &mut Tx) {
        for read in std::mem::take(&mut tx.pick_reads) {
            let picked = self.element_at_within(read.within.as_deref(), read.x, read.y, read.pick);
            if let Some(f) = read.take() {
                f(states!(self), picked, tx);
                self.dirty = true;
            }
        }
        // A callback may ask again, but cannot keep this dispatch running indefinitely.
        self.pending.pick_reads.append(&mut tx.pick_reads);
    }

    /// [`Runtime::element_at`] restricted to an element and its descendants, ignoring other
    /// subtrees. `within` accepts a key or key path as [`Cx::layout`] does.
    pub fn element_at_in(&self, within: &str, x: f32, y: f32) -> Option<Picked> {
        self.element_at_in_with(within, x, y, Pick::default())
    }

    /// [`Runtime::element_at_in`] with options, as [`Runtime::element_at_with`].
    pub fn element_at_in_with(&self, within: &str, x: f32, y: f32, pick: Pick) -> Option<Picked> {
        self.element_at_within(Some(within), x, y, pick)
    }

    fn element_at_within(&self, within: Option<&str>, x: f32, y: f32, pick: Pick) -> Option<Picked> {
        let scope = match within {
            Some(key) => Some(self.nodes.iter().position(|n| crate::flight::selects(key, &n.path, n.key.as_deref()))?),
            None => None,
        };
        let n = self.nodes.len();
        let at = |k: usize| if self.layer_paint_order.is_empty() { k } else { self.layer_paint_order[k] };
        let count = if self.layer_paint_order.is_empty() { n } else { self.layer_paint_order.len() };
        (0..count).rev().map(at).find(|&i| {
            scope.is_none_or(|scope| std::iter::successors(Some(i), |&p| self.nodes[p].parent).any(|p| p == scope))
                && self.pickable(i, x, y, pick)
        }).map(|i| Picked {
            path: self.nodes[i].path.clone(),
            key: self.nodes[i].key.clone(),
        })
    }

    /// Whether window point (`x`, `y`), in points, is inside the drawn, clipped and turned box
    /// of the element keyed `key`, whatever is drawn above it and whether or not it has handlers.
    /// False when there is no such element, it is exiting or fully transparent. Same geometry
    /// as [`Runtime::element_at`].
    pub fn contains(&self, key: &str, x: f32, y: f32) -> bool {
        self.contains_with(key, x, y, Pick::default())
    }

    /// [`Runtime::contains`] with options, as [`Runtime::element_at_with`].
    pub fn contains_with(&self, key: &str, x: f32, y: f32, pick: Pick) -> bool {
        self.nodes.iter().position(|n| crate::flight::selects(key, &n.path, n.key.as_deref())).is_some_and(|i| self.pickable(i, x, y, pick))
    }

    fn pickable(&self, i: usize, x: f32, y: f32, pick: Pick) -> bool {
        self.drawn.get(i).is_some()
            && self.inh_scratch.get(i).is_some_and(|h| pick.transparent || h.opacity > 0.0)
            && !self.subtree_exiting(i)
            && self.node_contains(i, x, y)
    }

    /// The presented rotation, scale and shadows of the element keyed `key`, exiting or not, as
    /// of the last frame, and whether any spring still moves them. It reads the springs on
    /// demand: nothing runs per frame for it. This is a driver/headless read: running view
    /// functions and handlers do not hold a `Runtime`.
    pub fn presented_pose(&self, key: &str) -> Option<Pose> {
        let i = self.nodes.iter().position(|n| crate::flight::selects(key, &n.path, n.key.as_deref()))?;
        let n = &self.nodes[i];
        let store = self.scene.store();
        let shadows = n.vis.shadows.iter().enumerate().map(|(k, sh)| {
            let sh = self.shadow_at(n.id, k, sh);
            match P_SHADOWS.get(k) {
                Some(&p) => crate::el::Shadow { color: self.color(n.id, p, sh.color), ..sh },
                None => sh,
            }
        }).collect();
        let moving = self.rect_springing(i)
            || std::iter::successors(Some(i), |&p| step_up(&self.nodes, p)).any(|p| {
                let id = self.nodes[p].id;
                paint_pose_springing(store, id)
                    || (!self.reduced_motion && self.loops.get(&id).is_some_and(|l| l.active(store.node_time(id))))
            });
        Some(Pose {
            rotation: self.scene.get(n.id, prop::ROTATION).unwrap_or(n.vis.rotate),
            scale: self.inh_scratch.get(i)?.scale,
            shadows,
            moving,
        })
    }

    /// Whether the root's own fill is dark, when it has an opaque one (headless `--keys` caps
    /// follow it, so they flip when the app changes theme itself).
    pub(crate) fn backdrop_dark(&self) -> Option<bool> {
        if self.transparent { return None; }
        let n = self.nodes.get(self.app_root())?;
        let c = self.color(n.id, P_FILL, n.vis.fill);
        (c.a > 0.5).then(|| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b < 0.25)
    }
}

/// Whether `path`'s keyed segments end with `key`'s: `slider/knob` matches
/// `form/#2/slider/#0/knob`, since the `#n` segments stand for elements without keys.
fn keyed_suffix(path: &str, key: &str) -> bool {
    let unkeyed = |s: &&str| s.len() > 1 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_digit());
    let mut keyed = path.rsplit('/').filter(|s| !unkeyed(s));
    key.rsplit('/').all(|want| keyed.next() == Some(want))
}
