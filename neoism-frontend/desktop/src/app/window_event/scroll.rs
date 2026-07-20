use neoism_window::event::{MouseScrollDelta, TouchPhase};
use neoism_window::window::WindowId;

use crate::app::Application;
use crate::router::routes::RoutePath;

impl Application<'_> {
    pub(in crate::app) fn handle_mouse_wheel(
        &mut self,
        window_id: WindowId,
        delta: MouseScrollDelta,
        phase: TouchPhase,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        if route.path != RoutePath::Terminal {
            return;
        }

        let hover_dismissed = route.window.screen.dismiss_lsp_hover();
        let diagnostic_hover_dismissed =
            route.window.screen.clear_inline_diagnostic_hover();

        if self.config.hide_cursor_when_typing {
            route.window.set_cursor_visible(true);
            if route.window.screen.set_mouse_hidden_by_typing(false) {
                route.request_redraw();
            }
        }

        let notification_delta_x = match delta {
            MouseScrollDelta::LineDelta(columns, _) => columns * 48.0,
            MouseScrollDelta::PixelDelta(pos) => pos.x as f32,
        };
        if route
            .window
            .screen
            .renderer
            .notifications
            .scroll_hovered(notification_delta_x)
        {
            route.request_redraw();
            return;
        }

        // Rust-owned overlays sit above the editor/tree. When
        // hovered, wheel/trackpad input scrolls their internal
        // list/body instead of leaking into the pane behind it.
        if route.window.screen.handle_rust_overlay_wheel(&delta) {
            route.request_redraw();
            return;
        }

        // Wheel over the LSP completion popup changes the
        // popup selection instead of scrolling the editor pane
        // underneath. Tested before tree/editor routing because
        // the popup floats above those surfaces.
        let (completion_wheel_consumed, completion_wheel_changed) =
            route.window.screen.handle_completion_menu_wheel(&delta);
        if completion_wheel_consumed {
            if completion_wheel_changed || hover_dismissed || diagnostic_hover_dismissed {
                route.request_redraw();
            }
            return;
        }

        // Wheel over the diagnostics popup scrolls its row
        // list. Tested before the file-tree / buffer-tabs
        // wheels because the popup floats above them and the
        // visual hit-test should win.
        if route.window.screen.handle_diagnostics_popup_wheel(&delta) {
            route.request_redraw();
            return;
        }

        // If the wheel is over the file tree column, scroll
        // the tree's internal viewport instead of the
        // terminal/editor pane.
        if route.window.screen.handle_file_tree_wheel(&delta) {
            route.request_redraw();
            return;
        }

        // Same for the Alt+N notes sidebar column (it sits to the
        // right of the file tree). Hover-scroll routes into the note
        // list instead of the pane behind it.
        if route.window.screen.handle_notes_sidebar_wheel(&delta) {
            route.request_redraw();
            return;
        }

        // Wheel over the buffer-tabs strip slides tabs
        // horizontally — useful when many buffers are open and
        // the user wants to reach an off-screen tab without
        // clicking through them.
        if route.window.screen.handle_buffer_tabs_wheel(&delta) {
            route.request_redraw();
            return;
        }

        // Wheel over a `.neodraw` canvas pans/zooms it instead of
        // scrolling the terminal/editor beneath.
        if route.window.screen.handle_draw_wheel(&delta) {
            route.request_redraw();
            return;
        }

        // Hover-scroll: route the wheel to the split pane under the
        // cursor, not just the keyboard-focused one. Switching `current`
        // makes the shared `screen.scroll` target the hovered pane.
        {
            let mx = route.window.screen.mouse.x as f32;
            let my = route.window.screen.mouse.y as f32;
            let grid = route.window.screen.context_manager.current_grid_mut();
            if grid.panel_count() > 1 {
                if let Some(node) = grid.find_context_at_position(mx, my) {
                    if node != grid.current {
                        grid.set_current_node_without_layout(node);
                    }
                }
            }
        }

        match delta {
            MouseScrollDelta::LineDelta(columns, lines) => {
                let current_id = route.window.screen.ctx().current().rich_text_id;
                if let Some(layout) =
                    route.window.screen.sugarloaf.get_text_layout(&current_id)
                {
                    // LineDelta is in terminal cells. Feed the
                    // real physical cell size into the pixel
                    // accumulators; font_size misses line-height
                    // and makes wheel notches irregular.
                    let new_scroll_px_x =
                        columns * layout.dimensions.width.round().max(1.0);
                    let new_scroll_px_y =
                        lines * layout.dimensions.height.round().max(1.0);
                    route
                        .window
                        .screen
                        .scroll(new_scroll_px_x as f64, new_scroll_px_y as f64);
                }
            }
            MouseScrollDelta::PixelDelta(mut lpos) => {
                // A new gesture clears only the legacy mouse
                // reporting accumulator. Pixel-scroll residuals
                // intentionally persist like Ghostty/Neovide so
                // trackpad motion stays continuous across
                // gesture boundaries.
                if matches!(phase, TouchPhase::Started) {
                    route.window.screen.mouse.accumulated_scroll = Default::default();
                }
                // Apply pixel deltas in ALL phases (not just
                // Moved). winit on Wayland/Linux frequently
                // tags the FIRST event of a touchpad gesture
                // as Started — dropping its delta is what
                // made each new gesture feel like it lost a
                // frame. Neovide handles all phases uniformly.
                if matches!(phase, TouchPhase::Started | TouchPhase::Moved) {
                    // Axis-snap: when the gesture is mostly
                    // horizontal (cos(angle) > 0.9), zero the
                    // y component, else zero x. Both NaN-safe
                    // (0/0 → NaN, NaN > 0.9 → false → falls
                    // to else branch).
                    if lpos.x.abs() / lpos.x.hypot(lpos.y) > 0.9 {
                        lpos.y = 0.;
                    } else {
                        lpos.x = 0.;
                    }

                    route.window.screen.scroll(lpos.x, lpos.y);
                }
            }
        }

        route.request_redraw();
    }

    /// Trackpad pinch — zooms the active `.neodraw` canvas about the
    /// cursor. `delta` is the incremental magnification (+ = zoom in).
    pub(in crate::app) fn handle_pinch_gesture(
        &mut self,
        window_id: WindowId,
        delta: f64,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };
        if route.path != RoutePath::Terminal {
            return;
        }
        if route.window.screen.handle_draw_pinch(delta as f32) {
            route.request_redraw();
        }
    }
}
