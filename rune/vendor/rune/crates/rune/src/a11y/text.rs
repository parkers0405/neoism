//! Text inside the accessibility tree: a text field's lines as text runs with each
//! character's place, so a reader moves by character, word and line and a magnifier follows
//! the caret; a selectable text's lines likewise; and a text's inline links as link nodes.

use accesskit::{Action as Ak, Node as AkNode, NodeId as AkId, Rect as AkRect, Role, TextPosition, TextSelection};

use super::*;
use super::a11y_tree::{line_id, link_id, round};

/// Text longer than this gets no per-character runs (a reader still gets its whole value).
const RUN_MAX_BYTES: usize = 64 * 1024;

/// One laid-out line: its bytes in the drawn text and each character's place in points.
pub(crate) struct Line {
    pub range: std::ops::Range<usize>,
    pub rect: [f32; 4],
    pub lengths: Vec<u8>,
    pub positions: Vec<f32>,
    pub widths: Vec<f32>,
    pub word_starts: Vec<u8>,
}

/// `content`'s lines from `layout` (device px at `s`), placed from `origin` in points.
pub(crate) fn lines(layout: &Layout<Ink>, content: &str, s: f32, origin: [f32; 2]) -> Vec<Line> {
    let mut out = Vec::new();
    for line in layout.lines() {
        let m = line.metrics();
        let range = line.text_range();
        if range.end > content.len() { break; }
        let (mut lengths, mut positions, mut widths, mut word_starts) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut x = m.offset;
        let mut at = range.start;
        for run in line.runs() {
            for cluster in run.clusters() {
                let r = cluster.text_range();
                if r.start < at || r.end > range.end || r.is_empty() { continue; }
                // Bytes a cluster skipped (an ignorable) join the character before.
                if r.start > at && let Some(last) = lengths.last_mut() { *last = (*last as usize + r.start - at).min(255) as u8; }
                if cluster.is_word_boundary() && !lengths.is_empty() && lengths.len() < 256 { word_starts.push(lengths.len() as u8); }
                let adv = cluster.advance();
                lengths.push((r.end - r.start).min(255) as u8);
                positions.push((x - m.offset) / s);
                widths.push(adv / s);
                x += adv;
                at = r.end;
            }
        }
        if at < range.end {
            match lengths.last_mut() {
                Some(last) => *last = (*last as usize + range.end - at).min(255) as u8,
                None => { lengths.push((range.end - at).min(255) as u8); positions.push(0.0); widths.push(0.0); }
            }
        }
        // AccessKit indexes word starts by u8: a line past 255 characters keeps the first ones.
        word_starts.retain(|&w| (w as usize) < lengths.len());
        let top = m.block_min_coord / s;
        let height = (m.block_max_coord - m.block_min_coord) / s;
        let rect = [origin[0] + m.offset / s, origin[1] + top, (x - m.offset) / s, height];
        out.push(Line { range, rect, lengths, positions, widths, word_starts });
    }
    out
}

/// The line and character index of drawn byte `byte`.
fn position(lines: &[Line], byte: usize) -> Option<(usize, usize)> {
    let last = lines.len().checked_sub(1)?;
    let k = lines.iter().position(|l| byte < l.range.end).unwrap_or(last);
    let l = &lines[k];
    let mut at = l.range.start;
    for (c, len) in l.lengths.iter().enumerate() {
        if byte < at + *len as usize { return Some((k, c)); }
        at += *len as usize;
    }
    Some((k, l.lengths.len()))
}

/// Drawn byte at line `k`'s character `c`.
pub(crate) fn byte_at(lines: &[Line], k: usize, c: usize) -> Option<usize> {
    let l = lines.get(k)?;
    Some(l.range.start + l.lengths.iter().take(c).map(|&n| n as usize).sum::<usize>())
}

impl<S: 'static> Runtime<S> {
    /// Node `i`'s text as AccessKit sees it: the lines of a field or selectable text, and the
    /// links of any text, pushed after it as its children.
    pub(super) fn a11y_text_nodes(&self, i: usize, w: &a11y_tree::Walk, out: &mut Vec<(AkId, AkNode)>) {
        let n = &self.nodes[i];
        let Some(spec) = n.text.as_deref() else { return };
        let me = out.len() - 1;
        let runs = n.input.is_some() || (spec.selectable && !n.is_secure());
        if runs && let Some(lines) = self.a11y_lines(i) {
            for (k, l) in lines.iter().enumerate() {
                let id = line_id(n.id, k);
                let mut run = AkNode::new(Role::TextRun);
                let value = self.a11y_drawn_text(i).map(|t| t.get(l.range.clone()).unwrap_or("").to_owned()).unwrap_or_default();
                run.set_value(value);
                run.set_bounds(AkRect {
                    x0: round(l.rect[0] + w.shift[0]), y0: round(l.rect[1] + w.shift[1]),
                    x1: round(l.rect[0] + l.rect[2] + w.shift[0]), y1: round(l.rect[1] + l.rect[3] + w.shift[1]),
                });
                run.set_character_lengths(l.lengths.clone());
                run.set_character_positions(l.positions.clone());
                run.set_character_widths(l.widths.clone());
                run.set_word_starts(l.word_starts.clone());
                out[me].1.push_child(id);
                out.push((id, run));
            }
            if let Some(sel) = self.a11y_selection(i, &lines) { out[me].1.set_text_selection(sel); }
        }
        if spec.links.is_empty() || spec.redacted { return; }
        let boxes = self.link_boxes(i, self.drawn_box(i));
        for (k, l) in spec.links.iter().enumerate() {
            let Some(r) = boxes.iter().filter(|(j, _)| *j == k).fold(None, |u: Option<[f32; 4]>, (_, r)| {
                Some(u.map_or(*r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]))
            }) else { continue };
            let role = crate::a11y::roles::map(l.role, &crate::a11y::roles::RoleCtx {
                pressable: true, input: false, multiline: false, secure: false, scrolls: false, modal: false,
                default_button: false, labelled: true, keyed: true, has_text: true, image: false,
            }).unwrap_or(Role::Link);
            let mut link = AkNode::new(role);
            let words = spec.source_slice(l.start, l.end).unwrap_or("");
            link.set_label(l.label.as_deref().unwrap_or(words));
            link.set_author_id(l.id.as_str());
            if role == Role::Link && l.id.contains("://") { link.set_url(l.id.as_str()); }
            link.set_bounds(AkRect {
                x0: round(r[0] + w.shift[0]), y0: round(r[1] + w.shift[1]),
                x1: round(r[2] + w.shift[0]), y1: round(r[3] + w.shift[1]),
            });
            if self.allowed(i) {
                if spec.on_link.is_some() { link.add_action(Ak::Click); }
                if self.can_focus(i) { link.add_action(Ak::Focus); }
                if spec.on_link_secondary.is_some() { link.add_action(Ak::ShowContextMenu); }
            }
            let id = link_id(n.id, k);
            out[me].1.push_child(id);
            out.push((id, link));
        }
    }

    /// What node `i` draws as text: a field's display text (its mask when secure), else its
    /// content.
    pub(super) fn a11y_drawn_text(&self, i: usize) -> Option<String> {
        let n = &self.nodes[i];
        match n.input.as_deref() {
            Some(input) => Some(input.edit.display().0),
            None => n.text.as_deref().map(|t| t.content.clone()),
        }
    }

    /// Node `i`'s laid-out lines, in points in its scroll viewport's content space (before the
    /// walk's shift).
    pub(super) fn a11y_lines(&self, i: usize) -> Option<Vec<Line>> {
        let n = &self.nodes[i];
        let (spec, layout) = (n.text.as_deref()?, n.text_layout.as_deref()?);
        if n.elided.is_some() { return None; }
        let content = self.a11y_drawn_text(i)?;
        if content.len() > RUN_MAX_BYTES { return None; }
        let s = self.env.scale;
        let r = self.drawn[i];
        let (x0, x1) = (r[0] + n.inset[0], r[0] + r[2] - n.inset[2]);
        let mut x = text_x(spec, x0, x1, layout.full_width() / s, crate::el::Flow::of_layout(layout));
        if let Some(input) = n.input.as_deref() && input.single_line {
            x -= self.input_scroll.get(&n.path).copied().unwrap_or(0.0);
        }
        Some(lines(layout, &content, s, [x, r[1] + n.inset[1]]))
    }

    /// A field's caret or selection, in its lines.
    fn a11y_selection(&self, i: usize, lines: &[Line]) -> Option<TextSelection> {
        let n = &self.nodes[i];
        let input = n.input.as_deref()?;
        let edit = &input.edit;
        let (anchor, focus) = if edit.preedit().is_some() {
            let caret = caret_index(edit, &self.a11y_drawn_text(i)?);
            (caret, caret)
        } else {
            (edit.display_index(edit.anchor), edit.display_index(edit.focus))
        };
        let at = |byte: usize| position(lines, byte).map(|(k, c)| TextPosition { node: line_id(n.id, k), character_index: c });
        Some(TextSelection { anchor: at(anchor)?, focus: at(focus)? })
    }

    /// The value and state a text field adds to its node.
    pub(super) fn a11y_input(&self, _i: usize, input: &crate::el::InputSpec, node: &mut AkNode) {
        if !input.placeholder.is_empty() { node.set_placeholder(input.placeholder.as_str()); }
        if !input.edit.is_secure() { node.set_value(input.edit.text.as_str()); }
        if input.access.is_none() { node.set_read_only(); }
    }
}
