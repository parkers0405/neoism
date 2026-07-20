//! Per-pane terminal smooth-scroll state.
//!
//! Pixel-perfect smooth scroll for terminal panes — explicitly NOT a
//! spring. The spring is reserved for editor panes (matches
//! neovide's "content lags into position over animation_length" feel).
//! Terminals here behave like dragging a piece of paper: every wheel
//! pixel moves content by exactly that pixel, and the sub-row residual
//! stays where the user left it. No decay, no overshoot, no settle
//! time — input → motion is a 1:1 function.
//!
//! The offset is APPLIED to `grid_padding` in the desktop renderer (the
//! GPU uniforms that anchor the cell grid). Same surface as editor_scroll
//! uses, just with no animation tick.
//!
//! Lifted from the desktop `terminal/scroll.rs` so the web frontend can
//! mirror the same per-pane state for terminal scroll cursors,
//! detachment, and frame-source tracking.

use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlockScrollCursor {
    pub raw_top_abs: usize,
    pub chrome_row: usize,
}

#[derive(Default)]
pub struct TerminalScroll {
    panes: HashMap<usize, f32>,
    block_cursors: HashMap<usize, BlockScrollCursor>,
    block_bottom_cursors: HashMap<usize, BlockScrollCursor>,
    block_echo_rows: HashMap<usize, BTreeSet<usize>>,
    block_detached: HashSet<usize>,
    block_frame_sources: HashMap<usize, Vec<Option<usize>>>,
}

impl TerminalScroll {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take a wheel delta (physical pixels, positive = scroll up
    /// direction matching winit) and convert to (a) a possibly-non-zero
    /// number of rows to commit to terminal scrollback (`Scroll::Delta`)
    /// and (b) a sub-row residual stored as the pane's offset for the
    /// renderer to add to `panel_top` each frame. `cell_height` is the
    /// per-pane scaled cell height in physical pixels.
    pub fn add_wheel_delta(
        &mut self,
        rich_text_id: usize,
        delta_pixels: f32,
        cell_height: f32,
    ) -> i32 {
        if cell_height <= 0.0 {
            return 0;
        }
        let offset = self.panes.entry(rich_text_id).or_insert(0.0);
        *offset += delta_pixels;

        let mut committed = 0i32;
        while offset.abs() >= cell_height {
            let sign = offset.signum();
            *offset -= sign * cell_height;
            committed += sign as i32;
        }
        committed
    }

    /// Current visual offset in physical pixels for `rich_text_id`.
    /// Zero if no offset is registered. Caller adds this to `panel_top`
    /// when computing the cell grid origin each frame.
    pub fn current_offset(&self, rich_text_id: usize) -> f32 {
        self.panes.get(&rich_text_id).copied().unwrap_or(0.0)
    }

    #[allow(dead_code)]
    pub fn forget(&mut self, rich_text_id: usize) {
        self.panes.remove(&rich_text_id);
        self.block_cursors.remove(&rich_text_id);
        self.block_bottom_cursors.remove(&rich_text_id);
        self.block_echo_rows.remove(&rich_text_id);
        self.block_detached.remove(&rich_text_id);
        self.block_frame_sources.remove(&rich_text_id);
    }

    /// Reset the residual offset for a single pane to zero. Used when
    /// terminal scrollback hits a hard edge so rejected wheel input
    /// cannot leave the content visually parked between rows.
    pub fn reset_wheel(&mut self, rich_text_id: usize) {
        self.panes.remove(&rich_text_id);
    }

    /// Reset every pane's offset to zero. Used by `change_font_size`
    /// since cell_height changes underneath us and any pre-zoom
    /// residual is no longer meaningful.
    pub fn reset_all(&mut self) {
        for offset in self.panes.values_mut() {
            *offset = 0.0;
        }
        self.block_cursors.clear();
        self.block_bottom_cursors.clear();
        self.block_echo_rows.clear();
        self.block_detached.clear();
        self.block_frame_sources.clear();
    }

    pub fn block_cursor(&self, rich_text_id: usize) -> Option<BlockScrollCursor> {
        self.block_cursors.get(&rich_text_id).copied()
    }

    pub fn set_block_cursor(&mut self, rich_text_id: usize, cursor: BlockScrollCursor) {
        self.block_cursors.insert(rich_text_id, cursor);
    }

    pub fn clear_block_cursor(&mut self, rich_text_id: usize) {
        self.block_cursors.remove(&rich_text_id);
        self.block_bottom_cursors.remove(&rich_text_id);
        self.block_echo_rows.remove(&rich_text_id);
        self.block_detached.remove(&rich_text_id);
        self.block_frame_sources.remove(&rich_text_id);
    }

    pub fn block_bottom_cursor(&self, rich_text_id: usize) -> Option<BlockScrollCursor> {
        self.block_bottom_cursors.get(&rich_text_id).copied()
    }

    pub fn set_block_bottom_cursor(
        &mut self,
        rich_text_id: usize,
        cursor: BlockScrollCursor,
    ) {
        self.block_bottom_cursors.insert(rich_text_id, cursor);
    }

    pub fn block_echo_rows(&self, rich_text_id: usize) -> Option<&BTreeSet<usize>> {
        self.block_echo_rows.get(&rich_text_id)
    }

    pub fn set_block_echo_rows(
        &mut self,
        rich_text_id: usize,
        echo_rows: BTreeSet<usize>,
    ) {
        self.block_echo_rows.insert(rich_text_id, echo_rows);
    }

    pub fn block_detached(&self, rich_text_id: usize) -> bool {
        self.block_detached.contains(&rich_text_id)
    }

    pub fn set_block_detached(&mut self, rich_text_id: usize, detached: bool) {
        if detached {
            self.block_detached.insert(rich_text_id);
        } else {
            self.block_detached.remove(&rich_text_id);
        }
    }

    pub fn set_block_frame_sources_changed(
        &mut self,
        rich_text_id: usize,
        sources: &[Option<usize>],
    ) -> bool {
        let changed = self
            .block_frame_sources
            .get(&rich_text_id)
            .map(|previous| previous.as_slice() != sources)
            .unwrap_or(true);
        if changed {
            self.block_frame_sources
                .insert(rich_text_id, sources.to_vec());
        }
        changed
    }

    pub fn clear_block_frame_sources(&mut self, rich_text_id: usize) {
        self.block_frame_sources.remove(&rich_text_id);
    }

    pub fn block_frame_source_at(
        &self,
        rich_text_id: usize,
        visual_row: usize,
    ) -> Option<Option<usize>> {
        self.block_frame_sources.get(&rich_text_id).map(|sources| {
            crate::render_policy::block_visual_to_source_row(sources, visual_row)
                .unwrap_or(None)
        })
    }
}
