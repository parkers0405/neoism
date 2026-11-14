//! Contents that slide with their growth row. When the top of a parent's contents moves in a
//! growth row (a padding or gap above them changed) and the row's height changes the other way
//! to fit, they slide as its growth opens: the slide left shrinks with the growth left. A list
//! pinned to its end moves the row by what it grows, so contents whose place doesn't change hold
//! still on every frame, instead of netting a spring against the growth. A growth retargeted so
//! it no longer opens toward them lets the rest spring out on the row's growth spring.
use super::*;

/// The contents of a growth stack's rows sliding with them, drawn outside the scene.
#[derive(Clone, Debug, Default)]
pub(super) struct ContentSlides(Vec<Slide>);

#[derive(Clone, Copy, Debug)]
struct Slide {
    node: NodeId,
    row: NodeId,
    /// How far down from its layout it shows, and how fast that changes (points, per second).
    dy: f32,
    v: f32,
    /// The row's presented height when `dy` was last measured, while it follows the growth.
    at: Option<f32>,
}

impl ContentSlides {
    /// Each sliding node and how far down from its layout it shows.
    pub(super) fn shifts(&self) -> impl Iterator<Item = (NodeId, f32)> + '_ {
        self.0.iter().map(|s| (s.node, s.dy))
    }

    /// Slides `node`, drawn `dy` points down from its layout plus any slide it still had, with
    /// row `row`'s growth `g`, if that growth opens toward its layout. Otherwise it stops
    /// following and the whole amount is returned, for the caller to slide some other way.
    fn follow(&mut self, node: NodeId, row: NodeId, g: &GrowTrack, dy: f32) -> f32 {
        let dy = dy + self.0.iter().position(|s| s.node == node).map_or(0.0, |k| self.0.swap_remove(k).dy);
        let left = g.target - g.h;
        if left * dy < 0.0 && left.abs() > 1.0 / 64.0 {
            self.0.push(Slide { node, row, dy, v: 0.0, at: Some(g.h) });
            return 0.0;
        }
        dy
    }

    /// Lands row `row`'s slides at once (its growth landed at once).
    pub(super) fn land(&mut self, row: NodeId) {
        self.0.retain(|s| s.row != row);
    }

    /// Steps row `row`'s slides after its growth `g` stepped `steps` substeps. Whether any still
    /// moves.
    pub(super) fn step(&mut self, row: NodeId, g: &GrowTrack, spring: Spring, steps: usize) -> bool {
        let dt = steps as f32 * SUBSTEP;
        for s in self.0.iter_mut().filter(|s| s.row == row) {
            if let Some(at) = s.at {
                let (was, left) = (g.target - at, g.target - g.h);
                if was * s.dy < 0.0 && was.abs() > 1.0 / 64.0 {
                    let dy = s.dy * (left / was).clamp(0.0, 1.0);
                    if dt > 0.0 { s.v = (dy - s.dy) / dt; }
                    (s.dy, s.at) = (dy, Some(g.h));
                    continue;
                }
                s.at = None;
            }
            for _ in 0..steps {
                (s.dy, s.v) = spring.sample(s.dy, s.v, 0.0, SUBSTEP);
            }
        }
        self.0.retain(|s| s.dy.abs() > 1.0 / 128.0 || (s.at.is_none() && s.v.abs() > 0.5));
        self.0.iter().any(|s| s.row == row)
    }

    /// Drops the slides of rows that left the stack.
    pub(super) fn keep(&mut self, rows: &[NodeId]) {
        self.0.retain(|s| rows.contains(&s.row));
    }
}

impl<S: 'static> Runtime<S> {
    /// Before node `id`, a child of `parent` whose contents' top moved `dy` points down in its
    /// growth row (drawn less new), takes that slide: with the row's growth where it opens
    /// toward them. How far it still has to slide from where it was drawn.
    pub(super) fn slide_with_row(&mut self, nodes: &[Node], parent: usize, id: NodeId, dy: f32) -> f32 {
        let Some(row) = std::iter::successors(Some(parent), |&p| nodes[p].parent)
            .find(|&r| nodes[r].parent.is_some_and(|c| nodes[c].growth.is_some()))
        else { return dy };
        let Some(c) = nodes[row].parent else { return dy };
        let (Some(stack), Some(g)) = (self.stacks.get_mut(&nodes[c].id), self.grows.get(&nodes[row].id)) else { return dy };
        stack.slides.follow(id, nodes[row].id, g, dy)
    }
}
