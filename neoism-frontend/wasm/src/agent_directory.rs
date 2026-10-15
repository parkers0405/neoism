//! Blank-agent execution directory drafts: no session creation or prompt IO.
use neoism_ui::panels::agent_pane::state::NeoismAgentPane;

pub(crate) fn set_blank_directory(
    pane: &mut NeoismAgentPane,
    path: &str,
    creation_in_flight: bool,
    prompt_pending: bool,
) -> bool {
    if pane.session_id_str().is_some()
        || creation_in_flight
        || prompt_pending
        || path.trim().is_empty()
    {
        return false;
    }
    pane.set_directory(Some(path.to_owned()));
    true
}

/// Both the footer bridge and first CreateThread use this execution directory.
pub(crate) fn execution_directory(
    pane: Option<&NeoismAgentPane>,
    fallback: Option<&str>,
) -> Option<String> {
    pane.and_then(|pane| pane.session_directory())
        .or(fallback)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_selection_preserves_input_and_emits_no_commands() {
        let mut pane = NeoismAgentPane::with_directory(Some("/workspace".into()));
        pane.replace_input("unfinished draft");
        // Discard initial catalog/config requests; directory selection itself
        // must not add any session or prompt commands.
        let _ = pane.drain_pending_outbound();
        assert!(set_blank_directory(
            &mut pane,
            "C:\\host\\picked",
            false,
            false
        ));
        assert_eq!(pane.input(), "unfinished draft");
        assert_eq!(pane.session_id_str(), None);
        assert_eq!(pane.session_directory(), Some("C:\\host\\picked"));
        assert_eq!(
            execution_directory(Some(&pane), Some("/workspace")),
            Some("C:\\host\\picked".into())
        );
        assert!(pane.drain_pending_outbound().is_empty());
        // No shared/default change is needed, and a different pane is untouched.
        let other = NeoismAgentPane::with_directory(Some("/other".into()));
        assert_eq!(
            execution_directory(Some(&other), Some("/workspace")),
            Some("/other".into())
        );
    }

    #[test]
    fn first_prompt_in_flight_or_existing_session_rejects_draft_change() {
        let mut pane = NeoismAgentPane::with_directory(Some("/original".into()));
        assert!(!set_blank_directory(&mut pane, "/picked", true, false));
        assert!(!set_blank_directory(&mut pane, "/picked", false, true));
        pane.switch_session("session-a".into());
        assert!(!set_blank_directory(&mut pane, "/picked", false, false));
        assert_eq!(pane.session_directory(), Some("/original"));
    }
}
