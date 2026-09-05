use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::NeoismAgentPane;

use super::user_input::AgentUserInputPane;
use super::wordmark::WordmarkState;
use super::{user_input, wordmark};
use crate::panels::agent_pane::input_controller::InputWrapRow;
use crate::primitives::ide_theme::IdeTheme;

pub trait AgentHomePane: AgentUserInputPane {
    type Wordmark: WordmarkState;

    fn wordmark_mut(&mut self) -> &mut Self::Wordmark;
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_home_pane {
    ($pane:ty, $wordmark:ty) => {
        impl $crate::panels::agent_pane::view::home::AgentHomePane for $pane {
            type Wordmark = $wordmark;

            fn wordmark_mut(&mut self) -> &mut Self::Wordmark {
                &mut self.wordmark
            }
        }
    };
}

impl AgentHomePane for NeoismAgentPane {
    type Wordmark = crate::panels::agent_pane::state::NeoismWordmarkState;

    fn wordmark_mut(&mut self) -> &mut Self::Wordmark {
        &mut self.wordmark
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_home_with<P: AgentHomePane>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    rect: [f32; 4],
    theme: &IdeTheme,
    active: bool,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    s: f32,
    input_rect: [f32; 4],
    occlusion_rects: &[[f32; 4]],
    prepared_input_rows: Option<&[InputWrapRow]>,
) {
    let [x, y, w, h] = rect;
    let input_y = input_rect[1];
    let aspect = crate::panels::terminal_splash::WORDMARK_ASPECT;
    let top_pad = 20.0 * s;
    let min_gap = 18.0 * s;
    let max_logo_w = (w - 32.0 * s).max(1.0);
    let max_logo_h =
        (max_logo_w / aspect).min((input_y - y - top_pad - min_gap).max(1.0));
    // Smaller hero: the per-letter wordmark reads as a header above
    // the composer, not a splash poster — anchored a fixed gap ABOVE
    // the input card (not floated in its own band) so logo + composer
    // read as one centered group instead of two far-apart pieces.
    let logo_h = (h * 0.12 * s).clamp(34.0 * s, 84.0 * s).min(max_logo_h);
    let logo_w = logo_h * aspect;
    let logo_x = x + (w - logo_w) * 0.5;
    let gap_above_input = (44.0 * s).max(min_gap);
    let logo_y = (input_y - gap_above_input - logo_h).max(y + top_pad);

    super::clear_overlays(sugarloaf);
    wordmark::render_wordmark(
        sugarloaf,
        pane.wordmark_mut(),
        [logo_x, logo_y, logo_w, logo_h],
        now_seconds,
        mouse,
        1,
        occlusion_rects,
    );

    user_input::render_input(
        sugarloaf,
        pane,
        input_rect,
        w,
        theme,
        active,
        mouse,
        s,
        true,
        now_seconds,
        occlusion_rects,
        prepared_input_rows,
    );
}
