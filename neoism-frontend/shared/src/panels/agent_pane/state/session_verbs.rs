//! `/sessions` picker verbs: pin (Ctrl+F), delete (Ctrl+D), and inline
//! rename (Ctrl+R) — the shared, host-neutral mirror of desktop's
//! synchronous implementations in `desktop/src/neoism/agent/pane/input.rs`.
//!
//! Desktop performs the HTTP mutation inline and refreshes the list
//! itself; the shared pane records an [`OutboundAgentCommand`] instead
//! and the host drains it (`SetPinned` / `DeleteThread` / `SetTitle`
//! over the daemon WebSocket on web). The list refresh is driven by the
//! daemon's post-mutation ack (`ThreadUpdated` / `ThreadDeleted`), so
//! the picker never re-fetches ahead of the mutation landing.

use super::*;

impl NeoismAgentPane {
    /// Whether an open picker is the `/sessions` picker (gates the
    /// pin/delete/rename shortcuts + inline rename in the key bridge).
    pub fn session_picker_open(&self) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Session)
    }

    /// The keyboard-selected `/sessions` row as `(id, title, pinned)`.
    /// `None` for headers and placeholder rows (loading / empty).
    fn selected_session_row(&self) -> Option<(String, String, bool)> {
        let picker = self.picker.as_ref()?;
        if picker.kind != NeoismAgentPickerKind::Session {
            return None;
        }
        let option = picker.selected_option()?;
        if option.value.trim().is_empty() {
            return None;
        }
        Some((option.value.clone(), option.title.clone(), option.pinned))
    }

    /// `ctrl+f` — toggle the pinned flag of the selected session.
    pub fn toggle_selected_session_pin(&mut self) -> bool {
        let Some((id, _title, pinned)) = self.selected_session_row() else {
            return false;
        };
        self.push_outbound(OutboundAgentCommand::SetSessionPinned {
            session_id: id,
            pinned: !pinned,
        });
        true
    }

    /// `ctrl+d` — delete the selected session. Deleting the session the
    /// user is currently inside resets the pane to a fresh chat, same
    /// as desktop's `create_new_session` fallback.
    pub fn delete_selected_session(&mut self) -> bool {
        let Some((id, _title, _pinned)) = self.selected_session_row() else {
            return false;
        };
        let deleting_current = self.session_id.as_deref() == Some(id.as_str());
        self.push_outbound(OutboundAgentCommand::DeleteSession { session_id: id });
        if deleting_current {
            self.start_new_conversation();
        }
        true
    }

    /// `ctrl+r` — start an inline rename of the selected session.
    pub fn begin_selected_session_rename(&mut self) -> bool {
        let Some((id, title, _pinned)) = self.selected_session_row() else {
            return false;
        };
        self.session_rename = Some((id, title));
        true
    }

    pub fn session_rename_active(&self) -> bool {
        self.session_rename.is_some()
    }

    pub fn session_rename_buffer(&self) -> Option<String> {
        self.session_rename
            .as_ref()
            .map(|(_, buffer)| buffer.clone())
    }

    pub fn push_session_rename(&mut self, text: &str) {
        if let Some((_, buffer)) = self.session_rename.as_mut() {
            buffer.push_str(text);
        }
    }

    pub fn backspace_session_rename(&mut self) {
        if let Some((_, buffer)) = self.session_rename.as_mut() {
            buffer.pop();
        }
    }

    pub fn cancel_session_rename(&mut self) {
        self.session_rename = None;
    }

    /// Commit the inline rename: publish the new title and let the
    /// host's ack-driven refresh update the list. An all-whitespace
    /// buffer cancels without renaming (desktop parity).
    pub fn commit_session_rename(&mut self) -> bool {
        let Some((id, buffer)) = self.session_rename.take() else {
            return false;
        };
        let title = buffer.trim().to_string();
        if title.is_empty() {
            return true;
        }
        self.push_outbound(OutboundAgentCommand::SetTitle {
            session_id: id,
            title,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_with_session_picker() -> NeoismAgentPane {
        let mut pane = NeoismAgentPane::default();
        let mut pinned_row =
            NeoismAgentPickerOption::new("Pinned one", "", "", "ses_pinned");
        pinned_row.pinned = true;
        pane.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Session,
            "Sessions",
            vec![
                NeoismAgentPickerOption::new("First", "", "", "ses_1"),
                pinned_row,
            ],
            0,
        ));
        pane
    }

    #[test]
    fn pin_toggle_emits_inverted_pinned_flag() {
        let mut pane = pane_with_session_picker();
        assert!(pane.session_picker_open());
        assert!(pane.toggle_selected_session_pin());
        assert!(matches!(
            pane.drain_pending_outbound().as_slice(),
            [OutboundAgentCommand::SetSessionPinned { session_id, pinned: true }]
                if session_id == "ses_1"
        ));

        pane.move_picker_selection(1);
        assert!(pane.toggle_selected_session_pin());
        assert!(matches!(
            pane.drain_pending_outbound().as_slice(),
            [OutboundAgentCommand::SetSessionPinned { session_id, pinned: false }]
                if session_id == "ses_pinned"
        ));
    }

    #[test]
    fn delete_emits_delete_session_for_selected_row() {
        let mut pane = pane_with_session_picker();
        assert!(pane.delete_selected_session());
        assert!(matches!(
            pane.drain_pending_outbound().as_slice(),
            [OutboundAgentCommand::DeleteSession { session_id }] if session_id == "ses_1"
        ));
    }

    #[test]
    fn deleting_current_session_resets_to_fresh_chat() {
        let mut pane = pane_with_session_picker();
        pane.session_id = Some("ses_1".to_string());
        assert!(pane.delete_selected_session());
        assert!(pane.session_id.is_none());
    }

    #[test]
    fn rename_flow_edits_buffer_then_emits_set_title() {
        let mut pane = pane_with_session_picker();
        assert!(pane.begin_selected_session_rename());
        assert!(pane.session_rename_active());
        assert_eq!(pane.session_rename_buffer().as_deref(), Some("First"));

        pane.backspace_session_rename();
        pane.push_session_rename("nal");
        assert_eq!(pane.session_rename_buffer().as_deref(), Some("Firsnal"));

        assert!(pane.commit_session_rename());
        assert!(!pane.session_rename_active());
        assert!(matches!(
            pane.drain_pending_outbound().as_slice(),
            [OutboundAgentCommand::SetTitle { session_id, title }]
                if session_id == "ses_1" && title == "Firsnal"
        ));
    }

    #[test]
    fn empty_rename_cancels_without_emitting() {
        let mut pane = pane_with_session_picker();
        assert!(pane.begin_selected_session_rename());
        for _ in 0..8 {
            pane.backspace_session_rename();
        }
        assert!(pane.commit_session_rename());
        assert!(pane.drain_pending_outbound().is_empty());
    }

    #[test]
    fn verbs_require_a_selectable_session_row() {
        let mut pane = NeoismAgentPane::default();
        assert!(!pane.session_picker_open());
        assert!(!pane.toggle_selected_session_pin());
        assert!(!pane.delete_selected_session());
        assert!(!pane.begin_selected_session_rename());

        // Placeholder rows (empty value) must not be actionable.
        pane.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Session,
            "Sessions",
            vec![NeoismAgentPickerOption::new(
                "No sessions",
                "No saved sessions for this workspace",
                "empty",
                "",
            )],
            0,
        ));
        assert!(!pane.toggle_selected_session_pin());
        assert!(!pane.delete_selected_session());
        assert!(pane.drain_pending_outbound().is_empty());
    }
}
