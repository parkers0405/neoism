//! The question rail (`agent_pane/question_rail.rs`, `view/timeline/render.rs`): one tick per
//! user question in the transcript's right gutter, evenly spaced (not proportional to answer
//! length), the tick for the question in view lit, ticks near the pointer widening, and the
//! hovered question previewed in a blue chat bubble to the rail's left. Pressing or dragging on
//! the rail brings that question to the top of the view.

use super::user::{BLUE, tail};
use crate::app::App;
use crate::ui::theme::rgb;
use neoism_protocol::agent::HistoryMessageKind;
use rune::{Color, El, Phase, Tx, col, el, text};

/// Rail and preview metrics in points (`timeline_scroll_rail_geometry`, `question_preview_rect`).
const SPACING: f32 = 8.0;
const MAX_WIDTH: f32 = 26.0;
const PREVIEW_MAX_W: f32 = 340.0;
const PREVIEW_FONT: f32 = 12.0;
const PREVIEW_LINE: f32 = 16.0;
const PREVIEW_LINES: usize = 4;

/// A question's place in the transcript: its message and the content offset of its row.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub id: String,
    /// The content offset of its row, once that row has been laid out.
    pub top: Option<f32>,
}

/// The rail's track in window points and its tick count (`TimelineQuestionRailFrame`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rail {
    pub track: [f32; 4],
    pub count: usize,
    scale: f32,
}

impl Rail {
    /// In the gutter right of the transcript column; `None` under two questions or no room.
    pub fn new([x, y, w, h]: [f32; 4], s: f32, count: usize) -> Option<Rail> {
        if count < 2 || ![x, y, w, h, s].iter().all(|v| v.is_finite()) || s <= 0.0 || w < 6.0 * s || h < 4.0 * s {
            return None;
        }
        let rail_h = ((count - 1) as f32 * SPACING * s).min((h - 4.0 * s).max(0.0));
        let width = w.min(MAX_WIDTH * s);
        let track = [x + ((w - width) * 0.5).min(8.0 * s), y + (h - rail_h) * 0.5, width, rail_h];
        Some(Rail { track, count, scale: s })
    }

    fn tick_y(&self, index: usize) -> f32 { self.track[1] + index as f32 * self.track[3] / (self.count - 1) as f32 }

    /// `question_index_at`: the nearest tick to `y` while the pointer is on the track.
    pub fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        let [tx, ty, tw, th] = self.track;
        let inside = x >= tx && x <= tx + tw && y >= ty && y <= ty + th;
        inside.then(|| self.index_near(y))
    }

    /// The tick nearest `y`, clamped to the track (a drag past its ends).
    pub fn index_near(&self, y: f32) -> usize {
        if self.track[3] <= 0.0 { return 0; }
        (((y - self.track[1]) / self.track[3]).clamp(0.0, 1.0) * (self.count - 1) as f32).round() as usize
    }

    /// `tick_rect`: 2 pt tall, full width under the pointer tapering to a third two ticks out.
    pub fn tick_rect(&self, index: usize, hovered: Option<usize>) -> [f32; 4] {
        let ratio = match hovered.map(|h| h.abs_diff(index)) {
            Some(0) => 1.0,
            Some(1) => 2.0 / 3.0,
            Some(2) => 5.0 / 12.0,
            _ => 1.0 / 3.0,
        };
        let pad = self.scale.min(self.track[2] * 0.125);
        [self.track[0] + pad, self.tick_y(index) - self.scale, (self.track[2] - 2.0 * pad) * ratio, 2.0 * self.scale]
    }
}

/// `active_question_index`: the last question at the live edge, else the first whose row is in
/// view, else the one above the view. `rows` are the questions' row indices in order.
pub fn active_index(rows: &[usize], visible: std::ops::Range<usize>, at_bottom: bool) -> Option<usize> {
    if at_bottom { return rows.len().checked_sub(1); }
    if let Some(i) = rows.iter().position(|r| visible.contains(r)) { return Some(i); }
    rows.iter().rposition(|r| *r < visible.start)
}

/// `compact_question_preview`: the message's plain text (Markdown dropped) on one line.
pub fn compact(source: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, TagEnd};
    let bounded: String = source.chars().take(1500).collect();
    let mut plain = String::new();
    for event in Parser::new_ext(&bounded, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS) {
        match event {
            Event::Text(t) | Event::Code(t) => plain.push_str(&t),
            Event::SoftBreak | Event::HardBreak | Event::Rule => plain.push(' '),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock | TagEnd::TableCell | TagEnd::TableRow) => plain.push(' '),
            _ => {}
        }
    }
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.is_empty() { "User question".to_owned() } else { plain }
}

/// `question_preview_lines`: wrap at spaces into `max_lines`, the last one ellipsised.
pub fn preview_lines(text: &str, width: f32, max_lines: usize, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut remaining = text;
    for line in 0..max_lines {
        if remaining.is_empty() { break; }
        if measure(remaining) <= width { lines.push(remaining.to_owned()); break; }
        let last = line + 1 == max_lines;
        let mut end = 0;
        for (i, ch) in remaining.char_indices() {
            let next = i + ch.len_utf8();
            let candidate = if last { format!("{}…", &remaining[..next]) } else { remaining[..next].to_owned() };
            if measure(&candidate) > width { break; }
            end = next;
        }
        if last { lines.push(format!("{}…", remaining[..end].trim_end())); break; }
        if end == 0 { break; }
        if let Some(space) = remaining[..end].rfind(' ').filter(|space| *space > 0) { end = space; }
        lines.push(remaining[..end].trim().to_owned());
        remaining = remaining[end..].trim_start();
    }
    lines
}

/// `question_preview_rect`: left of the track, centred on the tick, kept inside the pane.
fn preview_rect(pane: [f32; 4], rail: &Rail, index: usize, height: f32, s: f32) -> Option<[f32; 4]> {
    let right = (rail.track[0] - 6.0 * s).min(pane[0] + pane[2]);
    let width = (right - pane[0] - 4.0 * s).min(PREVIEW_MAX_W * s);
    let height = height.min(pane[3]);
    if width < 40.0 * s || height < 30.0 * s { return None; }
    let top = (rail.tick_y(index) - height * 0.5).clamp(pane[1], pane[1] + pane[3] - height);
    Some([right - width, top, width, height])
}

impl App {
    /// The questions of `session_id` in transcript order, at their last laid-out offsets.
    pub(super) fn questions(&self, session_id: &str) -> Vec<Question> {
        let Some(session) = self.sessions.get(session_id) else { return Vec::new() };
        session.messages.iter().filter(|m| m.kind == HistoryMessageKind::User)
            .map(|m| Question { id: m.id.clone(), top: session.question_tops.get(&m.id).copied() })
            .collect()
    }


    /// Bring question `index` to the top of the view on the wheel's glide.
    fn jump_to_question(&mut self, session_id: &str, index: usize, tx: &mut Tx) {
        let Some(question) = self.questions(session_id).get(index).cloned() else { return };
        let Some(session) = self.sessions.get_mut(session_id) else { return };
        match question.top {
            // A row laid out before glides there; one never mounted is revealed by key.
            Some(top) => {
                let (offset, max) = session.scroll.get();
                session.glide = Some(crate::ui::follow::Glide::retarget(session.glide, offset, top.clamp(0.0, max)));
                tx.wake_after(std::time::Duration::ZERO);
            }
            None => {
                session.glide = None;
                tx.scroll_into_view_in(format!("conversation:{session_id}"), format!("msg:{}", question.id));
            }
        }
    }
}

/// The rail and its preview over a conversation pane, placed in window points from the
/// transcript `view` and the `pane` it sits in; `gutter` is the transcript's right padding.
pub fn view(app: &App, session_id: &str, view: [f32; 4], pane: [f32; 4], gutter: f32) -> El {
    let s = app.ty.scale();
    let p = app.pal;
    let questions = app.questions(session_id);
    let Some(rail) = Rail::new([view[0] + view[2] - gutter, view[1], gutter, view[3]], s, questions.len()) else { return el() };
    let session = app.sessions.get(session_id);
    let active = session.and_then(|s| s.rail_active);
    let hovered = app.question_hover.as_ref().filter(|(id, _)| id == session_id).map(|(_, i)| *i);
    let local = |r: [f32; 4]| (r[0] - pane[0], r[1] - pane[1]);

    // Every tick is keyed, so widening under the pointer never re-pairs one with another.
    let mut layer = el().key(format!("question-rail:{session_id}")).w_full().h_full().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .instant_changes();
    for index in 0..rail.count {
        let r = rail.tick_rect(index, hovered);
        let color = if active == Some(index) { rgb(p.fg).alpha(0.9) }
            else { rgb(p.muted).alpha(if hovered == Some(index) { 0.75 } else { 0.35 }) };
        let (x, y) = local(r);
        layer = layer.child(el().key(format!("tick:{index}")).w(r[2]).h(r[3]).fill(color).absolute(x, y));
    }
    // The hit area is the track; pointer motion picks the tick, press or drag jumps to it.
    let (tx_, ty_) = local(rail.track);
    let hover_id = session_id.to_owned();
    let drag_id = session_id.to_owned();
    let leave_id = session_id.to_owned();
    layer = layer.child(el().key("rail-hit").w(rail.track[2]).h(rail.track[3]).absolute(tx_, ty_)
        .on_pointer_move_local(move |a: &mut App, ev: rune::PointerEvent, _: &mut Tx| {
            let next = rail.index_at(ev.window_x, ev.window_y).map(|i| (hover_id.clone(), i));
            if a.question_hover != next { a.question_hover = next; }
        })
        .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
            if !inside && a.question_hover.as_ref().is_some_and(|(id, _)| *id == leave_id) { a.question_hover = None; }
        })
        .on_drag(move |a: &mut App, d: &mut rune::Drag, tx: &mut Tx| {
            let index = rail.index_near(rail.track[1] + d.y);
            if d.phase != Phase::Up { a.question_hover = Some((drag_id.clone(), index)); }
            a.jump_to_question(&drag_id, index, tx);
        }));

    if let Some(index) = hovered {
        if let Some(bubble) = preview(app, session_id, &questions, &rail, index, pane) {
            layer = layer.child(bubble);
        }
    }
    layer
}

/// The hovered question in the chat's blue bubble with its tail (`build_question_preview`).
fn preview(app: &App, session_id: &str, questions: &[Question], rail: &Rail, index: usize, pane: [f32; 4]) -> Option<El> {
    let s = app.ty.scale();
    let question = questions.get(index)?;
    let message = app.sessions.get(session_id)?.messages.iter().find(|m| m.id == question.id)?;
    let text_body = compact(&message.text);
    let font = app.ty.sized(PREVIEW_FONT);
    let advance = PREVIEW_FONT * s * 0.6;
    let measure = |t: &str| t.chars().count() as f32 * advance;
    let provisional = preview_rect(pane, rail, index, 84.0 * s, s)?;
    let available = ((pane[3] - 22.0 * s) / (PREVIEW_LINE * s)).floor().max(0.0) as usize;
    let lines = preview_lines(&text_body, provisional[2] - 24.0 * s, available.min(PREVIEW_LINES), measure);
    if lines.is_empty() { return None; }
    let fitted = (lines.iter().map(|l| measure(l)).fold(0.0_f32, f32::max) + 24.0 * s).clamp(40.0 * s, provisional[2]);
    let height = 22.0 * s + PREVIEW_LINE * s * lines.len() as f32;
    let mut rect = preview_rect(pane, rail, index, height, s)?;
    rect[0] += rect[2] - fitted;
    rect[2] = fitted;
    let (body_w, body_h) = (rect[2] - 4.0 * s, rect[3] - 6.0 * s);
    let tail_scale = (0.82 * s).min(body_w / 44.0).min(body_h / 44.0);
    let blue = rgb(BLUE);
    let mut body = col().w(body_w).h(body_h).radius(22.0 * tail_scale).fill(blue).pl(10.0 * s).pt(8.0 * s);
    for (i, line) in lines.into_iter().enumerate() {
        body = body.child(text(line).key(format!("line:{i}")).font(font.lh(PREVIEW_LINE * s)).color(Color::WHITE));
    }
    // The tail's 22-unit corner patch sits on the body's lower-right corner, as in the chat.
    let bubble = el().key(format!("question-preview:{index}")).w(rect[2]).h(rect[3])
        .absolute(rect[0] - pane[0], rect[1] - pane[1])
        .child(tail(tail_scale, blue).absolute(body_w - 22.0 * tail_scale, body_h - 22.0 * tail_scale))
        .child(body.absolute(0.0, 0.0));
    Some(bubble)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_evenly_spaced_and_hit_by_nearest() {
        let rail = Rail::new([400.0, 100.0, 20.0, 160.0], 1.0, 3).unwrap();
        // Two gaps of 8 pt, centred: the track runs 172..188.
        assert_eq!(rail.track, [400.0, 172.0, 20.0, 16.0]);
        assert_eq!(rail.index_at(410.0, 172.0), Some(0));
        assert_eq!(rail.index_at(410.0, 181.0), Some(1));
        assert_eq!(rail.index_at(410.0, 188.0), Some(2));
        assert_eq!(rail.index_at(399.0, 180.0), None);
        assert!(Rail::new([400.0, 100.0, 20.0, 160.0], 1.0, 1).is_none());
    }

    #[test]
    fn ticks_widen_near_the_pointer() {
        let rail = Rail::new([0.0, 0.0, 26.0, 200.0], 1.0, 5).unwrap();
        let w = |i| rail.tick_rect(i, Some(2))[2];
        assert_eq!(w(2), 24.0);
        assert_eq!(w(1), 16.0);
        assert_eq!(w(0), 10.0);
        assert_eq!(rail.tick_rect(4, None)[2], 8.0);
    }

    #[test]
    fn the_lit_question_follows_the_view() {
        // Questions sit on rows 0, 4 and 9.
        let rows = [0, 4, 9];
        assert_eq!(active_index(&rows, 0..3, true), Some(2));
        assert_eq!(active_index(&rows, 2..6, false), Some(1));
        assert_eq!(active_index(&rows, 5..8, false), Some(1));
        assert_eq!(active_index(&rows, 8..12, false), Some(2));
    }

    #[test]
    fn previews_wrap_and_ellipsise() {
        let measure = |t: &str| t.chars().count() as f32;
        assert_eq!(preview_lines("short", 10.0, 4, measure), ["short"]);
        // GPUiX breaks at the last space inside the prefix that fits, as here.
        assert_eq!(preview_lines("aaa bbb ccc ddd", 7.0, 2, measure), ["aaa", "bbb cc…"]);
        assert_eq!(compact("# Title\n\nSome `code` and **bold**"), "Title Some code and bold");
        assert_eq!(compact(""), "User question");
    }
}
