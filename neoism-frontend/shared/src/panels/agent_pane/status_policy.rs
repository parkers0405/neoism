//! Shared status/update policies for the agent pane.

/// Only a locally submitted idle primary is already represented by optimistic
/// activity. Never infer a reservation from history or a hydrated queue.
#[derive(Clone, Debug, Default)]
pub struct PromptAdmission {
    primary: Option<(String, bool)>,
}

impl PromptAdmission {
    pub fn admit(&mut self, message_id: &str) {
        self.primary = Some((message_id.to_owned(), true));
    }

    pub fn project(&mut self, count: usize, running: bool, preview: Option<&str>) -> usize {
        if running {
            if let Some((_, waiting)) = self.primary.as_mut() {
                *waiting = false;
            }
        }
        let reserved = self.primary.as_ref().is_some_and(|(_, waiting)| *waiting);
        // A compact-only queue must remain visible even if its snapshot
        // overtakes the primary's dequeue (or is a count-less notification).
        count.saturating_sub(usize::from(reserved))
            .max(usize::from(count > 0 && preview == Some("Compaction queued")))
    }

    /// The wire preview names the head, which may be the hidden primary.
    /// Without second-item identity we must not label real waiting work with
    /// that head. Compact previews are explicit control metadata, not a prompt.
    pub fn project_preview(&self, preview: Option<String>) -> Option<String> {
        if self.primary.as_ref().is_some_and(|(_, waiting)| *waiting)
            && preview.as_deref() != Some("Compaction queued")
        {
            None
        } else {
            preview
        }
    }

    /// Match before applying the post-dequeue snapshot. A late primary dequeue
    /// must not speculatively consume a real follow-up, even after running=true.
    pub fn dequeue(&mut self, message_id: Option<&str>) -> bool {
        if self.primary.as_ref().is_some_and(|(id, _)| Some(id.as_str()) == message_id) {
            self.primary = None;
            true
        } else {
            false
        }
    }

    pub fn settle(&mut self) {
        self.primary = None;
    }
}

/// Stop cancels compact controls, never ordinary queued prompts.
pub fn ordinary_queue_count(count: usize, preview: Option<&str>) -> usize {
    count.saturating_sub(usize::from(preview == Some("Compaction queued")))
}

/// Identity-only activity fence. Saved transcript snapshots remain admissible;
/// hosts ignore late cancelled live parts before applying them. A cancelled
/// assistant can never drive a successor's lifecycle. Retained across
/// dequeue/start, idle, hydration, and cache swaps.
#[derive(Clone, Debug, Default)]
pub struct StoppedParts {
    seen: std::collections::BTreeSet<String>,
    stopped: std::collections::BTreeSet<String>,
}

impl StoppedParts {
    pub fn allows(&self, part: &str, parent: Option<&str>) -> bool {
        !self.stopped.contains(part)
            && !parent.is_some_and(|id| self.stopped.contains(id))
    }

    pub fn observe(&mut self, part: &str, parent: Option<&str>) -> bool {
        let allowed = self.allows(part, parent);
        for id in std::iter::once(part).chain(parent).filter(|id| !id.is_empty()) {
            self.seen.insert(id.to_owned());
            if !allowed {
                self.stopped.insert(id.to_owned());
            }
        }
        allowed
    }

    pub fn stop(&mut self) {
        self.stopped.append(&mut self.seen);
    }
}

/// Session-scoped bridge between a stopped run and real queued work. This is
/// lifecycle bookkeeping, not a delay: dequeue/new start promotes it, an empty
/// queue settles it, and the old run's idle edge must not unmount the footer.
#[derive(Clone, Debug, Default)]
pub struct QueueHandoff {
    pub stopped_parts: StoppedParts,
    waiting: bool,
    started_at: Option<u64>,
    stopped_execution: Option<String>,
}

impl QueueHandoff {
    pub fn begin(&mut self, count: usize, preview: Option<&str>) -> bool {
        self.waiting = ordinary_queue_count(count, preview) > 0;
        self.waiting
    }

    pub fn waiting(&self) -> bool {
        self.waiting
    }

    pub fn stop_execution(&mut self, execution: Option<&str>) {
        if self.waiting {
            self.stopped_execution = execution.map(str::to_owned);
        }
    }

    /// A handoff uses its local neutral/new-turn clock until the successor's
    /// execution arrives. This does not retire activity or change platform
    /// clock policies for ordinary turns.
    pub fn accepts_clock(&self, execution: &str) -> bool {
        self.stopped_execution.as_deref() != Some(execution)
    }

    /// running=true can still describe the stopped run. An empty, non-clear
    /// snapshot alone is not proof of a successor: stay neutral until dequeue,
    /// a distinct start, or idle, rather than unmounting between those edges.
    pub fn defer_empty_snapshot(
        &self,
        count: usize,
        preview: Option<&str>,
        running: bool,
        action: Option<&str>,
    ) -> bool {
        self.waiting && running
            && !matches!(action, Some("clear" | "failed" | "remove"))
            && ordinary_queue_count(count, preview) == 0
    }

    pub fn settle_snapshot(
        &mut self,
        count: usize,
        preview: Option<&str>,
        running: bool,
        action: Option<&str>,
    ) -> bool {
        !self.defer_empty_snapshot(count, preview, running, action)
            && self.empty_queue(count, preview)
    }

    pub fn observe_start(&mut self, started_at: Option<u64>) -> bool {
        let promoted = self.waiting && started_at.is_some() && started_at != self.started_at;
        if !self.waiting || promoted {
            self.started_at = started_at.or(self.started_at);
        }
        if promoted {
            self.waiting = false;
        }
        promoted
    }

    pub fn dequeue(&mut self) -> bool {
        std::mem::take(&mut self.waiting)
    }

    pub fn empty_queue(&mut self, count: usize, preview: Option<&str>) -> bool {
        if self.waiting && ordinary_queue_count(count, preview) == 0 {
            self.waiting = false;
            true
        } else {
            false
        }
    }
}

/// Local admission feedback is separate from authoritative prompt counters.
pub fn visible_queue_count(count: usize, preview: Option<&str>, pending: bool) -> usize {
    count.saturating_add(usize::from(pending && preview != Some("Compaction queued")))
}

pub fn visible_queue_preview(preview: Option<&str>, pending: bool) -> Option<&str> {
    preview.or(pending.then_some("Compaction queued"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueStatusDecision {
    pub count: usize,
    pub preview: Option<String>,
    /// A busy status proves that a run exists, but says nothing about what the
    /// model is doing. Use the neutral post-submit state until a real
    /// reasoning/tool part supplies stronger semantic evidence.
    pub should_enter_generating: bool,
    pub started_at: Option<u64>,
}

pub fn queue_status_decision(
    count: usize,
    preview: Option<String>,
    started_at: Option<u64>,
    is_streaming: bool,
) -> QueueStatusDecision {
    QueueStatusDecision {
        count,
        preview,
        should_enter_generating: started_at.is_some() && !is_streaming,
        started_at,
    }
}

/// Turn a raw provider retry message into one short status-row reason.
///
/// Provider bodies are often full sentences (and frequently repeat
/// "please try again"). The timeline status already says "Retrying", so keep
/// only the useful cause and cap unknown messages before they can crowd the
/// composer.
pub fn compact_retry_reason(message: &str) -> Option<String> {
    let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    let known = if lower.contains("overloaded") || lower.contains("at capacity") {
        Some("Provider overloaded")
    } else if lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("429")
    {
        Some("Rate limited")
    } else if lower.contains("timed out") || lower.contains("timeout") {
        Some("Provider timeout")
    } else if lower.contains("connection reset")
        || lower.contains("connection closed")
        || lower.contains("stream error")
    {
        Some("Connection interrupted")
    } else if lower.contains("service unavailable") || lower.contains("503") {
        Some("Service unavailable")
    } else {
        None
    };
    if let Some(known) = known {
        return Some(known.to_string());
    }

    const MAX_CHARS: usize = 72;
    let mut compact = message
        .trim_end_matches(|ch: char| matches!(ch, '.' | '!' | '?'))
        .to_string();
    if compact.chars().count() > MAX_CHARS {
        compact = compact.chars().take(MAX_CHARS - 1).collect();
        compact.push('…');
    }
    Some(compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimistic_compact_projection_deduplicates_authority_and_preserves_prompts() {
        assert_eq!(visible_queue_count(0, None, true), 1);
        assert_eq!(visible_queue_preview(None, true), Some("Compaction queued"));
        assert_eq!(visible_queue_count(1, Some("Compaction queued"), true), 1);
        assert_eq!(visible_queue_count(2, Some("ordinary"), true), 3);
        assert_eq!(visible_queue_preview(Some("ordinary"), true), Some("ordinary"));
        assert_eq!(visible_queue_count(2, Some("ordinary"), false), 2);
        assert_eq!(visible_queue_preview(None, false), None);
    }

    #[test]
    fn idle_queue_status_preserves_authoritative_count_and_preview() {
        assert_eq!(
            queue_status_decision(3, Some("queued prompt".to_string()), None, false),
            QueueStatusDecision {
                count: 3,
                preview: Some("queued prompt".to_string()),
                should_enter_generating: false,
                started_at: None,
            }
        );
    }

    #[test]
    fn active_queue_status_preserves_prompt_and_enters_generating_when_idle() {
        assert_eq!(
            queue_status_decision(2, Some("next prompt".to_string()), Some(1234), false),
            QueueStatusDecision {
                count: 2,
                preview: Some("next prompt".to_string()),
                should_enter_generating: true,
                started_at: Some(1234),
            }
        );
    }

    #[test]
    fn active_queue_status_does_not_replace_existing_streaming_state() {
        let decision = queue_status_decision(1, None, Some(1234), true);
        assert_eq!(decision.count, 1);
        assert!(!decision.should_enter_generating);
    }

    #[test]
    fn retry_reason_compacts_provider_noise() {
        assert_eq!(
            compact_retry_reason(
                "Our servers are currently overloaded. Please try again later."
            )
            .as_deref(),
            Some("Provider overloaded")
        );
        assert_eq!(
            compact_retry_reason("  connection   reset by peer  ").as_deref(),
            Some("Connection interrupted")
        );
        assert_eq!(compact_retry_reason("   "), None);
    }
}
