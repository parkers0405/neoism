use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::NeoismAgentPane;
use crate::primitives::ide_theme::IdeTheme;

pub mod assistant;
pub mod chat;
pub mod code_block;
pub mod derivations;
pub mod draw;
pub mod home;
pub mod layout;
pub mod markdown;
pub mod message_card;
pub mod picker;
pub mod side_panel;
pub mod timeline;
pub mod tool_message;
pub mod user_input;
pub mod wordmark;

pub(super) const WORDMARK_PNG: &[u8] =
    include_bytes!("../../../../assets/splash/neoism-wordmark.png");
pub(super) const WORDMARK_IMAGE_ID: u32 = 0xA0DE_1001;
pub(super) const OVERLAY_PANEL_ID: usize = usize::MAX - 13;

pub(super) const LETTER_COUNT: usize = 6;
pub(super) const LETTER_HOVER_RATE: f32 = 14.0;
pub(super) const LETTER_HOVER_SCALE: f32 = 0.18;
pub(super) const LETTER_HOVER_LIFT: f32 = 0.18;
pub(super) const LETTER_SHIMMER_AMP: f32 = 0.025;
pub(super) const LETTER_SHIMMER_PERIOD: f32 = 3.4;

pub(super) const DEPTH: f32 = 0.0;
pub(super) const ORDER_BG: u8 = 18;
pub(super) const ORDER_PANEL: u8 = 19;
pub(super) const ORDER_TEXT: u8 = 20;
pub(super) const ORDER_CARET: u8 = 21;
// Input rect = the bordered input box (text + send band) plus the
// skirt band below it holding the dropdown-chip row — see
// `user_input::CHIPS_BAND_H`.
pub(super) const HOME_INPUT_MIN_H: f32 = 106.0;
pub(super) const CHAT_INPUT_MIN_H: f32 = 98.0;
pub(super) const INPUT_LINE_H: f32 = 22.0;
pub(super) const MAX_INPUT_LINES: usize = 5;
/// Height (in logical px, pre-scale) reserved at the very bottom of the pane
/// for the streaming status line. Stays fixed regardless of the input rect.
pub(super) const STREAMING_STATUS_LINE_H: f32 = 26.0;

pub fn clear_overlays(sugarloaf: &mut Sugarloaf) {
    sugarloaf.clear_image_overlays_for(OVERLAY_PANEL_ID);
    crate::panels::agent_pane::icon::clear_side_panel_icon_overlays(sugarloaf);
}

pub trait AgentPaneView:
    chat::AgentChatPane
    + home::AgentHomePane
    + layout::AgentPaneInput
    + picker::AgentPickerPane
    + side_panel::AgentSidePanelPane
{
    fn tick_timeline_scroll(&mut self) -> bool;
    fn picker_options_len(&self) -> Option<usize>;

    /// Whether the open picker carries the `/sessions` footer band, so the
    /// occlusion rect can cover it. Defaults to `false`.
    fn picker_has_session_footer(&self) -> bool {
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn log_render_perf(
        &mut self,
        _elapsed_us: u128,
        _rect: [f32; 4],
        _input_rect: [f32; 4],
        _active: bool,
        _ticked_scroll: bool,
        _occlusion_count: usize,
        _panel_bottom_override: Option<f32>,
        _panel_top_override: Option<f32>,
    ) {
    }
}

impl AgentPaneView for NeoismAgentPane {
    fn tick_timeline_scroll(&mut self) -> bool {
        NeoismAgentPane::tick_timeline_scroll(self)
    }

    fn picker_options_len(&self) -> Option<usize> {
        NeoismAgentPane::picker(self).map(|picker| picker.options().len())
    }

    fn picker_has_session_footer(&self) -> bool {
        use crate::panels::agent_pane::state::picker::NeoismAgentPickerKind;
        NeoismAgentPane::picker(self)
            .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Session)
    }
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
    // `panel_bottom_override` lets the active pane's side panel run
    // all the way to the window bottom, matching the file tree's
    // full-height column. `None` keeps the panel inside the pane rect.
    panel_bottom_override: Option<f32>,
    // `panel_top_override` lets the side panel extend ABOVE the pane
    // rect so it reaches the very top of the window (under the chrome
    // top bar), again matching the file tree's full-height column.
    // `None` anchors the panel at the pane rect's top.
    panel_top_override: Option<f32>,
    occlusion_rects: &[[f32; 4]],
) {
    render_agent_pane_with::<
        NeoismAgentPane,
        timeline::SharedTimelineDelegate,
        side_panel::SharedAgentSidePanelIcons,
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

#[allow(clippy::too_many_arguments)]
pub fn render_agent_pane_with<P, D, I>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    rect: [f32; 4],
    theme: &IdeTheme,
    active: bool,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    chrome_scale: f32,
    panel_bottom_override: Option<f32>,
    panel_top_override: Option<f32>,
    occlusion_rects: &[[f32; 4]],
) where
    P: AgentPaneView,
    D: timeline::AgentTimelineDelegate<P>,
    I: side_panel::AgentSidePanelIconHost,
{
    let render_started = web_time::Instant::now();
    let [x, y, w, h] = rect;
    if w <= 8.0 || h <= 8.0 {
        return;
    }

    let chrome_scale = chrome_scale.clamp(0.5, 3.0);
    // Advance kinetic scroll one frame before laying anything out. This keeps
    // the inertial motion and the rest of the render in lockstep — no
    // jitter between tick and paint.
    let ticked_scroll = AgentPaneView::tick_timeline_scroll(pane);
    // The side panel lives in a strip carved off the right of the
    // agent rect. Subtract it BEFORE computing input / timeline layout
    // so the chat column never paints under the panel frame.
    let (main_rect, side_panel_rect) =
        match side_panel::carve_panel_rect(pane, rect, chrome_scale) {
            Some((main, mut panel)) => {
                // Stretch the side-panel column up to the chrome top
                // (under the top bar) when the caller asked us to.
                // Mirrors `panel_bottom_override` on the bottom side
                // — the chrome already insets the buffer-tabs strip
                // horizontally so it doesn't paint behind the panel.
                if let Some(extended_top) = panel_top_override {
                    let new_top = extended_top.min(panel[1]);
                    let delta = panel[1] - new_top;
                    if delta > 0.0 {
                        panel[1] = new_top;
                        panel[3] += delta;
                    }
                }
                // Stretch the side-panel column down to the window
                // bottom when the caller asked us to (active pane);
                // status bar already shrinks past the panel so nothing
                // else paints in this strip.
                if let Some(extended_bottom) = panel_bottom_override {
                    let new_h = (extended_bottom - panel[1]).max(panel[3]);
                    panel[3] = new_h;
                }
                (main, Some(panel))
            }
            None => {
                // Pane is too narrow to host the panel — drop the cached
                // hit-test rect so click/wheel/Alt+arrow don't treat a
                // stale strip as still live.
                side_panel::AgentSidePanelPane::side_panel_mut(pane)
                    .clear_last_panel_rect();
                (rect, None)
            }
        };
    let input_rect = if chat::AgentChatPane::has_conversation(pane) {
        layout::chat_input_rect(pane, main_rect, chrome_scale)
    } else {
        layout::home_input_rect(pane, main_rect, chrome_scale)
    };
    let mut local_occlusions = occlusion_rects.to_vec();
    let picker_has_footer = pane.picker_has_session_footer();
    if let Some(picker_rect) = pane.picker_options_len().and_then(|len| {
        crate::widgets::inline_picker::layout(
            len,
            input_rect,
            chrome_scale,
            picker_has_footer,
        )
    }) {
        local_occlusions.push(picker_rect);
    }
    // Island corner notches: the timeline clips at the island's
    // straight top edge, but the rounded border curves away BELOW
    // that line at the corners — sliced glyphs would hover over the
    // bare notch with no border under them. Occlude a corner-radius
    // square at each top corner, hung 2px ABOVE the line downward:
    // only rows whose boxes actually cross the clip line (i.e. rows
    // being sliced) intersect it, so intact rows resting above the
    // island — like the streaming status row's ╰─ connector — keep
    // their corner-column glyphs.
    let corner = user_input::ISLAND_CORNER * chrome_scale;
    for corner_x in [input_rect[0], input_rect[0] + input_rect[2] - corner] {
        local_occlusions.push([
            corner_x,
            input_rect[1] - 2.0 * chrome_scale,
            corner,
            corner,
        ]);
    }
    clear_overlays(sugarloaf);
    sugarloaf.rect(None, x, y, w, h, theme.f32(theme.bg), DEPTH, ORDER_BG);
    if chat::AgentChatPane::has_conversation(pane) {
        chat::render_chat_with::<P, D>(
            sugarloaf,
            pane,
            main_rect,
            theme,
            active,
            now_seconds,
            mouse,
            chrome_scale,
            input_rect,
            &local_occlusions,
        );
    } else {
        home::render_home_with(
            sugarloaf,
            pane,
            main_rect,
            theme,
            active,
            now_seconds,
            mouse,
            chrome_scale,
            input_rect,
            &local_occlusions,
        );
    };
    if let Some(panel_rect) = side_panel_rect {
        side_panel::render_side_panel_with_icons::<P, I>(
            sugarloaf,
            pane,
            panel_rect,
            theme,
            chrome_scale,
            now_seconds,
            &local_occlusions,
        );
    }
    // Side-panel toggle button used to live here — moved to the chrome
    // top bar's right edge. Hosts set
    // `chrome.top_bar.set_right_button_visible(true)` while an agent
    // pane is active and route the resulting `ToggleRightPanel`
    // action through `agent.side_panel().set_user_hidden(!visible)`.
    picker::render_picker(sugarloaf, pane, input_rect, theme, chrome_scale);
    pane.log_render_perf(
        render_started.elapsed().as_micros(),
        rect,
        input_rect,
        active,
        ticked_scroll,
        local_occlusions.len(),
        panel_bottom_override,
        panel_top_override,
    );
}
