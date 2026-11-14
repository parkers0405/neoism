//! The sidebar: Back, the filter field, the story list as an indented tree, and the stage's
//! controls along its foot.

use super::view::{CHEVRON_DOWN, CHEVRON_LEFT, CHEVRON_RIGHT, MOTION, selected, tertiary};
use super::*;

/// The filter field's row: the field and the gap under it.
const FIELD: f32 = 32.0;

impl<S: 'static> Live<S> {
    pub(super) fn sidebar(&self, t: Theme, dark: bool, accent: Color, inset: f32, sidebar_w: f32, height: f32) -> El {
        let back = row().key("playground-back").h(CONTROL).pl(4.0).pr(8.0).gap(2.0).center_y().radius(radius::SM)
            .hover(t.hover).press(t.press)
            .child(icon(&CHEVRON_LEFT, 16.0).color(t.text_secondary))
            .child(text("Back").font(type_::LABEL).color(t.text))
            .role("button").label("Back to the app")
            .on_press_tx(|_: &mut Controls, tx: &mut Tx| tx.close_playground());
        // Back sits at the list's text inset; where the window's own controls take the corner,
        // it goes in the row under them.
        let band = if inset > 0.0 { BAND } else { 0.0 };
        let header = row().w(sidebar_w).h(HEADER).pl(8.0).center_y().child(back);
        let drag = (band > 0.0).then(|| el().w(sidebar_w).h(band).window_drag());
        // The list's selection takes the accent while the list has the keyboard.
        let on_fill = if self.region == Region::List { focused(accent) } else { selected(t, dark) };
        // A scroll measures its first child, so the rows go in one content column.
        let mut list = col().key("playground-rows").w_full();
        let rows = self.rows();
        for (n, entry) in rows.iter().copied().enumerate() {
            list = list.child(match entry {
                Row::Header(group, first, depth) => self.header_row(t, on_fill, group, first, depth, n == 0),
                Row::Story(i, depth) => self.story_row(t, on_fill, i, depth),
            });
        }
        if rows.is_empty() {
            let empty = if self.order.is_empty() { "No stories" } else { "No matches" };
            list = list.child(text(empty).font(type_::BODY).color(t.text_secondary).px(8.0).pt(4.0));
        }
        let list = col().key("playground-list").w(sidebar_w).h(height - 2.0 * HEADER - FIELD - band).px(8.0).pb(8.0)
            .scroll(ScrollSpec { stick_bottom: false }).child(list);
        col().w(sidebar_w).h(height)
            .children(drag)
            .child(header)
            .child(self.filter_field(t, sidebar_w))
            .child(list)
            .child(self.stage_controls(t, dark, sidebar_w))
    }

    /// Narrows the list to the stories whose path has every word typed; / focuses it.
    fn filter_field(&self, t: Theme, sidebar_w: f32) -> El {
        let field = text_input(&self.controls.filter, type_::LABEL).key(FILTER)
            .single_line()
            .color(t.text)
            .caret(t.text, t.text.alpha(0.2))
            .placeholder("Filter", tertiary(t))
            .grow(1.0)
            .min_w(0.0)
            .label("Filter stories")
            .on_edit(|c: &mut Controls| &mut c.filter);
        row().w(sidebar_w).h(FIELD).px(8.0).pb(8.0).child(
            row().w_full().h(FIELD - 8.0).px(8.0).center_y().radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border).child(field),
        )
    }

    /// A story's row, indented a step for each group it's in.
    fn story_row(&self, t: Theme, on_fill: Color, i: usize, depth: usize) -> El {
        let on = i == self.selected && self.header.is_none();
        let name = self.name(i);
        // Stories with triggers move, so a quiet mark tells them apart.
        let mark = self.has_triggers(i).then(|| [
            spacer(),
            icon(&MOTION, 12.0).key(format!("playground-motion-{i}")).color(tertiary(t)).ml(6.0),
        ]);
        row().key(format!("playground-item-{i}")).w_full().h(ROW).pl(indent(depth)).pr(8.0).center_y().radius(radius::MD)
            .fill(if on { on_fill } else { Color::TRANSPARENT })
            .hover(if on { on_fill } else { t.hover })
            .child(text(name).font(type_::BODY).color(if on || i == self.selected { t.text } else { t.text_secondary }).truncate())
            .children(mark.into_iter().flatten())
            .role("button").label(name)
            .on_press_tx(move |_: &mut Controls, tx: &mut Tx| tx.playground.push(Request::Select(i)))
    }

    /// A group's header, keyed by its first story and depth: a disclosure chevron, its name a
    /// step bolder than a story's, and how many stories it holds. A press selects it and folds
    /// or unfolds it.
    fn header_row(&self, t: Theme, on_fill: Color, group: &'static str, first: usize, depth: usize, top: bool) -> El {
        let on = self.header == Some(group);
        let folded = self.is_folded(group);
        let name = Self::parent(group).map_or(group, |up| &group[up.len() + 1..]);
        // A top group reads in full text colour, the groups inside it a step quieter.
        let ink = if on || depth == 0 { t.text } else { t.text_secondary };
        let row = row().key(Row::header_key(first, depth)).w_full().h(24.0).pl(indent(depth)).pr(8.0).center_y().radius(radius::MD)
            .fill(if on { on_fill } else { Color::TRANSPARENT })
            .hover(if on { on_fill } else { t.hover })
            .child(row().w(INDENT).h(24.0).center().child(icon(if folded { &CHEVRON_RIGHT } else { &CHEVRON_DOWN }, 12.0).color(tertiary(t))))
            .child(text(name).font(type_::SECTION).color(ink).truncate())
            .child(spacer())
            .child(text(self.count_in(group).to_string()).key(Row::header_key(first, depth).replace("group", "count")).font(type_::LABEL).color(tertiary(t)).ml(6.0))
            .role("button").label(if folded { format!("Show {name}") } else { format!("Hide {name}") })
            .on_press_tx(move |_: &mut Controls, tx: &mut Tx| {
                tx.playground.push(Request::Header(group));
                tx.playground.push(Request::Fold(group, None));
            });
        // Top groups stand apart; the groups inside them sit closer.
        let above = match (top, depth) { (true, _) => 2.0, (_, 0) => 10.0, _ => 4.0 };
        col().w_full().pt(above).pb(2.0).child(row)
    }
}

/// A row's left inset at `depth`: its text lines up with its group's name.
fn indent(depth: usize) -> f32 {
    8.0 + INDENT * depth as f32
}

/// A selection's fill in the column that has the keyboard: the accent, softly.
pub(super) fn focused(accent: Color) -> Color {
    accent.alpha(0.2)
}
