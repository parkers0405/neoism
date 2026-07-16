use std::time::{Duration, Instant};

use neoism_backend::clipboard::ClipboardType;
use neoism_window::dpi::PhysicalPosition;
use neoism_window::event::{ElementState, MouseButton};
use neoism_window::window::{CursorIcon, WindowId};

use crate::app::scheduler::{TimerId, Topic};
use crate::app::Application;
use crate::event::{ClickState, EventPayload, RioEvent, RioEventType};
use crate::router::routes::RoutePath;

impl Application<'_> {
    pub(in crate::app) fn handle_mouse_input(
        &mut self,
        window_id: WindowId,
        state: ElementState,
        button: MouseButton,
    ) {
        // Gathered before borrowing the route so the buffer-tab
        // right-click menu can offer workspaces that live in OTHER OS
        // windows (e.g. detached) as move targets.
        let cross_window_workspaces =
            if button == MouseButton::Right && state == ElementState::Pressed {
                self.router.cross_window_workspaces(window_id)
            } else {
                Vec::new()
            };

        let mut route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        if route.path != RoutePath::Terminal {
            return;
        }

        if self.config.hide_cursor_when_typing {
            route.window.set_cursor_visible(true);
            if route.window.screen.set_mouse_hidden_by_typing(false) {
                route.request_redraw();
            }
        }

        match button {
            MouseButton::Left => route.window.screen.mouse.left_button_state = state,
            MouseButton::Middle => route.window.screen.mouse.middle_button_state = state,
            MouseButton::Right => route.window.screen.mouse.right_button_state = state,
            _ => (),
        }

        match state {
            ElementState::Pressed => {
                // Calculate time since the last click to handle double/triple clicks.
                // Do this early so island clicks can use the click state
                let now = Instant::now();
                let elapsed = now - route.window.screen.mouse.last_click_timestamp;
                route.window.screen.mouse.last_click_timestamp = now;

                let threshold = Duration::from_millis(300);
                let mouse = &route.window.screen.mouse;
                route.window.screen.mouse.click_state = match mouse.click_state {
                    // Reset click state if button has changed.
                    _ if button != mouse.last_click_button => {
                        route.window.screen.mouse.last_click_button = button;
                        ClickState::Click
                    }
                    ClickState::Click if elapsed < threshold => ClickState::DoubleClick,
                    ClickState::DoubleClick if elapsed < threshold => {
                        ClickState::TripleClick
                    }
                    _ => ClickState::Click,
                };

                if route.window.screen.renderer.modal.is_active() {
                    route.window.screen.dismiss_lsp_hover();
                    if button == MouseButton::Left {
                        route.window.screen.handle_modal_click();
                    }
                    route.request_redraw();
                    return;
                }

                // The command palette is an overlay too: while it is
                // open, EVERY click is its business — a press on a row
                // acts, a press anywhere else closes the palette. It
                // must never fall through to panes/tabs behind it.
                if route.window.screen.renderer.command_palette.is_enabled() {
                    route.window.screen.dismiss_lsp_hover();
                    if button == MouseButton::Left
                        && !route
                            .window
                            .screen
                            .handle_palette_click(&mut self.router.clipboard)
                    {
                        route.window.screen.renderer.command_palette.set_enabled(false);
                    }
                    route.request_redraw();
                    return;
                }

                if let MouseButton::Right = button {
                    route.window.screen.close_context_menu();
                    route.window.screen.dismiss_lsp_hover();
                    // Workspace ("Island") tab strip sits at the very top
                    // and spans the width, so check it before the panels
                    // below it.
                    if route.window.screen.handle_workspace_tab_context_click() {
                        route.request_redraw();
                        return;
                    }
                    // Buffer-tab strip sits just below the workspace
                    // strip: offer "Move to Workspace …" for movable tabs
                    // (including workspaces in other OS windows).
                    if route
                        .window
                        .screen
                        .handle_buffer_tab_context_click(&cross_window_workspaces)
                    {
                        route.request_redraw();
                        return;
                    }
                    if route.window.screen.handle_notes_sidebar_context_click() {
                        route.request_redraw();
                        return;
                    }
                    if route.window.screen.handle_file_tree_context_click() {
                        route.request_redraw();
                        return;
                    }
                    if route.window.screen.handle_editor_context_click() {
                        route.request_redraw();
                        return;
                    }
                }

                if let MouseButton::Left = button {
                    if route
                        .window
                        .screen
                        .handle_context_menu_click(&mut self.router.clipboard)
                    {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.begin_file_tree_resize() {
                        route.window.winit_window.set_cursor(CursorIcon::ColResize);
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.begin_notes_sidebar_resize() {
                        route.window.winit_window.set_cursor(CursorIcon::ColResize);
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.begin_git_diff_panel_resize() {
                        route.window.winit_window.set_cursor(CursorIcon::ColResize);
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.begin_git_diff_panel_scrollbar_drag() {
                        route.request_redraw();
                        return;
                    }

                    // Check if clicking on a panel border to start resize
                    {
                        let mx = route.window.screen.mouse.x as f32;
                        let my = route.window.screen.mouse.y as f32;
                        let grid = route.window.screen.context_manager.current_grid();
                        if let Some(border) = grid.find_border_at_position(mx, my) {
                            let start_pos = match border.direction {
                                crate::layout::BorderDirection::Vertical => mx,
                                crate::layout::BorderDirection::Horizontal => my,
                            };
                            route.window.screen.resize_state =
                                Some(crate::layout::ResizeState { border, start_pos });
                            return;
                        }
                    }

                    if route.window.screen.handle_modal_click() {
                        route.request_redraw();
                        return;
                    }

                    if route
                        .window
                        .screen
                        .handle_diagnostics_click(&mut self.router.clipboard)
                    {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_git_diff_panel_click() {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_assistant_click() {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_splash_overlay_click() {
                        route.request_redraw();
                        return;
                    }

                    if route
                        .window
                        .screen
                        .handle_neoism_agent_click(&mut self.router.clipboard)
                    {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_finder_click() {
                        route.request_redraw();
                        return;
                    }

                    if route
                        .window
                        .screen
                        .handle_palette_click(&mut self.router.clipboard)
                    {
                        route.request_redraw();
                        return;
                    }

                    if route
                        .window
                        .screen
                        .handle_search_click(&mut self.router.clipboard)
                    {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_minimap_click() {
                        route.request_redraw();
                        return;
                    }

                    let handled_by_island = route.window.screen.handle_island_click(
                        &route.window.winit_window,
                        &mut self.router.clipboard,
                    );

                    if handled_by_island {
                        // Island handled the click, don't process further
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_top_bar_click() {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_file_tree_click() {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_notes_sidebar_click() {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_buffer_tabs_click() {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_notebook_chrome_click() {
                        route.request_redraw();
                        return;
                    }

                    if route.window.screen.handle_scrollbar_click() {
                        route.request_redraw();
                        return;
                    }
                }

                // Always try panel switching first: if the click
                // targets a different panel, switch to it regardless
                // of mouse mode (e.g. neovim capturing clicks).
                let switched_panel = route.window.screen.select_current_based_on_mouse();
                if switched_panel {
                    route.request_redraw();
                }

                if route.window.screen.handle_editor_mouse_click(button) {
                    route.request_redraw();
                    return;
                } else if route.window.screen.handle_neoism_tags_mouse_press(button) {
                    route.request_redraw();
                    return;
                } else if route.window.screen.handle_extensions_click(button) {
                    route.request_redraw();
                    return;
                } else if route.window.screen.handle_draw_mouse_press(button) {
                    route.request_redraw();
                    return;
                } else if route
                    .window
                    .screen
                    .handle_markdown_mouse_press(button, &mut self.router.clipboard)
                {
                    route.window.set_cursor(
                        if route.window.screen.markdown_grab_drag_active() {
                            CursorIcon::Grabbing
                        } else {
                            CursorIcon::Text
                        },
                    );
                    route.request_redraw();
                    return;
                } else if !switched_panel
                    && !route.window.screen.modifiers.state().shift_key()
                    && route.window.screen.mouse_mode()
                {
                    // Process mouse press before bindings to update the `click_state`.
                    route.window.screen.mouse.click_state = ClickState::None;

                    let code = match button {
                        MouseButton::Left => 0,
                        MouseButton::Middle => 1,
                        MouseButton::Right => 2,
                        // Can't properly report more than three buttons..
                        MouseButton::Back
                        | MouseButton::Forward
                        | MouseButton::Other(_) => return,
                    };

                    route
                        .window
                        .screen
                        .mouse_report(code, ElementState::Pressed);

                    route
                        .window
                        .screen
                        .process_mouse_bindings(button, &mut self.router.clipboard);
                } else {
                    if route.window.screen.trigger_hyperlink() {
                        return;
                    }

                    // Load mouse point, treating message bar and padding as the closest square.
                    let display_offset = route.window.screen.display_offset();

                    if let MouseButton::Left = button {
                        let current = route.window.screen.context_manager.current();
                        let pos = if current.editor.is_none()
                            && current.markdown.is_none()
                            && current.neoism_agent.is_none()
                            && current.neoism_tags.is_none()
                            && current.neoism_extensions.is_none()
                        {
                            route
                                .window
                                .screen
                                .terminal_body_mouse_position(display_offset)
                        } else {
                            Some(route.window.screen.mouse_position(display_offset))
                        };
                        route
                            .window
                            .screen
                            .on_left_click(pos, &mut self.router.clipboard);
                    }

                    route.request_redraw();
                }
                route
                    .window
                    .screen
                    .process_mouse_bindings(button, &mut self.router.clipboard);
            }
            ElementState::Released => {
                // 5D-drag: finish a Workspaces-modal drag. A drop on a
                // host header emits MoveWorkspaceToHost (5D-wire then
                // dispatches the real promote/demote); a plain click
                // (no threshold crossed) switches to that workspace.
                if button == MouseButton::Left
                    && route
                        .window
                        .screen
                        .handle_palette_drag_release(&mut self.router.clipboard)
                {
                    route.window.set_cursor(CursorIcon::Default);
                    route.request_redraw();
                    return;
                }

                if button == MouseButton::Left
                    && route.window.screen.end_file_tree_resize()
                {
                    route.window.set_cursor(CursorIcon::Default);
                    route.request_redraw();
                    return;
                }

                if button == MouseButton::Left
                    && route.window.screen.end_notes_sidebar_resize()
                {
                    route.window.set_cursor(CursorIcon::Default);
                    route.request_redraw();
                    return;
                }

                if button == MouseButton::Left
                    && route.window.screen.end_git_diff_panel_resize()
                {
                    route.window.set_cursor(CursorIcon::Default);
                    route.request_redraw();
                    return;
                }

                if button == MouseButton::Left
                    && route.window.screen.end_git_diff_panel_scrollbar_drag()
                {
                    route.request_redraw();
                    return;
                }

                // Stop selection auto-scroll on button release.
                if let MouseButton::Left | MouseButton::Right = button {
                    let scroll_timer_id = route.window.screen.ctx().current_route();
                    let timer_id =
                        TimerId::new(Topic::SelectionScrolling, scroll_timer_id);
                    self.scheduler.unschedule(timer_id);
                }

                // End any in-progress buffer-tabs drag. We do
                // this on every release (even if no drag was
                // active) so the lazy "armed but not yet
                // dragging" state from a plain click is also
                // cleared. Real reorders short-circuit the
                // rest of the release path.
                if button == MouseButton::Left
                    && route.window.screen.handle_editor_mouse_release()
                {
                    route.window.set_cursor(CursorIcon::Text);
                    route.request_redraw();
                    return;
                }

                if button == MouseButton::Left
                    && route.window.screen.handle_draw_mouse_release()
                {
                    route.request_redraw();
                    return;
                }

                if button == MouseButton::Left
                    && route.window.screen.handle_markdown_mouse_release()
                {
                    route.window.set_cursor(CursorIcon::Text);
                    route.request_redraw();
                    return;
                }

                if button == MouseButton::Left
                    && route
                        .window
                        .screen
                        .handle_neoism_agent_mouse_release(&mut self.router.clipboard)
                {
                    route.window.set_cursor(CursorIcon::Text);
                    route.request_redraw();
                    return;
                }

                // Workspace-strip drag (top-level tabs in the Island).
                // Mirrors the buffer-tabs release path — runs first so
                // a release that crossed the detach threshold isn't
                // mistaken for a stale buffer-tabs drag.
                if button == MouseButton::Left
                    && route.window.screen.handle_island_drag_release()
                {
                    route.request_redraw();
                    return;
                }

                // Cross-window tab drop (C5 / R4): if a buffer-tabs
                // drag is in progress, give the router a chance to
                // route the release into a different OS window's
                // `Screen` before the in-window release pipeline runs.
                // The router call needs exclusive `self.router` access
                // (to two routes at once), so we release the current
                // `route` borrow and re-acquire it on fall-through.
                if button == MouseButton::Left
                    && route.window.screen.has_active_buffer_tab_drag()
                {
                    let _ = route; // end the borrow before re-borrowing router
                    if self.router.try_cross_window_tab_drop(window_id) {
                        return;
                    }
                    let Some(reacquired) = self.router.routes.get_mut(&window_id) else {
                        return;
                    };
                    route = reacquired;
                }

                if button == MouseButton::Left
                    && route.window.screen.handle_buffer_tabs_drag_release()
                {
                    route.request_redraw();
                    return;
                }

                if button == MouseButton::Left
                    && route.window.screen.handle_minimap_release()
                {
                    route.window.set_cursor(CursorIcon::Default);
                    route.request_redraw();
                    return;
                }

                if route.window.screen.renderer.scrollbar.is_dragging() {
                    route.window.screen.handle_scrollbar_release();
                    route.request_redraw();
                    return;
                }

                if route.window.screen.resize_state.is_some() {
                    route.window.screen.resize_state = None;
                    route.window.set_cursor(CursorIcon::Default);
                    return;
                }

                if !route.window.screen.modifiers.state().shift_key()
                    && route.window.screen.mouse_mode()
                {
                    let code = match button {
                        MouseButton::Left => 0,
                        MouseButton::Middle => 1,
                        MouseButton::Right => 2,
                        // Can't properly report more than three buttons.
                        MouseButton::Back
                        | MouseButton::Forward
                        | MouseButton::Other(_) => return,
                    };
                    route
                        .window
                        .screen
                        .mouse_report(code, ElementState::Released);
                    return;
                }

                // Trigger hints highlighted by the mouse
                if button == MouseButton::Left
                    && route.window.screen.trigger_hint(&mut self.router.clipboard)
                {
                    return;
                }

                if let MouseButton::Left | MouseButton::Right = button {
                    if self.config.copy_on_select {
                        route.window.screen.copy_selection(
                            ClipboardType::Clipboard,
                            &mut self.router.clipboard,
                        );
                    }
                }
            }
        }
    }

    pub(in crate::app) fn handle_cursor_moved(
        &mut self,
        window_id: WindowId,
        position: PhysicalPosition<f64>,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        if self.config.hide_cursor_when_typing {
            route.window.set_cursor_visible(true);
            if route.window.screen.set_mouse_hidden_by_typing(false) {
                route.request_redraw();
            }
        }

        if route.path != RoutePath::Terminal {
            route.window.set_cursor(CursorIcon::Default);
            return;
        }

        let x = position.x;
        let y = position.y;

        let layout = route.window.screen.sugarloaf.window_size();

        let x = x.clamp(0.0, (layout.width as i32 - 1).into()) as usize;
        let y = y.clamp(0.0, (layout.height as i32 - 1).into()) as usize;

        // Snapshot the old mouse position before updating coordinates
        // so we can detect whether the cursor moved to a new cell.
        let old_x = route.window.screen.mouse.x;
        let old_y = route.window.screen.mouse.y;

        route.window.screen.mouse.x = x;
        route.window.screen.mouse.y = y;
        route.window.screen.mouse.raw_y = position.y;
        route.window.screen.mouse.raw_x = position.x;

        {
            let scale = route.window.screen.sugarloaf.scale_factor();
            let win_w = route.window.screen.sugarloaf.window_size().width;
            let mx = x as f32 / scale;
            let my = y as f32 / scale;
            let notification_top = route
                .window
                .screen
                .renderer
                .notifications_top_offset(&route.window.screen.context_manager);
            if route.window.screen.renderer.notifications.hover(
                mx,
                my,
                win_w,
                scale,
                notification_top,
            ) {
                route.request_redraw();
            }
        }

        if route.window.screen.file_tree_resize_active() {
            route.window.screen.drag_file_tree_resize();
            route.window.set_cursor(CursorIcon::ColResize);
            route.request_redraw();
            return;
        }

        if route.window.screen.notes_sidebar_resize_active() {
            route.window.screen.drag_notes_sidebar_resize();
            route.window.set_cursor(CursorIcon::ColResize);
            route.request_redraw();
            return;
        }

        if route.window.screen.git_diff_panel_resize_active() {
            route.window.screen.drag_git_diff_panel_resize();
            route.window.set_cursor(CursorIcon::ColResize);
            route.request_redraw();
            return;
        }

        if route.window.screen.git_diff_panel_scrollbar_drag_active() {
            route.window.screen.drag_git_diff_panel_scrollbar();
            route.request_redraw();
            return;
        }

        // Input modals own pointer hover while they are open. Keep this
        // before the background hover chain so file-tree rows, tab
        // buttons, composer/status seams, and terminal links behind the
        // modal do not keep toggling hover colors under the late overlay.
        if route.window.screen.renderer.finder.is_enabled() {
            let scale = route.window.screen.sugarloaf.scale_factor();
            let size = route.window.screen.sugarloaf.window_size();
            let mx = x as f32 / scale;
            let my = y as f32 / scale;
            if route.window.screen.renderer.finder.hover(
                mx,
                my,
                (size.width as f32, size.height as f32, scale),
            ) {
                route.request_redraw();
            }
            route.window.set_cursor(CursorIcon::Default);
            return;
        }

        if route.window.screen.renderer.command_palette.is_enabled() {
            // 5D-drag: while a Workspaces-modal drag is in flight the
            // cursor grips the dragged row — track the drop target and
            // skip the hover/selection update.
            if route.window.screen.handle_palette_drag_move() {
                route.window.set_cursor(CursorIcon::Grabbing);
                route.request_redraw();
                return;
            }
            let scale = route.window.screen.sugarloaf.scale_factor();
            let win_w = route.window.screen.sugarloaf.window_size().width;
            let mx = x as f32 / scale;
            let my = y as f32 / scale;
            if route
                .window
                .screen
                .renderer
                .command_palette
                .hover(mx, my, win_w, scale)
            {
                route.request_redraw();
            }
            let over_row = route
                .window
                .screen
                .renderer
                .command_palette
                .pointer_over_row(mx, my, win_w, scale);
            route.window.set_cursor(if over_row {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
            return;
        }

        if route.window.screen.renderer.modal.is_active() {
            let scale = route.window.screen.sugarloaf.scale_factor();
            let win_w = route.window.screen.sugarloaf.window_size().width;
            let (mx, my) = route.window.screen.mouse_logical_for_hit_test();
            if let Ok(Some(index)) = route
                .window
                .screen
                .renderer
                .modal
                .hit_test(mx, my, win_w, scale)
            {
                route.window.screen.renderer.modal.set_selected_index(index);
                route.request_redraw();
            }
            route.window.screen.dismiss_lsp_hover();
            route.window.set_cursor(CursorIcon::Default);
            return;
        }

        // Workspace-strip drag-to-reorder (top-level Island tabs).
        // Runs before buffer-tabs so a workspace drag doesn't bleed
        // into the workspace's interior strip.
        if route.window.screen.handle_island_drag_move() {
            route.request_redraw();
            return;
        }

        // Handle buffer-tabs drag-to-reorder. A press-and-hold
        // over a tab arms the drag; once the cursor crosses the
        // activation threshold the tab follows the cursor and
        // swaps with neighbors when its center crosses a slot
        // boundary. We short-circuit other hover paths while a
        // drag is live so the cursor keeps gripping the tab.
        if route.window.screen.handle_buffer_tabs_drag_move() {
            route.request_redraw();
            return;
        }

        if route.window.screen.handle_editor_mouse_drag_move() {
            route.window.set_cursor(CursorIcon::Text);
            route.request_redraw();
            return;
        }

        if route.window.screen.markdown_drag_active() {
            if route.window.screen.handle_markdown_drag_move() {
                route.window.set_cursor(
                    if route.window.screen.markdown_grab_drag_active() {
                        CursorIcon::Grabbing
                    } else {
                        CursorIcon::Text
                    },
                );
                route.request_redraw();
            }
            return;
        }

        if route.window.screen.draw_drag_active() {
            if route.window.screen.handle_draw_drag_move() {
                route.request_redraw();
            }
            return;
        }

        if route.window.screen.handle_neoism_agent_drag_move() {
            route.window.set_cursor(CursorIcon::Text);
            route.request_redraw();
            return;
        }

        if route.window.screen.handle_neoism_agent_hover_move() {
            route
                .window
                .set_cursor(if route.window.screen.neoism_agent_link_hovered() {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                });
            route.request_redraw();
            return;
        }

        if route.window.screen.neoism_agent_link_hovered() {
            route.window.set_cursor(CursorIcon::Pointer);
            return;
        }

        // Handle context menu hover before Markdown hover. The /
        // block picker is a context menu opened above Markdown;
        // Markdown should not steal hover/cursor while it is up.
        if route.window.screen.renderer.context_menu.is_visible() {
            let scale = route.window.screen.sugarloaf.scale_factor();
            let mx = x as f32 / scale;
            let my = y as f32 / scale;
            let over_row = route
                .window
                .screen
                .renderer
                .context_menu
                .hit_test(mx, my)
                .is_ok_and(|hit| hit.is_some());
            if route.window.screen.renderer.context_menu.hover(mx, my) {
                route.request_redraw();
            }
            route.window.set_cursor(if over_row {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
            return;
        }

        if route.window.screen.handle_status_line_hover() {
            route.request_redraw();
        }
        if route.window.screen.status_line_git_hovered() {
            route.window.set_cursor(CursorIcon::Pointer);
            return;
        }
        if route.window.screen.status_line_lsp_hovered() {
            route.window.set_cursor(CursorIcon::Pointer);
            return;
        }
        if route.window.screen.handle_top_bar_hover() {
            // Hover state may have changed; redraw and let other
            // hover handlers below still run (they early-return for
            // their own region).
            route.request_redraw();
        }

        // Top-level workspace (Island) strip hover — lights up the
        // hovered workspace tab like a buffer tab. Sits above the
        // buffer-tab strip, so it's checked first and short-circuits when
        // the cursor is over a workspace tab.
        let (island_hover_changed, over_island_tab) =
            route.window.screen.handle_island_hover();
        if island_hover_changed {
            route.request_redraw();
        }
        if over_island_tab {
            route.window.set_cursor(CursorIcon::Default);
            return;
        }

        let (tab_hit, tab_hover_changed) = route.window.screen.handle_buffer_tabs_hover();
        if tab_hover_changed {
            route.request_redraw();
        }
        if let Some(hit) = tab_hit {
            route.window.set_cursor(
                if matches!(hit, neoism_ui::panels::buffer_tabs::TabHit::Close(_)) {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                },
            );
            return;
        }

        if route.window.screen.handle_notebook_chrome_hover() {
            route.request_redraw();
        }
        if route.window.screen.notebook_chrome_action_hovered() {
            route.window.set_cursor(CursorIcon::Pointer);
            return;
        }

        if route.window.screen.draw_is_graph() {
            if route.window.screen.handle_draw_hover() {
                route.request_redraw();
            }
            route
                .window
                .set_cursor(if route.window.screen.draw_graph_hovering() {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                });
            return;
        }

        if route
            .window
            .screen
            .context_manager
            .current()
            .markdown
            .is_some()
        {
            if route.window.screen.handle_markdown_hover() {
                route.request_redraw();
            }
            route.window.set_cursor(
                if route.window.screen.markdown_link_hovered()
                    || route.window.screen.markdown_notebook_action_hovered()
                {
                    CursorIcon::Pointer
                } else if route.window.screen.markdown_handle_hovered() {
                    CursorIcon::Grab
                } else {
                    CursorIcon::Text
                },
            );
            return;
        }

        if route.window.screen.handle_minimap_drag_move() {
            route.window.set_cursor(CursorIcon::Pointer);
            route.request_redraw();
            return;
        }

        if route.window.screen.handle_minimap_hover() {
            route.window.set_cursor(CursorIcon::Pointer);
            route.request_redraw();
            return;
        }

        // Handle assistant overlay hover
        if route.window.screen.renderer.assistant.is_active() {
            let scale = route.window.screen.sugarloaf.scale_factor();
            let win_w = route.window.screen.sugarloaf.window_size().width;
            let mx = x as f32 / scale;
            let my = y as f32 / scale;
            if route
                .window
                .screen
                .renderer
                .assistant
                .hover(mx, my, win_w, scale)
            {
                route.request_redraw();
            }

            if route
                .window
                .screen
                .renderer
                .assistant
                .hovered_button()
                .is_some()
            {
                route.window.set_cursor(CursorIcon::Pointer);
            } else {
                route.window.set_cursor(CursorIcon::Default);
            }
            return;
        }

        // Splash overlay hover — the menu buttons and
        // the wordmark itself live in GPU space, so we
        // forward the cursor here so the overlay can
        // paint its hover state and we flip the OS
        // cursor to a pointer when over a hit region.
        if route.window.screen.handle_splash_overlay_hover() {
            route.window.set_cursor(CursorIcon::Pointer);
            route.request_redraw();
            return;
        }

        // Handle search overlay hover
        if route.window.screen.renderer.search.is_active() {
            let scale = route.window.screen.sugarloaf.scale_factor();
            let win_w = route.window.screen.sugarloaf.window_size().width;
            let mx = x as f32 / scale;
            let my = y as f32 / scale;
            if route
                .window
                .screen
                .renderer
                .search
                .hover(mx, my, win_w, scale)
            {
                // UI-only change (hover highlight). `set_dirty`
                // passes `Renderer::run`'s per-context gate;
                // the inner damage match hits
                // `(None, None) => TerminalDamage::Noop` so
                // no rows rebuild. The search overlay itself
                // is drawn unconditionally after the per-context
                // loop in `Renderer::run`.
                route
                    .window
                    .screen
                    .ctx_mut()
                    .current_mut()
                    .renderable_content
                    .pending_update
                    .set_dirty();
                route.request_redraw();
            }
        }

        // Check if mouse is over island and set cursor to default
        use neoism_ui::widgets::island::ISLAND_HEIGHT;
        let scale_factor = route.window.screen.sugarloaf.scale_factor();
        let island_height_px = (ISLAND_HEIGHT * scale_factor) as usize;
        if route.window.screen.renderer.navigation.is_enabled() && y <= island_height_px {
            route.window.set_cursor(CursorIcon::Default);
            return;
        }

        // Handle scrollbar drag
        if route.window.screen.renderer.scrollbar.is_dragging() {
            let scale = route.window.screen.sugarloaf.scale_factor();
            let mouse_y = y as f32 / scale;
            route.window.screen.handle_scrollbar_drag(mouse_y);
            route.window.set_cursor(CursorIcon::Default);
            route.request_redraw();
            return;
        }

        // Handle panel border resize
        if route.window.screen.resize_state.is_some() {
            let state = route.window.screen.resize_state.unwrap();
            let current_pos = match state.border.direction {
                crate::layout::BorderDirection::Vertical => x as f32,
                crate::layout::BorderDirection::Horizontal => y as f32,
            };
            let delta = current_pos - state.start_pos;
            let border = state.border;
            let new_ratio = if border.node_extent > f32::EPSILON {
                border.start_ratio + delta / border.node_extent
            } else {
                border.start_ratio
            };
            route
                .window
                .screen
                .context_manager
                .current_grid_mut()
                .resize_border(&border, new_ratio, &mut route.window.screen.sugarloaf);
            let cursor = match border.direction {
                crate::layout::BorderDirection::Vertical => CursorIcon::ColResize,
                crate::layout::BorderDirection::Horizontal => CursorIcon::RowResize,
            };
            route.window.set_cursor(cursor);
            route.window.screen.context_manager.request_render();
            route.request_redraw();
            return;
        }

        // Check if hovering over the file-tree resize edge
        if route.window.screen.is_hovering_file_tree_resize_edge() {
            route.window.set_cursor(CursorIcon::ColResize);
            route.window.screen.mouse.on_border = true;
            return;
        }

        if route.window.screen.is_hovering_notes_sidebar_resize_edge() {
            route.window.set_cursor(CursorIcon::ColResize);
            route.window.screen.mouse.on_border = true;
            return;
        }

        // Same hover treatment for the git diff panel's
        // leading edge — drag to widen/narrow the panel.
        if route.window.screen.is_hovering_git_diff_panel_resize_edge() {
            route.window.set_cursor(CursorIcon::ColResize);
            route.window.screen.mouse.on_border = true;
            return;
        }

        // Check if hovering over a panel border
        {
            let grid = route.window.screen.context_manager.current_grid();
            if let Some(border) = grid.find_border_at_position(x as f32, y as f32) {
                let cursor = match border.direction {
                    crate::layout::BorderDirection::Vertical => CursorIcon::ColResize,
                    crate::layout::BorderDirection::Horizontal => CursorIcon::RowResize,
                };
                route.window.set_cursor(cursor);
                route.window.screen.mouse.on_border = true;
                return;
            }
        }

        // Check if hovering over scrollbar
        if route.window.screen.is_hovering_scrollbar() {
            route.window.set_cursor(CursorIcon::Default);
            return;
        }

        // Track leaving a border to force cursor reset below
        let was_on_border = route.window.screen.mouse.on_border;
        route.window.screen.mouse.on_border = false;

        let lmb_pressed =
            route.window.screen.mouse.left_button_state == ElementState::Pressed;
        let rmb_pressed =
            route.window.screen.mouse.right_button_state == ElementState::Pressed;

        let has_selection = !route.window.screen.selection_is_empty();
        if has_selection && (lmb_pressed || rmb_pressed) {
            // Only start the timer when the mouse enters the scroll
            // zone. Once running, the tick reads mouse.raw_y each
            // iteration so it keeps scrolling after CursorMoved
            // stops (mouse left window). Cancelled on button release.
            let delta = route.window.screen.selection_scroll_delta(position.y);
            if delta != 0 {
                let scroll_timer_id = route.window.screen.ctx().current_route();
                let timer_id = TimerId::new(Topic::SelectionScrolling, scroll_timer_id);
                if !self.scheduler.scheduled(timer_id) {
                    let event = EventPayload::new(
                        RioEventType::Rio(RioEvent::SelectionScrollTick),
                        window_id,
                    );
                    self.scheduler.schedule(
                        event,
                        Duration::from_millis(15),
                        true,
                        timer_id,
                    );
                }
            }
        }

        let display_offset = route.window.screen.display_offset();
        let point = route.window.screen.mouse_position(display_offset);

        // Detect cell change by comparing pixel positions against cell
        // dimensions, avoiding a second mouse_position() call.
        let square_changed = x != old_x || y != old_y;

        let inside_text_area = route.window.screen.contains_point(x, y);
        let square_side = route.window.screen.side_by_pos(x);

        // If the mouse hasn't changed cells, do nothing.
        // Force update when transitioning off a border so the cursor resets.
        if !square_changed
            && !was_on_border
            && route.window.screen.mouse.square_side == square_side
            && route.window.screen.mouse.inside_text_area == inside_text_area
        {
            return;
        }

        // Skip hint/hyperlink highlighting during active selection
        // drag to avoid unnecessary terminal locks and regex matching.
        let is_selecting = (lmb_pressed || rmb_pressed)
            && (route.window.screen.modifiers.state().shift_key()
                || !route.window.screen.mouse_mode());

        if !is_selecting && {
            let _span = crate::app::freeze_watchdog::global_span(
                "cursor_moved.update_highlighted_hints",
                format!("window_id={window_id:?}"),
            );
            route.window.screen.update_highlighted_hints()
        } {
            route.window.set_cursor(CursorIcon::Pointer);
            route.window.screen.context_manager.request_render();
        } else if !is_selecting {
            // File-link hover: when the cell under the mouse
            // resolves to a real file/dir, switch to the hand
            // cursor and trigger a redraw so the underline +
            // blue tint paint reliably even when nothing else
            // marks the frame dirty.
            let over_file_link = {
                let _span = crate::app::freeze_watchdog::global_span(
                    "cursor_moved.terminal_file_link_at_mouse",
                    format!("window_id={window_id:?}"),
                );
                route.window.screen.terminal_file_link_at_mouse().is_some()
            };
            let cursor_icon = if over_file_link {
                route.window.screen.context_manager.request_render();
                CursorIcon::Pointer
            } else if !route.window.screen.modifiers.state().shift_key()
                && route.window.screen.mouse_mode()
            {
                CursorIcon::Default
            } else {
                // The link state can change OFF as well — a
                // request_render here paints the un-hovered
                // (no underline) frame so stale underlines
                // don't linger when the mouse moves off.
                route.window.screen.context_manager.request_render();
                CursorIcon::Text
            };

            route.window.set_cursor(cursor_icon);

            // In case hyperlink range has cleaned trigger one more render
            if route
                .window
                .screen
                .context_manager
                .current()
                .has_hyperlink_range()
            {
                route
                    .window
                    .screen
                    .context_manager
                    .current_mut()
                    .set_hyperlink_range(None);
                route.window.screen.context_manager.request_render();
            }
        }

        route.window.screen.mouse.inside_text_area = inside_text_area;
        route.window.screen.mouse.square_side = square_side;

        // VS Code-style hover: over an editor pane, ask the Rust LSP for docs
        // at the cell under the mouse (deduped per cell); off the text area,
        // dismiss. `request_lsp_hover_at_mouse` no-ops on non-editor contexts.
        if !is_selecting {
            if inside_text_area
                && route
                    .window
                    .screen
                    .context_manager
                    .current()
                    .editor
                    .is_some()
            {
                route.window.screen.request_lsp_hover_at_mouse();
            } else {
                route.window.screen.dismiss_lsp_hover();
            }
        }

        if is_selecting {
            let current = route.window.screen.context_manager.current();
            let should_update = if current.editor.is_none()
                && current.markdown.is_none()
                && current.neoism_agent.is_none()
                && current.neoism_tags.is_none()
                && current.neoism_extensions.is_none()
            {
                if let Some(point) = route
                    .window
                    .screen
                    .terminal_body_mouse_position(display_offset)
                {
                    route.window.screen.update_selection(point, square_side);
                    true
                } else {
                    false
                }
            } else {
                route.window.screen.update_selection(point, square_side);
                true
            };
            if should_update {
                route.window.screen.context_manager.request_render();
            }
        } else if square_changed && route.window.screen.has_mouse_motion_and_drag() {
            if lmb_pressed {
                route.window.screen.mouse_report(32, ElementState::Pressed);
            } else if route.window.screen.mouse.middle_button_state
                == ElementState::Pressed
            {
                route.window.screen.mouse_report(33, ElementState::Pressed);
            } else if route.window.screen.mouse.right_button_state
                == ElementState::Pressed
            {
                route.window.screen.mouse_report(34, ElementState::Pressed);
            } else if route.window.screen.has_mouse_motion() {
                route.window.screen.mouse_report(35, ElementState::Pressed);
            }
        }
    }

    pub(in crate::app) fn handle_cursor_left(&mut self, window_id: WindowId) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        if route.window.screen.clear_status_line_hover() {
            route.request_redraw();
        }
        if route.window.screen.renderer.notifications.clear_hover() {
            route.request_redraw();
        }
    }
}
