//! Fold rows that open or shut together move as one block. Consecutive `El::fold_glide` rows a
//! commit brings into a growth stack (or takes out of it) share one spring, which fills them top
//! to bottom (or empties them bottom to top) under one feathered edge, as a code diff opens a
//! hunk. On their own spring each, every row of the block would open at once, each half open
//! under an edge of its own, and the rows below would cross the block's lower lines.
use super::*;
use crate::growth::{FoldGlide, GrowTrack};

#[derive(Clone, Debug)]
pub(super) struct GrowthRun {
    rows: Vec<NodeId>,
    /// Each row's full extent: its target when opening, its height when shutting.
    extents: Vec<f32>,
    /// What each row aimed at when the block formed; a later change to any breaks the block.
    targets: Vec<f32>,
    /// The block's height on the rows' fold spring.
    track: GrowTrack,
}

impl GrowthRun {
    /// One block of `ids` (consecutive rows of one stack), from their tracks as they stand.
    fn link(ids: &[NodeId], grows: &HashMap<NodeId, GrowTrack>) -> Option<GrowthRun> {
        let tracks: Vec<&GrowTrack> = ids.iter().map(|id| grows.get(id)).collect::<Option<_>>()?;
        let spring = tracks[0].glide?;
        let mut track = GrowTrack::settled(tracks.iter().map(|g| g.h).sum());
        track.set_kind(false, true, Some((FoldGlide { spring, feather: tracks[0].feather }, f32::INFINITY)));
        track.retarget(tracks.iter().map(|g| g.target).sum());
        Some(GrowthRun {
            rows: ids.to_vec(),
            extents: tracks.iter().map(|g| g.target.max(g.h)).collect(),
            targets: tracks.iter().map(|g| g.target).collect(),
            track,
        })
    }

    /// Every row still aims where it did and they still stand together in `order`.
    fn intact(&self, order: &[NodeId], grows: &HashMap<NodeId, GrowTrack>) -> bool {
        let aims = self.rows.iter().zip(&self.targets).all(|(id, &t)| grows.get(id).is_none_or(|g| (g.target - t).abs() < 1e-4));
        let present: Vec<usize> = self.rows.iter().filter_map(|id| order.iter().position(|o| o == id)).collect();
        aims && present.windows(2).all(|w| w[1] == w[0] + 1)
    }

    /// Breaks the block: each row carries on alone from where it is, the row at the edge with
    /// the block's speed.
    fn unlink(&self, grows: &mut HashMap<NodeId, GrowTrack>) {
        for id in &self.rows {
            if let Some(g) = grows.get_mut(id) && g.h > 1e-4 && g.h < g.target.max(g.h) - 1e-4 {
                g.v = self.track.v;
            }
        }
    }

    /// Steps the block and gives each row its share: rows above the edge whole, the row at the
    /// edge its part and the feather, rows past it nothing. Returns how far the rows grew.
    fn step(&mut self, steps: usize, spring: rune_motion::Spring, edge: &crate::growth::StreamEdge, grows: &mut HashMap<NodeId, GrowTrack>) -> f32 {
        for _ in 0..steps {
            self.track.step(spring, edge, false);
        }
        let (mut left, mut grown) = (self.track.h, 0.0);
        for (id, &extent) in self.rows.iter().zip(&self.extents) {
            let h = left.clamp(0.0, extent);
            left -= extent;
            let Some(g) = grows.get_mut(id) else { continue };
            let at_edge = h > 1e-4 && h < extent - 1e-4;
            grown += (h - g.h).max(0.0);
            g.h = h;
            g.v = if at_edge { self.track.v } else { 0.0 };
            g.mask = if at_edge { self.track.mask } else { 0.0 };
        }
        grown
    }
}

impl<S: 'static> Runtime<S> {
    /// At a commit, after the stack's rows have their new targets: breaks blocks this commit
    /// changed and forms new ones from runs of rows in `order` that were `opened` (new fold rows
    /// growing from nothing) or `shut` (fold rows that left) by it.
    pub(super) fn link_growth_runs(&mut self, stack: &mut Stack, order: &[NodeId], opened: &[NodeId], shut: &[NodeId]) {
        let grows = &mut self.grows;
        stack.runs.retain(|run| {
            let keep = run.intact(order, grows);
            if !keep { run.unlink(grows); }
            keep
        });
        for id in shut {
            if let Some(g) = grows.get_mut(id) { g.retarget(0.0); }
        }
        let fold = |g: &GrowTrack| (g.glide, g.feather);
        for set in [opened, shut] {
            let mut block: Vec<NodeId> = Vec::new();
            for id in order.iter().map(Some).chain([None]) {
                let eligible = id.filter(|id| set.contains(id))
                    .and_then(|id| grows.get(id))
                    .filter(|g| g.glide.is_some() && g.stage.is_none() && g.lift() == 0.0);
                if let (Some(g), Some(id)) = (eligible, id)
                    && block.last().and_then(|last| grows.get(last)).is_none_or(|last| fold(last) == fold(g))
                {
                    block.push(*id);
                    continue;
                }
                if block.len() > 1 && let Some(run) = GrowthRun::link(&block, grows) {
                    stack.runs.push(run);
                }
                block.clear();
                if let (Some(_), Some(id)) = (eligible, id) { block.push(*id); }
            }
        }
    }

    /// Steps a stack's blocks before its rows, and returns the rows they moved (which skip their
    /// own step this frame) and how far they grew.
    pub(super) fn step_growth_runs(stack: &mut Stack, grows: &mut HashMap<NodeId, GrowTrack>, steps: usize, spec: crate::el::GrowthSpec) -> (Vec<NodeId>, f32) {
        let (mut rows, mut grown) = (Vec::new(), 0.0);
        stack.runs.retain_mut(|run| {
            grown += run.step(steps, spec.spring, &spec.edge, grows);
            rows.extend(run.rows.iter().copied());
            run.track.moving()
        });
        (rows, grown)
    }
}
