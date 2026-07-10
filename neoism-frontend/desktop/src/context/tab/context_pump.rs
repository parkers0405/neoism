use super::*;

impl<T: EventListener> Context<T> {
    #[inline]
    pub fn active_markdown(&self) -> Option<&MarkdownPane> {
        self.markdown
            .as_ref()
            .or_else(|| self.notebook.as_ref().map(|notebook| &notebook.markdown))
    }

    #[inline]
    pub fn active_markdown_mut(&mut self) -> Option<&mut MarkdownPane> {
        if self.markdown.is_some() {
            return self.markdown.as_mut();
        }
        self.notebook
            .as_mut()
            .map(|notebook| &mut notebook.markdown)
    }

    /// True when this pane mounts a non-terminal surface — an editor,
    /// markdown preview, `.neodraw` sketch, agent pane, tags pane, or
    /// extensions pane. The Warp-style command composer (and its footer
    /// row reservation / scrollbar accounting) only belongs on a plain
    /// terminal pane, so every composer gate keys off
    /// `!has_non_terminal_surface()`. Mirrors the non-terminal field set
    /// the `Drop` impl uses to decide whether a `kill_pid` is safe — keep
    /// the two in sync when a new surface kind is added.
    #[inline]
    pub fn has_non_terminal_surface(&self) -> bool {
        self.editor.is_some()
            || self.markdown.is_some()
            || self.draw.is_some()
            || self.notebook.is_some()
            || self.neoism_agent.is_some()
            || self.neoism_tags.is_some()
            || self.neoism_extensions.is_some()
    }

    /// Drain any redraw notifications queued by the editor pane's nvim
    /// runtime, parse them into typed events, and apply to `Crosswords`.
    /// No-op for PTY contexts. Returns `(applied, hit_frame_limit)`.
    /// `hit_frame_limit` tells the caller to request another frame so
    /// a large nvim burst drains over several vsyncs instead of
    /// monopolizing one render.
    #[inline]
    pub fn pump_editor_redraws(&mut self) -> (usize, bool) {
        // NETCODE typing echo: revert any predicted cells whose TTL
        // lapsed without an authoritative frame. Cheap no-op when the
        // prediction list is empty (the overwhelmingly common case).
        let expired_predictions = self.expire_editor_predictions();
        if self.editor_redraw_rx.is_none() && self.editor_daemon_messages.is_empty() {
            return (usize::from(expired_predictions), false);
        }
        if let Some(editor) = self.editor.as_ref() {
            editor.clear_redraw_wake();
        }

        let mut all_events: Vec<RedrawEvent> = Vec::new();
        let mut drained_notifications = 0usize;
        let mut hit_frame_limit = false;

        if let Some(rx) = self.editor_redraw_rx.as_ref() {
            loop {
                if drained_notifications >= MAX_EDITOR_REDRAW_NOTIFICATIONS_PER_FRAME
                    || all_events.len() >= MAX_EDITOR_REDRAW_EVENTS_PER_FRAME
                {
                    hit_frame_limit = true;
                    break;
                }

                match rx.try_recv() {
                    Ok(notification) => {
                        drained_notifications += 1;
                        match parse_redraw_batch(notification.raw) {
                            Ok(mut events) => {
                                all_events.append(&mut events);
                            }
                            Err(e) => tracing::warn!(
                                target: "neoism::nvim_pump",
                                "redraw parse failed: {e}"
                            ),
                        }
                    }
                    Err(std_mpsc::TryRecvError::Empty) => break,
                    Err(std_mpsc::TryRecvError::Disconnected) => {
                        // nvim runtime exited — stop draining; the pane is
                        // about to be torn down.
                        self.editor_redraw_rx = None;
                        break;
                    }
                }
            }
        }

        while drained_notifications < MAX_EDITOR_REDRAW_NOTIFICATIONS_PER_FRAME
            && all_events.len() < MAX_EDITOR_REDRAW_EVENTS_PER_FRAME
        {
            let Some(message) = self.editor_daemon_messages.pop_front() else {
                break;
            };
            drained_notifications += 1;
            self.apply_daemon_editor_sideband(&message);
            all_events.extend(editor_message_to_redraw_events(message));
        }
        if !self.editor_daemon_messages.is_empty() {
            hit_frame_limit = true;
        }

        if all_events.is_empty() {
            return (usize::from(expired_predictions), hit_frame_limit);
        }

        // NETCODE typing echo: nvim's authoritative repaint of a row
        // supersedes any prediction on it (identical char confirms it,
        // different content corrects it — either way the prediction's
        // job is done). Scroll/clear/resize invalidate coordinates
        // wholesale, so drop everything on those.
        if !self.editor_predicted_cells.is_empty() {
            let mut clear_all = false;
            let mut touched_rows: Vec<(u64, u64)> = Vec::new();
            for event in &all_events {
                match event {
                    RedrawEvent::GridLine { grid, row, .. } => {
                        touched_rows.push((*grid, *row));
                    }
                    RedrawEvent::Scroll { .. }
                    | RedrawEvent::Clear { .. }
                    | RedrawEvent::Resize { .. } => {
                        clear_all = true;
                    }
                    _ => {}
                }
            }
            if clear_all {
                self.editor_predicted_cells.clear();
            } else if !touched_rows.is_empty() {
                self.editor_predicted_cells
                    .retain(|cell| !touched_rows.contains(&(cell.grid, cell.row)));
            }
        }

        tracing::debug!(
            target: "neoism::nvim_trace",
            route_id = self.route_id,
            events = all_events.len(),
            from_daemon = self.editor_redraw_rx.is_none(),
            "[nvim-trace] applying redraw events to editor grid"
        );

        if hit_frame_limit {
            tracing::debug!(
                target: "neoism::nvim_pump",
                route_id = self.route_id,
                drained_notifications,
                drained_events = all_events.len(),
                "editor redraw pump hit per-frame limit"
            );
        }

        // Latch the most recent mode_change before applying the batch so
        // the chrome can ask "what mode is the editor in *right now*?"
        // even on the same frame the mode flipped. Last-write-wins is
        // correct here — the batch may contain several mode changes
        // (e.g. cmdline → normal) and only the trailing one is current.
        for event in all_events.iter().rev() {
            if let RedrawEvent::ModeChange { mode, .. } = event {
                self.editor_mode = mode.clone();
                // The Rust-engine completion popup only lives in insert mode;
                // leaving it (Esc, `:`, etc.) closes the popup.
                if !matches!(self.editor_mode, EditorMode::Insert)
                    && self.editor_lsp_completion.is_some()
                {
                    self.editor_lsp_completion = None;
                }
                break;
            }
        }

        // Sum every grid_scroll's row delta in this batch — j/k/page-down
        // / `:NN` / `gg` all surface as `RedrawEvent::Scroll { rows }`.
        // The renderer drains `editor_pending_scroll_lines` per frame
        // and feeds it (× cell_height) into the EditorScroll spring so
        // keyboard navigation animates with the same neovide-style slide
        // as a wheel scroll. No conversion / sign flip here — pass nvim's
        // native row signing through and let the renderer decide cell
        // size.
        // Drive the scroll-animation spring from `win_viewport` events.
        // This is the canonical signal — fires on EVERY viewport
        // movement with the line-count delta, including the big jumps
        // (Ctrl-U/D, page-up/down, gg, G, `:42`) where nvim skips
        // `grid_scroll` and sends grid_line redraws for the whole new
        // viewport instead. Neovide drives its entire scroll_animation
        // from this same event.
        let scroll_log_enabled = std::env::var_os(SCROLL_LOG_ENV).is_some();
        let mut total_scroll_delta: i32 = 0;
        let mut win_viewport_events = 0u32;
        let mut grid_line_events = 0u32;
        let mut grid_line_cells = 0u32;
        let mut grid_line_repeat_cells = 0u32;
        let mut grid_line_first_row: Option<u64> = None;
        let mut grid_line_last_row: Option<u64> = None;
        let mut grid_line_unique_rows = std::collections::BTreeSet::new();
        let mut grid_scroll_events = 0u32;
        let mut grid_scroll_rows_total = 0i32;
        #[derive(Clone, Copy, Default)]
        struct GridActivity {
            events: u32,
            cells: u32,
            max_row: u64,
            max_col: u64,
            resize: Option<(u64, u64)>,
        }
        let mut grid_activity: std::collections::BTreeMap<u64, GridActivity> =
            std::collections::BTreeMap::new();
        for event in &all_events {
            match event {
                RedrawEvent::GridLine {
                    grid,
                    row,
                    column_start,
                    cells,
                } => {
                    let entry = grid_activity.entry(*grid).or_default();
                    entry.events = entry.events.saturating_add(1);
                    entry.cells = entry
                        .cells
                        .saturating_add(cells.len().min(u32::MAX as usize) as u32);
                    entry.max_row = entry.max_row.max(*row);
                    let row_cells = cells
                        .iter()
                        .map(|cell| cell.repeat.unwrap_or(1))
                        .fold(0u64, u64::saturating_add);
                    entry.max_col =
                        entry.max_col.max(column_start.saturating_add(row_cells));
                }
                RedrawEvent::Resize {
                    grid,
                    width,
                    height,
                } => {
                    let entry = grid_activity.entry(*grid).or_default();
                    entry.events = entry.events.saturating_add(1);
                    entry.resize = Some((*width, *height));
                }
                RedrawEvent::Clear { grid }
                | RedrawEvent::Scroll { grid, .. }
                | RedrawEvent::CursorGoto { grid, .. } => {
                    let entry = grid_activity.entry(*grid).or_default();
                    entry.events = entry.events.saturating_add(1);
                }
                _ => {}
            }
        }
        let selected_viewport_grid = all_events
            .iter()
            .filter_map(|event| {
                if let RedrawEvent::WinViewport {
                    grid,
                    topline,
                    botline,
                    line_count,
                    ..
                } = event
                {
                    Some((*grid, *line_count, botline.saturating_sub(*topline)))
                } else {
                    None
                }
            })
            .max_by_key(|(_, line_count, span)| (*line_count, *span))
            .map(|(grid, _, _)| grid);
        let previous_editor_grid = self.editor_grid_id;
        let editor_cols = self.dimension.columns.max(1) as u64;
        let editor_rows = self.dimension.lines.max(1) as u64;
        let editor_like_score = |activity: &GridActivity| {
            let mut score = u64::from(activity.cells);
            if let Some((width, height)) = activity.resize {
                if width.saturating_add(4) >= editor_cols
                    && height.saturating_add(3) >= editor_rows
                {
                    score = score.saturating_add(1_000_000);
                }
            }
            if activity.max_row.saturating_add(4) >= editor_rows {
                score = score.saturating_add(500_000);
            }
            if activity.max_col.saturating_add(4) >= editor_cols {
                score = score.saturating_add(250_000);
            }
            score
        };
        let has_editor_surface = |activity: &GridActivity| {
            activity.cells > 0 && editor_like_score(activity) >= 250_000
        };
        let active_grid_by_lines = grid_activity
            .iter()
            .filter(|(_, activity)| has_editor_surface(activity))
            .max_by_key(|(_, activity)| (editor_like_score(activity), activity.events))
            .map(|(grid, _)| *grid);
        let selected_viewport_has_surface = selected_viewport_grid
            .and_then(|grid| grid_activity.get(&grid))
            .map(has_editor_surface)
            .unwrap_or(false);
        let previous_has_activity = self
            .editor_grid_id
            .map(|grid| grid_activity.contains_key(&grid))
            .unwrap_or(false);
        let active_grid_has_surface = active_grid_by_lines
            .and_then(|grid| grid_activity.get(&grid))
            .map(has_editor_surface)
            .unwrap_or(false);
        let grid_activity_summary = if scroll_log_enabled {
            grid_activity
                .iter()
                .map(|(grid, activity)| {
                    let resize = activity
                        .resize
                        .map(|(w, h)| format!("{w}x{h}"))
                        .unwrap_or_else(|| "-".to_string());
                    format!(
                        "{}:ev{}:cells{}:max{}x{}:resize{}:score{}",
                        grid,
                        activity.events,
                        activity.cells,
                        activity.max_col,
                        activity.max_row,
                        resize,
                        editor_like_score(activity)
                    )
                })
                .collect::<Vec<_>>()
                .join("|")
        } else {
            String::new()
        };
        let mut target_editor_grid = match (
            self.editor_grid_id,
            selected_viewport_grid,
            active_grid_by_lines,
            selected_viewport_has_surface,
            previous_has_activity,
            active_grid_has_surface,
        ) {
            (Some(_), Some(viewport_grid), Some(active_grid), true, _, true)
                if active_grid != viewport_grid =>
            {
                active_grid
            }
            (Some(_), Some(viewport_grid), _, true, _, _) => viewport_grid,
            (Some(_), Some(_), Some(active_grid), false, _, true) => active_grid,
            (Some(_), None, Some(active_grid), _, _, true) => active_grid,
            (Some(current), _, _, _, _, _) => current,
            (None, Some(viewport_grid), Some(active_grid), true, _, true)
                if active_grid != viewport_grid =>
            {
                active_grid
            }
            (None, Some(viewport_grid), _, true, _, _) => viewport_grid,
            (None, _, Some(active_grid), _, _, true) => active_grid,
            // Neovim may emit `win_viewport` for grid 2 before any
            // `grid_line` content for the real editor grid arrives.
            // Default to the main grid until a content-bearing surface
            // proves otherwise; latching onto a viewport-only grid leaves
            // the editor blank and drops later real redraws.
            (None, _, _, _, _, _) => 1,
        };
        // If the selector still picked a blank/stale grid while another
        // full editor surface is drawing real content, recover in this
        // batch instead of leaving the pane blank until restart.
        let mut target_cells = grid_activity
            .get(&target_editor_grid)
            .map(|activity| activity.cells)
            .unwrap_or(0);
        let one_row_cells = editor_cols.max(1) as u32;
        let shadow_editor = grid_activity
            .iter()
            .filter(|(grid, activity)| {
                **grid != target_editor_grid
                    && editor_like_score(activity) >= 750_000
                    && activity.cells >= one_row_cells.saturating_mul(2)
            })
            .map(|(grid, _)| *grid)
            .next();
        let mut recovered_from_grid = None;
        if let Some(shadow_grid) = shadow_editor {
            if target_cells < one_row_cells {
                recovered_from_grid = Some(target_editor_grid);
                target_editor_grid = shadow_grid;
                target_cells = grid_activity
                    .get(&target_editor_grid)
                    .map(|activity| activity.cells)
                    .unwrap_or(0);
            }
        }
        let wedge_suspected = shadow_editor.is_some()
            && recovered_from_grid.is_none()
            && target_cells < one_row_cells;
        let recovered_editor_grid = recovered_from_grid.is_some();

        self.editor_grid_id = Some(target_editor_grid);
        let editor_grid_changed = previous_editor_grid != self.editor_grid_id;
        if recovered_editor_grid {
            if let Some(editor) = self.editor.as_ref() {
                editor.command("redraw!");
            }
        }

        // Always-on diagnostic so a live wedged process can be inspected
        // without relaunching or special env flags (the app logs to
        // /dev/null and the freeze watchdog is env-gated):
        //   cat ~/.config/neoism/log/editor-grid-<pid>.log
        // Records grid-id flips and any wedge we just self-healed. Writes
        // only on a state change (deduped per route), so steady-state cost
        // is zero.
        if editor_grid_changed || wedge_suspected || recovered_editor_grid {
            let summary = grid_activity
                .iter()
                .map(|(grid, activity)| {
                    format!(
                        "{grid}:cells{}:max{}x{}:score{}",
                        activity.cells,
                        activity.max_col,
                        activity.max_row,
                        editor_like_score(activity)
                    )
                })
                .collect::<Vec<_>>()
                .join("|");
            crate::app::editor_grid_diag::record(crate::app::editor_grid_diag::Record {
                route_id: self.route_id,
                from_grid: previous_editor_grid,
                to_grid: target_editor_grid,
                selected_viewport_grid,
                active_grid_by_lines,
                viewport_has_surface: selected_viewport_has_surface,
                previous_has_activity,
                active_has_surface: active_grid_has_surface,
                editor_cols,
                editor_rows,
                target_cells,
                shadow_editor,
                wedge_suspected,
                recovered_from_grid,
                grid_activity_summary: &summary,
            });
        }
        // hl_attr_define count — direct signal of treesitter / LSP
        // highlight churn. A nonzero count during a held-arrow scroll
        // means a highlighter is republishing styles on viewport
        // motion (treesitter incremental re-parse, or LSP semantic
        // tokens). On Rust files this typically lights up; on
        // flake.nix / Makefile (no LSP, no treesitter parser) it
        // stays zero — the symptom the user is chasing.
        let mut hl_attr_define_events = 0u32;
        let mut default_colors_set_events = 0u32;
        let mut cursor_goto_events = 0u32;
        let mut cursor_first_row: Option<u64> = None;
        let mut cursor_last_row: Option<u64> = None;
        let mut cursor_first_col: Option<u64> = None;
        let mut cursor_last_col: Option<u64> = None;
        let mut flush_events = 0u32;
        let mut mode_change_events = 0u32;
        let mut mode_changes: Vec<String> = Vec::new();
        let mut clear_events = 0u32;
        let mut resize_events = 0u32;
        let mut destroy_events = 0u32;
        let mut non_main_grid_events = 0u32;
        let mut viewport_line_count_changed = false;
        let mut ui_changed = editor_grid_changed || recovered_editor_grid;
        for event in &all_events {
            match event {
                RedrawEvent::WinViewport {
                    grid,
                    scroll_delta,
                    topline,
                    botline,
                    line_count,
                    curline,
                    curcol,
                    textoff,
                } => {
                    if selected_viewport_grid.is_some()
                        && Some(*grid) != selected_viewport_grid
                    {
                        non_main_grid_events = non_main_grid_events.saturating_add(1);
                        continue;
                    }
                    win_viewport_events = win_viewport_events.saturating_add(1);
                    // Only treat line_count change as "different buffer,
                    // wipe scrollback ring" when it shifts by a large
                    // amount. LSP virt_lines, gitsigns extmarks, fold
                    // recompute and similar tooling all wiggle nvim's
                    // reported line_count by a handful of rows on a big
                    // file — every wiggle tripped the reset, dropped the
                    // in-flight scroll spring, and produced a faint
                    // snap → repaint → snap stutter during held-arrow.
                    // Real buffer swaps (`:e`, `:bnext`, file delete)
                    // shift by far more than one viewport.
                    let prev_lc = self.editor_viewport_line_count;
                    let new_lc = *line_count;
                    let viewport_rows = (self
                        .editor_viewport_botline
                        .saturating_sub(self.editor_viewport_topline))
                    .max(1);
                    if prev_lc != 0 && prev_lc.abs_diff(new_lc) > viewport_rows {
                        viewport_line_count_changed = true;
                    }
                    if self.editor_viewport_topline != *topline
                        || self.editor_viewport_botline != *botline
                        || self.editor_viewport_line_count != *line_count
                        || scroll_delta.round() != 0.0
                    {
                        ui_changed = true;
                    }
                    // Always update viewport state so the renderer's
                    // at-edge detection has fresh data.
                    self.editor_viewport_topline = *topline;
                    self.editor_viewport_botline = *botline;
                    self.editor_viewport_line_count = *line_count;
                    // Buffer-coordinate caret for the presence plane —
                    // remote screens draw this pane's cursor from it.
                    self.editor_presence_line = *curline;
                    self.editor_presence_col = *curcol;
                    if *textoff != 0 {
                        self.editor_textoff = *textoff;
                    }

                    // ACCUMULATE the per-event scroll_delta across the
                    // batch. nvim's `win_viewport.scroll_delta` is the
                    // INCREMENTAL topline change since the previous
                    // win_viewport, so the net buffer movement for the
                    // drain is the SUM of the deltas — same as
                    // `grid_scroll_rows_total` above.
                    //
                    // The earlier code mirrored Neovide's
                    // `self.scroll_delta = scroll_delta.round()`
                    // (assign-last) on the theory that nvim emits one
                    // WinViewport per redraw. But our pump drains ALL
                    // pending notifications at once and a single batch
                    // can span multiple redraw/Flush cycles (see
                    // `flush_events`). When ≥2 WinViewports land in one
                    // drain, assign-last kept only the final delta while
                    // `visible_rows` reflected the cumulative scroll, so
                    // `flush_editor_scrollback` advanced the scrollback
                    // ring origin by LESS than the content actually
                    // moved. That broke the ring's overlap invariant
                    // (origin step must equal content step) and the
                    // spring then sampled misaligned ring rows — the
                    // intermittent "fake lines / hidden lines that
                    // reappear on the next scroll" glitch during fast
                    // Ctrl-D/U. Summing keeps the ring origin aligned
                    // with the buffer and matches the documented
                    // "sum of scroll rows" model of
                    // `editor_pending_scroll_lines`. In steady state
                    // (one WinViewport per drain) the sum equals the
                    // single delta, so smooth scroll is unchanged.
                    let delta = scroll_delta.round() as i32;
                    total_scroll_delta = total_scroll_delta.saturating_add(delta);
                }
                RedrawEvent::GridLine {
                    grid, row, cells, ..
                } => {
                    if *grid != target_editor_grid {
                        non_main_grid_events = non_main_grid_events.saturating_add(1);
                        continue;
                    }
                    grid_line_events = grid_line_events.saturating_add(1);
                    grid_line_first_row =
                        Some(grid_line_first_row.map_or(*row, |v| v.min(*row)));
                    grid_line_last_row =
                        Some(grid_line_last_row.map_or(*row, |v| v.max(*row)));
                    grid_line_unique_rows.insert(*row);
                    grid_line_cells = grid_line_cells
                        .saturating_add(cells.len().min(u32::MAX as usize) as u32);
                    let repeat_sum = cells
                        .iter()
                        .map(|cell| {
                            cell.repeat.unwrap_or(1).min(u64::from(u32::MAX)) as u32
                        })
                        .fold(0u32, u32::saturating_add);
                    grid_line_repeat_cells =
                        grid_line_repeat_cells.saturating_add(repeat_sum);
                }
                RedrawEvent::Scroll { grid, rows, .. } => {
                    if *grid != target_editor_grid {
                        non_main_grid_events = non_main_grid_events.saturating_add(1);
                        continue;
                    }
                    grid_scroll_events = grid_scroll_events.saturating_add(1);
                    grid_scroll_rows_total = grid_scroll_rows_total.saturating_add(
                        (*rows).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                    );
                }
                RedrawEvent::CursorGoto { grid, row, column } => {
                    if *grid != target_editor_grid {
                        non_main_grid_events = non_main_grid_events.saturating_add(1);
                        continue;
                    }
                    cursor_goto_events = cursor_goto_events.saturating_add(1);
                    if cursor_first_row.is_none() {
                        cursor_first_row = Some(*row);
                        cursor_first_col = Some(*column);
                    }
                    cursor_last_row = Some(*row);
                    cursor_last_col = Some(*column);
                }
                RedrawEvent::Flush => {
                    flush_events = flush_events.saturating_add(1);
                }
                RedrawEvent::ModeChange { .. } => {
                    mode_change_events = mode_change_events.saturating_add(1);
                    if scroll_log_enabled {
                        if let RedrawEvent::ModeChange { mode, .. } = event {
                            mode_changes.push(format!("{mode:?}"));
                        }
                    }
                }
                RedrawEvent::Clear { grid } => {
                    if *grid != target_editor_grid {
                        non_main_grid_events = non_main_grid_events.saturating_add(1);
                        continue;
                    }
                    clear_events = clear_events.saturating_add(1);
                }
                RedrawEvent::Resize { grid, .. } => {
                    if *grid != target_editor_grid {
                        non_main_grid_events = non_main_grid_events.saturating_add(1);
                        continue;
                    }
                    resize_events = resize_events.saturating_add(1);
                }
                RedrawEvent::Destroy { grid } => {
                    if *grid != target_editor_grid {
                        non_main_grid_events = non_main_grid_events.saturating_add(1);
                        continue;
                    }
                    destroy_events = destroy_events.saturating_add(1);
                }
                RedrawEvent::HighlightAttributesDefine { .. } => {
                    hl_attr_define_events = hl_attr_define_events.saturating_add(1);
                }
                RedrawEvent::DefaultColorsSet { .. } => {
                    default_colors_set_events =
                        default_colors_set_events.saturating_add(1);
                }
                _ => {}
            }
        }

        for event in &all_events {
            match event {
                RedrawEvent::PopupMenuShow { menu } => {
                    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                        tracing::info!(
                            target: "neoism::lsp",
                            route_id = self.route_id,
                            items = menu.items.len(),
                            selected = menu.selected,
                            row = menu.row,
                            col = menu.col,
                            grid = menu.grid,
                            "popupmenu state show"
                        );
                    }
                    if self.editor_popup_menu.as_ref() != Some(menu) {
                        self.editor_popup_menu = Some(menu.clone());
                        ui_changed = true;
                    }
                }
                RedrawEvent::PopupMenuSelect { selected } => {
                    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                        tracing::info!(
                            target: "neoism::lsp",
                            route_id = self.route_id,
                            selected = *selected,
                            "popupmenu state select"
                        );
                    }
                    if let Some(menu) = &mut self.editor_popup_menu {
                        if menu.selected != *selected {
                            menu.selected = *selected;
                            ui_changed = true;
                        }
                    }
                }
                RedrawEvent::PopupMenuHide => {
                    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                        tracing::info!(
                            target: "neoism::lsp",
                            route_id = self.route_id,
                            was_visible = self.editor_popup_menu.is_some(),
                            "popupmenu state hide"
                        );
                    }
                    if self.editor_popup_menu.take().is_some() {
                        ui_changed = true;
                    }
                }
                _ => {}
            }
        }

        let mut title_out = None;
        let mut terminal = self.terminal.lock();
        let n = apply_redraw_events(
            &mut terminal,
            &mut self.editor_hl_table,
            &mut self.editor_default_colors,
            &mut title_out,
            &all_events,
            target_editor_grid,
        );
        let clear_requires_scrollback_reset =
            clear_events != 0 && total_scroll_delta == 0;
        let reset_editor_scrollback = clear_requires_scrollback_reset
            || recovered_editor_grid
            || resize_events != 0
            || destroy_events != 0
            || viewport_line_count_changed;
        if reset_editor_scrollback {
            terminal.clear_editor_scrollback();
            self.editor_pending_scroll_lines = 0;
            self.editor_pending_grid_scroll_lines = 0;
            self.editor_scroll_reset_pending = true;
        }
        if viewport_line_count_changed {
            terminal.mark_fully_damaged();
        }
        let damage_after_apply = terminal.peek_damage_event();
        let (damage_kind, damage_lines, damage_first_line, damage_last_line) =
            match &damage_after_apply {
                Some(neoism_terminal_core::damage::TerminalDamage::Full) => {
                    ("full", 0u32, None, None)
                }
                Some(neoism_terminal_core::damage::TerminalDamage::CursorOnly) => {
                    ("cursor_only", 0u32, None, None)
                }
                Some(neoism_terminal_core::damage::TerminalDamage::Noop) => {
                    ("noop", 0u32, None, None)
                }
                Some(neoism_terminal_core::damage::TerminalDamage::Partial(lines)) => {
                    let first = lines.iter().next().map(|line| line.line);
                    let last = lines.iter().next_back().map(|line| line.line);
                    (
                        "partial",
                        lines.len().min(u32::MAX as usize) as u32,
                        first,
                        last,
                    )
                }
                None => ("none", 0u32, None, None),
            };
        let suppress_mode_viewport_scroll = mode_change_events != 0
            && grid_scroll_events == 0
            && total_scroll_delta.abs() == 1;
        let mut model_scroll_delta = if suppress_mode_viewport_scroll {
            0
        } else {
            total_scroll_delta
        };
        let mut consumed_pending_grid_scroll = 0i32;
        if model_scroll_delta != 0 && self.editor_pending_grid_scroll_lines != 0 {
            let pending_before = self.editor_pending_grid_scroll_lines;
            let (next_pending, remaining_viewport) =
                editor_consume_pending_grid_scroll_animation(
                    self.editor_pending_grid_scroll_lines,
                    model_scroll_delta,
                );
            self.editor_pending_grid_scroll_lines = next_pending;
            consumed_pending_grid_scroll = pending_before.saturating_sub(next_pending);
            model_scroll_delta = remaining_viewport;
        }
        let seeded_from_grid_scroll_only = model_scroll_delta == 0
            && total_scroll_delta == 0
            && win_viewport_events == 0
            && grid_scroll_rows_total != 0;
        if seeded_from_grid_scroll_only {
            model_scroll_delta = grid_scroll_rows_total;
            self.editor_pending_grid_scroll_lines = self
                .editor_pending_grid_scroll_lines
                .saturating_add(grid_scroll_rows_total);
        }
        let scrollback_ready = terminal.flush_editor_scrollback(model_scroll_delta);
        let scrollback_origin_after_flush = scroll_log_enabled
            .then(|| terminal.editor_scrollback_origin())
            .flatten();
        drop(terminal);

        if model_scroll_delta != 0 && scrollback_ready {
            self.editor_pending_scroll_lines = self
                .editor_pending_scroll_lines
                .saturating_add(model_scroll_delta);
        }

        // SetTitle: title bar wiring lives elsewhere (Phase 3 plumbs
        // this into the buffer_tabs strip); for now just log.
        if let Some(title) = title_out {
            tracing::trace!(target: "neoism::nvim_pump", "editor pane title: {title}");
        }

        if n > 0 || ui_changed {
            self.renderable_content.pending_update.set_dirty();
        }

        if scroll_log_enabled
            && (total_scroll_delta != 0
                || grid_line_events != 0
                || grid_scroll_events != 0
                || mode_change_events != 0
                || hit_frame_limit)
        {
            tracing::info!(
                target: "neoism::nvim_scroll_batch",
                route_id = self.route_id,
                drained_notifications,
                drained_events = all_events.len(),
                hit_frame_limit,
                applied_events = n,
                ui_changed,
                target_editor_grid,
                selected_viewport_grid = ?selected_viewport_grid,
                selected_viewport_has_surface,
                active_grid_by_lines = ?active_grid_by_lines,
                active_grid_has_surface,
                previous_has_activity,
                grid_activity = %grid_activity_summary,
                editor_grid_changed,
                win_viewport_events,
                total_scroll_delta,
                model_scroll_delta,
                suppress_mode_viewport_scroll,
                seeded_from_grid_scroll_only,
                consumed_pending_grid_scroll,
                pending_grid_scroll_lines = self.editor_pending_grid_scroll_lines,
                scrollback_ready,
                scrollback_origin_after_flush = ?scrollback_origin_after_flush,
                pending_scroll_lines = self.editor_pending_scroll_lines,
                topline = self.editor_viewport_topline,
                botline = self.editor_viewport_botline,
                line_count = self.editor_viewport_line_count,
                grid_scroll_events,
                grid_scroll_rows_total,
                grid_line_events,
                grid_line_unique_rows = grid_line_unique_rows.len(),
                grid_line_first_row = ?grid_line_first_row,
                grid_line_last_row = ?grid_line_last_row,
                grid_line_cells,
                grid_line_repeat_cells,
                hl_attr_define_events,
                default_colors_set_events,
                cursor_goto_events,
                cursor_first_row = ?cursor_first_row,
                cursor_first_col = ?cursor_first_col,
                cursor_last_row = ?cursor_last_row,
                cursor_last_col = ?cursor_last_col,
                flush_events,
                mode_change_events,
                mode_changes = %mode_changes.join(","),
                clear_events,
                resize_events,
                destroy_events,
                non_main_grid_events,
                viewport_line_count_changed,
                damage_kind,
                damage_lines,
                damage_first_line = ?damage_first_line,
                damage_last_line = ?damage_last_line,
                "nvim editor scroll/redraw batch"
            );
        }

        (n + usize::from(ui_changed), hit_frame_limit)
    }
}
