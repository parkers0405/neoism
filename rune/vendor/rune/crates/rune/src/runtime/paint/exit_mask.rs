//! Which nodes sit in an exiting subtree, found once a frame. Hit registration, reports and
//! sticky headers ask this of most nodes; without it each asks the scene for every ancestor.

use super::*;

/// One flag per node of the commit it was filled for: the node or an ancestor is exiting.
#[derive(Default)]
pub(super) struct ExitMask {
    exiting: Vec<bool>,
    /// The commit the flags describe; they answer only while it and the node count hold.
    commit: Option<u64>,
}

impl ExitMask {
    pub(super) fn forget(&mut self) {
        self.commit = None;
    }

    fn get(&self, i: usize, len: usize, commit: u64) -> Option<bool> {
        (self.commit == Some(commit) && self.exiting.len() == len).then(|| self.exiting[i])
    }
}

impl<S: 'static> Runtime<S> {
    /// Fills the mask for this frame's paint and reports. Call [`Self::end_exit_mask`] once
    /// they're done, so nothing reads it after a phase changes.
    pub(super) fn begin_exit_mask(&mut self) {
        let mut mask = std::mem::take(&mut self.exit_mask);
        mask.commit = None;
        mask.exiting.clear();
        mask.exiting.reserve(self.nodes.len());
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            let up = n.parent.is_some_and(|p| if p < i { mask.exiting[p] } else { self.subtree_exiting(p) });
            mask.exiting.push(up || self.is_exiting(n.id));
        }
        mask.commit = Some(self.commit_gen);
        self.exit_mask = mask;
    }

    pub(super) fn end_exit_mask(&mut self) {
        self.exit_mask.forget();
    }

    pub(super) fn subtree_exiting(&self, mut i: usize) -> bool {
        if let Some(exiting) = self.exit_mask.get(i, self.nodes.len(), self.commit_gen) {
            return exiting;
        }
        loop {
            if self.is_exiting(self.nodes[i].id) { return true; }
            let Some(parent) = self.nodes[i].parent else { return false };
            i = parent;
        }
    }
}
