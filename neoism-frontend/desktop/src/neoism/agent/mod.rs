mod api;
mod commands;
mod pane;
pub(crate) mod picker;
pub(crate) mod prompt_history;
pub(crate) mod side_panel;
mod updates;

pub(crate) mod perf {
    use std::sync::OnceLock;
    use std::time::Instant;

    static ENABLED: OnceLock<bool> = OnceLock::new();

    pub(crate) fn enabled() -> bool {
        *ENABLED.get_or_init(|| {
            std::env::var_os("NEOISM_AGENT_UI_PERF_LOG").is_some()
                || std::env::var_os("NEOISM_AGENT_PERF").is_some()
        })
    }

    pub(crate) fn now() -> Option<Instant> {
        enabled().then(Instant::now)
    }

    pub(crate) fn elapsed_us(started: Option<Instant>) -> Option<u128> {
        started.map(|started| started.elapsed().as_micros())
    }
}

pub(crate) use api::{
    agent_reverse_proxy_for_daemon_endpoint, agent_reverse_proxy_for_daemon_workspace,
    neoism_agent_server, register_agent_server_credential,
};
pub(crate) use pane::TimelineMeasureKey;
pub use pane::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentNoticeLevel,
    NeoismAgentOutputKind, NeoismAgentPane, NeoismAgentPendingPermission,
    NeoismAgentPermissionChoice, NeoismAgentStreamingState, NeoismAgentTodo,
    NeoismAgentUiEvent, NeoismWordmarkState,
};
