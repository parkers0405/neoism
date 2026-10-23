use super::ContextGrid;
use crate::layout::dimensions::ContextDimension;
use neoism_backend::config::layout::Margin;
use neoism_backend::event::EventListener;
use neoism_backend::sugarloaf::Sugarloaf;
use neoism_ui::layout::Rect;
use neoism_ui::session_layout::geometry::{self as pane_geometry, SolveOpts};
use taffy::{geometry, style_helpers::length, NodeId, TaffyError};

/// Commit solved geometry even when the PTY parser owns the terminal lock.
/// The pending bit refers to `ctx.dimension`, not a captured winsize: another
/// layout (or pane-chrome inset) may supersede it before the render retry.
fn apply_layout_dimensions<T: EventListener>(
    ctx: &mut crate::context::Context<T>,
    width: f32,
    height: f32,
    visible: bool,
) {
    ctx.dimension.margin = Margin::all(0.0);
    ctx.dimension.restore_nominal_cell_height();
    ctx.dimension.update_width(width);
    ctx.dimension.update_height(height);

    let Some(mut terminal) = ctx.terminal.try_lock_unfair() else {
        ctx.pending_terminal_resize = true;
        ctx.renderable_content.pending_update.set_dirty();
        return;
    };
    let winsize = crate::bridges::utils::terminal_dimensions(&ctx.dimension);
    terminal.resize(crate::bridges::utils::resize_dimensions(
        winsize.cols,
        winsize.rows,
    ));
    drop(terminal);
    ctx.pending_terminal_resize = false;

    // Keep hidden stacked terminals locally sized, but only notify their PTY
    // when visible. The render retry also runs only for visible contexts and
    // sends through the same messenger (including daemon-backed PTYs).
    if visible {
        let _ = ctx.messenger.send_resize(winsize);
    }
}

impl<T: EventListener> ContextGrid<T> {
    pub(crate) fn try_update_size(
        &mut self,
        width: f32,
        height: f32,
    ) -> Result<(), TaffyError> {
        // Subtract window margin from available size
        let available_width = width - self.scaled_margin.left - self.scaled_margin.right;
        let available_height =
            height - self.scaled_margin.top - self.scaled_margin.bottom;

        let mut style = self.tree.style(self.root_node)?.clone();
        style.size = geometry::Size {
            width: length(available_width),
            height: length(available_height),
        };
        self.tree.set_style(self.root_node, style)?;
        Ok(())
    }

    /// Geometry is now sourced from the canonical [`SessionTree`] via the
    /// shared solver (`pane_geometry::solve_with`) — Taffy no longer
    /// computes pane rects. Taffy remains only as the structural/id store
    /// behind splits until the full re-key lands.
    pub(crate) fn compute_layout(&mut self) -> Result<(), TaffyError> {
        self.recompute_rects_from_tree();
        Ok(())
    }

    /// Populate every panel's `layout_rect` from the shared solver over
    /// the canonical `SessionTree`, honoring panel margins / per-axis
    /// gaps, then fan the visible slot rect across each stacked-tab group.
    pub(crate) fn recompute_rects_from_tree(&mut self) {
        if self.inner.is_empty() {
            return;
        }
        let avail_w =
            (self.width - self.scaled_margin.left - self.scaled_margin.right).max(0.0);
        let avail_h =
            (self.height - self.scaled_margin.top - self.scaled_margin.bottom).max(0.0);
        let opts = SolveOpts {
            gap_x: self.panel_config.column_gap * self.scale,
            gap_y: self.panel_config.row_gap * self.scale,
            margin: self.panel_config.margin.left * self.scale,
            divider_tol: 0.0,
        };
        let solved = pane_geometry::solve_with(
            &self.session_tree,
            Rect::new(0.0, 0.0, avail_w, avail_h),
            &opts,
        );
        // `solve` returns one rect per *visible* leaf — for a stacked-tab
        // group that's whichever member is currently shown. Record which
        // nodes it actually wrote so the fan-out below uses the real slot
        // rect instead of guessing the active member.
        let mut written: Vec<NodeId> = Vec::with_capacity(solved.panes.len());
        for pane in &solved.panes {
            if let Some(&node) = self.leaf_to_node.get(&pane.leaf) {
                if let Some(item) = self.inner.get_mut(&node) {
                    let rect = [pane.rect.x, pane.rect.y, pane.rect.w, pane.rect.h];
                    item.slot_rect = rect;
                    item.layout_rect = rect;
                    written.push(node);
                }
            }
        }
        self.propagate_stacked_rects(&written);
    }

    /// Each stacked-tab group shares one slot rect. `solve` only wrote the
    /// visible member's rect (in `written`); copy that onto the host panel
    /// and every other tab in the group so switching tabs keeps them
    /// positioned — and so a freshly-split editor pane (which carries a
    /// file-tab strip = a `Tabbed` group) lands in its half instead of
    /// inheriting a stale full-width rect.
    fn propagate_stacked_rects(&mut self, written: &[NodeId]) {
        if self.stacked_nodes.is_empty() {
            return;
        }
        let mut hosts: Vec<NodeId> = Vec::new();
        for host in self.stacked_parents.values().copied() {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        for host in hosts {
            let mut members = vec![host];
            members.extend(self.stacked_children_of(host));
            // The slot rect is whichever member `solve` actually wrote
            // (the visible one). Fall back to the active member only if
            // none was written this pass.
            let slot_node = members
                .iter()
                .copied()
                .find(|node| written.contains(node))
                .or_else(|| {
                    let active = if Some(host) == self.root {
                        self.active_stacked
                    } else {
                        self.active_stacked_by_parent.get(&host).copied()
                    };
                    active.filter(|node| members.contains(node))
                })
                .unwrap_or(host);
            let slot_rect = self
                .inner
                .get(&slot_node)
                .map(|item| item.slot_rect)
                .unwrap_or([0.0, 0.0, 0.0, 0.0]);
            for member in members {
                if member == slot_node {
                    continue;
                }
                if let Some(item) = self.inner.get_mut(&member) {
                    item.slot_rect = slot_rect;
                    item.layout_rect = slot_rect;
                }
            }
        }
    }

    pub fn find_context_at_position(&self, x: f32, y: f32) -> Option<NodeId> {
        // Adjust for window margin - layout_rect is relative to root container
        let adj_x = x - self.scaled_margin.left;
        let adj_y = y - self.scaled_margin.top;

        for (&node_id, item) in &self.inner {
            if !self.is_context_visible(node_id) {
                continue;
            }
            let [left, top, width, height] = item.slot_rect;
            if adj_x >= left
                && adj_x < left + width
                && adj_y >= top
                && adj_y < top + height
            {
                return Some(node_id);
            }
        }
        None
    }

    pub(crate) fn apply_taffy_layout(&mut self, sugarloaf: &mut Sugarloaf) -> bool {
        if self.compute_layout().is_err() {
            return false;
        }
        self.repair_single_visible_pane_rect();

        let scale = sugarloaf.ctx.scale();
        let is_multi_panel = self.panel_count() > 1;
        let stacked_nodes = self.stacked_nodes.clone();
        let stacked_parents = self.stacked_parents.clone();
        let active_stacked = self
            .active_stacked
            .filter(|node| stacked_nodes.contains(node));
        let active_stacked_by_parent = self.active_stacked_by_parent.clone();
        let root = self.root;

        for (&node, item) in self.inner.iter_mut() {
            let [abs_x, abs_y, width, height] = item.layout_rect;
            let node_is_stacked = stacked_nodes.contains(&node);
            let visible = if node_is_stacked {
                match stacked_parents.get(&node).copied() {
                    Some(parent) if Some(parent) == root => Some(node) == active_stacked,
                    Some(parent) => {
                        active_stacked_by_parent.get(&parent).copied() == Some(node)
                    }
                    None => false,
                }
            } else if active_stacked.is_some() && Some(node) == root {
                false
            } else if active_stacked_by_parent.contains_key(&node) {
                false
            } else {
                true
            };

            let x = (abs_x + self.scaled_margin.left) / scale;
            let y = (abs_y + self.scaled_margin.top) / scale;

            // Clear margins / restore nominal terminal pitch before deriving
            // rows. A busy terminal must not prevent geometry/visibility from
            // reaching Sugarloaf; Screen retries its latest dimension on paint.
            apply_layout_dimensions(&mut item.val, width, height, visible);

            // Update position via sugarloaf (handles scaling)
            sugarloaf.set_position(item.val.rich_text_id, x, y);
            sugarloaf.set_visibility(item.val.rich_text_id, visible);

            // Set clipping bounds for multi-panel text overflow prevention
            if is_multi_panel {
                let bounds_x = abs_x + self.scaled_margin.left;
                let bounds_y = abs_y + self.scaled_margin.top;
                sugarloaf.set_bounds(
                    item.val.rich_text_id,
                    Some([bounds_x, bounds_y, width, height]),
                );
            } else {
                sugarloaf.set_bounds(item.val.rich_text_id, None);
            }
        }
        true
    }

    fn repair_single_visible_pane_rect(&mut self) {
        let mut visible = self
            .inner
            .keys()
            .copied()
            .filter(|node| self.is_context_visible(*node));
        let Some(node) = visible.next() else {
            return;
        };
        if visible.next().is_some() {
            return;
        }

        let available_width =
            (self.width - self.scaled_margin.left - self.scaled_margin.right).max(0.0);
        let available_height =
            (self.height - self.scaled_margin.top - self.scaled_margin.bottom).max(0.0);
        let Some(item) = self.inner.get_mut(&node) else {
            return;
        };
        let rect = [0.0, 0.0, available_width, available_height];
        item.slot_rect = rect;
        item.layout_rect = rect;
    }

    #[inline]
    pub fn grid_dimension(&self) -> ContextDimension {
        if let Some(current_item) = self.inner.get(&self.current) {
            let current_context_dimension = current_item.val.dimension;
            let scale = current_context_dimension.dimension.scale;
            // scaled_margin is already in physical pixels, but
            // ContextDimension::build scales the margin again via compute(),
            // so unscale it here to avoid double-scaling.
            let unscaled_margin = if scale > 0.0 {
                Margin::new(
                    self.scaled_margin.top / scale,
                    self.scaled_margin.right / scale,
                    self.scaled_margin.bottom / scale,
                    self.scaled_margin.left / scale,
                )
            } else {
                self.scaled_margin
            };
            ContextDimension::build(
                self.width,
                self.height,
                current_context_dimension.dimension,
                current_context_dimension.line_height,
                unscaled_margin,
            )
        } else {
            tracing::error!("Current key {:?} not found in grid", self.current);
            ContextDimension::default()
        }
    }

    pub fn update_scaled_margin(&mut self, scaled_margin: Margin) {
        self.scaled_margin = scaled_margin;
    }

    pub fn update_line_height(&mut self, line_height: f32) {
        for context in self.inner.values_mut() {
            context.val.dimension.update_line_height(line_height);
        }
    }

    pub fn update_dimensions(&mut self, sugarloaf: &mut Sugarloaf) {
        self.refresh_cell_dimensions(sugarloaf);

        // Always apply Taffy layout for consistent positioning
        self.apply_taffy_layout(sugarloaf);
    }

    /// Pull fresh cell width/height from sugarloaf into each context's
    /// `dimension` WITHOUT re-running Taffy. Used by `change_font_size`
    /// so the subsequent single `resize` pass derives cols/rows from
    /// the new cell sizes — the previous "resize then update_dimensions"
    /// double pass sent two `editor.resize` notifications to nvim
    /// (first with stale cell dims, second with the correct ones), and
    /// the in-flight first redraw arrived at the still-old top position
    /// so the chrome appeared to "eat" nvim's first lines for one frame.
    pub fn refresh_cell_dimensions(&mut self, sugarloaf: &mut Sugarloaf) {
        for context in self.inner.values_mut() {
            if let Some(layout) = sugarloaf.get_text_layout(&context.val.rich_text_id) {
                context.val.dimension.update_dimensions(layout.dimensions);
            }
        }
    }

    /// Resize grid - always uses Taffy for consistent layout
    pub fn resize(&mut self, new_width: f32, new_height: f32, sugarloaf: &mut Sugarloaf) {
        self.width = new_width;
        self.height = new_height;

        // Update Taffy size and recompute layout
        let _ = self.try_update_size(new_width, new_height);

        // Apply layout - works for both single and multi-panel
        self.apply_taffy_layout(sugarloaf);
    }

    #[inline]
    pub fn calculate_positions(&mut self) {
        if self.inner.is_empty() {
            return;
        }

        // Compute Taffy layout (also updates layout_rect via update_layout_rects)
        if self.compute_layout().is_err() {
            return;
        }

        // Update positions from layout_rect for all panels
        for item in self.inner.values_mut() {
            let x = item.layout_rect[0] + self.scaled_margin.left;
            let y = item.layout_rect[1] + self.scaled_margin.top;
            item.set_position([x, y]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::messenger::Messenger;
    use crate::context::factories::create_dead_context;
    use crate::event::Msg;
    use neoism_backend::event::{VoidListener, WindowId};
    use neoism_backend::sugarloaf::layout::TextDimensions;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn busy_layout_resize_is_nonblocking_and_paint_retry_sends_latest_winsize() {
        let dimension = ContextDimension::build(
            800.0,
            600.0,
            TextDimensions {
                width: 10.0,
                height: 20.0,
                scale: 1.0,
            },
            1.0,
            Margin::all(0.0),
        );
        let mut ctx =
            create_dead_context(VoidListener {}, WindowId::from(0), 7, 11, dimension);
        let (sender, receiver) = corcovado::channel::channel();
        ctx.messenger = Messenger::new(sender);
        let terminal = ctx.terminal.clone();
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        // Release even if a blocking lock regresses, rather than hang forever.
        let holder = std::thread::spawn(move || {
            let guard = terminal.lock();
            locked_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            drop(guard);
        });
        locked_rx.recv().unwrap();
        let started = Instant::now();
        // Opening a stacked chat lays out the now-hidden terminal, too.
        apply_layout_dimensions(&mut ctx, 600.0, 400.0, false);
        assert!(ctx.pending_terminal_resize);
        assert!(ctx.renderable_content.pending_update.is_dirty());
        assert_eq!((ctx.dimension.width, ctx.dimension.height), (600.0, 400.0));
        // A subsequent layout supersedes the old pending geometry.
        apply_layout_dimensions(&mut ctx, 500.0, 300.0, true);
        assert!(ctx.pending_terminal_resize);
        assert!(!crate::screen::Screen::apply_context_resize(&mut ctx));
        assert!(
            receiver.try_recv().is_err(),
            "no PTY resize before local acceptance"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "layout waited for terminal lock"
        );
        release_tx.send(()).unwrap();
        holder.join().unwrap();

        // Pane chrome can refine geometry after the grid pass. Exercise the
        // real paint retry, not another full layout, to catch stale sizes.
        ctx.dimension.update_height(260.0);
        let expected = crate::bridges::utils::terminal_dimensions(&ctx.dimension);
        assert!(crate::screen::Screen::apply_context_resize(&mut ctx));
        assert!(!ctx.pending_terminal_resize);
        let guard = ctx.terminal.lock();
        assert_eq!(guard.columns(), expected.cols as usize);
        assert_eq!(guard.screen_lines(), expected.rows as usize);
        drop(guard);
        let Msg::Resize(actual) = receiver.try_recv().expect("required PTY resize")
        else {
            panic!("expected resize message");
        };
        assert_eq!(
            (actual.cols, actual.rows, actual.width, actual.height),
            (
                expected.cols,
                expected.rows,
                expected.width,
                expected.height
            ),
        );
        assert!(receiver.try_recv().is_err());

        // Hidden success still sizes locally without notifying the PTY.
        // Showing the terminal later must send the latest geometry.
        apply_layout_dimensions(&mut ctx, 400.0, 200.0, false);
        assert!(!ctx.pending_terminal_resize);
        assert_eq!(ctx.terminal.lock().columns(), ctx.dimension.columns);
        assert!(receiver.try_recv().is_err());
        apply_layout_dimensions(&mut ctx, 400.0, 200.0, true);
        assert!(matches!(receiver.try_recv(), Ok(Msg::Resize(_))));
    }
}
