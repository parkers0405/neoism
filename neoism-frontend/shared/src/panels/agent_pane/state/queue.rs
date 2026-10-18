use super::session_cache::CachedAgentRuntime;
use super::*;

impl CachedAgentRuntime {
    pub(super) fn suppress_streaming_after_abort(&self) -> bool {
        self.queue_handoff.waiting()
            || self.abort_requested_at.is_some_and(|requested| {
                Instant::now().saturating_duration_since(requested)
                    <= ABORT_STREAM_SUPPRESSION
            })
    }

    pub(super) fn promote_queue_handoff(&mut self) {
        if self.queue_handoff.dequeue() {
            self.abort_requested_at = None;
            self.streaming_started_at = None;
            self.prompt_admission.settle();
            self.note_streaming(NeoismAgentStreamingState::Generating, None);
        }
    }

    pub(super) fn reconcile_queue_handoff(
        &mut self,
        started_at: Option<u64>,
        running: bool,
        action: Option<&str>,
    ) {
        if self.queue_handoff.observe_start(started_at) {
            self.abort_requested_at = None;
            self.streaming_started_at = None;
            self.note_streaming(NeoismAgentStreamingState::Generating, None);
        } else if self.queue_handoff.settle_snapshot(
            self.queued_prompt_count,
            self.queued_prompt_preview.as_deref(),
            running,
            action,
        ) {
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
        }
    }
}

impl NeoismAgentPane {
    pub fn note_remote_streaming(
        &mut self,
        state: NeoismAgentStreamingState,
        tool: Option<String>,
    ) {
        if state == NeoismAgentStreamingState::Idle
            || !self.suppress_streaming_after_abort()
        {
            self.note_streaming(state, tool);
        }
    }

    pub(super) fn begin_queue_stop(&mut self) {
        for message in &self.messages {
            if !matches!(
                message.kind,
                NeoismAgentMessageKind::User | NeoismAgentMessageKind::System
            ) {
                self.queue_handoff.stopped_parts.observe(&message.id, None);
            }
        }
        self.queue_handoff.stopped_parts.stop();
        self.pending_compact = None;
        self.prompt_admission.settle();
        self.queued_prompt_count = status_policy::ordinary_queue_count(
            self.queued_prompt_count,
            self.queued_prompt_preview.as_deref(),
        );
        if self.queued_prompt_preview.as_deref() == Some("Compaction queued") {
            self.queued_prompt_preview = None;
        }
        let waiting = self.queue_handoff.begin(
            self.queued_prompt_count,
            self.queued_prompt_preview.as_deref(),
        );
        self.queue_handoff.stop_execution(
            self.execution_activity
                .as_ref()
                .map(|activity| activity.execution_id.as_str()),
        );
        self.abort_requested_at = Some(Instant::now());
        self.note_streaming(
            if waiting {
                NeoismAgentStreamingState::Generating
            } else {
                NeoismAgentStreamingState::Idle
            },
            None,
        );
    }

    pub(super) fn promote_queue_handoff(&mut self) {
        if self.queue_handoff.dequeue() {
            self.abort_requested_at = None;
            self.streaming_started_at = None;
            self.prompt_admission.settle();
            self.side_panel.clear_status_display_hold();
            self.note_streaming(NeoismAgentStreamingState::Generating, None);
        }
    }

    pub(super) fn reconcile_queue_handoff(
        &mut self,
        started_at: Option<u64>,
        running: bool,
        action: Option<&str>,
    ) {
        if self.queue_handoff.observe_start(started_at) {
            self.abort_requested_at = None;
            self.streaming_started_at = None;
            self.note_streaming(NeoismAgentStreamingState::Generating, None);
        } else if self.queue_handoff.settle_snapshot(
            self.queued_prompt_count,
            self.queued_prompt_preview.as_deref(),
            running,
            action,
        ) {
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
        }
    }
}
