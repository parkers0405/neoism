//! Reusable per-commit scratch buffers for `apply` and `prune_exited`, plus fast clearing of the
//! previous commit's `Node` buffer without walking `moved` memo spans.
use super::*;

#[derive(Default)]
pub(crate) struct ApplyScratch {
    pub(super) culled: Vec<bool>,
    pub(super) concealed: Vec<bool>,
    pub(super) rows: Vec<usize>,
    pub(super) new_ids: Vec<NodeId>,
    pub(super) new_set: HashSet<NodeId>,
    pub(super) order: Vec<NodeId>,
    pub(super) time_scales: Vec<f32>,
    pub(super) arrivals: Vec<(Rc<str>, usize, bool)>,
    pub(super) mounted: Vec<bool>,
    pub(super) slid: Vec<bool>,
    pub(super) top_moved: Vec<f32>,
    /// Per node: held at its landing width this commit (`El::hold_landing_width`).
    pub(super) held: Vec<bool>,
    pub(super) old_ids: Vec<NodeId>,
    pub(super) moved_spans: Vec<(usize, usize)>,
    pub(super) remap: Vec<Option<usize>>,
}

/// Drops the un-moved `Node`s in `old_nodes` and returns the empty `Vec<Node>` allocation for the
/// next commit's `flatten`, skipping `moved` memo spans whose placeholders own nothing.
pub(super) fn clear_old_nodes(
    mut old_nodes: Vec<Node>,
    reused: &[memo_reuse::ReusedSpan],
    spans_scratch: &mut Vec<(usize, usize)>,
) -> Vec<Node> {
    let len = old_nodes.len();
    if len == 0 {
        return old_nodes;
    }
    spans_scratch.clear();
    spans_scratch.extend(reused.iter().filter(|s| s.moved && s.len > 0).map(|s| (s.old_start, s.len)));
    if spans_scratch.is_empty() {
        old_nodes.clear();
        return old_nodes;
    }
    if !spans_scratch.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0) {
        spans_scratch.sort_unstable_by_key(|s| s.0);
    }
    let ptr = old_nodes.as_mut_ptr();
    // SAFETY: `0..len` was initialized; every slot inside a `moved` span holds a `Node::moved_stub`
    // with zero heap allocations or reference counts (asserted in debug builds), and every slot
    // outside those spans is dropped once via `drop_in_place` after `set_len(0)` prevents unwinds
    // from double-dropping.
    unsafe {
        old_nodes.set_len(0);
        let mut cur = 0usize;
        for &(start, slen) in spans_scratch.iter() {
            let s = start.min(len);
            while cur < s {
                std::ptr::drop_in_place(ptr.add(cur));
                cur += 1;
            }
            let end = (start + slen).min(len);
            #[cfg(debug_assertions)]
            for k in s..end {
                let stub = &*ptr.add(k);
                debug_assert!(
                    stub.is_moved_stub()
                        && stub.path.capacity() == 0
                        && stub.key.is_none()
                        && stub.text.is_none()
                        && stub.text_layout.is_none()
                        && stub.input.is_none()
                        && stub.focus.is_empty()
                        && stub.handlers.is_empty()
                        && stub.cold.is_empty()
                        && stub.vis.cold.is_empty()
                );
            }
            if end > cur {
                cur = end;
            }
        }
        while cur < len {
            std::ptr::drop_in_place(ptr.add(cur));
            cur += 1;
        }
    }
    old_nodes
}
