//! Desktop hosting for working-tree review and historical Git diff tabs.
use super::super::*;

/// Grid layout already excludes the status bar through its bottom margin.
/// Use this same logical viewport for painting and every pointer input path.
fn git_diff_viewport(
    layout: [f32; 4],
    origin: [f32; 2],
    scale: f32,
) -> neoism_ui::layout::Rect {
    neoism_ui::layout::Rect::new(
        (origin[0] + layout[0]) / scale,
        (origin[1] + layout[1]) / scale,
        layout[2] / scale,
        layout[3] / scale,
    )
}

/// Extend only window-facing slot edges into terminal padding. Keep internal
/// split boundaries intact, and paint below tab chrome without moving content.
fn git_diff_backdrop(
    slot: neoism_ui::layout::Rect,
    grid: neoism_ui::layout::Rect,
    frame: neoism_ui::layout::Rect,
) -> neoism_ui::layout::Rect {
    let tolerance = 0.5;
    let x = if slot.x <= grid.x + tolerance {
        frame.x
    } else {
        slot.x
    }
    .max(frame.x);
    let y = if slot.y <= grid.y + tolerance {
        frame.y
    } else {
        slot.y
    }
    .max(frame.y);
    let right = if slot.x + slot.w >= grid.x + grid.w - tolerance {
        frame.x + frame.w
    } else {
        slot.x + slot.w
    }
    .min(frame.x + frame.w);
    let bottom = if slot.y + slot.h >= grid.y + grid.h - tolerance {
        frame.y + frame.h
    } else {
        slot.y + slot.h
    }
    .min(frame.y + frame.h);
    neoism_ui::layout::Rect::new(x, y, (right - x).max(0.0), (bottom - y).max(0.0))
}

/// Wheel deltas have the opposite sign to the shared view's scroll offsets.
/// Preserve native horizontal gestures; only remap vertical-only Shift+wheel.
fn git_diff_scroll_delta(x: f32, y: f32, shift: bool) -> (f32, f32) {
    if shift && x.abs() <= 0.01 {
        (-y, 0.0)
    } else {
        (-x, -y)
    }
}

/// Normalize once, while the native event still carries its device class.
/// Vertical pixels use the same event units as code.scroll_pixels and the
/// native agent timeline (including OS-provided acceleration/momentum). Do not
/// divide that axis by DPI here: those canonical consumers do not either.
/// Horizontal overflow keeps its existing logical-pixel / Shift-wheel policy.
fn git_diff_native_wheel(
    delta: &neoism_window::event::MouseScrollDelta,
    scale: f32,
    line_height: f32,
    shift: bool,
) -> (f32, f32) {
    use neoism_ui::editor::scroll_model::agent_timeline_wheel;
    use neoism_ui::panels::completion_menu::ScrollDelta;
    use neoism_window::event::MouseScrollDelta;

    let (x, y, vertical) = match delta {
        MouseScrollDelta::LineDelta(x, y) => {
            (*x * 48.0, *y * 48.0, ScrollDelta::Lines { x: *x, y: *y })
        }
        MouseScrollDelta::PixelDelta(pos) => (
            pos.x as f32 / scale,
            pos.y as f32 / scale,
            ScrollDelta::Pixels {
                x: 0.0,
                y: pos.y as f32,
            },
        ),
    };
    if shift && x.abs() <= 0.01 {
        (y, 0.0)
    } else {
        (x, agent_timeline_wheel(&vertical, line_height).pixels)
    }
}

impl Screen<'_> {
    pub(crate) fn clear_git_diff_view_hover(&mut self) -> bool {
        let mut changed = false;
        for grid in self.context_manager.all_grids_mut() {
            for item in grid.contexts_mut().values_mut() {
                if let Some(view) = item.val.git_diff_view.as_mut() {
                    changed |= view.pointer_leave();
                }
            }
        }
        changed
    }

    /// Navigate the existing document owner; a Markdown owner must never be
    /// duplicated or replaced merely to present a diff's source coordinate.
    fn open_git_diff_source_location(
        &mut self,
        target: PathBuf,
        line: usize,
        column: usize,
    ) {
        if self
            .context_manager
            .markdown_pane_mut_by_path(&target)
            .is_some()
        {
            self.open_markdown_source_location(target, line, column);
        } else {
            self.open_code_source_location(target, line, column);
        }
    }
    pub(crate) fn handle_git_diff_file_navigation(
        &mut self,
        next: bool,
        pressed: bool,
    ) -> bool {
        if self.context_manager.current().git_diff_view.is_none()
            || self.is_island_strip_focused()
            || self.focused_buffer_tabs_strip().is_some()
            || self.renderer.git_diff_panel.is_focused()
            || self.renderer.git_diff_panel.actions_menu_is_open()
            || self.renderer.git_diff_panel.branch_menu_is_open()
            || self.renderer.file_tree.is_focused()
            || self.renderer.notes_sidebar.is_focused()
            || (self.renderer.conversations_visible
                && self.renderer.conversations_pane.side_panel().is_focused())
            || self.renderer.command_palette.is_enabled()
            || self.renderer.finder.is_enabled()
            || self.renderer.modal.owns_editor_focus()
        {
            return false;
        }
        if pressed {
            if let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut()
            {
                view.navigate_file(next);
            }
            self.mark_dirty();
        }
        true
    }

    pub(crate) fn focus_git_diff_column(&mut self, column: usize) -> bool {
        let node = {
            let grid = self.context_manager.current_grid();
            if grid.current().git_diff_view.is_some() {
                Some(grid.current)
            } else {
                let root = self.renderer.git_diff_panel.repo_root();
                grid.contexts()
                    .iter()
                    .filter(|(node, item)| {
                        grid.is_context_visible(**node)
                            && item.val.git_diff_view.as_ref().is_some_and(|view| {
                                view.commit_id.is_none()
                                    && root.as_ref() == Some(&view.root)
                            })
                    })
                    .max_by(|(_, a), (_, b)| a.slot_rect[0].total_cmp(&b.slot_rect[0]))
                    .map(|(node, _)| *node)
            }
        };
        let Some(node) = node else {
            return false;
        };
        self.context_manager
            .current_grid_mut()
            .set_current_node(node, &mut self.sugarloaf);
        self.context_manager.select_route_from_current_grid();
        self.focus_selected_workspace_content();
        let request = {
            let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut()
            else {
                return false;
            };
            if !view.focus_column(column) {
                return false;
            }
            view.take_edit_request()
        };
        if let Some(request) = request {
            self.acquire_git_diff_editor(request, false);
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn move_git_diff_column_focus(&mut self, right: bool) -> bool {
        let request = {
            let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut()
            else {
                return false;
            };
            if !view.move_column_focus(right) {
                return false;
            }
            view.take_edit_request()
        };
        self.focus_selected_workspace_content();
        if let Some(request) = request {
            self.acquire_git_diff_editor(request, false);
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn blur_git_diff_columns(&mut self) {
        if let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut() {
            view.blur_columns();
        }
    }

    /// Raw wheel path must run before terminal axis snapping / cell conversion.
    pub(crate) fn handle_git_diff_view_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        let line_height = self
            .sugarloaf
            .get_text_layout(&self.context_manager.current().rich_text_id)
            .map(|layout| layout.dimensions.height.round().max(1.0))
            .unwrap_or(24.0);
        let (dx, dy) = git_diff_native_wheel(
            delta,
            self.sugarloaf.scale_factor(),
            line_height,
            self.modifiers.state().shift_key(),
        );
        self.handle_git_diff_view_scroll(dx, dy)
    }

    pub(crate) fn take_git_commit_copy(&mut self) -> Option<String> {
        self.renderer
            .git_diff_panel
            .take_latest_commit_copy()
            .or_else(|| {
                self.context_manager
                    .current_grid_mut()
                    .contexts_mut()
                    .values_mut()
                    .find_map(|item| {
                        item.val
                            .git_diff_view
                            .as_mut()
                            .and_then(|view| view.take_commit_copy())
                    })
            })
    }

    pub(crate) fn open_git_commit_detail(
        &mut self,
        root: PathBuf,
        detail: neoism_protocol::git::GitCommitDetail,
    ) {
        let title = format!(
            "{} - {}",
            detail.sha.chars().take(7).collect::<String>(),
            detail.message.lines().next().unwrap_or("")
        );
        let existing = self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .find_map(|item| {
                item.val
                    .git_diff_view
                    .as_ref()
                    .filter(|view| {
                        view.root == root
                            && view.commit_id.as_deref() == Some(&detail.sha)
                    })
                    .map(|_| item.val.route_id)
            });
        let route = if let Some(route) = existing {
            route
        } else {
            let rich_text = crate::context::factories::next_rich_text_id();
            let _ = self.sugarloaf.text(Some(rich_text));
            let Some(route) = self.context_manager.add_stacked_git_diff(
                root.clone(),
                PathBuf::new(),
                rich_text,
                &mut self.sugarloaf,
            ) else {
                return;
            };
            if let Some(item) = self
                .context_manager
                .current_grid_mut()
                .contexts_mut()
                .values_mut()
                .find(|item| item.val.route_id == route)
            {
                item.val.git_diff_view =
                    Some(neoism_ui::panels::git_diff_view::GitDiffView::from_commit(
                        root, detail,
                    ));
                item.val.git_diff_load_deadline = None;
            }
            route
        };
        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.activate_git_diff_route(route);
        let index = self
            .renderer
            .buffer_tabs
            .open_git_diff(route, std::path::Path::new(""));
        self.renderer.buffer_tabs.set_title(index, &title);
        self.renderer.file_tree.set_active_path(None);
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    /// Reveal only in the current working review. Unlike open_git_diff_file,
    /// this never creates/activates tabs or changes sidebar focus or buffers.
    pub(crate) fn reveal_existing_git_diff_file(
        &mut self,
        root: &Path,
        path: &Path,
    ) -> bool {
        use neoism_ui::panels::buffer_tabs::{BufferTabTarget, ChromePageKind};
        let route = match self
            .renderer
            .buffer_tabs
            .target_at(self.renderer.buffer_tabs.active())
        {
            Some(BufferTabTarget::ChromePage(page))
                if page.kind == ChromePageKind::GitDiff =>
            {
                page.route_id
            }
            _ => return false,
        };
        if self.context_manager.current().route_id != route {
            return false;
        }
        let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut() else {
            return false;
        };
        if view.root != root || view.commit_id.is_some() {
            return false;
        }
        view.set_edit_focus(false);
        view.reveal_file(path);
        self.mark_dirty();
        true
    }

    pub(crate) fn open_git_diff_file(&mut self, root: PathBuf, path: PathBuf) {
        // Panel targets are absolute; the view/cache key must remain repo-relative.
        let path = neoism_protocol::host_path::HostPath::new(root.to_string_lossy())
            .relative(&path.to_string_lossy())
            .map(PathBuf::from)
            .unwrap_or(path);
        let existing = self
            .context_manager
            .current_grid()
            .contexts()
            .iter()
            .find_map(|(_, item)| {
                item.val
                    .git_diff_view
                    .as_ref()
                    .filter(|view| view.root == root && view.commit_id.is_none())
                    .map(|_| item.val.route_id)
            });
        self.renderer.buffer_tabs.ensure_terminal_tab();
        let route = if let Some(route) = existing {
            route
        } else {
            let rich_text_id = crate::context::factories::next_rich_text_id();
            let _ = self.sugarloaf.text(Some(rich_text_id));
            let Some(route) = self.context_manager.add_stacked_git_diff(
                root.clone(),
                path.clone(),
                rich_text_id,
                &mut self.sugarloaf,
            ) else {
                return;
            };
            route
        };
        let files = (self.renderer.git_diff_panel.repo_root().as_ref() == Some(&root))
            .then(|| self.renderer.git_diff_panel.files());
        for item in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .values_mut()
        {
            if item.val.route_id == route {
                if let Some(view) = item.val.git_diff_view.as_mut() {
                    if let Some(files) = &files {
                        view.set_files(files.clone());
                    }
                    view.reveal_file(&path);
                }
                // A click also retries a previously timed-out repository load.
                item.val.git_diff_cache_generation = None;
                item.val.git_diff_load_deadline =
                    Some(web_time::Instant::now() + std::time::Duration::from_secs(15));
            }
        }
        self.activate_git_diff_route(route);
        let index = self.renderer.buffer_tabs.open_git_diff(route, &path);
        self.renderer
            .buffer_tabs
            .set_title(index, "Uncommitted Changes");
        self.sync_review_document_modified();
        self.renderer.file_tree.set_active_path(None);
        self.renderer.file_tree.set_focused(false);
        self.renderer.git_diff_panel.set_focused(false);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    pub(crate) fn activate_git_diff_route(&mut self, route: usize) -> bool {
        let node = self
            .context_manager
            .current_grid()
            .contexts()
            .iter()
            .find_map(|(node, item)| {
                (item.val.route_id == route && item.val.git_diff_view.is_some())
                    .then_some(*node)
            });
        let Some(node) = node else {
            return false;
        };
        let _ = self
            .context_manager
            .current_grid_mut()
            .set_current_node(node, &mut self.sugarloaf);
        self.context_manager.select_route_from_current_grid();
        self.renderer.git_diff_panel.set_focused(false);
        self.renderer.notes_sidebar.set_focused(false);
        true
    }

    pub(crate) fn render_git_diff_views(&mut self) -> bool {
        for (root, detail) in self.renderer.git_diff_panel.take_commit_details() {
            self.open_git_commit_detail(root, detail);
        }
        let mut loading = self.poll_review_stage_save();
        let scale = self.sugarloaf.scale_factor();
        let theme = self.renderer.theme;
        let font_size = 13.0 * self.renderer.chrome_scale();
        let size = self.sugarloaf.window_size();
        let occlusions =
            self.renderer
                .active_text_occlusion_rects(size.width, size.height, scale);
        let (visible, margin, grid_bounds) = {
            let grid = self.context_manager.current_grid();
            let visible = grid
                .contexts()
                .keys()
                .copied()
                .filter(|node| grid.is_context_visible(*node))
                .collect::<Vec<_>>();
            let margin = grid.scaled_margin;
            let bounds = visible
                .iter()
                .filter_map(|node| grid.contexts().get(node))
                .map(|item| {
                    git_diff_viewport(item.slot_rect, [margin.left, margin.top], scale)
                })
                .reduce(|a, b| {
                    let x = a.x.min(b.x);
                    let y = a.y.min(b.y);
                    neoism_ui::layout::Rect::new(
                        x,
                        y,
                        (a.x + a.w).max(b.x + b.w) - x,
                        (a.y + a.h).max(b.y + b.h) - y,
                    )
                });
            (visible, margin, bounds)
        };
        let (band_top, band_bottom) = self.side_panel_band();
        let frame_left = self.chrome_x_offset();
        let frame_right = size.width as f32 / scale - self.chrome_x_offset_right();
        let backdrop_frame = neoism_ui::layout::Rect::new(
            frame_left,
            band_top,
            (frame_right - frame_left).max(0.0),
            (band_bottom - band_top).max(0.0),
        );
        let panel_root = self
            .renderer
            .git_diff_panel
            .repo_root()
            .map(|p| p.to_path_buf());
        if self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .any(|item| {
                item.val.git_diff_view.as_ref().is_some_and(|view| {
                    view.commit_id.is_none() && panel_root.as_ref() == Some(&view.root)
                })
            })
        {
            self.renderer.git_diff_panel.request_review_diffs();
        }
        for item in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .values_mut()
        {
            let Some(view) = item.val.git_diff_view.as_mut() else {
                continue;
            };
            // Keep the repository snapshot after the sidebar closes or changes roots.
            // The panel is the sole status/cache owner; its request API only queues
            // work (bounded native worker or host RPC), never runs Git here.
            if view.commit_id.is_none() && panel_root.as_ref() == Some(&view.root) {
                let generation = self.renderer.git_diff_panel.diff_cache_generation();
                if item.val.git_diff_cache_generation != Some(generation) {
                    let files = self.renderer.git_diff_panel.files();
                    view.set_files(files.clone());
                    item.val.git_diff_cache_generation = Some(generation);
                    let mut pending = self.renderer.git_diff_panel.loading();
                    for file in &files {
                        let path = std::path::Path::new(&file.path);
                        if let Some(lines) =
                            self.renderer.git_diff_panel.diff_lines_for_path(path)
                        {
                            view.set_file_lines(path, lines);
                        } else {
                            pending = true;
                        }
                    }
                    item.val.git_diff_load_deadline = pending.then(|| {
                        web_time::Instant::now() + std::time::Duration::from_secs(15)
                    });
                }
            }
            if let Some(deadline) = item.val.git_diff_load_deadline {
                if web_time::Instant::now() < deadline {
                    loading = true;
                } else {
                    view.set_error("Timed out loading repository changes. Click a file in the Git sidebar to retry.".into());
                    item.val.git_diff_load_deadline = None;
                }
            }
        }
        let focused_route = self.context_manager.current().route_id;
        let chrome_owns_focus = self.renderer.git_diff_panel.is_focused()
            || self.renderer.file_tree.is_focused()
            || self.renderer.notes_sidebar.is_focused()
            || self.renderer.buffer_tabs.is_focused()
            || self
                .renderer
                .pane_tabs
                .values()
                .any(|tabs| tabs.is_focused())
            || (self.renderer.conversations_visible
                && self.renderer.conversations_pane.side_panel().is_focused())
            || self.renderer.command_palette.is_enabled()
            || self.renderer.finder.is_enabled()
            || self.renderer.modal.owns_editor_focus();
        // Take only the view out while borrowing its canonical context-owned
        // editor. The CodePane itself never moves out of the IO/CRDT owner.
        for node in visible {
            let (mut view, rect, slot, view_focused) = {
                let Some(item) = self
                    .context_manager
                    .current_grid_mut()
                    .contexts_mut()
                    .get_mut(&node)
                else {
                    continue;
                };
                let Some(view) = item.val.git_diff_view.take() else {
                    continue;
                };
                (
                    view,
                    git_diff_viewport(item.layout_rect, [margin.left, margin.top], scale),
                    git_diff_viewport(item.slot_rect, [margin.left, margin.top], scale),
                    item.val.route_id == focused_route && !chrome_owns_focus,
                )
            };
            view.set_host_focused(view_focused);
            if let Some(bounds) = grid_bounds {
                let backing = git_diff_backdrop(slot, bounds, backdrop_frame);
                self.sugarloaf.quad(
                    None,
                    backing.x,
                    backing.y,
                    backing.w,
                    backing.h,
                    theme.f32(theme.bg),
                    [0.0; 4],
                    0.0,
                    neoism_ui::panels::buffer_tabs::consts::ORDER_BG.saturating_sub(1),
                );
            }
            view.set_text_occlusions(&occlusions);
            let path = view.document_path();
            if view.commit_id.is_none() && panel_root.as_ref() == Some(&view.root) {
                if let Some(path) = path.as_ref() {
                    let relative = neoism_protocol::host_path::HostPath::new(
                        view.root.to_string_lossy(),
                    )
                    .relative(&path.to_string_lossy())
                    .map(PathBuf::from);
                    if let Some(relative) = relative {
                        self.renderer.git_diff_panel.request_diff_base(&relative);
                        if let Some(text) =
                            self.renderer.git_diff_panel.diff_base_for_path(&relative)
                        {
                            view.set_base_text(&relative, text);
                        } else if let Some(error) =
                            self.renderer.git_diff_panel.diff_base_error(&relative)
                        {
                            view.set_error(format!(
                                "Could not load review base: {error}"
                            ));
                        }
                        loading |=
                            self.renderer.git_diff_panel.diff_base_pending(&relative);
                    }
                }
            }
            let editor = path
                .as_ref()
                .and_then(|path| self.context_manager.code_pane_mut_by_path(path));
            let mut editor = editor;
            if let Some(code) = editor.as_deref_mut() {
                code.caret_drawn_by_host = true;
                code.cursor_rect = None;
                // These belong to the ordinary full-document viewport, not
                // the review projection. Its own scrollbars keep capture.
                code.scrollbar_track = None;
                code.scrollbar_thumb = None;
                code.plugin_hit_regions.clear();
                if !code.remote_content_pending
                    && code.error.is_none()
                    && self
                        .pending_review_code_cursor
                        .as_ref()
                        .is_some_and(|(path, _, _)| path == &code.path)
                {
                    if let Some((_, line, column)) =
                        self.pending_review_code_cursor.take()
                    {
                        code.buffer.set_cursor_position(line, column, false);
                    }
                }
            }
            // Keep the actual patch visible until the canonical source has
            // loaded. Never project an empty/error placeholder as working text.
            let editor = editor
                .filter(|code| !code.remote_content_pending && code.error.is_none());
            view.render_with_editor(
                &mut self.sugarloaf,
                rect,
                0,
                font_size,
                &theme,
                editor,
            );
            loading |= view.needs_redraw();
            if let Some(item) = self
                .context_manager
                .current_grid_mut()
                .contexts_mut()
                .get_mut(&node)
            {
                item.val.git_diff_view = Some(view);
            }
        }
        loading
    }

    pub(crate) fn handle_git_diff_view_pointer_down(
        &mut self,
        button: neoism_window::event::MouseButton,
    ) -> bool {
        let [x, y] = self.markdown_mouse_logical();
        let scale = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        if self
            .renderer
            .active_text_occlusion_rects(size.width, size.height, scale)
            .iter()
            .any(|r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3])
        {
            return false;
        }
        let grid = self.context_manager.current_grid();
        let target = grid.contexts().iter().find_map(|(node, item)| {
            (grid.is_context_visible(*node)
                && item.val.git_diff_view.is_some()
                && git_diff_viewport(
                    item.layout_rect,
                    [grid.scaled_margin.left, grid.scaled_margin.top],
                    scale,
                )
                .contains(x, y))
            .then_some(*node)
        });
        let Some(node) = target else {
            return false;
        };
        // A split may contain more than one review surface; dispatch the
        // header click to the pane under the pointer, not the previous focus.
        let _ = self
            .context_manager
            .current_grid_mut()
            .set_current_node(node, &mut self.sugarloaf);
        self.context_manager.select_route_from_current_grid();
        self.renderer.git_diff_panel.set_focused(false);
        let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut() else {
            return false;
        };
        if button == neoism_window::event::MouseButton::Left {
            view.pointer_down(x, y);
            let edit_request = view.take_edit_request();
            let actions = view.take_actions();
            for action in actions {
                use neoism_ui::panels::git_diff_view::GitDiffViewAction;
                match action {
                    GitDiffViewAction::OpenSource {
                        root,
                        path,
                        line,
                        column,
                    } => {
                        let target = PathBuf::from(
                            neoism_protocol::host_path::HostPath::new(
                                root.to_string_lossy(),
                            )
                            .join(&path)
                            .as_str(),
                        );
                        self.open_git_diff_source_location(target, line, column);
                        // Do not reacquire inline edit focus after leaving review.
                        return true;
                    }
                    GitDiffViewAction::Stage {
                        root,
                        path,
                        unstage,
                    } => self.stage_review_path(root, path.into(), unstage),
                    GitDiffViewAction::StageAll { root } => {
                        if self.renderer.git_diff_panel.repo_root().as_ref()
                            == Some(&root)
                            && !self.renderer.git_diff_panel.loading()
                        {
                            self.stage_review_all(root);
                        }
                    }
                    GitDiffViewAction::Commit { root } => {
                        if self.renderer.git_diff_panel.repo_root().as_ref()
                            == Some(&root)
                            && !self.renderer.git_diff_panel.loading()
                        {
                            if !self.renderer.git_diff_panel.is_visible() {
                                let branch = self.renderer.git_diff_panel.branch();
                                self.renderer.git_diff_panel.open(Some(root), branch);
                            }
                            self.renderer.git_diff_panel.focus_commit_box(true);
                        }
                    }
                    GitDiffViewAction::ChangeBlock {
                        root,
                        path,
                        block,
                        restore,
                    } => {
                        if self.renderer.git_diff_panel.repo_root().as_ref()
                            != Some(&root)
                            || self.renderer.git_diff_panel.loading()
                        {
                            continue;
                        }
                        if restore {
                            if !self.refuse_dirty_review_restore(
                                &root,
                                std::path::Path::new(&path),
                            ) {
                                self.renderer.modal.open(neoism_ui::panels::git_diff::review_actions::restore_block_confirmation(root, path, block));
                            }
                        } else {
                            self.stage_review_block(root, path, block);
                        }
                    }
                    GitDiffViewAction::Restore { root, path } => {
                        if self.refuse_dirty_review_restore(
                            &root,
                            std::path::Path::new(&path),
                        ) {
                            continue;
                        }
                        if self.renderer.git_diff_panel.repo_root().as_ref()
                            == Some(&root)
                            && !self.renderer.git_diff_panel.loading()
                        {
                            self.renderer.modal.open(neoism_ui::panels::git_diff::review_actions::restore_confirmation(root, path));
                        }
                    }
                }
            }
            if let Some(request) = edit_request {
                let (line, column) = (request.line, request.column);
                if self.acquire_git_diff_editor(request, true) {
                    // The displayed review already resolved this press. Reusing
                    // canonical geometry here can target a different excerpt.
                    self.handle_code_mouse_press_at_resolved(button, line, column);
                }
            }
            self.mark_dirty();
        }
        true
    }

    /// The review tab remains selected. Only a real context may own text,
    /// history and CRDT state; the shared view borrows it for projection.
    fn acquire_git_diff_editor(
        &mut self,
        request: neoism_ui::panels::git_diff_view::GitDiffEditRequest,
        pointer: bool,
    ) -> bool {
        let path = PathBuf::from(
            neoism_protocol::host_path::HostPath::new(request.root.to_string_lossy())
                .join(&request.path.to_string_lossy())
                .as_str(),
        );
        let rich_text_id = if self.context_manager.code_pane_mut_by_path(&path).is_none()
        {
            let id = crate::context::factories::next_rich_text_id();
            let _ = self.sugarloaf.text(Some(id));
            id
        } else {
            0 // No context or renderer allocation when reusing an owner.
        };
        if let Err(error) = self.context_manager.ensure_review_code(
            path.clone(),
            rich_text_id,
            &mut self.sugarloaf,
        ) {
            if let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut()
            {
                view.set_edit_focus(false);
            }
            self.file_tree_notify(
                error,
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return false;
        }
        self.request_remote_code_content(&path);
        if let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut() {
            view.set_edit_focus(true);
        }
        self.pending_review_code_cursor = None;
        if let Some(code) = self.context_manager.active_code_pane_mut() {
            if code.remote_content_pending {
                self.pending_review_code_cursor =
                    Some((path.clone(), request.line, request.column));
            }
            if !self.renderer.vim_mode {
                code.input_mode = neoism_ui::editor::code::CodeInputMode::Standard;
                code.buffer.mode = neoism_ui::editor::code::CodeMode::Insert;
            }
            if !pointer {
                code.buffer
                    .set_cursor_position(request.line, request.column, false);
                code.buffer.follow_cursor = true;
                code.mouse_selecting = false;
            }
        }
        self.dismiss_code_lsp_popups();
        self.renderer.file_tree.set_focused(false);
        self.renderer.notes_sidebar.set_focused(false);
        true
    }

    pub(crate) fn git_diff_view_pointer_at(
        &self,
        x: f32,
        y: f32,
    ) -> Option<neoism_window::window::CursorIcon> {
        let scale = self.sugarloaf.scale_factor();
        let grid = self.context_manager.current_grid();
        grid.contexts().iter().find_map(|(node, item)| {
            let view = item.val.git_diff_view.as_ref()?;
            let rect = git_diff_viewport(
                item.layout_rect,
                [grid.scaled_margin.left, grid.scaled_margin.top],
                scale,
            );
            (grid.is_context_visible(*node) && rect.contains(x, y)).then(|| {
                use neoism_window::window::CursorIcon;
                if view.is_dragging() {
                    CursorIcon::Default
                } else if view.pointer_cursor(x, y) {
                    CursorIcon::Pointer
                } else if view.text_cursor(x, y) {
                    CursorIcon::Text
                } else {
                    CursorIcon::Default
                }
            })
        })
    }

    /// Wheel input targets the pane under the pointer, including its bottom edge.
    pub(crate) fn handle_git_diff_view_scroll(&mut self, dx: f32, dy: f32) -> bool {
        let [x, y] = self.markdown_mouse_logical();
        let scale = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        if self
            .renderer
            .active_text_occlusion_rects(size.width, size.height, scale)
            .iter()
            .any(|r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3])
        {
            return false;
        }
        let (dx, dy) = git_diff_scroll_delta(dx, dy, self.modifiers.state().shift_key());
        let grid = self.context_manager.current_grid_mut();
        let origin = [grid.scaled_margin.left, grid.scaled_margin.top];
        let target = grid.contexts().iter().find_map(|(node, item)| {
            (grid.is_context_visible(*node)
                && item.val.git_diff_view.is_some()
                && git_diff_viewport(item.layout_rect, origin, scale).contains(x, y))
            .then_some(*node)
        });
        let Some(node) = target else {
            return false;
        };
        if let Some(view) = grid
            .contexts_mut()
            .get_mut(&node)
            .and_then(|item| item.val.git_diff_view.as_mut())
        {
            view.scroll_horizontal_at(x, y, dx);
            view.scroll(dy);
        }
        self.mark_dirty();
        true
    }

    /// Captured drags deliberately ignore viewport/occlusion bounds.
    pub(crate) fn handle_git_diff_view_pointer_move(&mut self) -> bool {
        let [x, y] = self.markdown_mouse_logical();
        let mut handled = false;
        for item in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .values_mut()
        {
            if let Some(view) = item.val.git_diff_view.as_mut() {
                handled |= view.pointer_move(x, y);
            }
        }
        if handled {
            self.mark_dirty();
        }
        handled
    }

    pub(crate) fn handle_git_diff_view_pointer_up(&mut self) -> bool {
        let mut handled = false;
        for item in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .values_mut()
        {
            if let Some(view) = item.val.git_diff_view.as_mut() {
                handled |= view.pointer_up();
            }
        }
        if handled {
            self.mark_dirty();
        }
        handled
    }

    pub(crate) fn handle_git_diff_view_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
    ) -> bool {
        use neoism_window::keyboard::{Key, NamedKey};
        let mods = self.modifiers.state();
        if mods.control_key() || mods.super_key() || mods.alt_key() {
            return false;
        }
        let Some(view) = self.context_manager.current_mut().git_diff_view.as_mut() else {
            return false;
        };
        if view.editing_path().is_some() {
            return false; // Canonical Vim/Standard dispatcher owns this input.
        }
        if key.state != neoism_window::event::ElementState::Pressed {
            return true;
        }
        let name = match &key.logical_key {
            Key::Character(s) => s.as_str(),
            Key::Named(NamedKey::ArrowLeft) => "ArrowLeft",
            Key::Named(NamedKey::ArrowRight) => "ArrowRight",
            Key::Named(NamedKey::Enter) => "Enter",
            Key::Named(NamedKey::Tab) => "Tab",
            Key::Named(NamedKey::Space) => " ",
            Key::Named(NamedKey::ArrowUp) => "ArrowUp",
            Key::Named(NamedKey::ArrowDown) => "ArrowDown",
            Key::Named(NamedKey::PageUp) => "PageUp",
            Key::Named(NamedKey::PageDown) => "PageDown",
            Key::Named(NamedKey::Home) => "Home",
            Key::Named(NamedKey::End) => "End",
            _ => "",
        };
        view.key(name);
        self.mark_dirty();
        // Read-only pane owns unmodified input, even keys it doesn't implement.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_backdrop_fills_outer_padding_below_tabs_at_every_scale() {
        use neoism_ui::layout::Rect;
        for scale in [1.0, 1.5, 2.0] {
            let slot = git_diff_viewport(
                [0.0, 0.0, 880.0 * scale, 600.0 * scale],
                [108.0 * scale, 86.0 * scale],
                scale,
            );
            let frame = Rect::new(100.0, 50.0, 900.0, 650.0);
            let backing = git_diff_backdrop(slot, slot, frame);
            assert_eq!(
                [backing.x, backing.y, backing.w, backing.h],
                [100.0, 50.0, 900.0, 650.0]
            );
        }
    }

    #[test]
    fn review_backdrop_never_covers_an_adjacent_split_or_git_sidebar() {
        use neoism_ui::layout::Rect;
        let frame = Rect::new(100.0, 50.0, 900.0, 650.0);
        let grid = Rect::new(108.0, 86.0, 880.0, 600.0);
        let right_pane = Rect::new(558.0, 86.0, 430.0, 600.0);
        let backing = git_diff_backdrop(right_pane, grid, frame);
        assert_eq!(
            [backing.x, backing.y, backing.w, backing.h],
            [558.0, 50.0, 442.0, 650.0]
        );
        let interior = Rect::new(300.0, 200.0, 200.0, 200.0);
        let backing = git_diff_backdrop(interior, grid, frame);
        assert_eq!(
            [backing.x, backing.y, backing.w, backing.h],
            [300.0, 200.0, 200.0, 200.0]
        );
    }

    #[test]
    fn viewport_reaches_status_bar_without_a_second_bottom_inset() {
        for scale in [1.0, 1.5, 2.0] {
            // A 900-high window, 80 top chrome and 20 bottom status bar.
            let rect = git_diff_viewport(
                [0.0, 0.0, 1000.0 * scale, 800.0 * scale],
                [240.0 * scale, 80.0 * scale],
                scale,
            );
            assert_eq!(
                rect,
                neoism_ui::layout::Rect::new(240.0, 80.0, 1000.0, 800.0)
            );
            assert_eq!(rect.y + rect.h, 880.0);
            assert!(rect.contains(250.0, 879.0));
            assert!(!rect.contains(250.0, 881.0));
        }
    }

    #[test]
    fn split_viewport_keeps_its_own_origin_and_height() {
        let rect = git_diff_viewport([800.0, 600.0, 700.0, 500.0], [200.0, 100.0], 2.0);
        assert_eq!(
            rect,
            neoism_ui::layout::Rect::new(500.0, 350.0, 350.0, 250.0)
        );
    }

    #[test]
    fn native_vertical_pixels_match_code_and_agent_event_units_at_every_dpi() {
        use neoism_window::{dpi::PhysicalPosition, event::MouseScrollDelta};
        // Slow precision motion and OS-accelerated/momentum events take the
        // identical direct path. No notch cap or second acceleration on pixels.
        for scale in [1.0, 1.5, 2.0] {
            for y in [0.125, -17.5, 480.0] {
                let delta = MouseScrollDelta::PixelDelta(PhysicalPosition::new(12.0, y));
                assert_eq!(
                    git_diff_native_wheel(&delta, scale, 24.0, false),
                    (12.0 / scale, y as f32),
                );
            }
        }
    }

    #[test]
    fn native_notches_reuse_agent_three_row_conversion_and_burst_cap() {
        use neoism_window::event::MouseScrollDelta;
        for (lines, expected) in [(1.0, 72.0), (2.0, 144.0), (8.0, 216.0), (-8.0, -216.0)]
        {
            assert_eq!(
                git_diff_native_wheel(
                    &MouseScrollDelta::LineDelta(1.0, lines),
                    2.0,
                    24.0,
                    false
                ),
                (48.0, expected),
            );
        }
    }

    #[test]
    fn native_shift_and_horizontal_gestures_keep_existing_logical_units() {
        use neoism_window::{dpi::PhysicalPosition, event::MouseScrollDelta};
        assert_eq!(
            git_diff_native_wheel(
                &MouseScrollDelta::LineDelta(0.0, -8.0),
                2.0,
                24.0,
                true
            ),
            (-384.0, 0.0),
        );
        assert_eq!(
            git_diff_native_wheel(
                &MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -24.0)),
                2.0,
                24.0,
                true,
            ),
            (-12.0, 0.0),
        );
        assert_eq!(
            git_diff_native_wheel(
                &MouseScrollDelta::LineDelta(-1.0, 1.0),
                2.0,
                24.0,
                true
            ),
            (-48.0, 72.0),
        );
    }

    #[test]
    fn wheel_axes_and_shift_remapping() {
        assert_eq!(git_diff_scroll_delta(12.0, -24.0, false), (-12.0, 24.0));
        assert_eq!(git_diff_scroll_delta(0.0, -48.0, true), (48.0, 0.0));
        // Some platforms already translate Shift+wheel to the horizontal axis.
        assert_eq!(git_diff_scroll_delta(-48.0, 0.0, true), (48.0, -0.0));
        assert_eq!(git_diff_scroll_delta(12.0, -24.0, true), (-12.0, 24.0));
    }
}
