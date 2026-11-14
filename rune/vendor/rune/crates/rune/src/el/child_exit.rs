//! `El::child_exit`: children removed in a commit leave the way that commit says.
use rune_motion::Transition;

use crate::el::El;

impl El {
    /// Children removed from this element leave with `t` instead of the transition from their
    /// last render. A node normally leaves the way it was last drawn, before anyone knew why it
    /// would go; a rolling counter's old digit has to leave the way the new one enters, which
    /// only the commit that removes it knows. Set it on each build with that build's direction.
    /// Children still present, and those already leaving, keep their own transitions.
    pub fn child_exit(mut self, t: Transition) -> Self {
        self.vis.cold_mut().child_exit = Some(Box::new(t));
        self
    }
}
