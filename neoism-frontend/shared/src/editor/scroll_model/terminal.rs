use crate::terminal_blocks::chrome::COMMAND_BLOCK_CHROME_ROWS;
use crate::terminal_blocks::command::CommandBlockSnapshot;
use crate::widgets::scrollbar::PanelScrollState;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalScrollbarPanelContext {
    pub rich_text_id: usize,
    /// Pane rectangle in physical pixels: `[left, top, width, height]`.
    pub panel_rect: [f32; 4],
    pub display_offset: usize,
    pub history_size: usize,
    pub screen_lines: usize,
    /// Rows reserved by a host-side composer/footer inside the terminal pane.
    pub reserved_footer_rows: usize,
    pub cell_height_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalWheelEdgeAction {
    Continue,
    Reject { clear_block_detached: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalWheelEdgeContext {
    pub delta_pixels: f32,
    pub display_offset: usize,
    pub history_size: usize,
    pub use_block_scroll: bool,
    pub block_cursor_can_scroll_at_bottom: bool,
    pub block_at_composed_top: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalBlockWheelEdgeContext {
    pub delta_pixels: f32,
    pub display_offset: usize,
    pub history_size: usize,
    pub use_block_scroll: bool,
    pub content_top_abs: Option<usize>,
    pub stored_cursor: Option<TerminalBlockScrollCursor>,
    pub bottom_cursor: Option<TerminalBlockScrollCursor>,
    pub block_detached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TerminalBlockScrollCursor {
    pub raw_top_abs: usize,
    pub chrome_row: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalBlockScrollMoved {
    pub cursor: TerminalBlockScrollCursor,
    pub direction: i32,
    pub raw_delta: i64,
    pub raw_scroll_delta: Option<i32>,
    pub cursor_only_at_top: bool,
    pub cursor_only_at_bottom: bool,
    pub anchor_abs: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalBlockScrollPlan {
    MissingAnchor,
    CursorUnchanged { raw_scroll_delta: Option<i32> },
    CursorMoved(TerminalBlockScrollMoved),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalBlockScrollFinish {
    StoreCursor {
        cursor: TerminalBlockScrollCursor,
        set_detached: Option<bool>,
        notify_scrollbar: bool,
        reset_wheel: bool,
        clear_accumulated_scroll: bool,
        reached_top: bool,
        reached_bottom: bool,
    },
    ResetToAnchor {
        cursor: TerminalBlockScrollCursor,
        clear_detached: bool,
        reset_wheel: bool,
        clear_accumulated_scroll: bool,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct TerminalBlockScrollCommitContext<'a> {
    pub committed_rows: i32,
    pub display_offset: usize,
    pub history_size: usize,
    pub content_top_abs: Option<usize>,
    pub existing_cursor: Option<TerminalBlockScrollCursor>,
    pub bottom_cursor: Option<TerminalBlockScrollCursor>,
    pub snapshots: &'a [CommandBlockSnapshot],
    pub echo_rows: Option<&'a BTreeSet<usize>>,
}

impl<'a> TerminalBlockScrollCommitContext<'a> {
    pub fn plan(self) -> TerminalBlockScrollPlan {
        let Some(content_top_abs) = self.content_top_abs else {
            return TerminalBlockScrollPlan::MissingAnchor;
        };

        let mut cursor =
            block_scroll_cursor_or_anchor(self.existing_cursor, content_top_abs);
        cursor.chrome_row = cursor.chrome_row.min(
            block_row_visual_height(cursor.raw_top_abs, self.snapshots, self.echo_rows)
                .saturating_sub(1),
        );

        let old_cursor = cursor;
        let old_raw_top_abs = cursor.raw_top_abs;
        let direction = self.committed_rows.signum();
        for _ in 0..self.committed_rows.unsigned_abs() {
            advance_block_scroll_cursor(
                &mut cursor,
                direction,
                self.snapshots,
                self.echo_rows,
            );
        }
        if direction < 0 {
            if let Some(bottom_cursor) = self.bottom_cursor {
                cursor = cursor.min(bottom_cursor);
            }
        }

        if cursor == old_cursor {
            return TerminalBlockScrollPlan::CursorUnchanged {
                raw_scroll_delta: raw_scroll_has_room(
                    direction,
                    self.display_offset,
                    self.history_size,
                )
                .then_some(self.committed_rows),
            };
        }

        let raw_delta = old_raw_top_abs as i64 - cursor.raw_top_abs as i64;
        let cursor_only_at_bottom = self.display_offset == 0 && raw_delta < 0;
        let cursor_only_at_top =
            self.display_offset >= self.history_size && raw_delta > 0;
        let raw_scroll_delta =
            (raw_delta != 0 && !cursor_only_at_bottom && !cursor_only_at_top)
                .then_some(raw_delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32);

        TerminalBlockScrollPlan::CursorMoved(TerminalBlockScrollMoved {
            cursor,
            direction,
            raw_delta,
            raw_scroll_delta,
            cursor_only_at_top,
            cursor_only_at_bottom,
            anchor_abs: content_top_abs,
        })
    }
}

impl TerminalBlockScrollMoved {
    pub fn finish(
        self,
        terminal_scrolled: bool,
        display_after: usize,
        history_size: usize,
        bottom_cursor: Option<TerminalBlockScrollCursor>,
    ) -> TerminalBlockScrollFinish {
        if !terminal_scrolled {
            return TerminalBlockScrollFinish::ResetToAnchor {
                cursor: TerminalBlockScrollCursor {
                    raw_top_abs: self.anchor_abs,
                    chrome_row: 0,
                },
                clear_detached: true,
                reset_wheel: true,
                clear_accumulated_scroll: true,
            };
        }

        let reached_top = self.direction > 0
            && display_after >= history_size
            && self.cursor.raw_top_abs == 0
            && self.cursor.chrome_row == 0;
        let reached_bottom = self.direction < 0
            && bottom_cursor.map(|bottom| self.cursor >= bottom).unwrap_or(
                display_after == 0
                    && self.raw_delta != 0
                    && !self.cursor_only_at_bottom
                    && !self.cursor_only_at_top,
            );

        TerminalBlockScrollFinish::StoreCursor {
            cursor: self.cursor,
            set_detached: if reached_bottom {
                Some(false)
            } else if self.direction > 0 {
                Some(true)
            } else {
                None
            },
            notify_scrollbar: true,
            reset_wheel: reached_top || reached_bottom,
            clear_accumulated_scroll: reached_top || reached_bottom,
            reached_top,
            reached_bottom,
        }
    }
}

/// Mechanical side effect emitted by `commit_terminal_block_scroll` /
/// `commit_terminal_raw_scroll`. Hosts apply these in order. Each
/// variant maps 1:1 to a single renderer / terminal / mouse mutation
/// in the desktop big-else block (frontends/neoism/src/screen/
/// editor_scroll.rs ~2378-2563). Keeping the side-effect order in
/// data (instead of branches threaded through host code) is what
/// makes the wasm shell able to mirror desktop bit-for-bit.
///
/// Side effects are emitted in a strict order so the host can apply
/// them with a simple `for effect in plan.effects { ... }` loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerminalScrollSideEffect {
    /// `terminal_scroll.clear_block_cursor(rich_text_id)` — drop a
    /// stale block cursor before falling back to raw scroll.
    ClearBlockCursor,
    /// `terminal.scroll_display(Scroll::Delta(rows))` — raw alacritty
    /// viewport movement. Host MUST report the resulting
    /// `display_offset` back via [`TerminalScrollCommit::with_raw_result`]
    /// when it produced a `RawScrollResultRequired` follow-up, or just
    /// run the post-scroll effects unconditionally for the trailing
    /// raw-only branch.
    ScrollDisplayRows(i32),
    /// `terminal_scroll.set_block_cursor(rich_text_id, cursor)`.
    SetBlockCursor(TerminalBlockScrollCursor),
    /// `terminal_scroll.set_block_detached(rich_text_id, detached)`.
    SetBlockDetached(bool),
    /// `scrollbar.notify_scroll(rich_text_id)` — host calls when the
    /// underlying terminal display offset actually moved.
    NotifyScrollbar,
    /// `terminal_scroll.reset_wheel(rich_text_id)`.
    ResetWheel,
    /// `mouse.accumulated_scroll.y = 0.0` — drop residual sub-row
    /// drift so the next wheel input starts clean.
    ClearAccumulatedScrollY,
}

/// Trace payload mirroring the `block_log_enabled()` lines that the
/// desktop block-scroll commit emits. Hosts that want parity (and
/// the snapshot tests) read this; web can ignore it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalBlockScrollTrace {
    pub committed_rows: i32,
    pub direction: i32,
    pub old_cursor: TerminalBlockScrollCursor,
    pub new_cursor: TerminalBlockScrollCursor,
    pub old_raw_top_abs: usize,
    pub raw_delta: i64,
    pub cursor_only_at_top: bool,
    pub cursor_only_at_bottom: bool,
}

/// Whether the plan needs the host to feed back the post-`scroll_display`
/// `display_offset` so the finish-stage decisions (reached_top /
/// reached_bottom edge handling) are correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalScrollFollowup {
    /// No follow-up. All effects in the plan are unconditional.
    None,
    /// After running `ScrollDisplayRows`, the host must compute
    /// `terminal_scrolled = new_display_offset != old_display_offset`
    /// and the `post_display_offset` value, then call
    /// [`TerminalScrollCommit::resume`] (or apply the
    /// `RawScrollResultRequired` arm directly) to get the remaining
    /// effects. We keep this as a typed sentinel rather than running
    /// the terminal mutation in pure code because alacritty's
    /// `Term::scroll_display` is host-owned.
    BlockMoveRequiresRawResult { moved: TerminalBlockScrollMoved },
    /// Raw-only path (block_scroll disabled). After running
    /// `ScrollDisplayRows`, host applies `NotifyScrollbar` if
    /// `terminal_scrolled` else `ResetWheel + ClearAccumulatedScrollY`.
    RawOnlyRequiresScrollResult,
    /// Block path, `CursorUnchanged` recovery. After the
    /// `ScrollDisplayRows`, host applies `NotifyScrollbar` only if
    /// `terminal_scrolled`. The trailing `ResetWheel +
    /// ClearAccumulatedScrollY` are always applied.
    BlockRecoveryRequiresScrollResult,
}

/// Output of [`TerminalScrollCommit::commit`]. The host applies
/// `effects` in order, then if `followup != None` runs the appropriate
/// resume step.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalScrollPlan {
    pub effects: Vec<TerminalScrollSideEffect>,
    pub followup: TerminalScrollFollowup,
    pub trace: Option<TerminalBlockScrollTrace>,
    pub mark_dirty: bool,
}

impl TerminalScrollPlan {
    pub fn empty_with_dirty() -> Self {
        Self {
            effects: Vec::new(),
            followup: TerminalScrollFollowup::None,
            trace: None,
            mark_dirty: true,
        }
    }

    /// Resume the side effect list after the host has run
    /// `ScrollDisplayRows` and observed whether the terminal display
    /// offset actually changed.
    ///
    /// `display_after` is the `display_offset` AFTER the scroll
    /// (`post_display_offset` in desktop). For the
    /// `cursor_only_at_*` shortcut, desktop falls back to the
    /// pre-scroll display_offset and reports terminal_scrolled = true;
    /// pass that combination here too.
    pub fn resume(
        followup: TerminalScrollFollowup,
        terminal_scrolled: bool,
        display_after: usize,
        history_size: usize,
        bottom_cursor: Option<TerminalBlockScrollCursor>,
    ) -> Vec<TerminalScrollSideEffect> {
        match followup {
            TerminalScrollFollowup::None => Vec::new(),
            TerminalScrollFollowup::RawOnlyRequiresScrollResult => {
                if terminal_scrolled {
                    vec![TerminalScrollSideEffect::NotifyScrollbar]
                } else {
                    vec![
                        TerminalScrollSideEffect::ResetWheel,
                        TerminalScrollSideEffect::ClearAccumulatedScrollY,
                    ]
                }
            }
            TerminalScrollFollowup::BlockRecoveryRequiresScrollResult => {
                let mut out = Vec::new();
                if terminal_scrolled {
                    out.push(TerminalScrollSideEffect::NotifyScrollbar);
                }
                out.push(TerminalScrollSideEffect::ResetWheel);
                out.push(TerminalScrollSideEffect::ClearAccumulatedScrollY);
                out
            }
            TerminalScrollFollowup::BlockMoveRequiresRawResult { moved } => {
                let finish = moved.finish(
                    terminal_scrolled,
                    display_after,
                    history_size,
                    bottom_cursor,
                );
                match finish {
                    TerminalBlockScrollFinish::StoreCursor {
                        cursor,
                        set_detached,
                        notify_scrollbar,
                        reset_wheel,
                        clear_accumulated_scroll,
                        ..
                    } => {
                        let mut out =
                            vec![TerminalScrollSideEffect::SetBlockCursor(cursor)];
                        if let Some(detached) = set_detached {
                            out.push(TerminalScrollSideEffect::SetBlockDetached(
                                detached,
                            ));
                        }
                        if notify_scrollbar {
                            out.push(TerminalScrollSideEffect::NotifyScrollbar);
                        }
                        if reset_wheel {
                            out.push(TerminalScrollSideEffect::ResetWheel);
                        }
                        if clear_accumulated_scroll {
                            out.push(TerminalScrollSideEffect::ClearAccumulatedScrollY);
                        }
                        out
                    }
                    TerminalBlockScrollFinish::ResetToAnchor {
                        cursor,
                        clear_detached,
                        reset_wheel,
                        clear_accumulated_scroll,
                    } => {
                        let mut out =
                            vec![TerminalScrollSideEffect::SetBlockCursor(cursor)];
                        if clear_detached {
                            out.push(TerminalScrollSideEffect::SetBlockDetached(false));
                        }
                        if reset_wheel {
                            out.push(TerminalScrollSideEffect::ResetWheel);
                        }
                        if clear_accumulated_scroll {
                            out.push(TerminalScrollSideEffect::ClearAccumulatedScrollY);
                        }
                        out
                    }
                }
            }
        }
    }
}

/// Top-level commit driver. Wraps [`TerminalBlockScrollCommitContext`]
/// plus the raw-scroll fall-through path so the host can run a single
/// call and walk the resulting [`TerminalScrollPlan`].
///
/// Order of operations encoded:
///   1. If `use_block_scroll == false`, behave like the desktop
///      `else` arm at line 2540: emit `ScrollDisplayRows(committed_rows)`
///      with `RawOnlyRequiresScrollResult` follow-up.
///   2. If block scroll on but anchor missing (line 2353), drop wheel
///      and accumulator, mark dirty, no terminal mutation.
///   3. Block path → delegate to [`TerminalBlockScrollCommitContext::plan`]
///      and translate each variant into the ordered effect list.
#[derive(Clone, Copy, Debug)]
pub struct TerminalScrollCommit<'a> {
    pub use_block_scroll: bool,
    pub block: TerminalBlockScrollCommitContext<'a>,
}

impl<'a> TerminalScrollCommit<'a> {
    pub fn commit(self) -> TerminalScrollPlan {
        if !self.use_block_scroll {
            return TerminalScrollPlan {
                effects: vec![TerminalScrollSideEffect::ScrollDisplayRows(
                    self.block.committed_rows,
                )],
                followup: TerminalScrollFollowup::RawOnlyRequiresScrollResult,
                trace: None,
                mark_dirty: true,
            };
        }

        // Pre-compute the trace `old_cursor`/`old_raw_top_abs` view
        // the same way `plan()` does so the trace lines match desktop.
        let trace_pre = self.block.content_top_abs.map(|content_top_abs| {
            let mut cursor = block_scroll_cursor_or_anchor(
                self.block.existing_cursor,
                content_top_abs,
            );
            cursor.chrome_row = cursor.chrome_row.min(
                block_row_visual_height(
                    cursor.raw_top_abs,
                    self.block.snapshots,
                    self.block.echo_rows,
                )
                .saturating_sub(1),
            );
            cursor
        });

        match self.block.plan() {
            TerminalBlockScrollPlan::MissingAnchor => TerminalScrollPlan {
                effects: vec![
                    TerminalScrollSideEffect::ResetWheel,
                    TerminalScrollSideEffect::ClearAccumulatedScrollY,
                ],
                followup: TerminalScrollFollowup::None,
                trace: None,
                mark_dirty: true,
            },
            TerminalBlockScrollPlan::CursorUnchanged { raw_scroll_delta } => {
                let pre =
                    trace_pre.expect("anchor must exist when plan is CursorUnchanged");
                let trace = Some(TerminalBlockScrollTrace {
                    committed_rows: self.block.committed_rows,
                    direction: self.block.committed_rows.signum(),
                    old_cursor: pre,
                    new_cursor: pre,
                    old_raw_top_abs: pre.raw_top_abs,
                    raw_delta: 0,
                    cursor_only_at_top: false,
                    cursor_only_at_bottom: false,
                });
                match raw_scroll_delta {
                    Some(delta) => TerminalScrollPlan {
                        effects: vec![
                            TerminalScrollSideEffect::ClearBlockCursor,
                            TerminalScrollSideEffect::ScrollDisplayRows(delta),
                        ],
                        followup:
                            TerminalScrollFollowup::BlockRecoveryRequiresScrollResult,
                        trace,
                        mark_dirty: true,
                    },
                    None => TerminalScrollPlan {
                        effects: vec![
                            TerminalScrollSideEffect::ResetWheel,
                            TerminalScrollSideEffect::ClearAccumulatedScrollY,
                        ],
                        followup: TerminalScrollFollowup::None,
                        trace,
                        mark_dirty: true,
                    },
                }
            }
            TerminalBlockScrollPlan::CursorMoved(moved) => {
                let pre = trace_pre.expect("anchor must exist when plan is CursorMoved");
                let trace = Some(TerminalBlockScrollTrace {
                    committed_rows: self.block.committed_rows,
                    direction: moved.direction,
                    old_cursor: pre,
                    new_cursor: moved.cursor,
                    old_raw_top_abs: pre.raw_top_abs,
                    raw_delta: moved.raw_delta,
                    cursor_only_at_top: moved.cursor_only_at_top,
                    cursor_only_at_bottom: moved.cursor_only_at_bottom,
                });
                let effects = match moved.raw_scroll_delta {
                    Some(delta) => {
                        vec![TerminalScrollSideEffect::ScrollDisplayRows(delta)]
                    }
                    None => Vec::new(),
                };
                TerminalScrollPlan {
                    effects,
                    followup: TerminalScrollFollowup::BlockMoveRequiresRawResult {
                        moved,
                    },
                    trace,
                    mark_dirty: true,
                }
            }
        }
    }
}

impl TerminalWheelEdgeContext {
    pub fn raw_edge_rejected(self) -> bool {
        (self.delta_pixels > 0.0 && self.display_offset >= self.history_size)
            || (self.delta_pixels < 0.0 && self.display_offset == 0)
    }

    pub fn action(self) -> TerminalWheelEdgeAction {
        if !self.raw_edge_rejected() {
            return TerminalWheelEdgeAction::Continue;
        }

        let scrolling_down_at_bottom =
            self.delta_pixels < 0.0 && self.block_cursor_can_scroll_at_bottom;
        if self.use_block_scroll
            && scrolling_down_at_bottom
            && !self.block_at_composed_top
        {
            return TerminalWheelEdgeAction::Continue;
        }

        if self.use_block_scroll && self.delta_pixels > 0.0 && !self.block_at_composed_top
        {
            return TerminalWheelEdgeAction::Continue;
        }

        TerminalWheelEdgeAction::Reject {
            clear_block_detached: self.use_block_scroll && self.delta_pixels < 0.0,
        }
    }
}

impl TerminalBlockWheelEdgeContext {
    pub fn block_cursor_can_scroll_at_bottom(self) -> bool {
        self.use_block_scroll
            && self.delta_pixels < 0.0
            && self.display_offset == 0
            && self.block_detached
            && self
                .stored_cursor
                .zip(self.bottom_cursor)
                .is_some_and(|(cursor, bottom)| cursor < bottom)
    }

    pub fn block_at_composed_top(self) -> bool {
        self.use_block_scroll
            && self.delta_pixels > 0.0
            && self.display_offset >= self.history_size
            && self
                .stored_cursor
                .map(|cursor| cursor.raw_top_abs == 0 && cursor.chrome_row == 0)
                .unwrap_or(self.content_top_abs == Some(0))
    }

    pub fn wheel_edge_context(self) -> TerminalWheelEdgeContext {
        TerminalWheelEdgeContext {
            delta_pixels: self.delta_pixels,
            display_offset: self.display_offset,
            history_size: self.history_size,
            use_block_scroll: self.use_block_scroll,
            block_cursor_can_scroll_at_bottom: self.block_cursor_can_scroll_at_bottom(),
            block_at_composed_top: self.block_at_composed_top(),
        }
    }

    pub fn raw_edge_rejected(self) -> bool {
        self.wheel_edge_context().raw_edge_rejected()
    }

    pub fn action(self) -> TerminalWheelEdgeAction {
        self.wheel_edge_context().action()
    }
}

pub fn block_row_visual_height(
    abs_row: usize,
    snapshots: &[CommandBlockSnapshot],
    echo_rows: Option<&BTreeSet<usize>>,
) -> usize {
    if echo_rows.is_some_and(|rows| rows.contains(&abs_row))
        || snapshots
            .iter()
            .any(|block| block.output_start_row == Some(abs_row))
    {
        COMMAND_BLOCK_CHROME_ROWS
    } else {
        1
    }
}

pub fn advance_block_scroll_cursor(
    cursor: &mut TerminalBlockScrollCursor,
    direction: i32,
    snapshots: &[CommandBlockSnapshot],
    echo_rows: Option<&BTreeSet<usize>>,
) {
    if direction > 0 {
        if cursor.chrome_row > 0 {
            cursor.chrome_row -= 1;
        } else if cursor.raw_top_abs > 0 {
            cursor.raw_top_abs -= 1;
            cursor.chrome_row =
                block_row_visual_height(cursor.raw_top_abs, snapshots, echo_rows)
                    .saturating_sub(1);
        }
    } else if direction < 0 {
        let height = block_row_visual_height(cursor.raw_top_abs, snapshots, echo_rows);
        if cursor.chrome_row + 1 < height {
            cursor.chrome_row += 1;
        } else {
            cursor.raw_top_abs = cursor.raw_top_abs.saturating_add(1);
            cursor.chrome_row = 0;
        }
    }
}

pub fn raw_scroll_has_room(
    direction: i32,
    display_offset: usize,
    history_size: usize,
) -> bool {
    if direction > 0 {
        display_offset < history_size
    } else if direction < 0 {
        display_offset > 0
    } else {
        false
    }
}

pub fn block_scroll_cursor_or_anchor(
    existing: Option<TerminalBlockScrollCursor>,
    anchor_abs: usize,
) -> TerminalBlockScrollCursor {
    // The block cursor is a virtual stream cursor. Its raw row is not
    // expected to equal the raw terminal viewport top once command
    // chrome has expanded rows above it.
    existing.unwrap_or(TerminalBlockScrollCursor {
        raw_top_abs: anchor_abs,
        chrome_row: 0,
    })
}

impl TerminalScrollbarPanelContext {
    pub fn panel_state(self) -> PanelScrollState {
        let mut panel_rect = self.panel_rect;
        let mut screen_lines = self.screen_lines;
        let reserved_rows = self
            .reserved_footer_rows
            .min(screen_lines.saturating_sub(1));
        if reserved_rows > 0 {
            let cell_height = self.cell_height_px.round().max(1.0);
            panel_rect[3] =
                (panel_rect[3] - reserved_rows as f32 * cell_height).max(cell_height);
            screen_lines = screen_lines.saturating_sub(reserved_rows).max(1);
        }

        PanelScrollState {
            rich_text_id: self.rich_text_id,
            panel_rect,
            display_offset: self.display_offset,
            history_size: self.history_size,
            screen_lines,
        }
    }
}
