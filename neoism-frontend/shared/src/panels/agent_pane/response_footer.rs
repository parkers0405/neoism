//! Completed-answer provenance has its own paint identity, never the prose key.
use super::{text_reveal::TextRevealState, tool_motion::ToolMotionState};

pub const RESERVED_HEIGHT: f32 = 28.0;

/// Footer-only provenance: one-shot slot credit and bounded composer-edge
/// eligibility. Neither transcript refinements nor paint can create a birth.
#[derive(Default)]
pub struct Growth {
    births: std::collections::HashSet<String>,
    // Eligibility is ingested once. Paint only consumes the existing arrival
    // sample; it never creates a fresh clock or re-docks an old footer.
    edges: Vec<EdgeBirth>,
}

struct EdgeBirth {
    id: String,
    viewport: Option<[f32; 4]>,
}

impl Growth {
    pub fn record(&mut self, ids: impl IntoIterator<Item = String>) {
        self.record_with_edge(ids, false, None);
    }

    pub fn record_with_edge(
        &mut self,
        ids: impl IntoIterator<Item = String>,
        following: bool,
        viewport: Option<[f32; 4]>,
    ) {
        for id in ids.into_iter().filter(|id| !id.is_empty()) {
            self.births.insert(id.clone());
            if following && !self.edges.iter().any(|entry| entry.id == id) {
                // Same bound as the arrival ledger; unpainted/offscreen markers
                // do not own redraw, and expired samples cannot revive them.
                if self.edges.len() == 256 {
                    self.edges.remove(0);
                }
                self.edges.push(EdgeBirth { id, viewport });
            }
        }
    }

    pub fn paint_y(
        &mut self,
        id: &str,
        natural_y: f32,
        viewport: Option<[f32; 4]>,
        following: bool,
        sample: super::tool_motion::ToolMotionSample,
    ) -> f32 {
        let Some(index) = self.edges.iter().position(|entry| entry.id == id) else {
            return natural_y;
        };
        let active = sample
            .deadline
            .is_some_and(|end| end > web_time::Instant::now());
        let valid_view = viewport.is_some_and(|rect| {
            rect[3] > 0.0
                && natural_y >= rect[1]
                && self.edges[index].viewport.is_none_or(|old| old == rect)
        });
        if !following || !active || !valid_view || sample.opacity >= 1.0 {
            self.edges.remove(index);
            return natural_y;
        }
        let viewport = viewport.unwrap();
        self.edges[index].viewport = Some(viewport);
        let edge = viewport[1] + viewport[3];
        // If follow lag already leaves the line below the composer, let that
        // spring expose it naturally. Otherwise bridge only the missing gap.
        // Position shares the original birth curve: a late first paint must
        // already be near landing, not restart at the bar just before expiry.
        let progress = sample.opacity.clamp(0.0, 1.0);
        natural_y + (edge - natural_y).max(0.0) * (1.0 - progress)
    }

    pub fn clear(&mut self) {
        self.births.clear();
        self.edges.clear();
    }

    pub fn take(&mut self, scale: f32, measured_growth: Option<f32>) -> f32 {
        let reserve = self.births.len() as f32 * RESERVED_HEIGHT * scale;
        self.births.clear(); // Slot credit is one-shot; paint eligibility lasts until arrival expires.
        reserve.min(measured_growth.unwrap_or(reserve).max(0.0))
    }
}

/// Styled runs share one source line; the muted run owns the separator.
/// Ledger offsets count non-whitespace UTF-8 bytes, not chars or source bytes.
pub(crate) fn segments(footer: &str) -> (&str, Option<(&str, usize)>) {
    let agent = footer.split_once(" · ").map_or(footer, |(agent, _)| agent);
    let rest = (agent.len() < footer.len()).then(|| {
        let offset = agent
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(char::len_utf8)
            .sum();
        (&footer[agent.len()..], offset)
    });
    (agent, rest)
}

pub fn key(body_id: &str) -> String {
    format!("response-footer:{body_id}")
}

/// Only previously ingested answer rows can acquire live metadata via refresh.
/// Newly loaded history (including pagination) is always a sharp baseline.
pub fn record_refresh<'a>(
    reveal: &mut TextRevealState,
    motion: &mut ToolMotionState,
    before: impl Iterator<Item = (&'a str, &'a str)>,
    after: impl Iterator<Item = (&'a str, &'a str)>,
) {
    let before: std::collections::HashMap<_, _> = before.collect();
    for (id, footer) in after {
        if let Some(old) = before.get(id) {
            record(reveal, motion, id, old, footer);
        }
    }
}

pub fn record(
    reveal: &mut TextRevealState,
    motion: &mut ToolMotionState,
    id: &str,
    before: &str,
    after: &str,
) -> bool {
    if id.is_empty() {
        return false;
    }
    let before = before.trim();
    let after = after.trim();
    let key = key(id);
    reveal.record(&key, before, after);
    // Metadata revisions reveal changed ink, but never move/fade the old ink.
    if before.is_empty() && !after.is_empty() {
        motion.record(&key, None, "completed");
        return true;
    }
    false
}

#[cfg(test)]
mod tests;
