use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::NeoismAgentPane;
use crate::primitives::ide_theme::IdeTheme;

pub mod assistant;
pub mod chat;
pub mod code_block;
pub mod derivations;
pub mod draw;
pub mod fx;
pub mod home;
pub mod image_preview;
pub mod layout;
pub mod markdown;
pub mod message_card;
pub mod picker;
pub mod prompt_picker;
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
/// User bubbles intentionally show a compact prompt preview. Measurement,
/// rendering, and the lazy timeline estimator must all share this cap or a
/// large pasted prompt reserves a tall blank row beneath its visible lines.
pub(super) const USER_MESSAGE_MAX_LINES: usize = 6;
/// Help/status strip painted immediately below the composer island.
/// Chat layout reserves this much bottom space so the strip never gets
/// clipped by the pane edge.
pub(super) const INPUT_HELP_STRIP_H: f32 = 28.0;
/// Height (in logical px, pre-scale) reserved at the very bottom of the pane
/// for the streaming status line. Stays fixed regardless of the input rect.
pub(super) const STREAMING_STATUS_LINE_H: f32 = 26.0;
pub(super) const INPUT_IMAGE_RAIL_H: f32 = 82.0;

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
    fn begin_visible_animation_frame(&mut self) {}
    fn tick_timeline_scroll(&mut self) -> bool;
    fn picker_options_len(&self) -> Option<usize>;

    /// Whether the open picker carries the `/sessions` footer band, so the
    /// occlusion rect can cover it. Defaults to `false`.
    fn picker_has_session_footer(&self) -> bool {
        false
    }

    /// Easter-egg skit timer (`/piss`, `/cuss`). `take_fx_request`
    /// yields the queued skit exactly once so the render loop can
    /// stamp its start on its own animation clock; `fire_fx_prompt`
    /// is called once when the skit's prompt moment passes (the host
    /// submits the pending message there). Hosts without the eggs
    /// keep the defaults.
    fn take_fx_request(&mut self) -> Option<fx::AgentFxKind> {
        None
    }
    fn fx_started(&self) -> Option<(fx::AgentFxKind, f32)> {
        None
    }
    fn set_fx_started(&mut self, _at: Option<(fx::AgentFxKind, f32)>) {}
    fn fire_fx_prompt(&mut self) {}

    #[allow(clippy::too_many_arguments)]
    fn log_render_perf(
        &mut self,
        _elapsed_us: u128,
        _rect: [f32; 4],
        _input_rect: [f32; 4],
        _active: bool,
        _ticked_scroll: bool,
        _occlusion_count: usize,
    ) {
    }
}

impl AgentPaneView for NeoismAgentPane {
    fn begin_visible_animation_frame(&mut self) {
        NeoismAgentPane::begin_visible_animation_frame(self);
    }

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
    pane.begin_visible_animation_frame();
    // Advance kinetic scroll one frame before laying anything out. This keeps
    // the inertial motion and the rest of the render in lockstep — no
    // jitter between tick and paint.
    let ticked_scroll = AgentPaneView::tick_timeline_scroll(pane);
    // The side panel lives in a strip carved off the right of the
    // agent rect. Subtract it BEFORE computing input / timeline layout
    // so the chat column never paints under the panel frame.
    let (main_rect, side_panel_rect) =
        match side_panel::carve_panel_rect(pane, rect, chrome_scale) {
            // The panel is pane-owned: its geometry may carve the Agent
            // content, but may never escape into window-level chrome.
            Some((main, panel)) => (main, Some(panel)),
            None => {
                // Pane is too narrow to host the panel — drop the cached
                // hit-test rect so click/wheel/Alt+arrow don't treat a
                // stale strip as still live.
                side_panel::AgentSidePanelPane::side_panel_mut(pane)
                    .clear_last_panel_rect();
                (rect, None)
            }
        };
    let has_conversation = chat::AgentChatPane::has_conversation(pane);
    // Height must come from the same real glyph measurements used to draw
    // the prompt. The former char-count estimate could lag one visual row
    // behind a wide font: caret-follow then showed only the new row until
    // enough extra text made the estimate catch up.
    let input_w = if has_conversation {
        layout::chat_column(main_rect, chrome_scale).1
    } else {
        layout::home_input_width(main_rect, chrome_scale)
    };
    let prompt_wrap_rows = user_input::measure_prompt_visual_rows(
        sugarloaf,
        layout::AgentPaneInput::input(pane),
        input_w,
        chrome_scale,
    );
    let prompt_visual_rows = prompt_wrap_rows.len();
    let input_rect = if has_conversation {
        layout::chat_input_rect_for_visual_rows(
            pane,
            main_rect,
            chrome_scale,
            prompt_visual_rows,
        )
    } else {
        layout::home_input_rect_for_visual_rows(
            pane,
            main_rect,
            chrome_scale,
            prompt_visual_rows,
        )
    };
    // The pre-chat composer sits around the pane midpoint, leaving much
    // less safe vertical room than the bottom-docked chat composer. Give
    // its picker a compact five-row window and clamp it below this pane's
    // chrome boundary. Its horizontal edges still match the composer
    // exactly, just like the full-chat picker.
    let picker_input_rect = input_rect;
    let picker_min_y = if has_conversation {
        8.0 * chrome_scale
    } else {
        main_rect[1] + 12.0 * chrome_scale
    };
    let picker_has_footer = pane.picker_has_session_footer();
    let picker_max_rows = if has_conversation {
        crate::widgets::inline_picker::DEFAULT_MAX_ROWS
    } else {
        crate::widgets::inline_picker::row_limit_for_space(
            picker_input_rect[1],
            picker_min_y,
            chrome_scale,
            picker_has_footer,
            5,
        )
    };
    // The inline picker is a real late Sugarloaf overlay. Do not reserve its
    // predicted column in the normal text pass: doing so erases timeline text
    // above and outside the actual rounded menu surface.
    let local_occlusions = occlusion_rects.to_vec();
    clear_overlays(sugarloaf);
    sugarloaf.rect(None, x, y, w, h, theme.f32(theme.bg), DEPTH, ORDER_BG);
    // The "NEOISM" home page and the chat timeline render immediately —
    // no body skeleton. First-load shimmer belongs on the recent-sessions
    // TREE (date/name rows) in the side panel, handled there by
    // `draw_session_loading_skeleton`, not over this welcome/entry body.
    if has_conversation {
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
            Some(&prompt_wrap_rows),
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
            Some(&prompt_wrap_rows),
        );
    }
    if let Some(panel_rect) = side_panel_rect {
        side_panel::render_side_panel_with_icons::<P, I>(
            sugarloaf,
            pane,
            panel_rect,
            theme,
            chrome_scale,
            now_seconds,
            mouse,
            &local_occlusions,
        );
    }
    // Side-panel visibility is controlled by its in-pane affordance and
    // `/sidebar`; the top-bar Agent icon opens a new Agent tab.
    // Pending permission / model question takes the picker slot — the
    // prompt pops out of the input island exactly like the "/" menu.
    // The regular picker is suppressed while a prompt is pending (the
    // key bridge closes it anyway on the next keypress).
    let prompt_rect = prompt_picker::render_prompt_picker(
        sugarloaf,
        pane,
        input_rect,
        theme,
        chrome_scale,
    );
    pane.set_prompt_picker_rect(prompt_rect);
    if prompt_rect.is_none() {
        picker::render_picker(
            sugarloaf,
            pane,
            picker_input_rect,
            theme,
            chrome_scale,
            picker_max_rows,
            picker_min_y,
        );
    }
    if let Some(kind) = pane.take_fx_request() {
        pane.set_fx_started(Some((kind, now_seconds)));
    }
    if let Some((kind, started)) = pane.fx_started() {
        let elapsed = now_seconds - started;
        // Negative = the 10k-second animation clock wrapped; clear.
        if (0.0..=fx::total_seconds(kind)).contains(&elapsed) {
            if elapsed >= fx::prompt_at(kind) {
                // Idempotent: the host consumes its pending prompt on
                // the first call.
                pane.fire_fx_prompt();
            }
            fx::render(kind, sugarloaf, main_rect, elapsed, chrome_scale, theme);
        } else {
            pane.fire_fx_prompt();
            pane.set_fx_started(None);
        }
    }
    pane.log_render_perf(
        render_started.elapsed().as_micros(),
        rect,
        input_rect,
        active,
        ticked_scroll,
        local_occlusions.len(),
    );
}
