pub(crate) use neoism_ui::panels::agent_pane::view::layout::*;

use crate::neoism::agent::NeoismAgentPane;

impl AgentPaneInput for NeoismAgentPane {
    fn input(&self) -> &str {
        self.input()
    }

    fn input_help_visible(&self) -> bool {
        self.input_help_visible()
    }

    fn background_task_details_expanded(&self) -> bool {
        self.background_task_details_expanded()
    }
}
