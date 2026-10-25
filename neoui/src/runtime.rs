use std::collections::{HashMap, HashSet};
use std::fmt;

use taffy::prelude as tf;

use crate::element::Kind;
use crate::input::{InputEvent, Key, Modifiers, PointerButton};
use crate::routing::{Dispatch, EventKind, EventPhase, Mount, RouteNode, RoutedAction, Routes, RoutingState};
use crate::text::{MeasurementKey, TextContext};
use crate::{
    Align, Color, Element, Justify, Rect, Role, Semantics, Size, Style, TextMeasurer,
    TextStyle,
};

/// Layout rejects invalid input rather than passing NaNs or ambiguous identities
/// into the layout engine. A failed layout invalidates the previous frame/hits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    EmptyKey,
    DuplicateKey(String),
    InvalidInput {
        key: Option<String>,
        field: &'static str,
    },
    InvalidMeasurement,
    MeasurementContextChanged,
    IncompatibleRootWidth,
    TreeTooDeep,
    TooManyElements,
    Engine(String),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyKey => write!(f, "element keys must not be empty"),
            Self::DuplicateKey(key) => write!(f, "duplicate element key: {key}"),
            Self::InvalidInput { key, field } => write!(
                f,
                "invalid {field} for {}",
                key.as_deref().unwrap_or("layout")
            ),
            Self::InvalidMeasurement => {
                write!(f, "text measurement must be finite and nonnegative")
            }
            Self::MeasurementContextChanged => write!(f, "text measurement context changed during presentation"),
            Self::IncompatibleRootWidth => write!(f, "root padding or resolved layout is incompatible with the exact available width"),
            Self::TreeTooDeep => write!(f, "element tree exceeds 128 levels"),
            Self::TooManyElements => write!(f, "element tree exceeds 100000 nodes"),
            Self::Engine(message) => write!(f, "layout engine: {message}"),
        }
    }
}
impl std::error::Error for LayoutError {}
impl From<taffy::TaffyError> for LayoutError {
    fn from(error: taffy::TaffyError) -> Self {
        Self::Engine(error.to_string())
    }
}

/// Token for rejecting pointer events queued against a superseded frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FrameId(u64);

/// Owned commands in back-to-front painter order. `clip` is already intersected
/// with the viewport and all clipping ancestors. The adapter MUST honor it.
/// Text is one already-wrapped line per command; do not wrap it again. Its y is
/// the top of the line box, not a baseline. Quads are solid rounded rectangles.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintCommand {
    Quad {
        bounds: Rect,
        color: Color,
        radius: f32,
        clip: Rect,
    },
    Text {
        bounds: Rect,
        text: String,
        style: TextStyle,
        clip: Rect,
    },
}

/// A pointer target in painter order. Bounds are the node's border box; clip
/// uses the same ancestor geometry as painting. `None` actions are disabled
/// blockers, so disabled overlapping controls do not click through.
#[derive(Clone, Debug, PartialEq)]
pub struct HitRegion<Action> {
    pub key: String,
    pub bounds: Rect,
    pub clip: Rect,
    pub action: Option<Action>,
}

impl<Action> HitRegion<Action> {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.bounds.contains(x, y) && self.clip.contains(x, y)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticNode {
    pub key: String,
    pub parent: Option<String>,
    pub bounds: Rect,
    pub clip: Rect,
    pub semantics: Semantics,
    pub focused: bool,
}

/// An owned snapshot, with no references to the declarative tree or renderer.
/// Cloning a frame is safe, but old snapshots must not be used for dispatch;
/// send their id to `dispatch_pointer_down_in` to reject stale input.
#[derive(Clone, Debug)]
pub struct Frame<Action> {
    pub id: FrameId,
    /// Resolved root box, including its auto-measured height, even offscreen.
    pub bounds: Rect,
    pub paint: Vec<PaintCommand>,
    pub hits: Vec<HitRegion<Action>>,
    pub semantics: Vec<SemanticNode>,
    boxes: HashMap<String, Rect>,
    routes: Routes<Action>,
}

impl<Action> Default for Frame<Action> {
    fn default() -> Self {
        Self {
            id: FrameId::default(),
            bounds: Rect::ZERO,
            paint: vec![],
            hits: vec![],
            semantics: vec![],
            boxes: HashMap::new(),
            routes: Routes::default(),
        }
    }
}

impl<Action> Frame<Action> {
    /// Full (unclipped) box for every mounted key, including nonpainted spacers.
    pub fn bounds_for(&self, key: &str) -> Option<Rect> {
        self.boxes.get(key).copied()
    }

    /// Topmost target, including disabled blockers, with half-open bounds.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&HitRegion<Action>> {
        self.hits.iter().rev().find(|hit| hit.contains(x, y))
    }
}

/// Per-surface state. The host owns application state; actions are typed values
/// returned to the host, never executed inside layout. Focus/pressed identities
/// survive rebuilds by key, but never survive unmount, disable, failure or clear.
/// A pointer-down records the pressed key; call `release_pointer` on up/cancel.
/// Cumulative work counters, except `live_nodes`, which is the current resource gauge.
/// `measured_text` counts actual adapter calls, not text-cache lookups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStats {
    pub reconciled_nodes: u64,
    pub measured_text: u64,
    pub layout_computations: u64,
    pub emitted_frames: u64,
    pub created_nodes: u64,
    pub live_nodes: usize,
}

pub struct Runtime<Action> {
    tree: tf::TaffyTree<TextContext>,
    root: Option<Resolved<Action>>,
    stats: RuntimeStats,
    geometry_dirty: bool,
    presentation_dirty: bool,
    presentation: Option<([f32; 2], f32, Rect)>,
    measurement_key: Option<MeasurementKey>,
    frame: Frame<Action>,
    focus_order: Vec<String>,
    focused: Option<String>,
    pressed: Option<String>,
    revision: u64,
    mounts: HashMap<String, Mount>,
    routing: RoutingState,
}

impl<Action> Default for Runtime<Action> {
    fn default() -> Self {
        let mut tree = tf::TaffyTree::new();
        tree.disable_rounding();
        Self {
            tree,
            root: None,
            stats: RuntimeStats::default(),
            geometry_dirty: true,
            presentation_dirty: true,
            presentation: None,
            measurement_key: None,
            frame: Frame::default(),
            focus_order: vec![],
            focused: None,
            pressed: None,
            revision: 0,
            mounts: HashMap::new(),
            routing: RoutingState::default(),
        }
    }
}

impl<Action: Clone> Runtime<Action> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn frame(&self) -> &Frame<Action> {
        &self.frame
    }
    pub fn focused_key(&self) -> Option<&str> {
        self.focused.as_deref()
    }
    pub fn pressed_key(&self) -> Option<&str> {
        self.pressed.as_deref()
    }
    pub fn release_pointer(&mut self) {
        self.pressed = None;
        self.routing.capture = None;
        self.routing.down = None;
    }
    pub fn captured_key(&self) -> Option<&str> {
        self.routing.capture.as_ref().map(|(key, _)| key.as_str())
    }
    pub fn hovered_key(&self) -> Option<&str> {
        self.routing.hover.last().map(String::as_str)
    }

    /// Required when the owning surface is hidden, replaced, or closed. Do not
    /// leave a hidden surface's last frame installed as active input geometry.
    pub fn stats(&self) -> RuntimeStats { self.stats }

    /// Reset cumulative work counters without discarding retained resources.
    pub fn reset_stats(&mut self) {
        self.stats = RuntimeStats { live_nodes: self.stats.live_nodes, ..RuntimeStats::default() };
    }

    /// Revoke visible geometry and queued input tokens, retaining mounted state.
    pub fn invalidate_presentation(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.frame = Frame { id: FrameId(self.revision), ..Frame::default() };
        self.presentation_dirty = true;
    }

    pub fn clear(&mut self) {
        self.invalidate_presentation();
        self.root = None;
        self.tree.clear();
        self.stats.live_nodes = 0;
        self.geometry_dirty = true;
        self.presentation = None;
        self.measurement_key = None;
        self.focus_order.clear();
        self.focused = None;
        self.pressed = None;
        self.mounts.clear();
        self.routing = RoutingState::default();
    }

    pub fn blur(&mut self) {
        self.release_pointer();
        self.focused = None;
        self.sync_semantic_focus();
    }

    /// Only enabled, mounted action targets or interactive semantic roles can
    /// be focused. Offscreen mounted controls retain focus (scrolling is host-owned).
    pub fn focus(&mut self, key: &str) -> bool {
        if !self.focus_order.iter().any(|candidate| candidate == key) || !self.in_active_scope(key) {
            return false;
        }
        if self.focused.as_deref() != Some(key) { self.release_pointer(); }
        self.focused = Some(key.to_owned());
        self.sync_semantic_focus();
        true
    }

    /// Tab order follows declarative preorder, not hash order; wraps at either end.
    pub fn focus_next(&mut self, backwards: bool) -> Option<&str> {
        let order: Vec<_> = self.focus_order.iter().filter(|key| self.in_active_scope(key)).cloned().collect();
        let count = order.len();
        if count == 0 {
            self.blur();
            return None;
        }
        let current = self.focused.as_ref().and_then(|key| {
            order.iter().position(|candidate| candidate == key)
        });
        let index = match (current, backwards) {
            (Some(0), true) | (None, true) => count - 1,
            (Some(index), true) => index - 1,
            (Some(index), false) => (index + 1) % count,
            (None, false) => 0,
        };
        if self.focused.as_ref() != Some(&order[index]) { self.release_pointer(); }
        self.focused = Some(order[index].clone());
        self.sync_semantic_focus();
        self.focused_key()
    }

    fn in_active_scope(&self, key: &str) -> bool {
        let Some((scope, _, _)) = self.routing.traps.last() else { return true; };
        self.is_descendant(key, scope)
    }

    fn is_descendant(&self, key: &str, ancestor: &str) -> bool {
        let mut next = Some(key);
        while let Some(key) = next {
            if key == ancestor { return true; }
            next = self.mounts.get(key).and_then(|mount| mount.parent.as_deref());
        }
        false
    }

    fn reconcile_routing(&mut self, old: &HashMap<String, Mount>, traps: Vec<String>) {
        let previous = std::mem::take(&mut self.routing.traps);
        let shared = previous.iter().zip(&traps).take_while(|(entry, key)| {
            entry.0 == **key && self.mounts.get(*key).is_some_and(|mount| mount.id == entry.1)
        }).count();
        for (_, _, restore) in previous.iter().skip(shared).rev() {
            if let Some((key, id)) = restore {
                if self.mounts.get(key).is_some_and(|mount| mount.id == *id && !mount.disabled && mount.focusable) {
                    self.focused = Some(key.clone());
                }
            }
        }
        self.routing.traps.extend(previous.into_iter().take(shared));
        for key in traps.into_iter().skip(shared) {
            let restore = self.focused.as_ref().and_then(|key| {
                self.mounts.get(key).or_else(|| old.get(key)).map(|mount| (key.clone(), mount.id))
            });
            let id = self.mounts[&key].id;
            self.routing.traps.push((key, id, restore));
            self.focused = self.focus_order.iter().find(|key| self.in_active_scope(key)).cloned();
        }
        if !self.routing.traps.is_empty()
            && !self.focused.as_ref().is_some_and(|key| self.in_active_scope(key)) {
            self.focused = self.focus_order.iter().find(|key| self.in_active_scope(key)).cloned();
        }
        let valid = |key: &String| {
            self.mounts.get(key).is_some_and(|mount| !mount.disabled
                && old.get(key).is_some_and(|before| before.id == mount.id))
        };
        let capture_valid = self.routing.capture.as_ref().is_none_or(|(key, _)| valid(key) && self.in_active_scope(key));
        let down_valid = self.routing.down.as_ref().is_none_or(|(key, _)| valid(key) && self.in_active_scope(key));
        let hover: Vec<_> = if self.routing.hover.last().is_none_or(|key| self.in_active_scope(key)) {
            self.routing.hover.iter().take_while(|key| valid(key)).cloned().collect()
        } else { Vec::new() };
        if !capture_valid { self.routing.capture = None; }
        if !down_valid {
            self.routing.down = None;
            self.pressed = None;
        }
        self.routing.hover = hover;
    }

    /// Route one platform-neutral event against the active frame. Host code
    /// applies returned actions after dispatch, then rebuilds/presents as needed.
    pub fn dispatch(&mut self, event: &InputEvent) -> Dispatch<Action> {
        let mut result = Dispatch::default();
        let before = self.focused.clone();
        if matches!(event, InputEvent::Focus(false)) {
            if let Some(key) = &self.focused {
                result.merge(self.frame.routes.route(key, EventKind::Blur, event));
            }
            self.update_hover(None, event, &mut result);
            if let Some((key, _)) = &self.routing.capture {
                result.merge(self.frame.routes.route(key, EventKind::PointerCancel, &InputEvent::PointerCancel));
            }
            self.blur();
        } else if self.presentation_dirty {
            return result;
        } else {
            match event {
                InputEvent::PointerDown { x, y, button, .. } => {
                    if !x.is_finite() || !y.is_finite() { return result; }
                    let target = self.pointer_target(*x, *y);
                    self.update_hover(target.as_deref(), event, &mut result);
                    self.release_pointer();
                    if let Some(key) = target {
                        result.merge(self.frame.routes.route(&key, EventKind::PointerDown, event));
                        if self.frame.routes.enabled(&key) && !result.prevent_default {
                            let path = self.frame.routes.path(&key);
                            if let Some(focus) = path.iter().rev().find(|key| self.mounts.get(*key).is_some_and(|m| m.focusable)) {
                                let focus = focus.clone();
                                self.focus(&focus);
                            }
                            self.pressed = Some(key.clone());
                            self.routing.down = Some((key.clone(), *button));
                            if let Some(capture) = path.iter().rev().find(|key|
                                self.in_active_scope(key) && self.frame.routes.nodes[*key].capture_pointer) {
                                self.routing.capture = Some((capture.clone(), *button));
                            }
                            // The compatibility action is a default, not an extra
                            // listener. An explicit PointerDown listener on its
                            // owner replaces this alias rather than firing twice.
                            if *button == PointerButton::Primary {
                                if let Some(owner) = path.iter().rev().find(|key| self.frame.routes.nodes[*key].action.is_some()) {
                                    let node = &self.frame.routes.nodes[owner];
                                    if !node.bindings.iter().any(|b| b.kind == EventKind::PointerDown) {
                                        if let Some(action) = &node.action {
                                            result.actions.push(RoutedAction { key: owner.clone(), action: action.clone(),
                                                event: event.clone(), kind: EventKind::PointerDown, phase: EventPhase::Target });
                                            result.consumed = true;
                                        }
                                    }
                                }
                            }
                        }
                    } else if self.routing.traps.is_empty() {
                        self.blur();
                    } else { result.consumed = true; }
                }
                InputEvent::PointerMove { x, y } => {
                    if !x.is_finite() || !y.is_finite() { return result; }
                    let hit = self.pointer_target(*x, *y);
                    self.update_hover(hit.as_deref(), event, &mut result);
                    let target = self.routing.capture.as_ref().map(|(key, _)| key.clone()).or(hit);
                    if let Some(key) = target { result.merge(self.frame.routes.route(&key, EventKind::PointerMove, event)); }
                }
                InputEvent::PointerUp { x, y, button } => {
                    if !x.is_finite() || !y.is_finite() { return result; }
                    let hit = self.pointer_target(*x, *y);
                    self.update_hover(hit.as_deref(), event, &mut result);
                    let target = self.routing.capture.as_ref().filter(|(_, captured)| captured == button)
                        .map(|(key, _)| key.clone()).or_else(|| hit.clone());
                    if let Some(key) = target { result.merge(self.frame.routes.route(&key, EventKind::PointerUp, event)); }
                    if let Some((down, pressed_button)) = self.routing.down.clone() {
                        if pressed_button == *button {
                            if *button == PointerButton::Primary && !result.prevent_default
                                && hit.as_ref().is_some_and(|hit| self.is_descendant(hit, &down)) {
                                result.merge(self.frame.routes.route(&down, EventKind::Click, event));
                            }
                            self.release_pointer();
                        }
                    } else if self.routing.capture.as_ref().is_some_and(|(_, captured)| captured == button) {
                        self.release_pointer();
                    }
                }
                InputEvent::PointerCancel => {
                    let target = self.routing.capture.as_ref().or(self.routing.down.as_ref()).map(|(key, _)| key.clone());
                    if let Some(key) = target { result.merge(self.frame.routes.route(&key, EventKind::PointerCancel, event)); }
                    self.release_pointer();
                    self.update_hover(None, event, &mut result);
                }
                InputEvent::Wheel { x, y, dx, dy } => {
                    if ![x, y, dx, dy].into_iter().all(|v| v.is_finite()) { return result; }
                    if let Some(key) = self.pointer_target(*x, *y) {
                        result.merge(self.frame.routes.route(&key, EventKind::Wheel, event));
                    } else if !self.routing.traps.is_empty() { result.consumed = true; }
                }
                InputEvent::Key { key, modifiers, repeat } => {
                    let target = self.focused.clone().or_else(|| self.frame.routes.order.first().cloned());
                    if let Some(target) = target {
                        result.merge(self.frame.routes.route(&target, EventKind::Key, event));
                        if !result.prevent_default {
                            let path = self.frame.routes.path(&target);
                            let binding = path.iter().rev().find_map(|owner| {
                                self.frame.routes.nodes[owner].key_bindings.iter()
                                    .find(|binding| binding.key == *key && binding.modifiers == *modifiers)
                                    .map(|binding| (owner.clone(), binding.clone()))
                            });
                            if let Some((owner, binding)) = binding {
                                // A repeated shortcut is consumed even when its
                                // action is not repeatable; no fallback activation.
                                if !repeat || binding.repeat {
                                    result.actions.push(RoutedAction { key: owner, action: binding.action,
                                        event: event.clone(), kind: EventKind::Key, phase: EventPhase::Target });
                                }
                                result.consumed = true;
                                result.prevent_default = true;
                            } else if *key == Key::Tab && !modifiers.control && !modifiers.alt && !modifiers.meta {
                                self.focus_next(modifiers.shift);
                                result.consumed = true;
                                result.prevent_default = true;
                            } else if matches!(key, Key::Enter | Key::Space) && !repeat && *modifiers == Modifiers::default() {
                                if let Some(focused) = self.focused.clone() {
                                    self.activate(&focused, event, &mut result);
                                }
                            }
                        }
                    }
                }
                InputEvent::Focus(true) => {
                    if self.focused.is_none() && !self.routing.traps.is_empty() { self.focus_next(false); }
                    // Focus transitions below deliver the listener exactly once.
                }
                _ => {
                    if let Some(key) = &self.focused {
                        result.merge(self.frame.routes.route(key, EventKind::of(event), event));
                    }
                }
            }
        }
        if before != self.focused {
            if !matches!(event, InputEvent::Focus(false)) {
                if let Some(key) = &before { result.merge(self.frame.routes.route(key, EventKind::Blur, event)); }
            }
            if let Some(key) = &self.focused { result.merge(self.frame.routes.route(key, EventKind::Focus, event)); }
            result.focus_changed = true;
        }
        result.needs_redraw |= result.focus_changed || !result.actions.is_empty();
        result
    }

    pub fn dispatch_in(&mut self, frame: FrameId, event: &InputEvent) -> Dispatch<Action> {
        if frame != self.frame.id { return Dispatch::default(); }
        self.dispatch(event)
    }

    fn pointer_target(&self, x: f32, y: f32) -> Option<String> {
        self.frame.routes.hit(x, y).filter(|key| self.in_active_scope(key)).map(str::to_owned)
    }

    fn activate(&self, key: &str, event: &InputEvent, result: &mut Dispatch<Action>) {
        if !self.frame.routes.enabled(key) { return; }
        let path = self.frame.routes.path(key);
        if path.iter().any(|owner| self.frame.routes.has_click(owner)) {
            result.merge(self.frame.routes.route(key, EventKind::Click, event));
        } else if let Some(owner) = path.iter().rev().find(|owner| self.frame.routes.nodes[*owner].action.is_some()) {
            if let Some(action) = &self.frame.routes.nodes[owner].action {
                result.actions.push(RoutedAction { key: owner.clone(), action: action.clone(),
                    event: event.clone(), kind: EventKind::Click, phase: EventPhase::Target });
                result.consumed = true;
            }
        }
    }

    fn update_hover(&mut self, target: Option<&str>, event: &InputEvent, result: &mut Dispatch<Action>) {
        let next = target.filter(|key| self.frame.routes.enabled(key))
            .map(|key| self.frame.routes.path(key)).unwrap_or_default();
        let shared = self.routing.hover.iter().zip(&next).take_while(|(a, b)| a == b).count();
        for key in self.routing.hover.iter().skip(shared).rev() {
            result.merge_notification(self.frame.routes.transition(key, EventKind::HoverLeave, event));
        }
        for key in next.iter().skip(shared) {
            result.merge_notification(self.frame.routes.transition(key, EventKind::HoverEnter, event));
        }
        result.needs_redraw |= self.routing.hover != next;
        self.routing.hover = next;
    }

    fn sync_semantic_focus(&mut self) {
        for node in &mut self.frame.semantics {
            node.focused = self.focused.as_deref() == Some(node.key.as_str());
        }
    }

    /// Dispatch against the current layout only. For queued events prefer the
    /// frame-scoped variant. A miss blurs focus; a disabled target does not.
    pub fn dispatch_pointer_down(&mut self, x: f32, y: f32) -> Option<Action> {
        let target = self
            .frame
            .hit_test(x, y)
            .map(|hit| (hit.key.clone(), hit.action.clone()));
        self.release_pointer();
        match target {
            Some((key, Some(action))) => {
                if !self.in_active_scope(&key) { return None; }
                self.focus(&key);
                self.pressed = Some(key);
                Some(action)
            }
            Some((_, None)) => None,
            None => {
                if self.routing.traps.is_empty() { self.blur(); }
                None
            }
        }
    }

    pub fn dispatch_pointer_down_in(
        &mut self,
        frame: FrameId,
        x: f32,
        y: f32,
    ) -> Option<Action> {
        if frame != self.frame.id {
            return None;
        }
        self.dispatch_pointer_down(x, y)
    }

    /// Resolve a tree at a logical origin with an exact outer width and an
    /// unconstrained height. The root's width/min/max-width style is overridden
    /// by `available_width`; its height style is honored (Auto by default).
    /// `clip` is the host viewport, NOT a layout height constraint. Thus measuring
    /// at origin zero and painting at a scrolled origin give identical heights.
    ///
    /// Each successful call replaces all paint, hits and semantics atomically.
    /// No user callbacks are retained. Invalid inputs clear mounted state and
    /// active input geometry. An unstable measurement context revokes the frame
    /// and clears provisional measurements but retains the mounted description
    /// and focus for a later retry.
    pub fn layout(
        &mut self,
        root: Element<Action>,
        origin: [f32; 2],
        available_width: f32,
        clip: Rect,
        measurer: &mut dyn TextMeasurer,
    ) -> Result<&Frame<Action>, LayoutError> {
        self.set_tree(root)?;
        self.present(origin, available_width, clip, measurer)
    }

    /// Reconcile a validated description. Actions are always considered changed;
    /// no equality bound on the host's action type is required.
    pub fn set_tree(&mut self, root: Element<Action>) -> Result<(), LayoutError> {
        self.invalidate_presentation();
        if let Err(error) = validate(&root, &mut HashSet::new(), 0) {
            dispose_description(root);
            self.clear();
            return Err(error);
        }
        let old_mounts = std::mem::take(&mut self.mounts);
        let mut old = HashMap::new();
        if let Some(previous) = self.root.take() {
            collect_nodes(previous, &mut old);
        }
        let result = reconcile(root, &[], false, &mut old, &mut self.tree,
            &mut self.stats, &mut self.geometry_dirty, &mut self.focused, &mut self.pressed);
        match result {
            Ok(root) => {
                for (_, previous) in old {
                    if let Err(error) = self.tree.remove(previous.node) {
                        self.clear();
                        return Err(error.into());
                    }
                    self.geometry_dirty = true;
                }
                self.stats.live_nodes = self.tree.total_node_count();
                self.root = Some(root);
                // Disabled/unmounted controls cannot retain focus even before present.
                self.focus_order.clear();
                mounted_focus(self.root.as_ref().expect("installed root"), false, &mut self.focus_order);
                let mut traps = Vec::new();
                collect_mounts(self.root.as_ref().expect("installed root"), None, false,
                    &mut self.mounts, &mut traps);
                self.reconcile_routing(&old_mounts, traps);
                if !self.focused.as_ref().is_some_and(|key| self.focus_order.contains(key)) {
                    self.focused = None;
                }
                if !self.pressed.as_ref().is_some_and(|key| self.focus_order.contains(key)) {
                    self.pressed = None;
                }
                Ok(())
            }
            Err(error) => { self.clear(); Err(error) }
        }
    }

    /// Present retained layout. Origin and clip affect every emitted geometry
    /// channel but never participate in Taffy's layout constraints.
    pub fn present(
        &mut self,
        origin: [f32; 2],
        width: f32,
        clip: Rect,
        measurer: &mut dyn TextMeasurer,
    ) -> Result<&Frame<Action>, LayoutError> {
        let result = self.present_inner(origin, width, clip, measurer);
        if let Err(error) = result {
            if error == LayoutError::MeasurementContextChanged {
                self.invalidate_presentation();
                self.invalidate_measurements()?;
            } else {
                self.clear();
            }
            return Err(error);
        }
        Ok(&self.frame)
    }

    fn invalidate_measurements(&mut self) -> Result<(), LayoutError> {
        fn visit<A>(root: &Resolved<A>, tree: &mut tf::TaffyTree<TextContext>) -> Result<(), LayoutError> {
            if let Some(context) = tree.get_node_context_mut(root.node) {
                context.clear_cache();
                tree.mark_dirty(root.node)?;
            }
            for child in &root.children { visit(child, tree)?; }
            Ok(())
        }
        if let Some(root) = &self.root { visit(root, &mut self.tree)?; }
        self.geometry_dirty = true;
        self.measurement_key = None;
        Ok(())
    }

    fn present_inner(
        &mut self, origin: [f32; 2], width: f32, clip: Rect,
        measurer: &mut dyn TextMeasurer,
    ) -> Result<(), LayoutError> {
        if !origin.into_iter().all(f32::is_finite) { return Err(invalid(None, "origin")); }
        if !valid_length(width) { return Err(invalid(None, "available_width")); }
        if !clip.is_valid() { return Err(invalid(None, "clip")); }
        let Some(root) = &self.root else {
            return Err(LayoutError::Engine("no mounted tree".into()));
        };
        if root.element.style.padding.left + root.element.style.padding.right > width {
            return Err(LayoutError::IncompatibleRootWidth);
        }
        // A bounded transaction: an epoch change discards every provisional text
        // result and dirties the engine before retrying with the new context.
        for _ in 0..3 {
            let epoch = measurer.cache_key();
            if epoch.is_none() || epoch != self.measurement_key {
                self.invalidate_measurements()?;
            }
            let same_presentation = self.presentation == Some((origin, width, clip));
            if !self.geometry_dirty && !self.presentation_dirty && same_presentation {
                if measurer.cache_key() == epoch { return Ok(()); }
                self.invalidate_measurements()?;
                continue;
            }
            let mut checked = CheckedMeasurer { inner: measurer, epoch, changed: false, calls: 0 };
            let result = (|| {
                let root = self.root.as_ref().expect("mounted tree");
                let mut style = self.tree.style(root.node)?.clone();
                style.size.width = tf::length(width);
                style.min_size.width = tf::length(width);
                style.max_size.width = tf::length(width);
                if self.tree.style(root.node)? != &style {
                    self.tree.set_style(root.node, style)?;
                    self.geometry_dirty = true;
                }
                if self.geometry_dirty {
                    self.stats.layout_computations += 1;
                    compute(&mut self.tree, root.node, width, &mut checked)?;
                }
                if self.tree.layout(root.node)?.size.width != width {
                    return Err(LayoutError::IncompatibleRootWidth);
                }
                let mut frame = Frame::default();
                let mut order = Vec::new();
                emit(root, &mut self.tree, origin, clip, false, None,
                    &mut frame, &mut order, &mut checked)?;
                frame.bounds = frame.bounds_for(&root.element.key)
                    .ok_or_else(|| LayoutError::Engine("missing root box".into()))?;
                Ok((frame, order))
            })();
            // Query after even the last cached/empty text or geometry-only pass.
            checked.cache_key();
            self.stats.measured_text += checked.calls;
            let changed = checked.changed;
            if changed || matches!(result, Err(LayoutError::MeasurementContextChanged)) {
                self.invalidate_presentation();
                self.invalidate_measurements()?;
                continue;
            }
            let (mut frame, order) = result?;
            self.revision = self.revision.wrapping_add(1);
            frame.id = FrameId(self.revision);
            self.frame = frame;
            self.focus_order = order;
            if !self.focused.as_ref().is_some_and(|key| self.focus_order.contains(key)) {
                self.focused = None;
            }
            self.sync_semantic_focus();
            self.presentation = Some((origin, width, clip));
            self.measurement_key = epoch;
            self.geometry_dirty = false;
            self.presentation_dirty = false;
            self.stats.emitted_frames += 1;
            return Ok(());
        }
        Err(LayoutError::MeasurementContextChanged)
    }
}

struct Resolved<Action> {
    node: tf::NodeId,
    element: Element<Action>,
    children: Vec<Self>,
    identity: Vec<(String, u8)>,
}

fn invalid(key: Option<&str>, field: &'static str) -> LayoutError {
    LayoutError::InvalidInput {
        key: key.map(str::to_owned),
        field,
    }
}

fn valid_color(color: Color) -> bool {
    color
        .into_iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
}
fn valid_length(value: f32) -> bool {
    value.is_finite() && value >= 0.0
}
fn valid_size(value: Size) -> bool {
    match value {
        Size::Auto => true,
        Size::Px(v) => valid_length(v),
        Size::Percent(v) => v.is_finite() && (0.0..=1.0).contains(&v),
    }
}

fn validate<Action>(
    element: &Element<Action>,
    keys: &mut HashSet<String>,
    depth: usize,
) -> Result<(), LayoutError> {
    if depth > 128 {
        return Err(LayoutError::TreeTooDeep);
    }
    if keys.len() >= 100_000 {
        return Err(LayoutError::TooManyElements);
    }
    if element.key.is_empty() {
        return Err(LayoutError::EmptyKey);
    }
    if !keys.insert(element.key.clone()) {
        return Err(LayoutError::DuplicateKey(element.key.clone()));
    }
    let s = &element.style;
    for (field, size) in [
        ("width", s.width),
        ("height", s.height),
        ("min_width", s.min_width),
        ("min_height", s.min_height),
        ("max_width", s.max_width),
        ("max_height", s.max_height),
        ("flex_basis", s.flex_basis),
    ] {
        if !valid_size(size) {
            return Err(invalid(Some(&element.key), field));
        }
    }
    for (field, value) in [
        ("gap", s.gap),
        ("flex_grow", s.flex_grow),
        ("flex_shrink", s.flex_shrink),
        ("padding.top", s.padding.top),
        ("padding.right", s.padding.right),
        ("padding.bottom", s.padding.bottom),
        ("padding.left", s.padding.left),
    ] {
        if !valid_length(value) {
            return Err(invalid(Some(&element.key), field));
        }
    }
    match &element.kind {
        Kind::Text { style, .. } => {
            if !style.size.is_finite() || style.size <= 0.0 {
                return Err(invalid(Some(&element.key), "text.size"));
            }
            if !style.line_height.is_finite() || style.line_height <= 0.0 {
                return Err(invalid(Some(&element.key), "text.line_height"));
            }
            if !valid_color(style.color) {
                return Err(invalid(Some(&element.key), "text.color"));
            }
        }
        Kind::Quad { color, radius } => {
            if !valid_color(*color) {
                return Err(invalid(Some(&element.key), "quad.color"));
            }
            if !valid_length(*radius) {
                return Err(invalid(Some(&element.key), "quad.radius"));
            }
        }
        _ => {}
    }
    for child in &element.children {
        validate(child, keys, depth + 1)?;
    }
    Ok(())
}

fn dimension(size: Size) -> tf::Dimension {
    match size {
        Size::Auto => tf::Dimension::auto(),
        Size::Px(value) => tf::length(value),
        Size::Percent(value) => tf::percent(value),
    }
}
fn alignment(align: Align) -> tf::AlignItems {
    match align {
        Align::Start => tf::AlignItems::Start,
        Align::Center => tf::AlignItems::Center,
        Align::End => tf::AlignItems::End,
        Align::Stretch => tf::AlignItems::Stretch,
    }
}
fn engine_style(style: &Style, kind: &Kind, in_stack: bool) -> tf::Style {
    let mut output = tf::Style {
        display: if matches!(kind, Kind::Stack) {
            tf::Display::Grid
        } else {
            tf::Display::Flex
        },
        flex_direction: if matches!(kind, Kind::Row) {
            tf::FlexDirection::Row
        } else {
            tf::FlexDirection::Column
        },
        size: tf::Size {
            width: dimension(style.width),
            height: dimension(style.height),
        },
        min_size: tf::Size {
            width: dimension(style.min_width),
            height: dimension(style.min_height),
        },
        max_size: tf::Size {
            width: dimension(style.max_width),
            height: dimension(style.max_height),
        },
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        flex_basis: dimension(style.flex_basis),
        gap: tf::Size {
            width: tf::length(style.gap),
            height: tf::length(style.gap),
        },
        padding: tf::Rect {
            top: tf::length(style.padding.top),
            right: tf::length(style.padding.right),
            bottom: tf::length(style.padding.bottom),
            left: tf::length(style.padding.left),
        },
        align_items: Some(alignment(style.align_items)),
        align_self: style.align_self.map(alignment),
        justify_content: Some(match style.justify_content {
            Justify::Start => tf::JustifyContent::Start,
            Justify::Center => tf::JustifyContent::Center,
            Justify::End => tf::JustifyContent::End,
            Justify::SpaceBetween => tf::JustifyContent::SpaceBetween,
            Justify::SpaceAround => tf::JustifyContent::SpaceAround,
        }),
        ..tf::Style::default()
    };
    if matches!(kind, Kind::Stack) {
        // A single fractional column fills the stack's assigned width even
        // when every child is a quad with zero intrinsic width.
        output.grid_template_columns = vec![tf::fr(1.0)];
        output.justify_items = Some(tf::AlignItems::Stretch);
    }
    if in_stack {
        output.grid_row = tf::Line {
            start: tf::line(1),
            end: tf::line(2),
        };
        output.grid_column = tf::Line {
            start: tf::line(1),
            end: tf::line(2),
        };
    }
    output
}

fn kind_tag(kind: &Kind) -> u8 {
    match kind {
        Kind::Row => 0,
        Kind::Column => 1,
        Kind::Stack => 2,
        Kind::Text { .. } => 3,
        Kind::Quad { .. } => 4,
        Kind::Spacer => 5,
    }
}

fn collect_nodes<A>(mut root: Resolved<A>, nodes: &mut HashMap<String, Resolved<A>>) {
    for child in std::mem::take(&mut root.children) { collect_nodes(child, nodes); }
    nodes.insert(root.element.key.clone(), root);
}

#[allow(clippy::too_many_arguments)]
fn reconcile<A>(
    mut element: Element<A>, parent: &[(String, u8)], in_stack: bool,
    old: &mut HashMap<String, Resolved<A>>, tree: &mut tf::TaffyTree<TextContext>,
    stats: &mut RuntimeStats, dirty: &mut bool, focused: &mut Option<String>,
    pressed: &mut Option<String>,
) -> Result<Resolved<A>, LayoutError> {
    stats.reconciled_nodes += 1;
    let mut identity = parent.to_vec();
    identity.push((element.key.clone(), kind_tag(&element.kind)));
    let reusable = old.get(&element.key).is_some_and(|node| node.identity == identity);
    let mut style = engine_style(&element.style, &element.kind, in_stack);
    let node = if reusable {
        let previous = old.remove(&element.key).expect("matched retained node");
        // The public root width is controlled solely by present(), not Style.
        if parent.is_empty() {
            let previous_style = tree.style(previous.node)?;
            style.size.width = previous_style.size.width;
            style.min_size.width = previous_style.min_size.width;
            style.max_size.width = previous_style.max_size.width;
        }
        if tree.style(previous.node)? != &style {
            tree.set_style(previous.node, style)?;
            *dirty = true;
        }
        if let Kind::Text { text, style } = &element.kind {
            let context = tree.get_node_context_mut(previous.node)
                .ok_or_else(|| LayoutError::Engine("missing retained text context".into()))?;
            if !context.matches(text, style) {
                *context = TextContext::new(text.clone(), style.clone());
                tree.mark_dirty(previous.node)?;
                *dirty = true;
            } else {
                context.style = style.clone();
            }
        }
        previous.node
    } else {
        if focused.as_deref() == Some(element.key.as_str()) { *focused = None; }
        if pressed.as_deref() == Some(element.key.as_str()) { *pressed = None; }
        *dirty = true;
        stats.created_nodes += 1;
        match &element.kind {
            Kind::Text { text, style: text_style } => tree.new_leaf_with_context(
                style, TextContext::new(text.clone(), text_style.clone()))?,
            _ => tree.new_leaf(style)?,
        }
    };
    let is_stack = matches!(element.kind, Kind::Stack);
    let mut children = Vec::new();
    for child in std::mem::take(&mut element.children) {
        children.push(reconcile(child, &identity, is_stack, old, tree, stats, dirty, focused, pressed)?);
    }
    let child_ids: Vec<_> = children.iter().map(|child| child.node).collect();
    if tree.children(node)? != child_ids {
        tree.set_children(node, &child_ids)?;
        *dirty = true;
    }
    Ok(Resolved { node, element, children, identity })
}

fn focusable<A>(element: &Element<A>) -> bool {
    element.focusable || element.action.is_some()
        || element.semantics.as_ref().is_some_and(|s| interactive(&s.role))
        || element.event_bindings.iter().any(|binding| matches!(binding.kind,
            EventKind::Click | EventKind::Key | EventKind::Text | EventKind::Paste
            | EventKind::ImePreedit | EventKind::ImeCommit | EventKind::ImeCancel))
}

fn collect_mounts<A>(root: &Resolved<A>, parent: Option<&str>, inherited_disabled: bool,
    mounts: &mut HashMap<String, Mount>, traps: &mut Vec<String>) {
    let element = &root.element;
    let disabled = inherited_disabled || element.semantics.as_ref().is_some_and(|s| s.disabled);
    mounts.insert(element.key.clone(), Mount { id: root.node, parent: parent.map(str::to_owned),
        disabled, scope: element.focus_scope, focusable: focusable(element) });
    if !disabled && element.focus_scope == Some(true) { traps.push(element.key.clone()); }
    for child in &root.children {
        collect_mounts(child, Some(&element.key), disabled, mounts, traps);
    }
}

fn mounted_focus<A>(root: &Resolved<A>, inherited_disabled: bool, order: &mut Vec<String>) {
    let element = &root.element;
    let disabled = inherited_disabled || element.semantics.as_ref().is_some_and(|s| s.disabled);
    if !disabled && focusable(element) {
        order.push(element.key.clone());
    }
    for child in &root.children { mounted_focus(child, disabled, order); }
}

struct CheckedMeasurer<'a> {
    inner: &'a mut dyn TextMeasurer,
    epoch: Option<MeasurementKey>,
    changed: bool,
    calls: u64,
}

impl TextMeasurer for CheckedMeasurer<'_> {
    fn cache_key(&mut self) -> Option<MeasurementKey> {
        let key = self.inner.cache_key();
        self.changed |= key != self.epoch;
        key
    }
    fn measure(&mut self, text: &str, style: &TextStyle) -> f32 {
        self.cache_key();
        if self.changed { return f32::NAN; }
        self.calls += 1;
        let width = self.inner.measure(text, style);
        self.cache_key();
        width
    }
}

// Rejected descriptions have NOT passed the depth bound. Never let Rust's
// recursive Vec<Element> drop traverse them, including on input-validation
// errors that precede tree validation. Each popped node is made childless
// before it drops; accepted descriptions can use ordinary bounded-depth drop.
fn dispose_description<Action>(root: Element<Action>) {
    let mut pending = vec![root];
    while let Some(mut element) = pending.pop() {
        pending.append(&mut element.children);
    }
}

fn compute(
    tree: &mut tf::TaffyTree<TextContext>,
    root: tf::NodeId,
    width: f32,
    measurer: &mut dyn TextMeasurer,
) -> Result<(), LayoutError> {
    let mut measurement_error = None;
    tree.compute_layout_with_measure(
        root,
        tf::Size {
            width: tf::AvailableSpace::Definite(width),
            height: tf::AvailableSpace::MaxContent,
        },
        |_known, available, _, context, _| {
            let Some(context) = context else {
                return tf::Size::ZERO;
            };
            // Taffy 0.9's ComputeSize pass supplies border-box `known`
            // dimensions, but `available` has already had padding subtracted.
            // Its PerformLayout pass supplies no known dimensions at all.
            // Always measure and return CONTENT size using the latter contract;
            // the leaf algorithm applies known border-box sizes and padding.
            // Mixing these boxes undermeasures padded flex-row text height.
            let constraint = match available.width {
                tf::AvailableSpace::Definite(width) => Some(width.max(0.0)),
                tf::AvailableSpace::MinContent => Some(0.0),
                tf::AvailableSpace::MaxContent => None,
            };
            match context.layout(constraint, measurer) {
                Ok(text) => tf::Size {
                    width: text.width,
                    height: text.height,
                },
                Err(error) => {
                    measurement_error = Some(error);
                    tf::Size::ZERO
                }
            }
        },
    )?;
    if let Some(error) = measurement_error {
        return Err(error);
    }
    Ok(())
}

fn interactive(role: &Role) -> bool {
    matches!(role, Role::Button | Role::Checkbox | Role::Switch)
}

// Traversal order is the single source of painter and input z-order. Parent
// targets precede their children; overlapping later siblings win in both lists.
#[allow(clippy::too_many_arguments)]
fn emit<Action: Clone>(
    resolved: &Resolved<Action>,
    tree: &mut tf::TaffyTree<TextContext>,
    parent_origin: [f32; 2],
    inherited_clip: Rect,
    inherited_disabled: bool,
    semantic_parent: Option<&str>,
    frame: &mut Frame<Action>,
    focus_order: &mut Vec<String>,
    measurer: &mut dyn TextMeasurer,
) -> Result<(), LayoutError> {
    let layout = *tree.layout(resolved.node)?;
    let bounds = Rect::new(
        parent_origin[0] + layout.location.x,
        parent_origin[1] + layout.location.y,
        layout.size.width,
        layout.size.height,
    );
    if !bounds.is_valid() {
        return Err(invalid(Some(&resolved.element.key), "resolved bounds"));
    }
    let element = &resolved.element;
    frame.boxes.insert(element.key.clone(), bounds);
    let clip = if element.style.clip {
        inherited_clip.intersection(bounds)
    } else {
        inherited_clip
    };
    let visible = !bounds.intersection(clip).is_empty();
    let disabled =
        inherited_disabled || element.semantics.as_ref().is_some_and(|s| s.disabled);
    frame.routes.order.push(element.key.clone());
    frame.routes.nodes.insert(element.key.clone(), RouteNode {
        mount: Mount {
            id: resolved.node,
            parent: resolved.identity.iter().rev().nth(1).map(|(key, _)| key.clone()),
            disabled,
            scope: element.focus_scope,
            focusable: focusable(element),
        },
        bounds, clip,
        bindings: element.event_bindings.clone(),
        key_bindings: element.key_bindings.clone(),
        action: element.action.clone(),
        capture_pointer: element.capture_pointer,
    });
    if !disabled && focusable(element)
    {
        focus_order.push(element.key.clone());
    }
    if visible {
        match &element.kind {
            Kind::Quad { color, radius } => frame.paint.push(PaintCommand::Quad {
                bounds,
                color: *color,
                radius: radius.min(bounds.width.min(bounds.height) / 2.0),
                clip,
            }),
            Kind::Text { style, .. } => {
                let content = Rect::new(
                    bounds.x + layout.padding.left,
                    bounds.y + layout.padding.top,
                    (bounds.width - layout.padding.left - layout.padding.right).max(0.0),
                    (bounds.height - layout.padding.top - layout.padding.bottom).max(0.0),
                );
                let text_clip = clip.intersection(content);
                let context = tree
                    .get_node_context_mut(resolved.node)
                    .ok_or_else(|| LayoutError::Engine("missing text context".into()))?;
                let text = context.layout(Some(content.width), measurer)?;
                for (index, line) in text.lines.into_iter().enumerate() {
                    let line_bounds = Rect::new(
                        content.x,
                        content.y + index as f32 * style.line_height,
                        line.width,
                        style.line_height,
                    );
                    if !line.text.is_empty()
                        && !line_bounds.intersection(text_clip).is_empty()
                    {
                        frame.paint.push(PaintCommand::Text {
                            bounds: line_bounds,
                            text: line.text,
                            style: style.clone(),
                            clip: text_clip,
                        });
                    }
                }
            }
            _ => {}
        }
        if let Some(action) = &element.action {
            frame.hits.push(HitRegion {
                key: element.key.clone(),
                bounds,
                clip,
                action: if disabled { None } else { Some(action.clone()) },
            });
        }
        if let Some(semantics) = &element.semantics {
            let mut semantics = semantics.clone();
            semantics.disabled = disabled;
            frame.semantics.push(SemanticNode {
                key: element.key.clone(),
                parent: semantic_parent.map(str::to_owned),
                bounds,
                clip,
                semantics,
                focused: false,
            });
        }
    }
    // An offscreen parent may overflow; never cull its descendants solely by the
    // parent's box. Explicit clips still apply to every emitted descendant.
    let semantic_parent = if visible && element.semantics.is_some() {
        Some(element.key.as_str())
    } else {
        semantic_parent
    };
    for child in &resolved.children {
        emit(
            child,
            tree,
            [bounds.x, bounds.y],
            clip,
            disabled,
            semantic_parent,
            frame,
            focus_order,
            measurer,
        )?;
    }
    Ok(())
}
