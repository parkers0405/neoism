//! Cmd+A with nothing selected yet. A read-only selectable text has no selection to hold the
//! key, so it goes to the group the user last clicked or selected in, else the window's only
//! selectable group, as a click makes an AppKit text view the first responder.
use super::*;

/// The group (or ungrouped text) the selection last touched, by path.
#[derive(Default)]
pub(super) struct Pick(Option<String>);

impl<S: 'static> Runtime<S> {
    /// The selection group of text `i`, or the text itself when it's in none.
    fn select_unit(&self, i: usize) -> usize {
        self.group_of(i).unwrap_or(i)
    }

    /// Remembers the unit of the text holding the selection.
    pub(super) fn remember_pick(&mut self) {
        let unit = self.selection_holder_end().map(|h| {
            self.end_group_path(h).unwrap_or_else(|| self.end_path(h)).to_owned()
        });
        if unit.is_some() {
            self.text_sel.pick.0 = unit;
        }
    }

    /// Where Cmd+A goes: the selection's group, else the last picked while it's mounted, else
    /// the only one in the window.
    fn select_all_target(&self) -> Option<usize> {
        if let Some(h) = self.selection_holder_end() {
            return match h {
                selectable::virtual_selection::EndRef::Mounted(i) => Some(self.select_unit(i)),
                selectable::virtual_selection::EndRef::Offscreen(..) => {
                    self.end_group_path(h).and_then(|gp| self.live(gp))
                }
            };
        }
        let live = |i: usize| self.is_selectable(i) && !self.subtree_exiting(i) && self.allowed(i);
        let picked = self.text_sel.pick.0.as_deref().and_then(|p| self.live(p)).filter(|&u| {
            if self.nodes[u].selection_group { (u + 1..self.nodes.len()).any(|j| self.member(j, u)) } else { live(u) }
        });
        if picked.is_some() {
            return picked;
        }
        let mut units = (0..self.nodes.len()).filter(|&i| live(i)).map(|i| self.select_unit(i));
        let first = units.next()?;
        units.all(|u| u == first).then_some(first)
    }

    /// Cmd+A when nothing else took it: selects all of `select_all_target`.
    pub(super) fn select_all_key(&mut self) -> bool {
        if let Some(h @ selectable::virtual_selection::EndRef::Offscreen(vi, _)) = self.selection_holder_end() {
            match self.end_group_path(h).map(str::to_owned) {
                None => {
                    let path = self.end_path(h).to_owned();
                    self.select_span_ends(h, h);
                    log_event!(self, "select all {path}");
                    return true;
                }
                Some(gp) if self.live(&gp).is_none() => {
                    let texts = &self.virtuals.iter().nth(vi).unwrap().1.selection.texts;
                    let mut members = texts.iter().enumerate()
                        .filter(|(_, t)| t.group_path.as_deref() == Some(gp.as_str()))
                        .map(|(ti, _)| selectable::virtual_selection::EndRef::Offscreen(vi, ti));
                    let Some(first) = members.next() else { return false };
                    let last = members.last().unwrap_or(first);
                    self.select_span_ends(first, last);
                    log_event!(self, "select all {gp}");
                    return true;
                }
                Some(_) => {}
            }
        }
        let Some(u) = self.select_all_target() else { return false };
        if self.nodes[u].selection_group
            && let Some((first, last)) = self.virtual_span_under(u)
        {
            let path = self.nodes[u].path.clone();
            self.select_span_ends(first, last);
            log_event!(self, "select all {path}");
            return true;
        }
        let (first, last) = if self.nodes[u].selection_group {
            let texts = self.group_texts(u);
            let (Some(&first), Some(&last)) = (texts.first(), texts.last()) else { return false };
            (first, last)
        } else {
            (u, u)
        };
        self.select_span(first, last);
        log_event!(self, "select all {}", self.nodes[u].path);
        true
    }
}
