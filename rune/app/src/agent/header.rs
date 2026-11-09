//! The sub-agent family header (`view/chat.rs`, `render_conversation_header`): above a child
//! conversation, `← Back` (to the root) or `← Prev` on the left and `Next →` on the right, in
//! muted 12 pt text that brightens on hover. It reserves 28 pt above the transcript.

use crate::app::App;
use crate::ui::theme::rgb;
use rune::{El, Tx, el, row, text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Previous,
    Next,
}

pub fn view(app: &App, viewed: &str) -> Option<El> {
    let nav = app.families.navigation(viewed);
    let previous = nav.previous?;
    let s = app.ty.scale();
    let link = |step: Step, label: &'static str, target: &str| {
        let hovered = app.header_hover == Some(step);
        let target = target.to_owned();
        el().key(format!("family-nav:{step:?}")).h(22.0 * s).px(2.0 * s).pt(5.0 * s)
            .child(text(label).font(app.ty.sized(12.0)).color(rgb(if hovered { app.pal.fg } else { app.pal.muted })))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
                if inside { a.header_hover = Some(step); } else if a.header_hover == Some(step) { a.header_hover = None; }
            })
            .on_press(move |a: &mut App| { a.header_hover = None; a.view_session(&target); })
    };
    let back = if nav.previous_is_main { "\u{2190} Back" } else { "\u{2190} Prev" };
    let mut header = row().key("family-header").w_full().h(28.0 * s).pt(3.0 * s).justify_between()
        .child(link(Step::Previous, back, previous));
    if let Some(next) = nav.next {
        header = header.child(link(Step::Next, "Next \u{2192}", next));
    }
    Some(header)
}
