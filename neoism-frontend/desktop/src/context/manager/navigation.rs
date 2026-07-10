use super::*;
use crate::context::tab::Context;
use crate::context::title::{create_title_extra_from_context, update_title};
use crate::event::RioEvent;
use crate::layout::{ContextGrid, ContextGridItem};
use neoism_backend::event::EventListener;
use neoism_backend::sugarloaf::{Object, Sugarloaf};
use neoism_protocol::workspace::{PaneFocusDir, PaneLayoutOp};
use neoism_ui::session_layout::{
    active_tab_index_after_close, active_tab_move_target, adjacent_tab_index,
    focused_tab_strip, rebase_tab_index_after_move, rebase_tab_indexed_map_for_move,
    rebase_tab_indexed_map_for_remove,
    session_layout_first_secondary_route as session_layout_first_secondary_route_policy,
    session_layout_focus_adjacent_route, session_layout_focus_edge_route,
    session_layout_secondary_routes as session_layout_secondary_routes_policy,
    SessionLayout, SessionTabStripRef,
};
use smallvec::SmallVec;
use std::time::Instant;

impl<T: EventListener + Clone + std::marker::Send + Sync + 'static> ContextManager<T> {
    #[inline]
    pub fn select_next_split(&mut self) {
        let route = self.current_route as u64;
        if self.request_pane_layout_op(
            route,
            PaneLayoutOp::Focus {
                dir: PaneFocusDir::Right,
            },
        ) {
            return;
        }
        if self.focus_split_via_session_layout(false, true) {
            self.current_route = self.current().route_id;
            return;
        }
        self.contexts[self.current_index].select_next_split();
        self.current_route = self.current().route_id;
    }

    #[inline]
    pub fn select_prev_split(&mut self) {
        let route = self.current_route as u64;
        if self.request_pane_layout_op(
            route,
            PaneLayoutOp::Focus {
                dir: PaneFocusDir::Left,
            },
        ) {
            return;
        }
        if self.focus_split_via_session_layout(true, true) {
            self.current_route = self.current().route_id;
            return;
        }
        self.contexts[self.current_index].select_prev_split();
        self.current_route = self.current().route_id;
    }

    #[inline]
    pub fn switch_to_next_split_or_tab(&mut self) {
        if self.focus_split_via_session_layout(false, false) {
            self.current_route = self.current().route_id;
            return;
        }
        self.switch_to_next();
        if !self.focus_current_grid_edge_leaf(false) {
            // Make sure first split is selected - get the root key
            let current_tab = &mut self.contexts[self.current_index];
            if let Some(root) = current_tab.root {
                current_tab.current = root;
            }
        }
        self.current_route = self.current().route_id;
    }

    #[inline]
    pub fn switch_to_prev_split_or_tab(&mut self) {
        if self.focus_split_via_session_layout(true, false) {
            self.current_route = self.current().route_id;
            return;
        }
        self.switch_to_prev();
        if !self.focus_current_grid_edge_leaf(true) {
            // Make sure last split is selected - get the last key in order
            let current_tab = &mut self.contexts[self.current_index];
            let ordered_keys = current_tab.get_ordered_keys();
            if let Some(&last_key) = ordered_keys.last() {
                current_tab.current = last_key;
            }
        }
        self.current_route = self.current().route_id;
    }

    #[inline]
    pub fn move_divider_up(&mut self, amount: f32, sugarloaf: &mut Sugarloaf) -> bool {
        if self.request_pane_layout_op(
            self.current_route as u64,
            PaneLayoutOp::ResizeRatio { delta: -amount },
        ) {
            return false;
        }
        self.contexts[self.current_index].move_divider_up(amount, sugarloaf)
    }

    #[inline]
    pub fn move_divider_down(&mut self, amount: f32, sugarloaf: &mut Sugarloaf) -> bool {
        if self.request_pane_layout_op(
            self.current_route as u64,
            PaneLayoutOp::ResizeRatio { delta: amount },
        ) {
            return false;
        }
        self.contexts[self.current_index].move_divider_down(amount, sugarloaf)
    }

    #[inline]
    pub fn move_divider_left(&mut self, amount: f32, sugarloaf: &mut Sugarloaf) -> bool {
        if self.request_pane_layout_op(
            self.current_route as u64,
            PaneLayoutOp::ResizeRatio { delta: -amount },
        ) {
            return false;
        }
        self.contexts[self.current_index].move_divider_left(amount, sugarloaf)
    }

    #[inline]
    pub fn move_divider_right(&mut self, amount: f32, sugarloaf: &mut Sugarloaf) -> bool {
        if self.request_pane_layout_op(
            self.current_route as u64,
            PaneLayoutOp::ResizeRatio { delta: amount },
        ) {
            return false;
        }
        self.contexts[self.current_index].move_divider_right(amount, sugarloaf)
    }

    #[inline]
    pub fn select_tab(&mut self, tab_index: usize) {
        if self.config.is_native {
            self.set_current(tab_index);
            self.event_proxy
                .send_event(RioEvent::SelectNativeTabByIndex(tab_index), self.window_id);
            return;
        }

        self.set_current(tab_index);
    }

    #[inline]
    pub fn toggle_full_screen(&mut self) {
        self.event_proxy
            .send_event(RioEvent::ToggleFullScreen, self.window_id);
    }

    #[inline]
    pub fn toggle_appearance_theme(&mut self) {
        self.event_proxy
            .send_event(RioEvent::ToggleAppearanceTheme, self.window_id);
    }

    #[inline]
    pub fn minimize(&mut self) {
        self.event_proxy
            .send_event(RioEvent::Minimize(true), self.window_id);
    }

    #[inline]
    pub fn hide(&mut self) {
        self.event_proxy.send_event(RioEvent::Hide, self.window_id);
    }

    #[inline]
    pub fn quit(&mut self) {
        self.event_proxy.send_event(RioEvent::Quit, self.window_id);
    }

    #[cfg(target_os = "macos")]
    #[inline]
    pub fn hide_other_apps(&mut self) {
        self.event_proxy
            .send_event(RioEvent::HideOtherApplications, self.window_id);
    }

    #[inline]
    pub fn select_last_tab(&mut self) {
        if self.config.is_native {
            self.event_proxy
                .send_event(RioEvent::SelectNativeTabLast, self.window_id);
            return;
        }

        self.select_tab(self.contexts.len() - 1);
    }

    #[inline]
    pub fn switch_to_settings(&mut self) {
        self.event_proxy
            .send_event(RioEvent::CreateConfigEditor, self.window_id);
    }

    #[inline]
    pub fn select_route_from_current_grid(&mut self) {
        let (route_id, is_editor) = {
            let current = self.current();
            (current.route_id, current.editor.is_some())
        };
        self.current_route = route_id;
        if is_editor {
            self.current_mut().pending_terminal_resize = true;
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.contexts.len()
    }

    #[inline]
    pub fn resize_all_grids(
        &mut self,
        width: f32,
        height: f32,
        sugarloaf: &mut Sugarloaf,
    ) {
        for context_grid in self.contexts.iter_mut() {
            context_grid.resize(width, height, sugarloaf);
        }
    }

    pub fn update_titles(&mut self) {
        let now = Instant::now();
        if neoism_ui::context_policy::title_update_should_run(
            self.titles.last_title_update,
            now,
        ) {
            self.titles.last_title_update = Some(now);
            let mut id = String::default();
            for (i, context) in self.contexts.iter_mut().enumerate() {
                // OS window title follows the active pane (unchanged).
                let content = update_title(&self.config.title.content, context.current());

                self.event_proxy
                    .send_event(RioEvent::Title(content.to_owned()), self.window_id);

                id.push_str(&format!("{i}{content};"));

                // The workspace TAB title is derived from the workspace's
                // ROOT context (its base terminal), so it stays stable —
                // switching to an nvim/editor pane (whose cwd differs)
                // no longer flips the tab name to "~".
                let tab_content =
                    update_title(&self.config.title.content, context.root_context());
                if self.config.should_update_title_extra {
                    self.titles.set_key_val(
                        i,
                        tab_content,
                        create_title_extra_from_context(context.root_context()),
                    );
                } else {
                    self.titles.set_key_val(i, tab_content, None);
                }
            }

            self.titles.set_key(id);
        }
    }

    #[inline]
    pub fn get_by_route_id(
        &mut self,
        route_id: usize,
    ) -> Option<&mut ContextGridItem<T>> {
        self.contexts
            .iter_mut()
            .find_map(|grid| grid.get_by_route_id(route_id))
    }

    #[inline]
    pub fn contexts_mut(
        &mut self,
    ) -> &mut SmallVec<[ContextGrid<T>; DEFAULT_CONTEXT_CAPACITY]> {
        &mut self.contexts
    }

    /// Read-only access to every grid (for cross-grid drains like
    /// the per-frame `BufModified` sweep — touching panes outside the
    /// current tab as well so dirty dots stay live regardless of which
    /// tab the user is looking at).
    #[inline]
    pub fn all_grids(&self) -> &[ContextGrid<T>] {
        &self.contexts
    }

    /// Mutable variant of `all_grids` — needed for per-frame drains
    /// that take ownership of pending state (e.g.
    /// `editor_pending_scroll_lines` consumed via `mem::take`).
    #[inline]
    pub fn all_grids_mut(
        &mut self,
    ) -> &mut SmallVec<[ContextGrid<T>; DEFAULT_CONTEXT_CAPACITY]> {
        &mut self.contexts
    }

    pub fn pump_editor_redraws(&mut self) -> (usize, bool) {
        let mut applied = 0usize;
        let mut visible_hit_frame_limit = false;
        for (index, grid) in self.contexts.iter_mut().enumerate() {
            let (n, _limited, visible_limited) = grid.pump_editor_redraws();
            applied = applied.saturating_add(n);
            visible_hit_frame_limit |= index == self.current_index && visible_limited;
        }
        (applied, visible_hit_frame_limit)
    }

    #[inline]
    pub fn current_grid_len(&self) -> usize {
        self.contexts[self.current_index].panel_count()
    }

    #[inline]
    pub fn current_grid_splits_hidden(&self) -> bool {
        self.contexts[self.current_index].splits_hidden()
    }

    #[inline]
    pub fn current_grid_split_focused(&self) -> bool {
        self.contexts[self.current_index].is_split_focused()
    }

    #[inline]
    pub fn focus_current_grid_split(&mut self, sugarloaf: &mut Sugarloaf) -> bool {
        let changed = self.contexts[self.current_index].focus_first_split(sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        changed
    }

    #[inline]
    pub fn focus_current_grid_root(&mut self, sugarloaf: &mut Sugarloaf) -> bool {
        let changed = self.contexts[self.current_index].focus_root_panel(sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        changed
    }

    #[inline]
    pub fn focus_current_grid_horizontal_panel(
        &mut self,
        right: bool,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let changed =
            self.contexts[self.current_index].focus_horizontal_panel(right, sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        changed
    }

    #[inline]
    pub fn toggle_current_grid_splits_hidden(
        &mut self,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let changed = self.contexts[self.current_index].toggle_splits_hidden(sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        changed
    }

    #[inline]
    pub fn remove_current_grid(&mut self, sugarloaf: &mut Sugarloaf) {
        if self.request_pane_layout_op(self.current_route as u64, PaneLayoutOp::Close) {
            return;
        }
        if let Some((closing_route, focus_route)) =
            session_layout_close_current_grid_route(&self.contexts[self.current_index])
        {
            let grid = &mut self.contexts[self.current_index];
            let Some(closing_node) = grid.node_by_route_id(closing_route) else {
                tracing::warn!(
                    closing_route,
                    "SessionLayout close target was missing from native grid"
                );
                return;
            };

            grid.remove_node(closing_node, sugarloaf);
            if let Some(focus_node) = grid.node_by_route_id(focus_route) {
                let _ = grid.set_current_node(focus_node, sugarloaf);
            }
        } else {
            self.contexts[self.current_index].remove_current(sugarloaf);
        }
        self.current_route = self.contexts[self.current_index].current().route_id;
    }

    /// Remove the pane hosting `route_id` from the current grid directly
    /// (by node), bypassing the legacy SessionLayout focus-based close
    /// resolution. Used when the caller already knows exactly which pane to
    /// drop (e.g. a strip emptied by closing its last tab) so closing one
    /// pane never tears out a different (focused) one. Returns true if a
    /// pane was removed.
    pub fn remove_grid_route(
        &mut self,
        route_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        if self.request_pane_layout_op(route_id as u64, PaneLayoutOp::Close) {
            return true;
        }
        let grid = &mut self.contexts[self.current_index];
        if grid.len() <= 1 {
            return false;
        }
        let Some(node) = grid.node_by_route_id(route_id) else {
            return false;
        };
        grid.remove_node(node, sugarloaf);
        self.current_route = grid.current().route_id;
        true
    }

    #[inline]
    pub fn current_grid_mut(&mut self) -> &mut ContextGrid<T> {
        &mut self.contexts[self.current_index]
    }

    #[inline]
    pub fn current_grid(&self) -> &ContextGrid<T> {
        &self.contexts[self.current_index]
    }

    pub fn current_grid_first_secondary_route(&self) -> Option<usize> {
        let grid = &self.contexts[self.current_index];
        let layout = session_layout_for_grid(grid)?;
        let workspace_route = grid.workspace_route_id()? as u64;
        session_layout_first_secondary_route_policy(&layout, workspace_route)
            .map(|route| route as usize)
    }

    pub fn current_grid_secondary_routes(&self) -> Vec<usize> {
        let grid = &self.contexts[self.current_index];
        let Some(layout) = session_layout_for_grid(grid) else {
            return Vec::new();
        };
        let Some(workspace_route) = grid.workspace_route_id() else {
            return Vec::new();
        };
        session_layout_secondary_routes_policy(&layout, workspace_route as u64)
            .into_iter()
            .map(|route| route as usize)
            .collect()
    }

    pub fn current_grid_focused_tab_strip(
        &self,
        pane_tab_routes: impl IntoIterator<Item = usize>,
    ) -> SessionTabStripRef {
        let grid = &self.contexts[self.current_index];
        let layout = session_layout_for_grid(grid);
        let workspace_route = grid.workspace_route_id().map(|route| route as u64);
        focused_tab_strip(
            workspace_route,
            layout.as_ref().and_then(SessionLayout::focused_external_id),
            pane_tab_routes.into_iter().map(|route| route as u64),
        )
    }

    fn focus_split_via_session_layout(&mut self, previous: bool, wrap: bool) -> bool {
        let current_grid = &self.contexts[self.current_index];
        let current_panel = current_grid.current;
        let Some(current_route) = current_grid
            .contexts()
            .get(&current_panel)
            .map(|item| item.context().route_id)
        else {
            return false;
        };

        let layout = match session_layout_for_grid(current_grid) {
            Some(layout) => layout,
            None => return false,
        };
        let Some(target_route) = session_layout_focus_adjacent_route(
            layout,
            previous,
            wrap,
            current_route as u64,
        )
        .map(|route| route as usize) else {
            return false;
        };
        let Some(target_node) =
            self.contexts[self.current_index].node_by_route_id(target_route)
        else {
            return false;
        };

        self.contexts[self.current_index].current = target_node;
        debug_assert_eq!(
            session_layout_for_grid(&self.contexts[self.current_index])
                .and_then(|layout| session_leaf_route(&layout, layout.focused_leaf())),
            Some(target_route)
        );
        true
    }

    fn focus_current_grid_edge_leaf(&mut self, last: bool) -> bool {
        let layout =
            match session_layout_mirror_for_grid(&self.contexts[self.current_index]) {
                Some(layout) => layout,
                None => return false,
            };
        let Some(target_route) =
            session_layout_focus_edge_route(layout, last).map(|route| route as usize)
        else {
            return false;
        };
        let Some(target_node) =
            self.contexts[self.current_index].node_by_route_id(target_route)
        else {
            return false;
        };

        self.contexts[self.current_index].current = target_node;
        true
    }

    /// Get panel borders for the current grid (returns empty vec if single panel)
    #[inline]
    pub fn get_panel_borders(&self) -> Vec<Object> {
        self.contexts[self.current_index].get_panel_borders()
    }

    /// Get the scaled margin of the current grid (in physical pixels, for border positioning)
    #[inline]
    pub fn get_current_grid_scaled_margin(
        &self,
    ) -> neoism_backend::config::layout::Margin {
        self.contexts[self.current_index].get_scaled_margin()
    }

    #[cfg(test)]
    pub fn increase_capacity(&mut self, inc_val: usize) {
        self.capacity += inc_val;
    }

    #[inline]
    pub fn set_current(&mut self, context_id: usize) {
        if context_id < self.contexts.len() {
            self.request_switch_session_for_tab(context_id);
            self.current_index = context_id;
            self.current_route = self.current().route_id;
            self.sync_daemon_workspaces();
        }
    }

    /// Move the workspace at `from` to position `to`, shifting any
    /// workspaces between them. Used by the Island drag-to-reorder
    /// gesture — equivalent to `Vec::remove + insert` with `current_index`
    /// and the titles map both rebased so the user's active workspace
    /// stays focused after the move.
    #[inline]
    pub fn move_workspace(&mut self, from: usize, to: usize) {
        let len = self.contexts.len();
        if from == to || from >= len || to >= len {
            return;
        }
        let grid = self.contexts.remove(from);
        self.contexts.insert(to, grid);
        self.current_index = rebase_tab_index_after_move(self.current_index, from, to);
        // current_route may already be correct (the grid carried its
        // route with it), but `set_current` is the canonical resync.
        self.current_route = self.current().route_id;
        rebase_tab_indexed_map_for_move(&mut self.titles.titles, from, to);
        self.sync_daemon_workspaces();
    }

    /// Lift the workspace grid at `index` out of this window, returning
    /// it (with its live PTYs intact) so a caller can adopt it into a
    /// different OS window. Returns `None` when there is nothing safe to
    /// detach — an out-of-range index, or the last remaining workspace
    /// (detaching it would leave this window empty).
    ///
    /// Rich-text content is de-registered from this window's
    /// `sugarloaf`; the destination re-registers it via
    /// [`Self::adopt_workspace`].
    pub fn take_workspace(
        &mut self,
        index: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> Option<ContextGrid<T>> {
        if index >= self.contexts.len() || self.contexts.len() <= 1 {
            return None;
        }
        self.contexts[index].remove_all_rich_text(sugarloaf);
        let grid = self.contexts.remove(index);
        self.remove_title_at_index(index);

        // Keep the focused tab pointing at a live workspace.
        if self.current_index >= self.contexts.len() {
            self.current_index = self.contexts.len() - 1;
        } else if index < self.current_index {
            self.current_index -= 1;
        }
        self.current_route = self.current().route_id;
        self.keep_only_active_context_visible(sugarloaf);
        self.sync_daemon_workspaces();
        Some(grid)
    }

    /// Adopt a workspace grid lifted out of another window via
    /// [`Self::take_workspace`]. Re-homes every pane's PTY onto this
    /// window, re-registers its rich-text content with this window's
    /// `sugarloaf`, appends it as the trailing workspace, and focuses
    /// it.
    pub fn adopt_workspace(
        &mut self,
        grid: ContextGrid<T>,
        sugarloaf: &mut Sugarloaf,
        discard_existing_default: bool,
    ) {
        grid.rebind_window(self.window_id);
        grid.register_all_rich_text(sugarloaf);

        // A freshly-spawned window already holds one throwaway default
        // shell. When adopting into it, drop that default once the real
        // workspace is in place so only the detached one remains.
        let drop_default = discard_existing_default && self.contexts.len() == 1;

        let new_index = self.contexts.len();
        self.contexts.push(grid);
        self.current_index = new_index;
        self.current_route = self.current().route_id;
        // Seed a placeholder title; `update_titles` refreshes it from
        // the live terminal/program on the next tick.
        self.titles
            .set_key_val(new_index, String::from("tab"), None);

        if drop_default {
            // Focus the throwaway default (index 0) and close it; the
            // adopted workspace shifts down to index 0 and stays
            // focused. Reuses the tested close path (kills the default's
            // shell, drops its rich text, rebases titles/focus).
            self.current_index = 0;
            self.close_current_context(sugarloaf);
        } else {
            self.keep_only_active_context_visible(sugarloaf);
        }
        self.sync_daemon_workspaces();
    }

    /// Lift the live context for `route_id` out of the current workspace
    /// grid (session intact) so it can be spliced into another
    /// workspace. See [`ContextGrid::take_context_by_route`].
    pub fn take_current_grid_context_by_route(
        &mut self,
        route_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> Option<Context<T>> {
        let context =
            self.contexts[self.current_index].take_context_by_route(route_id, sugarloaf);
        if context.is_some() {
            self.sync_daemon_workspaces();
        }
        context
    }

    /// Splice a context lifted from another workspace into the current
    /// workspace grid as a stacked buffer tab. Re-homes the context onto
    /// this window first (a no-op for same-window moves, the correct
    /// rebind for cross-window moves). Returns `true` on success.
    pub fn add_stacked_context_to_current(
        &mut self,
        context: Context<T>,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        context.rebind_window(self.window_id);
        let added = self.contexts[self.current_index]
            .add_stacked_context(context, sugarloaf)
            .is_some();
        if added {
            self.sync_daemon_workspaces();
        }
        added
    }

    #[inline]
    pub fn close_current_context(&mut self, sugarloaf: &mut Sugarloaf) {
        self.request_close_current_session();
        if self.contexts.len() == 1 {
            // MacOS: Close last tab will work, leading to hide and
            // keep Rio running in background.
            #[cfg(target_os = "macos")]
            {
                self.event_proxy
                    .send_event(RioEvent::CloseWindow, self.window_id);
            }
            return;
        }

        let index_to_remove = self.current_index;

        // Remove all rich text from the grid before removing the context
        self.contexts[index_to_remove].remove_all_rich_text(sugarloaf);
        self.contexts.remove(index_to_remove);
        self.remove_title_at_index(index_to_remove);

        self.current_index =
            active_tab_index_after_close(self.contexts.len() + 1, index_to_remove)
                .expect("close_current_context returns before closing the last tab");
        self.current_route = self.current().route_id;

        self.keep_only_active_context_visible(sugarloaf);
        self.sync_daemon_workspaces();
    }

    pub(crate) fn remove_title_at_index(&mut self, removed_index: usize) {
        rebase_tab_indexed_map_for_remove(&mut self.titles.titles, removed_index);
    }

    #[inline]
    pub fn current_index(&self) -> usize {
        self.current_index
    }

    #[inline]
    pub fn current_route(&self) -> usize {
        self.current_route
    }

    #[inline]
    pub fn current(&self) -> &Context<T> {
        self.contexts[self.current_index].current()
    }

    #[inline]
    pub fn current_mut(&mut self) -> &mut Context<T> {
        self.contexts[self.current_index].current_mut()
    }

    #[inline]
    pub fn switch_to_next(&mut self) {
        if self.config.is_native {
            self.current_index =
                adjacent_tab_index(self.contexts.len(), self.current_index, false)
                    .expect("current tab index must stay valid");
            self.current_route = self.current().route_id;
            self.sync_daemon_workspaces();
            self.event_proxy
                .send_event(RioEvent::SelectNativeTabNext, self.window_id);
            return;
        }

        self.current_index =
            adjacent_tab_index(self.contexts.len(), self.current_index, false)
                .expect("current tab index must stay valid");
        self.request_switch_session_for_tab(self.current_index);

        self.current_route = self.current().route_id;
        self.sync_daemon_workspaces();
    }

    #[inline]
    pub fn switch_to_prev(&mut self) {
        if self.config.is_native {
            self.current_index =
                adjacent_tab_index(self.contexts.len(), self.current_index, true)
                    .expect("current tab index must stay valid");
            self.current_route = self.current().route_id;
            self.sync_daemon_workspaces();
            self.event_proxy
                .send_event(RioEvent::SelectNativeTabPrev, self.window_id);
            return;
        }

        self.current_index =
            adjacent_tab_index(self.contexts.len(), self.current_index, true)
                .expect("current tab index must stay valid");
        self.request_switch_session_for_tab(self.current_index);

        self.current_route = self.current().route_id;
        self.sync_daemon_workspaces();
    }

    #[inline]
    pub fn move_current_to_prev(&mut self) {
        let Some(target_index) =
            active_tab_move_target(self.contexts.len(), self.current_index, true)
        else {
            return;
        };

        let current = self.current_index;
        if self.request_pane_layout_op(
            self.current_route as u64,
            PaneLayoutOp::MoveTab {
                from: current as u32,
                to: target_index as u32,
            },
        ) {
            return;
        }
        self.contexts.swap(current, target_index);
        self.select_tab(target_index);
    }

    #[inline]
    pub fn move_current_to_next(&mut self) {
        let Some(target_index) =
            active_tab_move_target(self.contexts.len(), self.current_index, false)
        else {
            return;
        };

        let current = self.current_index;
        if self.request_pane_layout_op(
            self.current_route as u64,
            PaneLayoutOp::MoveTab {
                from: current as u32,
                to: target_index as u32,
            },
        ) {
            return;
        }
        self.contexts.swap(current, target_index);
        self.select_tab(target_index);
    }
}
