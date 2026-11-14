//! `El::hold_landing_width`: a column whose width springs (a side panel opening beside it) takes
//! its landing layout at once, so its text wraps once, and slides as one from where its first
//! child in view was drawn. The commit lands the held subtree's layout and gives the holding
//! element an offset of that child's jump, which springs back to its own offset.
use super::*;

/// The nodes the last commit held at their landing width, kept for the frame that draws them:
/// motion checks take that frame's re-layout as designed.
#[derive(Debug, Default)]
pub(super) struct LandingHolds(HashSet<NodeId>);

impl LandingHolds {
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    pub(super) fn holds(&self, id: NodeId) -> bool {
        !self.0.is_empty() && self.0.contains(&id)
    }
}

impl<S: 'static> Runtime<S> {
    /// Marks in `held`, per node of `nodes`, those this commit lands under an
    /// `El::hold_landing_width` element whose width changes, and returns each such element's
    /// index with how far it shifts so its anchor child stays where it was drawn. `held` stays
    /// empty when nothing holds.
    pub(super) fn landing_holds(&mut self, nodes: &[Node], animated: bool, held: &mut Vec<bool>) -> Vec<(usize, f32)> {
        held.clear();
        let mut out = Vec::new();
        if !animated || self.reduced_motion {
            return out;
        }
        let mut last = paint_tracks::LastBuild::default();
        for (i, n) in nodes.iter().enumerate() {
            if !n.vis.cold.get().is_some_and(|c| c.hold_landing) || !self.scene.contains(n.id) {
                continue;
            }
            let Some(drawn) = self.scene.get(n.id, prop::FRAME) else { continue };
            let was = self.scene.store().target(n.id, prop::FRAME).unwrap_or(drawn);
            if (was.w - n.local.w).abs() < 0.5 {
                continue;
            }
            let end = subtree_end(nodes, i);
            let present: Vec<usize> = (i + 1..end)
                .filter(|&c| nodes[c].parent == Some(i) && self.scene.contains(nodes[c].id) && !self.is_exiting(nodes[c].id))
                .collect();
            let anchor = present.iter().copied().find(|&c| last.find(&self.nodes, nodes[c].id).is_some_and(|o| self.drew(o))).or(present.first().copied());
            let jump = anchor.and_then(|c| self.scene.get(nodes[c].id, prop::FRAME).map(|r| r.x - nodes[c].local.x)).unwrap_or(0.0);
            held.resize(nodes.len(), false);
            for (j, h) in held.iter_mut().enumerate().take(end).skip(i) {
                *h = true;
                self.landing_holds.0.insert(nodes[j].id);
            }
            out.push((i, drawn.x - n.local.x + jump));
        }
        out
    }

    /// Whether old node `o`, or anything under it, drew in the last build.
    fn drew(&self, o: usize) -> bool {
        self.seen.len() == self.nodes.len() && (o..subtree_end(&self.nodes, o)).any(|j| self.seen[j])
    }
}

/// One past the last node of `i`'s subtree.
fn subtree_end(nodes: &[Node], i: usize) -> usize {
    (i + 1..nodes.len()).find(|&j| nodes[j].parent < Some(i)).unwrap_or(nodes.len())
}
