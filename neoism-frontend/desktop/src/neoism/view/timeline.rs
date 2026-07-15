use crate::neoism::agent::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentOutputKind, NeoismAgentPane,
    TimelineMeasureKey,
};

pub(crate) struct DesktopTimelineDelegate;

neoism_ui::neoism_ui_impl_agent_timeline_message!(
    NeoismAgentMessage,
    NeoismAgentMessageKind,
    NeoismAgentOutputKind
);

neoism_ui::neoism_ui_impl_agent_timeline_pane!(
    NeoismAgentPane,
    NeoismAgentMessage,
    TimelineMeasureKey,
    crate::neoism::agent::perf::enabled
);

neoism_ui::neoism_ui_impl_agent_timeline_delegate!(
    DesktopTimelineDelegate,
    NeoismAgentPane,
    NeoismAgentMessage,
    measure_message_height = super::message_card::measure_message_height,
    render_message_card = super::message_card::render_message_card,
    render_streaming_status_row = super::user_input::render_streaming_status_row,
);
