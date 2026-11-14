//! What assistive technology asks of an element (AccessKit action requests), run through the
//! handlers a pointer or keyboard user reaches: a click presses it, an increment steps it as
//! an arrow key does, a new value goes through the field's editor and its change handler.

use accesskit::{Action as Ak, ActionData, ActionRequest, ScrollUnit};

use super::*;
use super::a11y_tree::{ROOT, split_id, sub_link};

/// An action request resolved against the tree it named, run at the next frame.
#[derive(Debug)]
pub(super) enum A11yAct {
    /// Focus the element, or one of its text's links.
    Focus(Option<usize>),
    Step(i32),
    SetValue(String),
    Replace(String),
    /// Anchor and focus, in the field's display bytes.
    Select(usize, usize),
    ScrollIntoView,
    /// Pages (or, with `false`, lines) down and right.
    Scroll { x: i32, y: i32, pages: bool },
    ScrollTo(f32, f32),
}

/// One scroll "item" in points: about a line of body text.
const LINE: f32 = 40.0;

impl<S: 'static> Runtime<S> {
    /// Queues what `request` asks for its frame, if it names an element that can do it. Returns
    /// whether it queued anything (the host then draws a frame).
    pub(crate) fn a11y_action(&mut self, request: &ActionRequest) -> bool {
        if request.target_node == ROOT {
            return false;
        }
        let (id, sub) = split_id(request.target_node);
        let Some(i) = self.nodes.iter().position(|n| n.id == id) else { return false };
        if !self.allowed(i) || self.subtree_exiting(i) { return false; }
        let n = &self.nodes[i];
        let path = n.path.clone();
        let link = sub_link(sub);
        let handlers = n.handlers.get();
        let pressable = handlers.is_some_and(|h| h.on_press.is_some());
        let secondary = handlers.is_some_and(|h| h.on_secondary_press.is_some());
        let (input, editable) = (n.input.is_some(), n.input.as_ref().is_some_and(|i| i.access.is_some()));
        let (expanded, steps, scrolls) = (n.a11y.expanded, n.focus.step.is_some(), n.scroll.is_some());
        let focusable = self.can_focus(i);
        let action = match (request.action, link) {
            (Ak::Click, Some(k)) => Action::Link(path, k),
            (Ak::ShowContextMenu, Some(k)) => {
                let Some((_, r)) = self.link_boxes(i, self.drawn_box(i)).into_iter().find(|(j, _)| *j == k) else { return false };
                Action::LinkSecondary(path, k, self.pointer_event(i, r[0], r[3]))
            }
            (Ak::Focus, Some(k)) if focusable => Action::A11y(path, A11yAct::Focus(Some(k))),
            (Ak::Click, None) if pressable => Action::Press(path, crate::PointerButton::Primary),
            (Ak::Click, None) if input => Action::A11y(path, A11yAct::Focus(None)),
            (Ak::Focus, None) if focusable => Action::A11y(path, A11yAct::Focus(None)),
            (Ak::Expand | Ak::Collapse, None) if pressable && expanded == Some(request.action == Ak::Collapse) => {
                Action::Press(path, crate::PointerButton::Primary)
            }
            (Ak::ShowContextMenu, None) if secondary => {
                let r = self.drawn_box(i);
                Action::SecondaryPress(path, self.pointer_event(i, r[0], r[3]))
            }
            (Ak::Increment, None) if steps => Action::A11y(path, A11yAct::Step(1)),
            (Ak::Decrement, None) if steps => Action::A11y(path, A11yAct::Step(-1)),
            (Ak::SetValue, None) if editable => match &request.data {
                Some(ActionData::Value(v)) => Action::A11y(path, A11yAct::SetValue(v.to_string())),
                _ => return false,
            },
            // A slider with a stated step takes a number as that many steps from its value.
            (Ak::SetValue, None) if steps => {
                let Some(ActionData::NumericValue(target)) = request.data else { return false };
                let Some(delta) = n.a11y.more.as_deref().and_then(|m| m.range).and_then(|r| {
                    let step = r.step.filter(|s| *s > 0.0)?;
                    let target = target.clamp(r.min.min(r.max), r.max.max(r.min));
                    Some(((target - r.value) / step).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
                }) else { return false };
                if delta == 0 { return false; }
                Action::A11y(path, A11yAct::Step(delta))
            }
            (Ak::ReplaceSelectedText, None) if editable => match &request.data {
                Some(ActionData::Value(v)) => Action::A11y(path, A11yAct::Replace(v.to_string())),
                _ => return false,
            },
            (Ak::SetTextSelection, None) if editable => {
                let Some(ActionData::SetTextSelection(sel)) = &request.data else { return false };
                let lines = self.a11y_lines(i).unwrap_or_default();
                let byte = |p: accesskit::TextPosition| {
                    let (pid, line) = split_id(p.node);
                    if pid != id || line == 0 { return None; }
                    super::a11y_text::byte_at(&lines, line as usize - 1, p.character_index)
                };
                let (Some(a), Some(b)) = (byte(sel.anchor), byte(sel.focus)) else { return false };
                Action::A11y(path, A11yAct::Select(a, b))
            }
            (Ak::ScrollIntoView, _) => Action::A11y(path, A11yAct::ScrollIntoView),
            (Ak::ScrollUp | Ak::ScrollDown | Ak::ScrollLeft | Ak::ScrollRight, None) if scrolls => {
                let pages = !matches!(request.data, Some(ActionData::ScrollUnit(ScrollUnit::Item)));
                let (x, y) = match request.action {
                    Ak::ScrollUp => (0, -1),
                    Ak::ScrollDown => (0, 1),
                    Ak::ScrollLeft => (-1, 0),
                    _ => (1, 0),
                };
                Action::A11y(path, A11yAct::Scroll { x, y, pages })
            }
            (Ak::SetScrollOffset, None) if scrolls => match request.data {
                Some(ActionData::SetScrollOffset(p)) => Action::A11y(path, A11yAct::ScrollTo(p.x as f32, p.y as f32)),
                _ => return false,
            },
            _ => return false,
        };
        self.queue.push(action);
        true
    }

    /// Asks what an assistive client would of the element (or inline link) whose key is `key`,
    /// for tests: it runs at the next frame, through the same handlers pointer and keyboard
    /// input reach. False when nothing keyed so supports `action`.
    pub fn accessibility_action(&mut self, key: &str, action: accesskit::Action, data: Option<ActionData>) -> bool {
        let tree = self.accessibility_tree();
        let Some(target_node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).map(|(id, _)| *id) else { return false };
        self.a11y_action(&ActionRequest { action, target_tree: accesskit::TreeId::ROOT, target_node, data })
    }

    /// Runs a queued accessibility action in its frame.
    pub(super) fn run_a11y(&mut self, path: String, act: A11yAct, tx: &mut Tx) -> bool {
        match act {
            A11yAct::Focus(link) => {
                log_event!(self, "a11y focus {path}");
                self.set_focus(Some(path), true);
                if let Some(k) = link { self.set_link_focus(k); }
                true
            }
            A11yAct::Step(delta) => {
                let Some(f) = self.node(&path).and_then(|n| n.focus.step.clone()) else { return false };
                log_event!(self, "a11y step {delta} {path}");
                f(states!(self), delta, tx);
                true
            }
            A11yAct::SetValue(value) => self.a11y_edit(path, tx, move |e| {
                let len = e.text.len();
                e.set_selection(0, len);
                e.paste(&value)
            }),
            A11yAct::Replace(value) => self.a11y_edit(path, tx, move |e| e.paste(&value)),
            A11yAct::Select(a, b) => self.a11y_edit(path, tx, move |e| {
                let (a, b) = (e.model_index(a), e.model_index(b));
                e.set_selection(a, b);
                crate::edit::EditOutcome::default()
            }),
            A11yAct::ScrollIntoView => {
                tx.scroll_into_view(path);
                true
            }
            A11yAct::Scroll { x, y, pages } => {
                let Some(i) = self.nodes.iter().position(|n| n.path == path) else { return false };
                let Some(st) = self.scrolls.get(&self.nodes[i].id) else { return false };
                if pages && x == 0 {
                    tx.scroll_page(path, y);
                } else {
                    let r = self.drawn[i];
                    let (dx, dy) = if pages { (r[2] * 0.9, r[3] * 0.9) } else { (LINE, LINE) };
                    let nx = st.x.offset as f32 + dx * x as f32;
                    let ny = st.offset as f32 + dy * y as f32;
                    tx.scroll_to_instant(path, nx, ny);
                }
                true
            }
            A11yAct::ScrollTo(x, y) => {
                tx.scroll_to_instant(path, x, y);
                true
            }
        }
    }

    /// Focuses field `path` (as a reader focuses it first) and edits it through its editor, so
    /// its change and submit handlers run as for typing.
    fn a11y_edit(&mut self, path: String, tx: &mut Tx, f: impl FnOnce(&mut crate::edit::TextEdit) -> crate::edit::EditOutcome) -> bool {
        if self.focus.path() != Some(path.as_str()) { self.set_focus(Some(path.clone()), true); }
        log_event!(self, "a11y edit {path}");
        self.with_focused(f, tx)
    }
}
