use crate::neoism::agent::{
    NeoismAgentMessage, NeoismAgentOutputKind, NeoismAgentPane, NeoismAgentTodo,
};

pub(crate) struct DesktopSidePanelIcons;

neoism_ui::neoism_ui_impl_agent_side_panel!(
    todo = NeoismAgentTodo,
    message = NeoismAgentMessage,
    pane = NeoismAgentPane,
    output_kind = NeoismAgentOutputKind,
    icons = DesktopSidePanelIcons,
    sugarloaf = dyn neoism_ui::primitives::paint::Paint,
    register_icons = crate::neoism::icon::register_agent_icons
);
