//! Keys to nodes, so layout reads and highlight lookups find the element or text highlight a
//! key names without scanning the tree. Scroll frames check every layout read against the same
//! nodes, so the index of the current nodes is kept until a commit or a prune replaces them.

use std::hash::BuildHasher;

use super::*;

/// Lookups up to this many scan the nodes rather than index them, unless the same nodes come
/// round again: indexing every key of a large tree costs more than a handful of scans.
pub(super) const SCAN_READS: usize = 8;

const NONE: u32 = u32::MAX;

/// Nodes by key and by highlight key, each by the key's hash. A lookup still checks each
/// candidate with [`crate::flight::selects`], so two keys sharing a hash only share a chain.
#[derive(Default)]
pub(super) struct KeyIndex {
    /// The nodes the index describes: commit, count and buffer.
    built: Option<NodesId>,
    /// The nodes the last lookups were for; seeing them again builds the index.
    seen: Option<NodesId>,
    /// Each key's first node, and the next node with a key of the same hash.
    keys: HashMap<u64, u32>,
    next_key: Vec<u32>,
    /// Each highlight key's first entry, and entries of `(node, next entry)`.
    marks: HashMap<u64, u32>,
    mark_chain: Vec<(u32, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct NodesId {
    commit: u64,
    len: usize,
    at: *const Node,
}

impl NodesId {
    fn of(nodes: &[Node], commit: u64) -> Self {
        NodesId { commit, len: nodes.len(), at: nodes.as_ptr() }
    }
}

fn hash(key: &str) -> u64 {
    rustc_hash::FxBuildHasher.hash_one(key)
}

/// `selector` and each tail of it after a `/`: the keys a node or highlight it picks can have.
fn tails(selector: &str) -> impl Iterator<Item = &str> {
    std::iter::once(selector).chain(selector.match_indices('/').map(|(at, _)| &selector[at + 1..]))
}

impl KeyIndex {
    /// An index of `nodes`, for lookups that won't come round again.
    pub(super) fn of(nodes: &[Node]) -> Self {
        let mut index = KeyIndex::default();
        index.build(nodes);
        index
    }

    /// Before `lookups` lookups in `nodes` (the runtime's current nodes as of `commit`):
    /// builds the index when there are enough of them, or when the last lookups were for the
    /// same nodes.
    pub(super) fn prepare(&mut self, nodes: &[Node], commit: u64, lookups: usize) {
        let id = NodesId::of(nodes, commit);
        if self.built != Some(id) && (lookups > SCAN_READS || self.seen == Some(id)) {
            self.build(nodes);
            self.built = Some(id);
        }
        self.seen = Some(id);
    }

    /// The index, when it describes `nodes` as of `commit`.
    pub(super) fn get(&self, nodes: &[Node], commit: u64) -> Option<&Self> {
        (self.built == Some(NodesId::of(nodes, commit))).then_some(self)
    }

    /// The nodes changed under the same commit.
    pub(super) fn forget(&mut self) {
        self.built = None;
        self.seen = None;
    }

    fn build(&mut self, nodes: &[Node]) {
        self.built = None;
        self.keys.clear();
        self.next_key.clear();
        self.next_key.resize(nodes.len(), NONE);
        self.marks.clear();
        self.mark_chain.clear();
        for (i, n) in nodes.iter().enumerate().rev() {
            if let Some(key) = &n.key {
                self.next_key[i] = self.keys.insert(hash(key), i as u32).unwrap_or(NONE);
            }
            let Some(text) = n.text.as_ref() else { continue };
            for key in text.highlights.iter().filter_map(|h| h.key.as_deref()) {
                let first = self.marks.entry(hash(key)).or_insert(NONE);
                if *first != NONE && self.mark_chain[*first as usize].0 == i as u32 { continue; }
                self.mark_chain.push((i as u32, *first));
                *first = (self.mark_chain.len() - 1) as u32;
            }
        }
    }

    /// The first node `selector` picks (see [`crate::flight::selects`]).
    pub(super) fn first(&self, nodes: &[Node], selector: &str) -> Option<usize> {
        tails(selector).filter_map(|key| {
            let mut i = *self.keys.get(&hash(key))?;
            while i != NONE {
                let n = &nodes[i as usize];
                if crate::flight::selects(selector, &n.path, n.key.as_deref()) { return Some(i as usize); }
                i = self.next_key[i as usize];
            }
            None
        }).min()
    }

    /// The nodes holding a highlight `selector` may pick, in order. Each still needs checking.
    pub(super) fn marked(&self, selector: &str) -> Vec<usize> {
        let mut out = Vec::new();
        for key in tails(selector) {
            let Some(&first) = self.marks.get(&hash(key)) else { continue };
            let mut e = first;
            while e != NONE {
                let (node, next) = self.mark_chain[e as usize];
                out.push(node as usize);
                e = next;
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The first node `selector` picks, by scanning `nodes`.
pub(super) fn scan_first(nodes: &[Node], selector: &str) -> Option<usize> {
    nodes.iter().position(|n| n.key.as_deref().is_some_and(|key| tails(selector).any(|t| t == key))
        && crate::flight::selects(selector, &n.path, n.key.as_deref()))
}

impl<S: 'static> Runtime<S> {
    /// The kept index, when `nodes` are the current nodes it describes.
    pub(super) fn key_index_for(&self, nodes: &[Node]) -> Option<&KeyIndex> {
        self.key_index.get(nodes, self.commit_gen)
    }

    /// `self.nodes` changed without a commit: kept per-node answers no longer hold.
    pub(super) fn nodes_replaced(&mut self) {
        self.key_index.forget();
        self.exit_mask.forget();
    }
}
