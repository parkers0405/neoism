//! The shaped-layout caches' eviction: what stale text stays shaped between commits.

use std::rc::Rc;

use crate::runtime::HashMap;

use super::{Ink, Layout, TextStyle, TextSystem, shape_key};

/// Shaped text by hash.
pub(super) type Cache<T> = HashMap<u64, Entry<T>>;

#[derive(Clone)]
pub(crate) struct CachedElision {
    pub(crate) layout: Rc<Layout<Ink>>,
    pub(crate) head: usize,
    pub(crate) tail: usize,
}

pub(super) struct Entry<T> {
    pub(super) value: T,
    /// The commit generation it was last used in.
    pub(super) used: u32,
    /// About how many heap bytes it holds.
    pub(super) cost: u32,
    /// The [`shape_key`](super::shape_key) it lays out, or 0 for a run.
    shape: u64,
}

impl<T> Entry<T> {
    pub(super) fn new(value: T, used: u32, text_len: usize, shape: u64) -> Self {
        let cost = (text_len * LAYOUT_PER_BYTE + LAYOUT_BASE).min(u32::MAX as usize) as u32;
        Entry { value, used, cost, shape }
    }
}

/// Heap bytes a layout holds per byte of its text, and on top of that. A freshly shaped
/// layout holds 35 to 90 bytes per byte (its buffers keep the room they grew into; a re-broken
/// copy is tight): 605 bytes of prose take 21 to 55 KB, a ten-byte label 1 to 3 KB.
const LAYOUT_PER_BYTE: usize = 64;
const LAYOUT_BASE: usize = 2048;

/// Heap bytes of layouts kept after the commits that used them, so a view shown again after a
/// while (a list returned to from a grid someone browsed) doesn't reshape: about 1,100 short
/// labels or 48 KB of paragraphs.
pub(super) const STALE_BYTES: usize = 3 << 20;

/// Drops what `cache` hasn't used in the last two commits once it outgrows [`STALE_BYTES`],
/// least recently used first. A view's layouts all share the commit it was last shown in, so
/// that commit's are kept while the budget lasts rather than all dropped together.
fn evict<T>(cache: &mut Cache<T>, g: u32) {
    let stale = |used: u32| g.wrapping_sub(used) >= 2;
    let total: usize = cache.values().filter(|e| stale(e.used)).map(|e| e.cost as usize).sum();
    if total <= STALE_BYTES {
        return;
    }
    let mut ages: Vec<(u32, usize)> = cache.values().filter(|e| stale(e.used)).map(|e| (g.wrapping_sub(e.used), e.cost as usize)).collect();
    ages.sort_unstable_by_key(|a| a.0);
    let (mut younger, mut kept, mut cutoff) = (0, 0, u32::MAX);
    for (i, &(age, cost)) in ages.iter().enumerate() {
        if i > 0 && ages[i - 1].0 != age { younger = kept; }
        kept += cost;
        if kept > STALE_BYTES {
            cutoff = age;
            break;
        }
    }
    let mut room = STALE_BYTES - younger;
    cache.retain(|_, e| {
        let (age, cost) = (g.wrapping_sub(e.used), e.cost as usize);
        age < cutoff || age == cutoff && cost <= room && { room -= cost; true }
    });
}

impl TextSystem {
    /// Marks the layouts of `shape` as replaced by an edit of its text, as typing or streaming
    /// makes. Unless this commit used them, they go when it ends, rather than each edit's copy
    /// taking the stale budget from views that may be shown again.
    pub(crate) fn supersede(&mut self, shape: u64) {
        self.superseded.insert(shape);
    }

    /// Drops replaced layouts this commit didn't use, then keeps layouts used in the last two
    /// commits, and older ones, most recently used first, while they fit in [`STALE_BYTES`].
    pub fn end_commit(&mut self) {
        let g = self.generation;
        if !self.superseded.is_empty() {
            let gone = &self.superseded;
            self.layouts.retain(|_, e| e.used == g || !gone.contains(&e.shape));
            self.elisions.retain(|_, e| e.used == g || !gone.contains(&e.shape));
            self.superseded.clear();
        }
        evict(&mut self.layouts, g);
        let layouts = &self.layouts;
        self.shapes.retain(|_, key| layouts.contains_key(key));
        evict(&mut self.elisions, g);
        evict(&mut self.runs, g);
        self.generation = g.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn get_elision(&mut self, key: u64) -> Option<CachedElision> {
        let g = self.generation;
        let e = self.elisions.get_mut(&key)?;
        e.used = g;
        Some(e.value.clone())
    }

    #[inline]
    pub(crate) fn insert_elision(&mut self, key: u64, shape: u64, drawn_len: usize, value: CachedElision) {
        let g = self.generation;
        self.elisions.insert(key, Entry::new(value, g, drawn_len, shape));
    }

    /// A live run owns its latest layout; don't retain every timer tick in the commit cache.
    pub(crate) fn layout_live(&mut self, text: &str, style: TextStyle, scale: f32) -> Rc<Layout<Ink>> {
        let key = shape_key(text, &[], style, scale, false, &[]);
        let cached = self.layouts.contains_key(&key);
        let prev_shape = self.shapes.get(&key).copied();
        let layout = self.layout_rich(text, &[], style, scale, None);
        if !cached {
            self.layouts.remove(&key);
            match prev_shape {
                Some(prev) => { self.shapes.insert(key, prev); }
                None => { self.shapes.remove(&key); }
            }
        }
        layout
    }

    #[cfg(test)]
    pub(crate) fn cached_layout_count(&self) -> usize { self.layouts.len() }

    #[cfg(test)]
    pub(crate) fn cached_elision_count(&self) -> usize { self.elisions.len() }
}
