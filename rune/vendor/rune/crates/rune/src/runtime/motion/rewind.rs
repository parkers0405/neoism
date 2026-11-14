//! Feeds an attached [`Rewind`](crate::rewind::Rewind) each frame: every keyed or trace-named
//! element on screen, its drawn rect and opacity (ancestors included).

use super::*;

impl<S: 'static> Runtime<S> {
    pub(crate) fn record_rewind(&mut self) {
        let Some(rewind) = &self.rewind else { return };
        let mut tape = rewind.lock();
        if tape.paused { return; }
        tape.begin(self.clock, self.viewport);
        if self.drawn.len() == self.nodes.len() && self.inh_scratch.len() == self.nodes.len() {
            for (i, n) in self.nodes.iter().enumerate() {
                if n.key.is_none() && n.trace_name.is_none() { continue; }
                if !self.seen.get(i).copied().unwrap_or(true) { continue; }
                let q = crate::rewind::quantize(self.drawn[i], self.inh_scratch[i].opacity);
                tape.sample(n.id.0, q, || n.trace_name.map_or_else(|| n.key.clone().unwrap_or_else(|| n.path.clone()), str::to_owned));
            }
        }
        tape.end();
    }

    /// The recorder attached with `Tx::record_rewind`, if any.
    pub fn rewind(&self) -> Option<&crate::rewind::Rewind> { self.rewind.as_ref() }
}
