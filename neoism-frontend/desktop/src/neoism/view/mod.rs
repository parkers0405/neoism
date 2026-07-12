use neoism_backend::sugarloaf::Sugarloaf;
use neoism_ui::panels::agent_pane::view::{fx, render_agent_pane_with, AgentPaneView};
use neoism_ui::primitives::ide_theme::IdeTheme;

use crate::neoism::agent::NeoismAgentPane;

mod chat;
mod code_block;
mod draw;
mod home;
mod layout;
pub(crate) mod markdown;
mod message_card;
mod picker;
mod side_panel;
mod timeline;
mod user_input;

pub(super) const OVERLAY_PANEL_ID: usize = usize::MAX - 13;

impl AgentPaneView for NeoismAgentPane {
    fn tick_timeline_scroll(&mut self) -> bool {
        NeoismAgentPane::tick_timeline_scroll(self)
    }

    fn picker_options_len(&self) -> Option<usize> {
        NeoismAgentPane::picker(self).map(|picker| picker.options().len())
    }

    fn picker_has_session_footer(&self) -> bool {
        use neoism_ui::panels::agent_pane::state::picker::NeoismAgentPickerKind;
        NeoismAgentPane::picker(self)
            .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Session)
    }

    fn take_fx_request(&mut self) -> Option<fx::AgentFxKind> {
        NeoismAgentPane::take_fx_request(self)
    }

    fn fx_started(&self) -> Option<(fx::AgentFxKind, f32)> {
        NeoismAgentPane::fx_started(self)
    }

    fn set_fx_started(&mut self, at: Option<(fx::AgentFxKind, f32)>) {
        NeoismAgentPane::set_fx_started(self, at);
    }

    fn fire_fx_prompt(&mut self) {
        NeoismAgentPane::fire_fx_prompt(self);
    }

    fn log_render_perf(
        &mut self,
        elapsed_us: u128,
        rect: [f32; 4],
        input_rect: [f32; 4],
        active: bool,
        ticked_scroll: bool,
        occlusion_count: usize,
        panel_bottom_override: Option<f32>,
        panel_top_override: Option<f32>,
    ) {
        NeoismAgentPane::log_render_perf(
            self,
            Some(elapsed_us),
            rect,
            input_rect,
            active,
            ticked_scroll,
            occlusion_count,
            panel_bottom_override,
            panel_top_override,
        );
    }
}

pub fn clear_overlays(sugarloaf: &mut Sugarloaf) {
    sugarloaf.clear_image_overlays_for(OVERLAY_PANEL_ID);
    crate::neoism::icon::clear_side_panel_icon_overlays(sugarloaf);
}
#[allow(clippy::too_many_arguments)]
pub fn render(
    sugarloaf: &mut Sugarloaf,
    pane: &mut NeoismAgentPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    active: bool,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    chrome_scale: f32,
    panel_bottom_override: Option<f32>,
    panel_top_override: Option<f32>,
    occlusion_rects: &[[f32; 4]],
) {
    render_agent_pane_with::<
        NeoismAgentPane,
        timeline::DesktopTimelineDelegate,
        side_panel::DesktopSidePanelIcons,
    >(
        sugarloaf,
        pane,
        rect,
        theme,
        active,
        now_seconds,
        mouse,
        chrome_scale,
        panel_bottom_override,
        panel_top_override,
        occlusion_rects,
    );
}
