// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Input + selection handling for the palette.
//!
//! - Selection movement (`move_selection_*`, scroll wheel, scrolloff
//!   clamping).
//! - Hit-testing (mouse → row index) and hover.
//! - Read-only getters (`get_selected_*`) and the `filtered_rows`
//!   pipeline shared with rendering and tests.

use web_time::Instant;

use crate::panels::file_tree;

use super::actions::{
    HostKind, PaletteAction, PaletteBufferTarget, PaletteShaderEntry,
    PaletteWorkspaceEntry, PaletteWorkspaceTarget,
};
use super::commands::{COMMANDS, EX_COMMANDS};
use super::fuzzy::{fuzzy_score, scrolloff_for};
use super::modes::{PaletteMode, PaletteRow};
use super::state::{CommandPalette, WorkspaceDrag};
use super::{
    INPUT_HEIGHT, MAX_VISIBLE_RESULTS, PALETTE_PADDING, RESULTS_MARGIN_TOP,
    RESULTS_PADDING_BOTTOM, RESULT_ITEM_HEIGHT, SEPARATOR_HEIGHT,
    WORKSPACE_DRAG_ACTIVATION_PX,
};

impl CommandPalette {
    /// Move the selection to a clicked row, snapping past any
    /// non-selectable host header (a click on a separator selects the
    /// next selectable row instead of parking on the header). Callers
    /// that previously assigned `selected_index` directly from a click
    /// hit-test should route through here so header clicks behave.
    pub fn select_clicked(&mut self, index: usize) {
        let snapped = self
            .filtered_rows()
            .get(index)
            .map(|(_, row)| row.is_selectable())
            .unwrap_or(false)
            .then_some(index)
            .or_else(|| self.first_selectable_index(index));
        if let Some(i) = snapped {
            self.selected_index = i;
        }
    }

    pub fn move_selection_up(&mut self) {
        let rows = self.filtered_rows();
        if rows.is_empty() {
            return;
        }
        // Walk past any non-selectable host header rows so the cursor
        // never parks on a separator.
        let mut idx = self.selected_index;
        while idx > 0 {
            idx -= 1;
            if rows[idx].1.is_selectable() {
                self.move_selection_to(idx, rows.len());
                return;
            }
        }
    }

    pub fn move_selection_down(&mut self) {
        let rows = self.filtered_rows();
        let count = rows.len();
        let mut idx = self.selected_index;
        while idx + 1 < count {
            idx += 1;
            if rows[idx].1.is_selectable() {
                self.move_selection_to(idx, count);
                return;
            }
        }
    }

    /// First selectable row at or after `from`, falling back to the
    /// first selectable row anywhere (so an initial selection never
    /// lands on a host header). `None` when the list is all separators
    /// or empty.
    pub(super) fn first_selectable_index(&self, from: usize) -> Option<usize> {
        let rows = self.filtered_rows();
        rows.iter()
            .enumerate()
            .skip(from)
            .find(|(_, (_, row))| row.is_selectable())
            .map(|(i, _)| i)
            .or_else(|| rows.iter().position(|(_, row)| row.is_selectable()))
    }

    pub(super) fn move_selection_to(&mut self, new_selected: usize, count: usize) {
        if count == 0 {
            return;
        }
        let new_selected = new_selected.min(count - 1);
        if new_selected == self.selected_index {
            return;
        }

        let was_idle = self.cursor_spring.position == 0.0;
        let rows = self.selected_index as i32 - new_selected as i32;
        self.cursor_spring.position += rows as f32 * self.row_height();
        if was_idle {
            self.last_cursor_frame = Instant::now();
        }
        self.selected_index = new_selected;
        self.clamp_scroll(count);
    }

    pub(super) fn set_scroll_offset(&mut self, new_offset: usize, count: usize) {
        let max_offset = count.saturating_sub(MAX_VISIBLE_RESULTS);
        let new_offset = new_offset.min(max_offset);
        let old_offset = self.scroll_offset;
        if old_offset == new_offset {
            return;
        }

        self.scroll_offset = new_offset;
        let was_idle = self.list_scroll_spring.position == 0.0;
        let rows = new_offset as i32 - old_offset as i32;
        self.list_scroll_spring.position += rows as f32 * self.row_height();
        if was_idle {
            self.last_list_scroll_frame = Instant::now();
        }
        self.last_scroll_time = Some(Instant::now());
    }

    pub fn scroll_pixels(&mut self, delta_pixels: f32) {
        let count = self.filtered_rows().len();
        if count <= MAX_VISIBLE_RESULTS || delta_pixels == 0.0 {
            return;
        }
        let row_h = self.row_height().max(1.0);
        self.wheel_accumulator += delta_pixels;
        let mut rows = 0i32;
        while self.wheel_accumulator.abs() >= row_h {
            let sign = self.wheel_accumulator.signum();
            self.wheel_accumulator -= sign * row_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }
        if rows == 0 {
            return;
        }

        let max_offset = count.saturating_sub(MAX_VISIBLE_RESULTS);
        let next_offset = if rows < 0 {
            self.scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize)
        } else {
            self.scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset)
        };
        self.set_scroll_offset(next_offset, count);

        let visible = MAX_VISIBLE_RESULTS.min(count).max(1);
        self.clamp_selected_to_viewport(count, visible);
    }

    pub(super) fn clamp_selected_to_viewport(
        &mut self,
        count: usize,
        visible_rows: usize,
    ) {
        if count == 0 || visible_rows == 0 {
            return;
        }

        let scrolloff = scrolloff_for(visible_rows);
        let first_visible = self
            .scroll_offset
            .saturating_add(scrolloff)
            .min(count.saturating_sub(1));
        let last_visible = self
            .scroll_offset
            .saturating_add(visible_rows.saturating_sub(1).saturating_sub(scrolloff))
            .min(count.saturating_sub(1));

        let old = self.selected_index;
        if self.selected_index < first_visible {
            self.selected_index = first_visible;
        } else if self.selected_index > last_visible {
            self.selected_index = last_visible;
        }

        if self.selected_index != old {
            self.cursor_spring.reset();
        }
    }

    pub(super) fn clamp_scroll(&mut self, count: usize) {
        if count == 0 {
            self.set_scroll_offset(0, 0);
            return;
        }
        let visible_rows = MAX_VISIBLE_RESULTS.min(count).max(1);
        let scrolloff = scrolloff_for(visible_rows);
        if self.selected_index < self.scroll_offset.saturating_add(scrolloff) {
            self.set_scroll_offset(self.selected_index.saturating_sub(scrolloff), count);
        } else if self.selected_index.saturating_add(scrolloff)
            >= self.scroll_offset.saturating_add(visible_rows)
        {
            self.set_scroll_offset(
                self.selected_index + scrolloff + 1 - visible_rows,
                count,
            );
        }
        let max_offset = count.saturating_sub(visible_rows);
        if self.scroll_offset > max_offset {
            self.set_scroll_offset(max_offset, count);
        }
    }

    /// Tab-completion: replace the query with the selected row's title
    /// (so the user can refine without retyping). When there is no
    /// selection (empty list) we leave the query alone. Returns `true`
    /// when the query actually changed so the caller can short-circuit
    /// the redraw decision.
    ///
    /// Note: `set_query` resets selection / scroll, so the user lands
    /// on the (still-matching) first row of the now-narrowed list.
    pub fn tab_complete(&mut self) -> bool {
        let title = match self
            .filtered_rows()
            .get(self.selected_index)
            .map(|(_, row)| row.title().to_owned())
        {
            Some(t) => t,
            None => return false,
        };
        if title == self.query {
            return false;
        }
        self.set_query(title);
        true
    }

    pub fn get_selected_action(&self) -> Option<PaletteAction> {
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| row.action())
    }

    pub fn selected_server_id(&self) -> Option<String> {
        match self
            .filtered_rows()
            .get(self.selected_index)
            .map(|(_, row)| row)
        {
            Some(PaletteRow::Server { entry }) if !entry.local => Some(entry.id.clone()),
            _ => None,
        }
    }

    pub fn server_action_at(&self, x: f32, y: f32) -> Option<PaletteAction> {
        let contains = |rect: [f32; 4]| {
            x >= rect[0]
                && x <= rect[0] + rect[2]
                && y >= rect[1]
                && y <= rect[1] + rect[3]
        };
        if let Some((rect, id)) = self.server_edit_hit.as_ref() {
            if contains(*rect) {
                return Some(PaletteAction::EditServer { id: id.clone() });
            }
        }
        if let Some((rect, id)) = self.server_remove_hit.as_ref() {
            if contains(*rect) {
                return Some(PaletteAction::RemoveServer { id: id.clone() });
            }
        }
        None
    }

    /// Returns the buffer location of the currently-highlighted search
    /// match, if any. Used by the router to preview or commit the match.
    pub fn selected_buffer_match_location(&self) -> Option<(u64, u64)> {
        if !matches!(self.mode, PaletteMode::Search) {
            return None;
        }
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| match row {
                PaletteRow::BufferMatch { lnum, col, .. } => Some((*lnum, *col)),
                _ => None,
            })
    }

    /// Selected family name if (and only if) the palette is in fonts
    /// mode and the selection points at a valid row. Owned `String`
    /// so the caller can mutate the palette state (`set_enabled`) in
    /// the same statement without fighting the borrow checker.
    /// Selected recent-search term if (and only if) the palette is in
    /// Search mode and the highlighted row is a recent. Owned `String`
    /// so the caller can mutate the palette in the same statement.
    pub fn get_selected_search_term(&self) -> Option<String> {
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| match row {
                PaletteRow::Search { term } => Some((*term).to_owned()),
                PaletteRow::Command { .. }
                | PaletteRow::BufferMatch { .. }
                | PaletteRow::Font { .. }
                | PaletteRow::Theme { .. }
                | PaletteRow::Shader { .. }
                | PaletteRow::Buffer { .. }
                | PaletteRow::WorkspaceHost { .. }
                | PaletteRow::WorkspaceCreate
                | PaletteRow::Workspace { .. }
                | PaletteRow::Server { .. }
                | PaletteRow::ServerAdd
                | PaletteRow::ServerCreate
                | PaletteRow::Ex { .. } => None,
            })
    }

    /// Selected ex-command suggestion, if the palette is in `:` mode.
    /// This lets Enter accept the highlighted canonical command name
    /// (`ThemePicker`) instead of dispatching lowercase/partial input
    /// (`themepick`, `theme p`) the intercept table would reject.
    pub fn get_selected_ex_command(&self) -> Option<String> {
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| match row {
                PaletteRow::Ex { name, .. } => Some((*name).to_owned()),
                PaletteRow::Command { .. }
                | PaletteRow::BufferMatch { .. }
                | PaletteRow::Font { .. }
                | PaletteRow::Theme { .. }
                | PaletteRow::Shader { .. }
                | PaletteRow::Buffer { .. }
                | PaletteRow::WorkspaceHost { .. }
                | PaletteRow::WorkspaceCreate
                | PaletteRow::Workspace { .. }
                | PaletteRow::Server { .. }
                | PaletteRow::ServerAdd
                | PaletteRow::ServerCreate
                | PaletteRow::Search { .. } => None,
            })
    }

    pub fn get_selected_font(&self) -> Option<String> {
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| match row {
                PaletteRow::Font { family } => Some((*family).to_owned()),
                PaletteRow::Command { .. }
                | PaletteRow::BufferMatch { .. }
                | PaletteRow::Theme { .. }
                | PaletteRow::Shader { .. }
                | PaletteRow::Buffer { .. }
                | PaletteRow::WorkspaceHost { .. }
                | PaletteRow::WorkspaceCreate
                | PaletteRow::Workspace { .. }
                | PaletteRow::Server { .. }
                | PaletteRow::ServerAdd
                | PaletteRow::ServerCreate
                | PaletteRow::Ex { .. }
                | PaletteRow::Search { .. } => None,
            })
    }

    pub fn get_selected_theme(&self) -> Option<String> {
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| match row {
                PaletteRow::Theme { name } => Some((*name).to_owned()),
                PaletteRow::Command { .. }
                | PaletteRow::BufferMatch { .. }
                | PaletteRow::Font { .. }
                | PaletteRow::Shader { .. }
                | PaletteRow::Buffer { .. }
                | PaletteRow::WorkspaceHost { .. }
                | PaletteRow::WorkspaceCreate
                | PaletteRow::Workspace { .. }
                | PaletteRow::Server { .. }
                | PaletteRow::ServerAdd
                | PaletteRow::ServerCreate
                | PaletteRow::Ex { .. }
                | PaletteRow::Search { .. } => None,
            })
    }

    pub fn get_selected_shader(&self) -> Option<PaletteShaderEntry> {
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| match row {
                PaletteRow::Shader { entry } => Some((*entry).clone()),
                PaletteRow::Command { .. }
                | PaletteRow::BufferMatch { .. }
                | PaletteRow::Font { .. }
                | PaletteRow::Theme { .. }
                | PaletteRow::Buffer { .. }
                | PaletteRow::WorkspaceHost { .. }
                | PaletteRow::WorkspaceCreate
                | PaletteRow::Workspace { .. }
                | PaletteRow::Server { .. }
                | PaletteRow::ServerAdd
                | PaletteRow::ServerCreate
                | PaletteRow::Ex { .. }
                | PaletteRow::Search { .. } => None,
            })
    }

    pub fn get_selected_buffer_target(&self) -> Option<PaletteBufferTarget> {
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| match row {
                PaletteRow::Buffer { entry } => Some(entry.target),
                PaletteRow::Command { .. }
                | PaletteRow::BufferMatch { .. }
                | PaletteRow::Font { .. }
                | PaletteRow::Theme { .. }
                | PaletteRow::Shader { .. }
                | PaletteRow::Ex { .. }
                | PaletteRow::WorkspaceHost { .. }
                | PaletteRow::WorkspaceCreate
                | PaletteRow::Workspace { .. }
                | PaletteRow::Server { .. }
                | PaletteRow::ServerAdd
                | PaletteRow::ServerCreate
                | PaletteRow::Search { .. } => None,
            })
    }

    pub fn get_selected_workspace_target(&self) -> Option<PaletteWorkspaceTarget> {
        self.filtered_rows()
            .get(self.selected_index)
            .and_then(|(_, row)| match row {
                PaletteRow::Workspace { entry } => Some(entry.target.clone()),
                // Host headers are separators — selecting one switches
                // nothing (selection never lands here anyway).
                PaletteRow::WorkspaceHost { .. }
                | PaletteRow::WorkspaceCreate
                | PaletteRow::Server { .. }
                | PaletteRow::ServerAdd
                | PaletteRow::ServerCreate
                | PaletteRow::Command { .. }
                | PaletteRow::BufferMatch { .. }
                | PaletteRow::Font { .. }
                | PaletteRow::Theme { .. }
                | PaletteRow::Shader { .. }
                | PaletteRow::Buffer { .. }
                | PaletteRow::Ex { .. }
                | PaletteRow::Search { .. } => None,
            })
    }

    /// Filtered list of rows for the current mode. Most modes share the
    /// same fuzzy-score + descending-sort pipeline so typing behaves
    /// identically across them.
    ///
    /// `Workspaces` is the exception: it returns a host→workspace tree
    /// whose row order is *structural* (each host header immediately
    /// followed by its child workspaces), so it is built separately and
    /// returned before the sort below would scramble that ordering.
    pub(super) fn filtered_rows(&self) -> Vec<(i32, PaletteRow<'_>)> {
        if let PaletteMode::Workspaces(workspaces) = &self.mode {
            return self.grouped_workspace_rows(workspaces);
        }

        let mut results: Vec<(i32, PaletteRow<'_>)> = match &self.mode {
            PaletteMode::Commands => {
                let has_adaptive = self.has_adaptive_theme;
                let surface = self.surface;
                let workspace_visibility = self.workspace_visibility;
                COMMANDS
                    .iter()
                    .filter(|cmd| {
                        if cmd.action == PaletteAction::ToggleAppearanceTheme {
                            return has_adaptive;
                        }
                        match cmd.action {
                            PaletteAction::ShareCurrentWorkspace => {
                                return workspace_visibility
                                    == crate::panels::context_menu::WorkspaceChromeVisibility::Private;
                            }
                            // `StopSharingCurrentWorkspace` used to be gated
                            // to `!= Private`, but that visibility signal
                            // comes from the daemon workspace cache and is
                            // Private in the common case (a fresh/local
                            // workspace isn't in the cache, and a just-shared
                            // one may not have propagated yet), so the command
                            // was effectively unreachable. Make it generally
                            // visible — mirroring how ShareCurrentWorkspace is
                            // reachable in its default state — and let the
                            // execute path send the request (a no-op on the
                            // daemon when nothing is shared).
                            _ => {}
                        }
                        super::actions::command_visible_for_surface(&cmd.action, surface)
                    })
                    .filter_map(|cmd| {
                        // Search keys off the command name at the top level —
                        // the bare title (`Write File`) and the alias column —
                        // NOT the `{service}:` group prefix. Matching the
                        // namespaced form (`neoism: …`, `workspace: …`) made
                        // every command in a namespace flood the results the
                        // moment the query brushed the group word, which buried
                        // the actual command the user was typing. The service
                        // prefix is still shown on the row, just not searched.
                        // The alias column stays slightly demoted.
                        let prefix = cmd.service.prefix();
                        let title_score = fuzzy_score(&self.query, cmd.title);
                        let alias_score = fuzzy_score(&self.query, cmd.shortcut);
                        let score = match (title_score, alias_score) {
                            (Some(a), Some(b)) => a.max(b.saturating_sub(4)),
                            (Some(a), None) => a,
                            (None, Some(b)) => b.saturating_sub(4),
                            (None, None) => return None,
                        };
                        Some((
                            score,
                            PaletteRow::Command {
                                service: prefix,
                                title: cmd.title,
                                shortcut: cmd.shortcut,
                                // PaletteAction is no longer Copy (one
                                // variant carries an owned payload), so
                                // lift the catalog action by clone.
                                action: cmd.action.clone(),
                            },
                        ))
                    })
                    .collect()
            }
            PaletteMode::Fonts(fonts) => fonts
                .iter()
                .filter_map(|family| {
                    let score = fuzzy_score(&self.query, family)?;
                    Some((score, PaletteRow::Font { family }))
                })
                .collect(),
            PaletteMode::Themes(themes) => themes
                .iter()
                .filter_map(|name| {
                    let score = fuzzy_score(&self.query, name)?;
                    Some((score, PaletteRow::Theme { name }))
                })
                .collect(),
            PaletteMode::Shaders(shaders) => shaders
                .iter()
                .filter_map(|entry| {
                    let title_score = fuzzy_score(&self.query, &entry.title);
                    let detail_score = fuzzy_score(&self.query, &entry.detail);
                    let score = match (title_score, detail_score) {
                        (Some(a), Some(b)) => a.max(b.saturating_sub(4)),
                        (Some(a), None) => a,
                        (None, Some(b)) => b.saturating_sub(4),
                        (None, None) => return None,
                    };
                    Some((score, PaletteRow::Shader { entry }))
                })
                .collect(),
            PaletteMode::Buffers(buffers) => buffers
                .iter()
                .filter_map(|entry| {
                    let title_score = fuzzy_score(&self.query, &entry.title);
                    let detail_score = fuzzy_score(&self.query, &entry.detail);
                    let score = match (title_score, detail_score) {
                        (Some(a), Some(b)) => a.max(b.saturating_sub(4)),
                        (Some(a), None) => a,
                        (None, Some(b)) => b.saturating_sub(4),
                        (None, None) => return None,
                    };
                    Some((score, PaletteRow::Buffer { entry }))
                })
                .collect(),
            // Built + returned early above by `grouped_workspace_rows`
            // so the host→workspace tree keeps its structural ordering.
            PaletteMode::Workspaces(_) => unreachable!(
                "Workspaces mode is handled by the early return in filtered_rows"
            ),
            PaletteMode::Servers(servers) => {
                let mut rows = servers
                    .iter()
                    .filter_map(|entry| {
                        let name_score = fuzzy_score(&self.query, &entry.name);
                        let address_score = fuzzy_score(&self.query, &entry.address);
                        let score = match (name_score, address_score) {
                            (Some(a), Some(b)) => a.max(b.saturating_sub(4)),
                            (Some(a), None) => a,
                            (None, Some(b)) => b.saturating_sub(4),
                            (None, None) => return None,
                        };
                        Some((score, PaletteRow::Server { entry }))
                    })
                    .collect::<Vec<_>>();
                if self.query.is_empty() {
                    rows.push((i32::MIN, PaletteRow::ServerAdd));
                }
                rows
            }
            // Ex mode: fuzzy-match the *first word* of the query against
            // the curated `EX_COMMANDS` list so the user gets noice/
            // wildmenu-style live suggestions as they type. Once they
            // hit a space (i.e. they're typing arguments like a path),
            // hide suggestions — matching against arbitrary args would
            // produce noise. Enter still always dispatches the literal
            // query; suggestions exist only for visual aid + tab-fill.
            PaletteMode::Ex => {
                let first_word = self.query.split_whitespace().next().unwrap_or("");
                let typing_args = self.query.contains(char::is_whitespace);
                if typing_args {
                    Vec::new()
                } else {
                    EX_COMMANDS
                        .iter()
                        .filter_map(|(name, hint)| {
                            let score = fuzzy_score(first_word, name)?;
                            Some((score, PaletteRow::Ex { name, hint }))
                        })
                        .collect()
                }
            }
            // Search mode: empty query → recent-search history (so the
            // user can re-run prior queries with a single click).
            // Non-empty query → live buffer matches the lua side
            // pushed via `set_buffer_matches`. Each match scores by
            // its line number ascending so the top of the file lands
            // first in the list — `sort_by` below sorts descending,
            // so we negate to flip back to ascending.
            PaletteMode::Search => {
                if self.query.is_empty() {
                    self.recent_searches
                        .iter()
                        .map(|term| (0_i32, PaletteRow::Search { term }))
                        .collect()
                } else {
                    self.buffer_matches
                        .iter()
                        .enumerate()
                        .map(|(idx, (lnum, col, text))| {
                            // Newer matches (lower idx) score higher
                            // so `sort_by` keeps the lua-side ordering
                            // (which is already file-order).
                            let score = (i32::MAX / 2) - idx as i32;
                            (
                                score,
                                PaletteRow::BufferMatch {
                                    lnum: *lnum,
                                    col: *col,
                                    text: text.as_str(),
                                },
                            )
                        })
                        .collect()
                }
            }
        };

        results.sort_by(|a, b| b.0.cmp(&a.0));
        results
    }

    /// Build the host→workspace tree for `Workspaces` mode.
    ///
    /// Workspaces are grouped under their `host_id` in first-seen order;
    /// each surviving group emits a non-selectable `WorkspaceHost`
    /// header row immediately followed by its child `Workspace` rows
    /// (the render pass indents the children, mirroring file_tree's
    /// folder→file nesting).
    ///
    /// Fuzzy filtering spans both axes:
    /// - If the **host label** matches the query, the whole group is
    ///   kept (you searched for the host, so you want its workspaces).
    /// - Otherwise only the workspaces whose **title** matches survive,
    ///   and the header is shown only when at least one child remains.
    ///
    /// The returned scores are placeholders — order is structural, not
    /// score-ranked — but the tuple shape matches the rest of the
    /// pipeline so callers (`hit_test`, selection, render) stay uniform.
    ///
    /// 5D-drag seam: the `WorkspaceHost` rows are the future drop
    /// targets. A drag of a `Workspace` child onto a `WorkspaceHost`
    /// row will issue the move; this method already gives the renderer
    /// the host_id per header to hang that gesture off of.
    fn grouped_workspace_rows<'a>(
        &'a self,
        workspaces: &'a [PaletteWorkspaceEntry],
    ) -> Vec<(i32, PaletteRow<'a>)> {
        // First-seen host order. We can't use a HashMap here because we
        // need to preserve insertion order for a stable, scannable tree.
        let mut host_order: Vec<&'a str> = Vec::new();
        for entry in workspaces {
            if !host_order.contains(&entry.host_id.as_str()) {
                host_order.push(entry.host_id.as_str());
            }
        }

        let mut rows: Vec<(i32, PaletteRow<'a>)> = Vec::new();
        for &host_id in &host_order {
            // All workspaces under this host, in input order.
            let group: Vec<&'a PaletteWorkspaceEntry> =
                workspaces.iter().filter(|e| e.host_id == host_id).collect();
            let Some(first) = group.first() else {
                continue;
            };

            // Host-label match keeps the entire group; otherwise filter
            // children by their own title match.
            let host_matches = fuzzy_score(&self.query, &first.host_label).is_some();
            let kept: Vec<&'a PaletteWorkspaceEntry> = if host_matches {
                group.clone()
            } else {
                group
                    .iter()
                    .copied()
                    .filter(|e| fuzzy_score(&self.query, &e.title).is_some())
                    .collect()
            };
            if kept.is_empty() {
                continue;
            }

            // Header row (non-selectable). Score is irrelevant — the
            // list is returned in structural order without sorting.
            rows.push((
                0,
                PaletteRow::WorkspaceHost {
                    host_id: first.host_id.as_str(),
                    label: first.host_label.as_str(),
                    kind: first.host_kind,
                    daemon_url: first.daemon_url.as_deref(),
                    online: first.host_online,
                },
            ));
            for entry in kept {
                rows.push((0, PaletteRow::Workspace { entry }));
            }
        }

        // Wave 6A: workspace-less hosts (discovered tailnet peers) trail
        // the populated groups as header-only rows so they're droppable
        // targets even before they own any workspaces. Deduped by
        // host_id against the groups above (belt and braces — the
        // caller already dedupes peers against known hosts) and fuzzy-
        // filtered by host label like any other host header.
        for host in &self.workspace_peer_hosts {
            if host_order.contains(&host.host_id.as_str()) {
                continue;
            }
            if fuzzy_score(&self.query, &host.label).is_none() {
                continue;
            }
            rows.push((
                0,
                PaletteRow::WorkspaceHost {
                    host_id: host.host_id.as_str(),
                    label: host.label.as_str(),
                    kind: host.kind,
                    daemon_url: host.daemon_url.as_deref(),
                    online: host.online,
                },
            ));
        }

        rows.push((0, PaletteRow::WorkspaceCreate));

        rows
    }

    /// Visible row count after filtering, capped at the scroll window.
    /// Drives both palette height and skeleton suppression so the box
    /// shrinks to actual content instead of always reserving space for
    /// `MAX_VISIBLE_RESULTS` rows.
    pub(super) fn visible_row_count(&self) -> usize {
        self.filtered_rows()
            .len()
            .saturating_sub(self.scroll_offset)
            .min(MAX_VISIBLE_RESULTS)
    }

    /// Returns the palette geometry (x, y, width, height) for hit-testing.
    /// Height collapses to just the input field when there are no rows
    /// and grows by `RESULT_ITEM_HEIGHT` per visible row up to the cap.
    /// All dimensions are multiplied by `self.scale` so Ctrl+/Ctrl- on
    /// the workspace chrome resizes the palette in lockstep.
    pub(super) fn palette_rect(
        &self,
        window_width: f32,
        scale_factor: f32,
    ) -> (f32, f32, f32, f32) {
        let s = self.scale;
        let logical_w = window_width / scale_factor;
        // Clamp the card to the viewport (finder already does) so the
        // palette doesn't overflow phone-width screens.
        let width = (super::PALETTE_WIDTH * s).min((logical_w - 16.0 * s).max(160.0));
        let pad = PALETTE_PADDING * s;
        let input_h = INPUT_HEIGHT * s;
        let row_h = RESULT_ITEM_HEIGHT * s;
        let margin_top = RESULTS_MARGIN_TOP * s;
        let frame_stroke = (file_tree::FRAME_STROKE * s).max(2.0);
        let results_padding_bottom = RESULTS_PADDING_BOTTOM * s;
        let px = ((logical_w - width) / 2.0).max(8.0 * s);
        let py = super::PALETTE_MARGIN_TOP * s;
        let visible = self.visible_row_count();
        let body_h = if visible == 0 {
            0.0
        } else {
            SEPARATOR_HEIGHT
                + margin_top
                + row_h * visible as f32
                + results_padding_bottom
        };
        let h = frame_stroke * 2.0 + pad + input_h + body_h + pad;
        (px, py, width, h)
    }

    pub fn active_rect(&self, window_width: f32, scale_factor: f32) -> Option<[f32; 4]> {
        self.enabled.then(|| {
            let (x, y, w, h) = self.palette_rect(window_width, scale_factor);
            [x, y, w, h]
        })
    }

    /// Hit-test a mouse click. Returns Some(index) if a result row was clicked,
    /// or None if clicked outside the palette or on the input area.
    /// Returns Err(()) if clicked outside the palette entirely (should close).
    pub fn hit_test(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> Result<Option<usize>, ()> {
        let (px, py, pw, ph) = self.palette_rect(window_width, scale_factor);

        // Outside palette bounds
        if mouse_x < px || mouse_x > px + pw || mouse_y < py || mouse_y > py + ph {
            return Err(()); // Close palette
        }

        // Results area starts after input + separator
        let s = self.scale;
        let results_y = py
            + (file_tree::FRAME_STROKE * s).max(2.0)
            + PALETTE_PADDING * s
            + INPUT_HEIGHT * s
            + SEPARATOR_HEIGHT
            + RESULTS_MARGIN_TOP * s;
        if mouse_y < results_y {
            return Ok(None); // Clicked on input area
        }

        let relative_y = mouse_y - results_y - self.list_scroll_spring.position;
        let row = (relative_y / (RESULT_ITEM_HEIGHT * s)) as usize;
        let filtered_count = self.filtered_rows().len();
        let actual_index = self.scroll_offset + row;

        if actual_index < filtered_count {
            Ok(Some(actual_index))
        } else {
            Ok(None)
        }
    }

    /// Test-only inverse of `hit_test`: the `(x, y)` logical coordinate
    /// at the vertical center of the filtered row at `index`, for the
    /// given window dims. Lets gesture tests press/move/release on a
    /// known row without re-deriving the palette geometry inline (and
    /// staying correct if a constant changes). Assumes the default
    /// scroll/scale/spring state the tests use.
    #[cfg(test)]
    pub(super) fn row_center_coords(
        &self,
        index: usize,
        window_width: f32,
        scale_factor: f32,
    ) -> (f32, f32) {
        let (px, py, pw, _ph) = self.palette_rect(window_width, scale_factor);
        let s = self.scale;
        let results_y = py
            + (file_tree::FRAME_STROKE * s).max(2.0)
            + PALETTE_PADDING * s
            + INPUT_HEIGHT * s
            + SEPARATOR_HEIGHT
            + RESULTS_MARGIN_TOP * s;
        let row = index.saturating_sub(self.scroll_offset);
        let y = results_y
            + self.list_scroll_spring.position
            + (row as f32 + 0.5) * (RESULT_ITEM_HEIGHT * s);
        (px + pw / 2.0, y)
    }

    /// Update pointer hover without stealing keyboard selection.
    pub fn hover(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> bool {
        let next = self
            .hit_test(mouse_x, mouse_y, window_width, scale_factor)
            .ok()
            .flatten();
        if next == self.hovered_index {
            return false;
        }
        self.hovered_index = next;
        true
    }

    pub fn pointer_over_row(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> bool {
        self.hit_test(mouse_x, mouse_y, window_width, scale_factor)
            .ok()
            .flatten()
            .is_some()
    }

    // ------------------------------------------------------------------
    // 5D-drag: drag a workspace row onto a host header to move it.
    //
    // The host (desktop `mouse.rs`, web pointer handlers) drives the
    // raw pointer lifecycle; this module owns the pure gesture logic so
    // it's testable without a `Screen`. The pipeline mirrors the
    // established buffer-tabs / cross_window_drag press→active→drop
    // pattern:
    //   1. `workspace_drag_press` arms a drag if the pressed row is a
    //      selectable workspace (else returns `false` → host falls
    //      through to its normal click path).
    //   2. `workspace_drag_move` promotes the armed drag to `active`
    //      once the cursor passes `WORKSPACE_DRAG_ACTIVATION_PX`, and
    //      tracks the host header under the cursor as the drop target.
    //   3. `workspace_drag_release` emits `MoveWorkspaceToHost` when the
    //      release lands on a *different* host header; otherwise it's a
    //      no-op cancel (release off a header, or back on the source
    //      host). It returns whether the gesture was an active drag so
    //      the host knows whether to suppress the click-to-switch.
    // ------------------------------------------------------------------

    /// Identify the host header at a hit-test `index`, returning its
    /// `(host_id, daemon_url, is_local)`. `None` when the row at that
    /// index isn't a `WorkspaceHost` (e.g. a workspace child, or out of
    /// range), or when the host is **offline** — an unreachable host
    /// can't receive a workspace, so it is never a drop target (its
    /// header renders dimmed via the `○` dot instead). Owned `String`s
    /// so the caller can mutate the palette in the same statement
    /// without fighting the borrow checker.
    fn host_header_at(&self, index: usize) -> Option<(String, Option<String>, bool)> {
        self.filtered_rows()
            .get(index)
            .and_then(|(_, row)| match row {
                PaletteRow::WorkspaceHost {
                    host_id,
                    kind,
                    daemon_url,
                    online,
                    ..
                } if *online => Some((
                    (*host_id).to_string(),
                    daemon_url.map(str::to_string),
                    matches!(kind, HostKind::Local),
                )),
                _ => None,
            })
    }

    /// Arm a workspace drag if the mouse pressed on a selectable
    /// `Workspace` row in the Workspaces modal. Returns `true` when a
    /// drag was armed (the host should now route subsequent
    /// move/release through the drag methods); `false` leaves the
    /// host's normal click pipeline untouched.
    ///
    /// Arming is *not* the same as activating — the drag stays dormant
    /// (no ghost, click still fires) until the cursor crosses the
    /// activation threshold in `workspace_drag_move`.
    pub fn workspace_drag_press(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> bool {
        if !matches!(self.mode, PaletteMode::Workspaces(_)) {
            return false;
        }
        let Ok(Some(index)) = self.hit_test(mouse_x, mouse_y, window_width, scale_factor)
        else {
            return false;
        };
        let pressed = self
            .filtered_rows()
            .get(index)
            .and_then(|(_, row)| match row {
                PaletteRow::Workspace { entry } => {
                    Some((entry.target.workspace_id.clone(), entry.host_id.clone()))
                }
                _ => None,
            });
        let Some((workspace_id, source_host_id)) = pressed else {
            return false;
        };
        self.workspace_drag = Some(WorkspaceDrag {
            workspace_id,
            source_host_id,
            press_x: mouse_x,
            press_y: mouse_y,
            active: false,
            drop_host_id: None,
        });
        true
    }

    /// Advance an armed workspace drag with the current cursor position.
    /// Promotes the drag to `active` once the cursor moves past
    /// `WORKSPACE_DRAG_ACTIVATION_PX`, and (when active) updates the
    /// drop-target host header under the cursor. Returns `true` when
    /// something visible changed (drag activated, or the drop target
    /// moved) so the host can request a redraw. Returns `false` (and
    /// does nothing) when no drag is armed.
    pub fn workspace_drag_move(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> bool {
        let Some(drag) = self.workspace_drag.as_ref() else {
            return false;
        };
        let dx = mouse_x - drag.press_x;
        let dy = mouse_y - drag.press_y;
        let past_threshold = dx.hypot(dy) >= WORKSPACE_DRAG_ACTIVATION_PX;
        let was_active = drag.active;

        // Below the threshold and not yet active: still a potential
        // click, leave everything alone.
        if !was_active && !past_threshold {
            return false;
        }

        // Resolve the host header (if any) currently under the cursor.
        let drop_host_id =
            match self.hit_test(mouse_x, mouse_y, window_width, scale_factor) {
                Ok(Some(index)) => self.host_header_at(index).map(|(id, _, _)| id),
                _ => None,
            };

        let drag = self.workspace_drag.as_mut().expect("checked above");
        let changed = !was_active || drag.drop_host_id != drop_host_id;
        drag.active = true;
        drag.drop_host_id = drop_host_id;
        changed
    }

    /// Finish a workspace drag at the release cursor position.
    ///
    /// Returns `(was_active, action)`:
    /// - `was_active` is `true` when the gesture had crossed the drag
    ///   threshold — the host should then suppress the click-to-switch
    ///   that a release would otherwise trigger.
    /// - `action` is `Some(MoveWorkspaceToHost { .. })` only when the
    ///   release landed on a host header *different* from the source
    ///   host; it's `None` for every cancel case (release off any
    ///   header, or back onto the workspace's own current host, or no
    ///   active drag).
    ///
    /// Always clears the drag state. When `was_active` is `false` the
    /// caller had only an armed-but-not-activated press, i.e. a plain
    /// click — it should fall through to its normal selection path.
    pub fn workspace_drag_release(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> (bool, Option<PaletteAction>) {
        let Some(drag) = self.workspace_drag.take() else {
            return (false, None);
        };
        if !drag.active {
            // Press never became a drag — a plain click. Report
            // not-active so the host runs its click pipeline.
            return (false, None);
        }

        // Find the host header under the release point.
        let target = match self.hit_test(mouse_x, mouse_y, window_width, scale_factor) {
            Ok(Some(index)) => self.host_header_at(index),
            _ => None,
        };

        let action = match target {
            // Dropped on a host header that isn't the workspace's own
            // current host → emit the move intent. Dropping back on the
            // source host (or onto no header) is a no-op cancel.
            Some((target_host_id, target_daemon_url, target_is_local))
                if target_host_id != drag.source_host_id =>
            {
                Some(PaletteAction::MoveWorkspaceToHost {
                    workspace_id: drag.workspace_id,
                    target_host_id,
                    target_daemon_url,
                    target_is_local,
                })
            }
            _ => None,
        };
        (true, action)
    }

    /// Cancel any in-flight workspace drag (e.g. Esc, focus loss).
    /// Returns `true` when a drag was actually cleared so the host can
    /// redraw away the affordance.
    pub fn cancel_workspace_drag(&mut self) -> bool {
        self.workspace_drag.take().is_some()
    }

    /// `true` while an *activated* workspace drag is in flight (the
    /// cursor has crossed the threshold). The render pass uses this to
    /// paint the drop-target affordance; an armed-but-dormant press
    /// reports `false`.
    pub fn is_dragging_workspace(&self) -> bool {
        self.workspace_drag.as_ref().is_some_and(|d| d.active)
    }

    /// Host id of the header currently under the cursor during an active
    /// drag — the live drop target to highlight. `None` when no active
    /// drag, or the cursor isn't over a host header.
    pub fn workspace_drag_drop_host_id(&self) -> Option<&str> {
        self.workspace_drag
            .as_ref()
            .filter(|d| d.active)
            .and_then(|d| d.drop_host_id.as_deref())
    }
}
