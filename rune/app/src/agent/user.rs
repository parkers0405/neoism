//! A user message (`view/user_input.rs`, `render_user_message`): an Apple-blue capsule hugging
//! the right edge, white 13.5 pt text (one line centred), a curved tail at its lower right, and
//! the sender's presence orb in a lane beside the tail.

use crate::app::App;
use rune::{Color, El, Shape, canvas, col, el, row, text};

const ORB: f32 = 28.0;
const PAD_Y: f32 = 11.0;
const TAIL_H: f32 = 8.0;
const LINE_H: f32 = 19.0;
const FONT: f32 = 13.5;
/// Apple #007AFF.
pub(super) const BLUE: u32 = 0x007aff;

fn avatar_lane(width: f32, s: f32) -> f32 { ((ORB + 16.0) * s).min(width.max(0.0) * 0.4) }

/// Leave an incoming-message gutter on desktop widths; relax it on narrow panes.
fn max_width(width: f32, s: f32) -> f32 {
    let width = width.max(0.0);
    (width * 0.8).max((width - 8.0 * s).max(0.0).min(320.0 * s)).min((width - avatar_lane(width, s)).max(0.0))
}

fn pad_x(width: f32, s: f32) -> f32 { (12.0 * s).min(max_width(width, s) * 0.2) }

/// The tail's two edges at `t` in 0..=1: the outside leaves the bubble's right side, the
/// underside leaves its bottom; both meet at the short down-right tip.
fn tail_edges(right: f32, bottom: f32, s: f32, t: f32) -> [[f32; 2]; 2] {
    let cubic = |p: [[f32; 2]; 4]| {
        let u = 1.0 - t;
        let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
        let (x, y) = (0..4).fold((0.0, 0.0), |(x, y), i| (x + p[i][0] * w[i], y + p[i][1] * w[i]));
        [right + x * s, bottom + y * s]
    };
    [
        cubic([[0.0, -22.0], [0.0, -6.0], [0.0, 1.5], [3.0, 5.0]]),
        cubic([[-22.0, 0.0], [-10.0, 0.0], [-8.0, 5.0], [3.0, 5.0]]),
    ]
}

/// The tail patch replacing the capsule's lower-right quadrant, in a canvas whose origin is
/// `(right - 22 s, bottom - 22 s)`.
pub(super) fn tail(s: f32, color: Color) -> El {
    const STEPS: usize = 32;
    let (w, h) = (25.0 * s, 27.0 * s);
    let (right, bottom) = (22.0 * s, 22.0 * s);
    let mut points = vec![[0.0, 0.0]];
    points.extend((0..=STEPS).map(|i| tail_edges(right, bottom, s, i as f32 / STEPS as f32)[0]));
    points.extend((0..=STEPS).rev().map(|i| tail_edges(right, bottom, s, i as f32 / STEPS as f32)[1]));
    canvas(Shape::new().view_box(w, h).fill(points, color)).w(w).h(h)
}

/// The full-width row for one user message in a `width` point column.
pub fn view(app: &App, id: &str, message: &str, attachments: &[neoism_protocol::agent::HistoryAttachment], author: Option<&str>, width: f32) -> El {
    let s = app.ty.scale();
    let blue = crate::ui::theme::rgb(BLUE);
    let lane = avatar_lane(width, s);
    let max_w = max_width(width, s);
    let pad = pad_x(width, s);
    let radius = 22.0 * s;
    let strip = super::attach::message_strip(app, id, attachments, max_w.max(1.0));
    let message = if strip.is_some() { super::attach::display_text(message) } else { message.to_owned() };
    // A decoded image can change size after mounting. Keep a definite wrapping width and
    // do not let its caption shrink vertically or inherit animated layout changes.
    let mut content = col().w(max_w).items_end().gap(8.0 * s).instant_changes();
    if let Some(strip) = strip { content = content.child(strip); }
    if !message.is_empty() || attachments.is_empty() {
        let body = col().max_w(max_w).px(pad).py(PAD_Y * s).radius(radius).fill(blue)
            .child(text(message).max_w((max_w - 2.0 * pad).max(1.0))
            .font(app.ty.sized(FONT).lh(LINE_H * s)).color(Color::WHITE).wrap()
            // White over the blue bubble: the accent highlight would vanish into it.
            .selectable(true).selection_color(Color::WHITE.alpha(0.3)));
        content = content.child(col().max_w(max_w)
            .child(tail(s, blue).pin(None, None, Some(-3.0 * s), Some(-5.0 * s)))
            .child(body));
    }
    let (seed, _label) = crate::ui::orb::identity(author, &app.local_name);
    let orb = ORB * s;
    row().w_full().justify_end().items_end().pb(TAIL_H * s)
        .child(content)
        .child(row().w(lane).justify_end().child(app.orbs.view(&seed, orb.min(lane * 0.7))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_follow_the_original_rules() {
        let s = 1.0;
        assert_eq!(avatar_lane(600.0, s), 44.0);
        assert_eq!(max_width(600.0, s), 480.0);
        assert_eq!(max_width(200.0, s), 156.0, "narrow panes keep the avatar lane");
        assert_eq!(pad_x(600.0, s), 12.0);
    }

    #[test]
    fn tail_runs_from_both_roots_to_one_tip() {
        let root = tail_edges(100.0, 50.0, 1.0, 0.0);
        assert_eq!(root, [[100.0, 28.0], [78.0, 50.0]]);
        let tip = tail_edges(100.0, 50.0, 1.0, 1.0);
        assert_eq!(tip[0], tip[1]);
    }
}
