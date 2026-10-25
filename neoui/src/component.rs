//! Owned, explicitly invalidated components for one presentation surface.
//!
//! Props and events are the only invalidation inputs: this is not dependency
//! tracking, an async executor, or an automatic subscription system. External
//! work must send a typed event back through the owning host. Focus changes made
//! through `runtime_mut()` likewise need an explicit prop/event if they affect
//! component rendering.
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Element, Frame, LayoutError, Rect, Runtime, TextMeasurer};

static NEXT_HOST: AtomicU64 = AtomicU64::new(1);

/// A component definition, not a shared instance. All instance data lives in
/// `State`; no application references or callbacks are retained by the host.
pub trait Component: Send + 'static {
    type Props: PartialEq + Clone + Send + 'static;
    type State: Send + 'static;
    type Event: Send + 'static;
    type Action: Clone + 'static;

    fn mount(
        props: &Self::Props,
        cx: &mut ComponentContext<'_, Self::Action>,
    ) -> Self::State;
    fn update(
        state: &mut Self::State,
        props: &Self::Props,
        event: Self::Event,
        cx: &mut ComponentContext<'_, Self::Action>,
    ) -> Vec<Self::Action>;
    fn render(
        state: &Self::State,
        props: &Self::Props,
        cx: &mut ComponentContext<'_, Self::Action>,
    ) -> Element<Self::Action>;
    fn unmount(_state: &mut Self::State, _props: &Self::Props) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Identity {
    host: u64,
    slot: usize,
    generation: u64,
}

/// A weak identity: copying this handle never keeps an owner alive. Both host
/// identity and slot generation are checked before any event can mutate state.
pub struct ComponentHandle<C: Component> {
    id: Identity,
    marker: PhantomData<fn() -> C>,
}
impl<C: Component> Copy for ComponentHandle<C> {}
impl<C: Component> Clone for ComponentHandle<C> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C: Component> PartialEq for ComponentHandle<C> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<C: Component> Eq for ComponentHandle<C> {}
impl<C: Component> fmt::Debug for ComponentHandle<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ComponentHandle").field(&self.id).finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentError {
    NoRoot,
    EmptyKey,
    DuplicateChildKey(String),
    StaleHandle,
    WrongHost,
    WrongType,
    InvalidPhase,
    TreeTooDeep,
    Layout(LayoutError),
}
impl fmt::Display for ComponentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRoot => f.write_str("component host has no root"),
            Self::EmptyKey => f.write_str("component keys must not be empty"),
            Self::DuplicateChildKey(key) => {
                write!(f, "duplicate child component key: {key}")
            }
            Self::StaleHandle => f.write_str("component handle is no longer live"),
            Self::WrongHost => f.write_str("component handle belongs to another host"),
            Self::WrongType => f.write_str("component handle has the wrong type"),
            Self::InvalidPhase => {
                f.write_str("children may only be reconciled during render")
            }
            Self::TreeTooDeep => f.write_str("component tree exceeds 128 owners"),
            Self::Layout(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ComponentError {}
impl From<LayoutError> for ComponentError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

/// Counters are cumulative since reset; `owners` is always a live gauge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComponentStats {
    pub owners: usize,
    pub mounts: u64,
    pub renders: u64,
    pub events: u64,
    pub disposals: u64,
    pub cleanups: u64,
}

trait Erased<Action: Clone + 'static>: Send {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn render(&self, cx: &mut ComponentContext<'_, Action>) -> Element<Action>;
    fn unmount(&mut self);
}
struct Instance<C: Component> {
    props: C::Props,
    state: C::State,
}
impl<C: Component> Erased<C::Action> for Instance<C> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn render(&self, cx: &mut ComponentContext<'_, C::Action>) -> Element<C::Action> {
        C::render(&self.state, &self.props, cx)
    }
    fn unmount(&mut self) {
        C::unmount(&mut self.state, &self.props);
    }
}
struct Slot<Action: Clone + 'static> {
    generation: u64,
    key: String,
    kind: TypeId,
    parent: Option<usize>,
    instance: Option<Box<dyn Erased<Action>>>,
    children: Vec<usize>,
    cached: Option<Element<Action>>,
    dirty: bool,
    cleanups: Vec<Box<dyn FnOnce() + Send>>,
}

/// Contexts are borrowed only for a lifecycle call. `own` attaches an owned
/// disposer to this owner (not to this render). Register subscriptions/tasks in
/// mount/update, or guard render-time registration in your state. Disposers run
/// once, in reverse registration order, after the owner's unmount hook; all
/// descendants have already been disposed. Closures must not panic.
pub struct ComponentContext<'a, Action: Clone + 'static> {
    host: &'a mut ComponentHost<Action>,
    owner: usize,
    rendering: bool,
    depth: usize,
    claimed: HashSet<String>,
    children: Vec<usize>,
}
impl<Action: Clone + 'static> ComponentContext<'_, Action> {
    pub fn own(&mut self, cleanup: impl FnOnce() + Send + 'static) {
        self.host.slots[self.owner]
            .as_mut()
            .unwrap()
            .cleanups
            .push(Box::new(cleanup));
    }

    /// Obtain this owner's typed weak handle, useful in messages to external
    /// workers. It must be delivered back to this host, never dereferenced.
    pub fn handle<C: Component<Action = Action>>(
        &self,
    ) -> Result<ComponentHandle<C>, ComponentError> {
        let slot = self.host.slots[self.owner].as_ref().unwrap();
        if slot.kind != TypeId::of::<C>() {
            return Err(ComponentError::WrongType);
        }
        Ok(self.host.handle(self.owner))
    }

    /// Reconcile a child by parent identity + key + component type. Reordering
    /// preserves state; changing parent or type creates a new owner. Keys here
    /// are sibling-local, but the returned Element still needs globally unique
    /// element keys, as required by Runtime.
    ///
    /// Errors are latched and returned by host.present(). The placeholder never
    /// reaches layout on error. This permits render methods to return Element
    /// directly without hiding reconciliation failures or panicking.
    pub fn child<C: Component<Action = Action>>(
        &mut self,
        key: impl Into<String>,
        props: C::Props,
    ) -> Element<Action> {
        match self.try_child::<C>(key, props) {
            Ok(element) => element,
            Err(error) => {
                self.host.record_error(error);
                Element::spacer("__component_failed_render")
            }
        }
    }

    pub fn try_child<C: Component<Action = Action>>(
        &mut self,
        key: impl Into<String>,
        props: C::Props,
    ) -> Result<Element<Action>, ComponentError> {
        if !self.rendering {
            return Err(ComponentError::InvalidPhase);
        }
        if self.depth >= 128 {
            return Err(ComponentError::TreeTooDeep);
        }
        let key = key.into();
        if key.is_empty() {
            return Err(ComponentError::EmptyKey);
        }
        if !self.claimed.insert(key.clone()) {
            return Err(ComponentError::DuplicateChildKey(key));
        }
        let existing = self.host.slots[self.owner]
            .as_ref()
            .unwrap()
            .children
            .iter()
            .copied()
            .find(|index| {
                self.host.slots[*index]
                    .as_ref()
                    .is_some_and(|s| s.key == key && s.kind == TypeId::of::<C>())
            });
        let child = match existing {
            Some(index) => {
                self.host.set_props::<C>(index, props);
                index
            }
            None => self.host.mount::<C>(key, props, Some(self.owner)),
        };
        self.children.push(child);
        self.host.render_owner(child, self.depth + 1)
    }

    /// Look up a child already declared in this render (or the previous render
    /// during mount/update). No strong ownership escapes through this handle.
    pub fn child_handle<C: Component<Action = Action>>(
        &self,
        key: &str,
    ) -> Option<ComponentHandle<C>> {
        let children = if self.rendering {
            &self.children
        } else {
            &self.host.slots[self.owner].as_ref()?.children
        };
        children
            .iter()
            .copied()
            .find(|index| {
                self.host.slots[*index]
                    .as_ref()
                    .is_some_and(|s| s.key == key && s.kind == TypeId::of::<C>())
            })
            .map(|index| self.host.handle(index))
    }
}

/// A surface owns one host, its components, and its core Runtime. No global
/// registry, shared entity store, or reentrant lifecycle lock is involved.
///
/// Reconciliation defers removal until the complete rendered tree passes core
/// validation. On a rendering/validation failure, new owners are disposed,
/// existing owners are retained and marked dirty, and prior input is cleared.
/// Props/state changes and user side effects are NOT rolled back. Fix the props
/// or send another event, then present again. Panics in user hooks are not a
/// recoverable transaction; hooks and disposers must not panic.
pub struct ComponentHost<Action: Clone + 'static> {
    identity: u64,
    runtime: Runtime<Action>,
    slots: Vec<Option<Slot<Action>>>,
    free: Vec<usize>,
    generations: Vec<u64>,
    root: Option<usize>,
    proposed: HashMap<usize, Vec<usize>>,
    newly_mounted: Vec<usize>,
    error: Option<ComponentError>,
    stats: ComponentStats,
    tree_installed: bool,
}
impl<Action: Clone + 'static> Default for ComponentHost<Action> {
    fn default() -> Self {
        Self::new()
    }
}
impl<Action: Clone + 'static> ComponentHost<Action> {
    pub fn new() -> Self {
        // Do not recycle host IDs, even after Drop. Exhaustion must not alias a
        // live/stale handle (practically unreachable, but explicitly checked).
        let identity = NEXT_HOST
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("component host identity space exhausted");
        Self {
            identity,
            runtime: Runtime::new(),
            slots: Vec::new(),
            free: Vec::new(),
            generations: Vec::new(),
            root: None,
            proposed: HashMap::new(),
            newly_mounted: Vec::new(),
            error: None,
            stats: ComponentStats::default(),
            tree_installed: false,
        }
    }
    pub fn runtime(&self) -> &Runtime<Action> {
        &self.runtime
    }
    /// Mutating focus/dispatch does not implicitly invalidate components. If you
    /// replace/clear the core tree yourself, call invalidate() before presenting.
    pub fn runtime_mut(&mut self) -> &mut Runtime<Action> {
        &mut self.runtime
    }
    pub fn stats(&self) -> ComponentStats {
        self.stats
    }
    pub fn reset_stats(&mut self) {
        self.stats = ComponentStats {
            owners: self.stats.owners,
            ..ComponentStats::default()
        };
    }
    pub fn contains<C: Component<Action = Action>>(
        &self,
        handle: ComponentHandle<C>,
    ) -> bool {
        self.validate(handle).is_ok()
    }
    /// Equal props on the same root identity do no work. A different key/type
    /// explicitly replaces the root and immediately tears down its subtree.
    pub fn set_root<C: Component<Action = Action>>(
        &mut self,
        key: impl Into<String>,
        props: C::Props,
    ) -> Result<ComponentHandle<C>, ComponentError> {
        let key = key.into();
        if key.is_empty() {
            self.runtime.clear();
            self.tree_installed = false;
            return Err(ComponentError::EmptyKey);
        }
        if let Some(root) = self.root {
            let slot = self.slots[root].as_ref().unwrap();
            if slot.key == key && slot.kind == TypeId::of::<C>() {
                self.set_props::<C>(root, props);
                return Ok(self.handle(root));
            }
        }
        self.clear();
        let root = self.mount::<C>(key, props, None);
        self.root = Some(root);
        // Root mounts belong to the host immediately, not to a render attempt.
        self.newly_mounted.clear();
        Ok(self.handle(root))
    }
    pub fn send<C: Component<Action = Action>>(
        &mut self,
        handle: ComponentHandle<C>,
        event: C::Event,
    ) -> Result<Vec<Action>, ComponentError> {
        let index = self.validate(handle)?;
        let mut instance = self.slots[index].as_mut().unwrap().instance.take().unwrap();
        let typed = instance
            .as_any_mut()
            .downcast_mut::<Instance<C>>()
            .ok_or(ComponentError::WrongType)?;
        let mut cx = ComponentContext {
            host: self,
            owner: index,
            rendering: false,
            depth: 0,
            claimed: HashSet::new(),
            children: Vec::new(),
        };
        let actions = C::update(&mut typed.state, &typed.props, event, &mut cx);
        cx.host.slots[index].as_mut().unwrap().instance = Some(instance);
        cx.host.stats.events += 1;
        cx.host.mark_dirty(index);
        if let Some(error) = cx.host.error.take() {
            cx.host.runtime.clear();
            cx.host.tree_installed = false;
            return Err(error);
        }
        Ok(actions)
    }
    /// Force every owner to render, for explicit out-of-band inputs only.
    pub fn invalidate(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            slot.dirty = true;
        }
        self.tree_installed = false;
        self.runtime.invalidate_presentation();
    }
    /// Only invalidate geometry/text presentation, without rerendering owners.
    pub fn invalidate_presentation(&mut self) {
        self.runtime.invalidate_presentation();
    }

    pub fn present(
        &mut self,
        origin: [f32; 2],
        width: f32,
        clip: Rect,
        measurer: &mut dyn TextMeasurer,
    ) -> Result<&Frame<Action>, ComponentError> {
        let Some(root) = self.root else {
            self.runtime.clear();
            return Err(ComponentError::NoRoot);
        };
        if !self.tree_installed || self.slots[root].as_ref().unwrap().dirty {
            self.proposed.clear();
            self.newly_mounted.clear();
            let rendered = self.render_owner(root, 0);
            let result = match self.error.take() {
                Some(error) => Err(error),
                None => rendered.and_then(|tree| {
                    self.runtime.set_tree(tree).map_err(ComponentError::Layout)
                }),
            };
            if let Err(error) = result {
                self.abort_render();
                return Err(error);
            }
            self.commit_render(root);
            self.tree_installed = true;
        }
        self.runtime
            .present(origin, width, clip, measurer)
            .map_err(ComponentError::Layout)
    }
    /// Dispose descendants before parents, invalidate all handles, clear input.
    pub fn clear(&mut self) {
        if let Some(root) = self.root.take() {
            self.dispose(root);
        }
        // Also covers uncommitted owners after a failed user lifecycle call.
        for index in (0..self.slots.len()).rev() {
            if self.slots[index].is_some() {
                self.dispose(index);
            }
        }
        self.proposed.clear();
        self.newly_mounted.clear();
        self.error = None;
        self.tree_installed = false;
        self.runtime.clear();
    }

    fn handle<C: Component<Action = Action>>(&self, index: usize) -> ComponentHandle<C> {
        ComponentHandle {
            id: Identity {
                host: self.identity,
                slot: index,
                generation: self.slots[index].as_ref().unwrap().generation,
            },
            marker: PhantomData,
        }
    }
    fn validate<C: Component<Action = Action>>(
        &self,
        handle: ComponentHandle<C>,
    ) -> Result<usize, ComponentError> {
        if handle.id.host != self.identity {
            return Err(ComponentError::WrongHost);
        }
        let slot = self
            .slots
            .get(handle.id.slot)
            .and_then(Option::as_ref)
            .ok_or(ComponentError::StaleHandle)?;
        if slot.generation != handle.id.generation {
            return Err(ComponentError::StaleHandle);
        }
        if slot.kind != TypeId::of::<C>() {
            return Err(ComponentError::WrongType);
        }
        Ok(handle.id.slot)
    }
    fn record_error(&mut self, error: ComponentError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
    fn mount<C: Component<Action = Action>>(
        &mut self,
        key: String,
        props: C::Props,
        parent: Option<usize>,
    ) -> usize {
        let index = self.free.pop().unwrap_or_else(|| {
            self.slots.push(None);
            self.generations.push(0);
            self.slots.len() - 1
        });
        let generation = self.generations[index]
            .checked_add(1)
            .expect("component slot generation space exhausted");
        self.generations[index] = generation;
        self.slots[index] = Some(Slot {
            generation,
            key,
            kind: TypeId::of::<C>(),
            parent,
            instance: None,
            children: Vec::new(),
            cached: None,
            dirty: true,
            cleanups: Vec::new(),
        });
        self.stats.owners += 1;
        self.stats.mounts += 1;
        self.newly_mounted.push(index);
        let mut cx = ComponentContext {
            host: self,
            owner: index,
            rendering: false,
            depth: 0,
            claimed: HashSet::new(),
            children: Vec::new(),
        };
        let state = C::mount(&props, &mut cx);
        cx.host.slots[index].as_mut().unwrap().instance =
            Some(Box::new(Instance::<C> { props, state }));
        index
    }
    fn set_props<C: Component<Action = Action>>(
        &mut self,
        index: usize,
        props: C::Props,
    ) {
        let instance = self.slots[index]
            .as_mut()
            .unwrap()
            .instance
            .as_mut()
            .unwrap()
            .as_any_mut()
            .downcast_mut::<Instance<C>>()
            .unwrap();
        if instance.props != props {
            instance.props = props;
            self.mark_dirty(index);
        }
    }
    fn mark_dirty(&mut self, mut index: usize) {
        loop {
            let slot = self.slots[index].as_mut().unwrap();
            slot.dirty = true;
            match slot.parent {
                Some(parent) => index = parent,
                None => break,
            }
        }
    }
    fn render_owner(
        &mut self,
        index: usize,
        depth: usize,
    ) -> Result<Element<Action>, ComponentError> {
        if depth >= 128 {
            return Err(ComponentError::TreeTooDeep);
        }
        let slot = self.slots[index].as_ref().unwrap();
        if !slot.dirty {
            if let Some(tree) = &slot.cached {
                return Ok(tree.clone());
            }
        }
        let instance = self.slots[index].as_mut().unwrap().instance.take().unwrap();
        let mut cx = ComponentContext {
            host: self,
            owner: index,
            rendering: true,
            depth,
            claimed: HashSet::new(),
            children: Vec::new(),
        };
        let tree = instance.render(&mut cx);
        let children = std::mem::take(&mut cx.children);
        let host = cx.host;
        host.proposed.insert(index, children);
        let slot = host.slots[index].as_mut().unwrap();
        slot.instance = Some(instance);
        slot.cached = Some(tree.clone());
        slot.dirty = false;
        host.stats.renders += 1;
        Ok(tree)
    }
    fn abort_render(&mut self) {
        // New owners may not yet appear in committed child lists. Reverse mount
        // order guarantees descendants are torn down before their parents.
        let new = std::mem::take(&mut self.newly_mounted);
        for index in new.into_iter().rev() {
            self.dispose(index);
        }
        self.proposed.clear();
        for slot in self.slots.iter_mut().flatten() {
            slot.dirty = true;
        }
        self.runtime.clear();
        self.tree_installed = false;
    }
    fn commit_render(&mut self, root: usize) {
        for (index, children) in self.proposed.drain() {
            self.slots[index].as_mut().unwrap().children = children;
        }
        let mut reachable = HashSet::new();
        let mut pending = vec![root];
        while let Some(index) = pending.pop() {
            if reachable.insert(index) {
                pending
                    .extend(self.slots[index].as_ref().unwrap().children.iter().copied());
            }
        }
        // Dispose unreachable roots, not arbitrary slot order: a reused slot
        // number carries no information about lifecycle ordering.
        let removed: Vec<_> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let slot = slot.as_ref()?;
                if reachable.contains(&index) {
                    return None;
                }
                if slot.parent.is_some_and(|p| !reachable.contains(&p)) {
                    return None;
                }
                Some(index)
            })
            .collect();
        for index in removed {
            self.dispose(index);
        }
        self.newly_mounted.clear();
    }
    fn dispose(&mut self, index: usize) {
        let Some(mut slot) = self.slots[index].take() else {
            return;
        };
        for child in slot.children.drain(..) {
            self.dispose(child);
        }
        if let Some(mut instance) = slot.instance.take() {
            instance.unmount();
        }
        for cleanup in slot.cleanups.drain(..).rev() {
            cleanup();
            self.stats.cleanups += 1;
        }
        self.stats.disposals += 1;
        self.stats.owners -= 1;
        self.free.push(index);
    }
}
impl<Action: Clone + 'static> Drop for ComponentHost<Action> {
    fn drop(&mut self) {
        self.clear();
    }
}
