//! Inline marks and paragraph blocks on a [`TextEdit`]: keeping them in step with the text,
//! toggling them, and list indentation.

use super::*;

impl TextEdit {
    /// Synchronize `blocks`, `soft_breaks`, and `marks` with `self.text`.
    pub(crate) fn sync_invariants(&mut self) {
        self.anchor = Self::clamp_char_boundary(&self.text, self.anchor);
        self.focus = Self::clamp_char_boundary(&self.text, self.focus);
        if self.secure {
            self.marks.clear();
            self.blocks.clear();
            self.soft_breaks.clear();
            self.pending = None;
            return;
        }
        let line_count = 1 + count_newlines(self.text.as_bytes());
        if self.blocks.len() < line_count {
            self.blocks.resize(line_count, Block::Paragraph);
        } else if self.blocks.len() > line_count {
            self.blocks.truncate(line_count);
        }
        if let Some(bits) = self.allowed_blocks {
            for b in &mut self.blocks {
                if bits & b.kind_bit() == 0 {
                    *b = Block::Paragraph;
                }
            }
        }
        self.soft_breaks.sort_unstable();
        self.soft_breaks.dedup();
        self.soft_breaks.retain(|&idx| idx > 0 && (idx as usize) < line_count);
        self.normalize_marks();
        self.snap_to_pills();
        let anchor_line = self.line_of_byte(self.anchor);
        if let Some((armed, ref mut marks)) = self.pending {
            if armed != (self.anchor, self.focus) || self.blocks.get(anchor_line) == Some(&Block::Code) {
                self.pending = None;
            } else if let Some(bits) = self.allowed_marks {
                marks.retain(|m| bits & 1 << m.kind_order() != 0);
            }
        }
    }

    pub(super) fn normalize_marks(&mut self) {
        if let Some(bits) = self.allowed_marks {
            self.marks.retain(|m| bits & 1 << m.mark.kind_order() != 0);
        }
        if self.marks.is_empty() {
            return;
        }
        let len = self.text.len();
        for m in &mut self.marks {
            m.start = Self::clamp_char_boundary(&self.text, m.start);
            m.end = Self::clamp_char_boundary(&self.text, m.end);
        }
        self.marks.retain(|m| m.start < m.end && m.end <= len);

        // Strip inline marks from any `Block::Code` paragraph.
        if self.blocks.contains(&Block::Code) {
            let mut line_start = 0;
            let mut idx = 0;
            let mut code_ranges: Vec<Range<usize>> = Vec::new();
            for (i, b) in self.text.bytes().enumerate() {
                if b == b'\n' {
                    if self.blocks.get(idx) == Some(&Block::Code) {
                        code_ranges.push(line_start..i + 1);
                    }
                    line_start = i + 1;
                    idx += 1;
                }
            }
            if self.blocks.get(idx) == Some(&Block::Code) {
                code_ranges.push(line_start..len);
            }
            // One pass: cut each mark around the (sorted, disjoint) code ranges it overlaps.
            let mut out = Vec::with_capacity(self.marks.len());
            for m in self.marks.drain(..) {
                let mut start = m.start;
                let first = code_ranges.partition_point(|cr| cr.end <= start);
                for cr in &code_ranges[first..] {
                    if cr.start >= m.end {
                        break;
                    }
                    if start < cr.start {
                        out.push(MarkRange { start, end: cr.start, mark: m.mark.clone() });
                    }
                    start = start.max(cr.end);
                }
                if start < m.end {
                    out.push(MarkRange { start, end: m.end, mark: m.mark });
                }
            }
            self.marks = out;
        }

        // Merge overlapping or adjacent ranges of the same mark kind.
        // For `Mark::Link`, later entries win over earlier overlapping links.
        let mut by_kind: [Vec<MarkRange>; 7] = Default::default();
        for m in self.marks.drain(..) {
            if m.start < m.end {
                by_kind[m.mark.kind_order() as usize].push(m);
            }
        }

        let mut merged = Vec::new();
        for (kind_idx, mut list) in by_kind.into_iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            if kind_idx == 6 {
                // `Mark::Pill`: whole objects on one line that never merge; the first of two
                // overlapping pills wins.
                list.sort_by_key(|m| (m.start, m.end));
                let mut end = 0;
                for m in list {
                    if m.start >= end && !self.text.as_bytes()[m.start..m.end].contains(&b'\n') {
                        end = m.end;
                        merged.push(m);
                    }
                }
            } else if kind_idx == 5 {
                // `Mark::Link`: resolve overlaps so later links overwrite earlier ones, then merge adjacent equal URLs.
                let mut bounds: Vec<(usize, usize)> = list.iter().map(|m| (m.start, m.end)).collect();
                bounds.sort_unstable();
                let overlapping = bounds.windows(2).any(|w| w[1].0 < w[0].1);
                let mut resolved: Vec<MarkRange> = Vec::new();
                if !overlapping {
                    resolved = std::mem::take(&mut list);
                }
                for link in list {
                    let mut next_resolved = Vec::with_capacity(resolved.len() + 1);
                    for old in resolved {
                        if old.end <= link.start || old.start >= link.end {
                            next_resolved.push(old);
                        } else {
                            if old.start < link.start {
                                next_resolved.push(MarkRange {
                                    start: old.start,
                                    end: link.start,
                                    mark: old.mark.clone(),
                                });
                            }
                            if old.end > link.end {
                                next_resolved.push(MarkRange {
                                    start: link.end,
                                    end: old.end,
                                    mark: old.mark,
                                });
                            }
                        }
                    }
                    next_resolved.push(link);
                    resolved = next_resolved;
                }
                resolved.sort_by_key(|m| (m.start, m.end));
                let mut iter = resolved.into_iter();
                if let Some(mut cur) = iter.next() {
                    for m in iter {
                        if m.mark == cur.mark && m.start <= cur.end {
                            cur.end = cur.end.max(m.end);
                        } else {
                            merged.push(cur);
                            cur = m;
                        }
                    }
                    merged.push(cur);
                }
            } else {
                list.sort_by_key(|m| (m.start, m.end));
                let mut iter = list.into_iter();
                if let Some(mut cur) = iter.next() {
                    for m in iter {
                        if m.start <= cur.end {
                            cur.end = cur.end.max(m.end);
                        } else {
                            merged.push(cur);
                            cur = m;
                        }
                    }
                    merged.push(cur);
                }
            }
        }

        merged.sort_by_key(|m| (m.start, m.end, m.mark.kind_order()));
        self.marks = merged;
    }

    /// Marks inherited from the character before `pos` according to AppKit rules:
    /// typing inside (`m.start < pos < m.end`) or at the trailing edge (`m.start < pos == m.end`)
    /// of a mark inherits it; typing at the start of a mark (`pos == m.start`) does not extend it.
    pub(super) fn inherited_marks_at(&self, pos: usize) -> Vec<Mark> {
        if self.marks.is_empty() {
            return Vec::new();
        }
        let line_idx = self.line_of_byte(pos);
        if self.blocks.get(line_idx) == Some(&Block::Code) {
            return Vec::new();
        }
        let line_start = self.line_range(line_idx).start;
        if pos <= line_start {
            return Vec::new();
        }
        let mut out = Vec::new();
        for m in &self.marks {
            if m.start < pos && pos <= m.end && !m.mark.is_pill() && !out.iter().any(|existing: &Mark| existing.same_kind(&m.mark)) {
                out.push(m.mark.clone());
            }
        }
        out
    }

    /// Whether `mark` covers the entire byte range `start..end` (`start < end`).
    fn range_has_mark(&self, start: usize, end: usize, mark: &Mark) -> bool {
        if start >= end {
            return false;
        }
        // Because `normalize_marks` merges contiguous ranges of the same mark, a mark covers
        // the entire `start..end` iff a single `MarkRange` with `m.start <= start && end <= m.end` exists
        // (for `Link("")`, any link covering each subrange, or a single link covering `start..end`).
        let match_any_link = matches!(mark, Mark::Link(u) if u.is_empty());
        self.marks.iter().any(|m| {
            let kind_matches = if match_any_link {
                matches!(m.mark, Mark::Link(_))
            } else {
                &m.mark == mark
            };
            kind_matches && m.start <= start && end <= m.end
        })
    }

    /// Active inline marks at the caret (including pending typing style) or covering the whole
    /// selection, for driving a formatting bar's pressed state.
    pub fn active_marks(&self) -> Vec<Mark> {
        let (a, b) = self.range();
        if a == b {
            let line_idx = self.line_of_byte(a);
            if self.blocks.get(line_idx) == Some(&Block::Code) {
                return Vec::new();
            }
            if let Some((armed, ref p)) = self.pending
                && armed == (self.anchor, self.focus)
            {
                return p.clone();
            }
            return self.inherited_marks_at(a);
        }
        let mut out = Vec::new();
        for m in &self.marks {
            if m.start <= a && b <= m.end && !out.iter().any(|existing: &Mark| existing == &m.mark) {
                out.push(m.mark.clone());
            }
        }
        out
    }

    /// Whether `mark` is active at the caret or covers the whole selection.
    /// Passing `Mark::Link(String::new())` checks whether *any* link is active.
    pub fn has_mark(&self, mark: &Mark) -> bool {
        let match_any_link = matches!(mark, Mark::Link(u) if u.is_empty());
        self.active_marks().iter().any(|m| {
            if match_any_link {
                matches!(m, Mark::Link(_))
            } else {
                m == mark
            }
        })
    }

    /// The URL of the active [`Mark::Link`] at the caret or covering the selection, if any.
    pub fn active_link(&self) -> Option<&str> {
        let (a, b) = self.range();
        if a == b {
            if let Some((armed, ref p)) = self.pending
                && armed == (self.anchor, self.focus)
            {
                return p.iter().find_map(Mark::link_url);
            }
            let line_idx = self.line_of_byte(a);
            if self.blocks.get(line_idx) == Some(&Block::Code) || a <= self.line_range(line_idx).start {
                return None;
            }
            return self
                .marks
                .iter()
                .find_map(|m| (m.start < a && a <= m.end).then(|| m.mark.link_url()).flatten());
        }
        self.marks
            .iter()
            .find_map(|m| (m.start <= a && b <= m.end).then(|| m.mark.link_url()).flatten())
    }

    fn edit_pending_marks(&mut self, pos: usize, f: impl FnOnce(&mut Vec<Mark>)) {
        if self.blocks.get(self.line_of_byte(pos)) == Some(&Block::Code) {
            return;
        }
        let mut active = self.active_marks();
        f(&mut active);
        self.pending = Some(((self.anchor, self.focus), active));
    }

    /// Toggle `mark` on the selection, or in the pending typing style when the selection is empty.
    /// Inside a [`Block::Code`] paragraph, inline marks do not apply, nor does a mark the field
    /// doesn't [allow](Self::allow_marks).
    pub fn toggle_mark(&mut self, mark: Mark) {
        if self.secure || !self.allows_mark(&mark) {
            return;
        }
        self.sync_invariants();
        let (a, b) = self.range();
        if a == b {
            if !mark.is_pill() {
                self.edit_pending_marks(a, |active| {
                    if let Some(pos) = active.iter().position(|m| m.same_kind(&mark)) {
                        if active[pos] == mark || matches!(mark, Mark::Link(ref u) if u.is_empty()) {
                            active.remove(pos);
                        } else {
                            active[pos] = mark;
                        }
                    } else {
                        active.push(mark);
                    }
                });
            }
            return;
        }
        if self.range_has_mark(a, b, &mark) {
            self.remove_mark(&mark);
        } else {
            self.set_mark(mark);
        }
    }

    /// Apply `mark` to the current selection (or add it to pending typing style when empty).
    pub fn set_mark(&mut self, mark: Mark) {
        if self.secure || !self.allows_mark(&mark) {
            return;
        }
        self.sync_invariants();
        let (a, b) = self.range();
        if a == b {
            if !mark.is_pill() {
                self.edit_pending_marks(a, |active| {
                    if let Some(pos) = active.iter().position(|m| m.same_kind(&mark)) {
                        active[pos] = mark;
                    } else {
                        active.push(mark);
                    }
                });
            }
            return;
        }
        self.break_history_group();
        let before = self.snapshot();
        if mark.is_pill() {
            self.marks.retain(|m| !m.mark.is_pill() || m.end <= a || m.start >= b);
        }
        self.marks.push(MarkRange { start: a, end: b, mark });
        self.pending = None;
        self.normalize_marks();
        self.record_change(before, EditKind::Other);
    }

    /// Remove `mark` (matching by [`Mark::same_kind`]) from the current selection, or from pending
    /// typing style when the selection is empty.
    pub fn remove_mark(&mut self, mark: &Mark) {
        if self.secure {
            return;
        }
        self.sync_invariants();
        let (a, b) = self.range();
        if a == b {
            let mut active = self.active_marks();
            active.retain(|m| !m.same_kind(mark));
            self.pending = Some(((self.anchor, self.focus), active));
            return;
        }
        self.break_history_group();
        let before = self.snapshot();
        let mut out = Vec::with_capacity(self.marks.len());
        for m in self.marks.drain(..) {
            if !m.mark.same_kind(mark) || m.end <= a || m.start >= b {
                out.push(m);
            } else if m.mark.is_pill() {
                // A pill goes whole: its label stays as plain text.
            } else {
                if m.start < a {
                    out.push(MarkRange { start: m.start, end: a, mark: m.mark.clone() });
                }
                if m.end > b {
                    out.push(MarkRange { start: b, end: m.end, mark: m.mark });
                }
            }
        }
        self.marks = out;
        self.pending = None;
        self.normalize_marks();
        self.record_change(before, EditKind::Other);
    }

    /// Apply or remove a [`Mark::Link`] on the selection (or pending style when empty).
    /// Passing `None` or `Some("")` removes any link on the selection.
    pub fn set_link(&mut self, url: Option<&str>) {
        match url.map(str::trim).filter(|u| !u.is_empty()) {
            Some(u) => self.set_mark(Mark::Link(u.to_owned())),
            None => self.remove_mark(&Mark::Link(String::new())),
        }
    }

    /// Line index range `first_line..=last_line` covered by the current selection.
    fn selected_line_range(&self) -> (usize, usize) {
        let (a, b) = self.range();
        let first = self.line_of_byte(a);
        let last = if b > a && self.text.as_bytes().get(b - 1) == Some(&b'\n') {
            self.line_of_byte(b - 1)
        } else {
            self.line_of_byte(b)
        };
        (first, last.max(first))
    }

    /// The [`Block`] attribute of the paragraph at the caret or selection.
    pub fn active_block(&self) -> Block {
        let (first, _) = self.selected_line_range();
        self.blocks.get(first).copied().unwrap_or(Block::Paragraph)
    }

    /// Alias for [`TextEdit::active_block`]: the [`Block`] attribute of the paragraph at the caret.
    pub fn current_block(&self) -> Block {
        self.active_block()
    }

    /// Whether any paragraph in the current selection is a bullet or numbered list item.
    pub fn selection_in_list(&self) -> bool {
        let (first, last) = self.selected_line_range();
        (first..=last).any(|i| self.blocks.get(i).copied().unwrap_or(Block::Paragraph).is_list())
    }

    /// Set the [`Block`] attribute for all paragraphs in the current selection, unless the field
    /// doesn't [allow](Self::allow_blocks) it.
    pub fn set_block(&mut self, block: Block) {
        if self.secure || self.is_single_line() || !self.allows_block(block) {
            return;
        }
        let block = block.with_level(block.list_level().unwrap_or(0));
        self.sync_invariants();
        self.break_history_group();
        let before = self.snapshot();
        let (first, last) = self.selected_line_range();
        for i in first..=last {
            if let Some(slot) = self.blocks.get_mut(i) {
                *slot = block;
            }
        }
        if block == Block::Code {
            self.pending = None;
        }
        self.normalize_marks();
        self.record_change(before, EditKind::Other);
    }

    /// Toggle `block` on all paragraphs in the selection: if every selected paragraph already has
    /// the same block kind as `block`, resets them to [`Block::Paragraph`]; otherwise sets `block`
    /// (preserving list nesting level when switching between bullet and numbered lists).
    pub fn toggle_block(&mut self, block: Block) {
        if self.secure || self.is_single_line() || !self.allows_block(block) {
            return;
        }
        self.sync_invariants();
        let (first, last) = self.selected_line_range();
        let all_match = (first..=last).all(|i| {
            self.blocks.get(i).copied().unwrap_or(Block::Paragraph).same_kind(block)
        });
        if all_match {
            self.set_block(Block::Paragraph);
        } else {
            self.break_history_group();
            let before = self.snapshot();
            for i in first..=last {
                if let Some(slot) = self.blocks.get_mut(i) {
                    *slot = match (slot.list_level(), block) {
                        (Some(lvl), Block::Bullet(0)) => Block::Bullet(lvl),
                        (Some(lvl), Block::Numbered(0)) => Block::Numbered(lvl),
                        _ => block,
                    };
                }
            }
            if block == Block::Code {
                self.pending = None;
            }
            self.normalize_marks();
            self.record_change(before, EditKind::Other);
        }
    }

    /// Increase list nesting level (`Tab`) for list items in the selection, up to [`MAX_LIST_LEVEL`].
    /// Returns `true` if the selection was inside a list item.
    pub fn indent_list(&mut self) -> bool {
        if self.secure || self.is_single_line() {
            return false;
        }
        self.sync_invariants();
        if !self.selection_in_list() {
            return false;
        }
        self.break_history_group();
        let before = self.snapshot();
        let (first, last) = self.selected_line_range();
        for i in first..=last {
            if let Some(slot) = self.blocks.get_mut(i)
                && let Some(lvl) = slot.list_level()
            {
                *slot = slot.with_level((lvl + 1).min(MAX_LIST_LEVEL));
            }
        }
        self.record_change(before, EditKind::Other);
        true
    }

    /// Decrease list nesting level (`Shift+Tab`) for list items in the selection.
    /// Returns `true` if the selection was inside a list item.
    pub fn outdent_list(&mut self) -> bool {
        if self.secure || self.is_single_line() {
            return false;
        }
        self.sync_invariants();
        if !self.selection_in_list() {
            return false;
        }
        self.break_history_group();
        let before = self.snapshot();
        let (first, last) = self.selected_line_range();
        for i in first..=last {
            if let Some(slot) = self.blocks.get_mut(i)
                && let Some(lvl) = slot.list_level()
            {
                *slot = slot.with_level(lvl.saturating_sub(1));
            }
        }
        self.record_change(before, EditKind::Other);
        true
    }
}
