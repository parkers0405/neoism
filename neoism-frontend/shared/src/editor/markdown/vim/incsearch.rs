//! Native in-buffer `/` search for the markdown pane, sourcing the
//! **shared command-palette Search modal** (identical UI to the code
//! editor's `/`).
//!
//! The nvim-backed code editor answers the palette's Search mode from the
//! managed nvim (`rio.search` lua). Markdown panes run neoism's own vim
//! engine, so the host drives the *same* palette modal from here instead:
//!
//! - `search_begin` snapshots the pre-search view (Esc restores it).
//! - `search_scan` returns every match as `(lnum, col, text)` so the host
//!   can populate the palette's `buffer_matches` + `[cur/total]` count,
//!   ordered nearest-first so the auto-selected row jumps forward from the
//!   cursor (mirrors the lua `rotate_to_nearest`).
//! - `search_preview` jumps the cursor to the selected match (highlighted
//!   in the buffer behind the modal); `search_commit` lands it and hands
//!   the pattern to the `n`/`N` engine; `search_cancel` restores.
//!
//! Transient state lives in [`VimState::incsearch`] so no `MarkdownPane`
//! constructor changes; the committed pattern lands in [`VimState::search`]
//! where `n`/`N` already read it.

use super::*;

impl MarkdownPane {
    /// `true` while a `/` search session is live (palette open, matches
    /// highlighted in the buffer behind it).
    pub fn search_active(&self) -> bool {
        self.vim.incsearch.is_some()
    }

    /// Whether the open session searches backward (`?`).
    pub fn search_reverse(&self) -> bool {
        self.vim
            .incsearch
            .as_ref()
            .map(|s| s.reverse)
            .unwrap_or(false)
    }

    /// Open a `/` (forward) or `?` (backward) session, snapshotting the
    /// current view so a cancel restores it. The palette owns the query
    /// input; this only tracks the buffer-side origin + highlight state.
    pub fn search_begin(&mut self, reverse: bool) {
        self.vim.clear_pending();
        self.vim.incsearch = Some(MarkdownIncSearch {
            query: String::new(),
            reverse,
            origin_line: self.cursor_line,
            origin_col: self.cursor_col,
            origin_scroll_y: self.scroll_y,
            origin_target_scroll_y: self.target_scroll_y,
            matches: Vec::new(),
            current: usize::MAX,
        });
    }

    /// Re-scan the buffer for `query` and return every match as
    /// `(lnum, col, text)` (1-based line/col, nearest-first) for the
    /// palette's `buffer_matches`. Also records the match positions for
    /// the in-buffer highlight.
    pub fn search_scan(&mut self, query: &str) -> Vec<(u64, u64, String)> {
        if self.vim.incsearch.is_none() {
            self.search_begin(false);
        }
        let (oline, ocol, reverse) = self
            .vim
            .incsearch
            .as_ref()
            .map(|s| (s.origin_line, s.origin_col, s.reverse))
            .unwrap_or((self.cursor_line, self.cursor_col, false));

        // Every non-overlapping occurrence, case-sensitive, file order —
        // consistent with the `*`/`n` engine. `match_indices` keeps byte
        // offsets on char boundaries.
        let mut matches: Vec<(usize, usize)> = Vec::new();
        if !query.is_empty() {
            'outer: for (li, line) in self.lines.iter().enumerate() {
                for (col, _) in line.match_indices(query) {
                    matches.push((li, col));
                    if matches.len() >= 5000 {
                        break 'outer;
                    }
                }
            }
        }
        // Rotate so the nearest match (at/after the cursor, or at/before
        // for `?`) is first — the palette auto-selects row 0, giving the
        // same forward-jump feel as nvim `/`.
        if matches.len() > 1 {
            let pivot = if reverse {
                nearest_before(&matches, oline, ocol)
            } else {
                nearest_after(&matches, oline, ocol)
            };
            matches.rotate_left(pivot);
        }

        let out: Vec<(u64, u64, String)> = matches
            .iter()
            .map(|(li, col)| {
                let text = self
                    .lines
                    .get(*li)
                    .map(|line| display_search_line(line))
                    .unwrap_or_default();
                ((*li as u64) + 1, (*col as u64) + 1, text)
            })
            .collect();

        if let Some(s) = self.vim.incsearch.as_mut() {
            s.query = query.to_string();
            s.matches = matches;
            s.current = usize::MAX;
        }
        // Emptying the pattern returns the cursor to where the search
        // started (nvim incsearch behaviour); the scroll stays put.
        if query.is_empty() {
            if let Some((line, col)) = self
                .vim
                .incsearch
                .as_ref()
                .map(|s| (s.origin_line, s.origin_col))
            {
                let line = line.min(self.lines.len().saturating_sub(1));
                self.cursor_line = line;
                self.cursor_col = col.min(self.lines.get(line).map(String::len).unwrap_or(0));
                self.follow_cursor = true;
            }
        }
        out
    }

    /// Preview the palette-selected match: jump the cursor to it and mark
    /// it the focused (brighter) highlight. `lnum`/`col` are 1-based, as
    /// stored in the palette's `buffer_matches`.
    pub fn search_preview(&mut self, lnum: u64, col: u64) {
        let line = lnum.saturating_sub(1) as usize;
        let col0 = col.saturating_sub(1) as usize;
        if let Some(s) = self.vim.incsearch.as_mut() {
            s.current = s
                .matches
                .iter()
                .position(|(l, c)| *l == line && *c == col0)
                .unwrap_or(usize::MAX);
        }
        let line = line.min(self.lines.len().saturating_sub(1));
        let col0 = col0.min(self.lines.get(line).map(String::len).unwrap_or(0));
        self.cursor_line = line;
        self.cursor_col = col0;
        self.follow_cursor = true;
    }

    /// Enter: land the cursor on `(lnum, col)`, hand the pattern to the
    /// `n`/`N` engine, and end the session (highlight clears; `n`/`N`
    /// keep working off the committed pattern).
    pub fn search_commit(&mut self, lnum: u64, col: u64) {
        let (query, reverse) = self
            .vim
            .incsearch
            .as_ref()
            .map(|s| (s.query.clone(), s.reverse))
            .unwrap_or_default();
        let line =
            (lnum.saturating_sub(1) as usize).min(self.lines.len().saturating_sub(1));
        let col0 = (col.saturating_sub(1) as usize)
            .min(self.lines.get(line).map(String::len).unwrap_or(0));
        self.cursor_line = line;
        self.cursor_col = col0;
        self.follow_cursor = true;
        if !query.is_empty() {
            self.vim.search = Some(VimSearch {
                pattern: query,
                forward: !reverse,
                whole_word: false,
            });
        }
        self.vim.incsearch = None;
    }

    /// Esc: restore the pre-search cursor + scroll and end the session.
    pub fn search_cancel(&mut self) {
        if self.vim.incsearch.is_none() {
            return;
        }
        self.search_restore_origin_view();
        self.vim.incsearch = None;
    }

    /// Jump to the next/prev committed match (`n`/`N`). Used by the web
    /// mini-handler, which doesn't route through the full `VimAction`
    /// engine.
    pub fn search_repeat(&mut self, reverse: bool) -> bool {
        let Some(search) = self.vim.search.clone() else {
            return false;
        };
        let forward = search.forward != reverse;
        let pos = self.cursor_position();
        let next = if forward {
            vim_search_forward(&self.lines, pos, &search.pattern, search.whole_word)
        } else {
            vim_search_backward(&self.lines, pos, &search.pattern, search.whole_word)
        };
        let Some(next) = next else {
            return false;
        };
        self.cursor_line = next.line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = next
            .col
            .min(self.lines.get(self.cursor_line).map(String::len).unwrap_or(0));
        self.follow_cursor = true;
        true
    }

    /// Match ranges on `line_ix` for the renderer: `(start_byte, end_byte,
    /// is_current)`. The focused (selected) match reports `true` so it
    /// paints brighter (current-match accent vs. the dimmer all-match
    /// wash) — the same split nvim draws behind the palette.
    pub fn search_matches_for_line(&self, line_ix: usize) -> Vec<(usize, usize, bool)> {
        let Some(s) = self.vim.incsearch.as_ref() else {
            return Vec::new();
        };
        if s.query.is_empty() {
            return Vec::new();
        }
        let qlen = s.query.len();
        s.matches
            .iter()
            .enumerate()
            .filter(|(_, (li, _))| *li == line_ix)
            .map(|(ix, (_, col))| (*col, col + qlen, ix == s.current))
            .collect()
    }

    fn search_restore_origin_view(&mut self) {
        let Some(s) = self.vim.incsearch.as_ref() else {
            return;
        };
        let line = s.origin_line.min(self.lines.len().saturating_sub(1));
        let col = s
            .origin_col
            .min(self.lines.get(line).map(String::len).unwrap_or(0));
        let scroll_y = s.origin_scroll_y;
        let target_scroll_y = s.origin_target_scroll_y;
        self.cursor_line = line;
        self.cursor_col = col;
        self.scroll_y = scroll_y;
        self.target_scroll_y = target_scroll_y;
        // View already restored — don't let the follow-cursor reveal drag
        // the scroll somewhere else this frame.
        self.follow_cursor = false;
    }
}

/// Trim/normalise a buffer line for the palette row: tabs → spaces (so the
/// proportional font aligns) and clamp so a 5kB line doesn't bloat the
/// list. Mirrors the lua `display_line`.
fn display_search_line(line: &str) -> String {
    let mut s = line.replace('\t', "    ");
    if s.len() > 200 {
        let mut end = 197;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str("...");
    }
    s
}

/// Index of the first match at/after `(oline, ocol)`, wrapping to the
/// first match when every occurrence sits before the cursor.
fn nearest_after(matches: &[(usize, usize)], oline: usize, ocol: usize) -> usize {
    for (ix, (l, c)) in matches.iter().enumerate() {
        if *l > oline || (*l == oline && *c >= ocol) {
            return ix;
        }
    }
    0
}

/// Index of the last match at/before `(oline, ocol)`, wrapping to the
/// last match when every occurrence sits after the cursor.
fn nearest_before(matches: &[(usize, usize)], oline: usize, ocol: usize) -> usize {
    let mut best = matches.len().saturating_sub(1);
    for (ix, (l, c)) in matches.iter().enumerate() {
        if *l < oline || (*l == oline && *c <= ocol) {
            best = ix;
        } else {
            break;
        }
    }
    best
}
