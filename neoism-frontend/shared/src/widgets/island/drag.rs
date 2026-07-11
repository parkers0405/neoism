use super::*;
use rustc_hash::FxHashMap;
use web_time::Duration;
use web_time::Instant;

impl Island {
    /// Move the focus cursor one tab left (`previous`) or right, wrapping
    /// at the edges. Mirror of `BufferTabs::move_focused` — moves the
    /// CURSOR only; it does NOT switch the active workspace. Returns
    /// `true` when the cursor actually moved.
    pub fn move_focus_cursor(&mut self, previous: bool, num_tabs: usize) -> bool {
        if !self.focused || num_tabs <= 1 {
            return false;
        }
        let current = self.focus_cursor.min(num_tabs - 1);
        self.focus_cursor = if previous {
            if current == 0 {
                num_tabs - 1
            } else {
                current - 1
            }
        } else {
            (current + 1) % num_tabs
        };
        self.focus_cursor != current
    }

    /// Focus-cursor rect (logical px) recomputed each `render` while
    /// focused. Mirror of `BufferTabs::focused_cursor_rect` — the host
    /// feeds this into the shared animated trail cursor.
    #[inline]
    pub fn focused_cursor_rect(&self) -> Option<[f32; 4]> {
        if self.focused {
            self.focused_cursor_rect
        } else {
            None
        }
    }

    // ---------------------------------------------------------------
    // Animated hover — mirror of `BufferTabs::set_hover` /
    // `clear_hover_immediate`. `set_hover(Some(ix))` starts an ease-out
    // grow on the hovered tab; transitions animate from the previous
    // tab so hover slides between tabs instead of snapping.
    // ---------------------------------------------------------------

    /// Set (or clear) the hovered workspace tab. Returns `true` when the
    /// hover changed (so the host can request a redraw).
    pub fn set_hover(&mut self, hover: Option<usize>, num_tabs: usize) -> bool {
        let hover = hover.filter(|&ix| ix < num_tabs);
        if self.hover == hover {
            return false;
        }
        if self.hover != hover {
            self.hover_from = self.hover;
            self.hover_to = hover;
            self.hover_anim_started = Some(Instant::now());
        }
        self.hover = hover;
        true
    }

    /// Clear hover without playing the hover-out animation. Used when a
    /// modal opens over the strip so stale hover doesn't keep repainting.
    pub fn clear_hover_immediate(&mut self) -> bool {
        let changed = self.hover.is_some()
            || self.hover_anim_started.is_some()
            || self.hover_from.is_some()
            || self.hover_to.is_some();
        self.hover = None;
        self.hover_anim_started = None;
        self.hover_from = None;
        self.hover_to = None;
        changed
    }

    /// Whether the hover grow/shrink animation is still in flight.
    pub(crate) fn hover_is_animating(&self) -> bool {
        self.hover_anim_started.is_some_and(|started| {
            started.elapsed() < Duration::from_millis(TAB_HOVER_ANIM_MS)
        })
    }

    // ---------------------------------------------------------------
    // Workspace-tab drag — mirrors the press/move/release pipeline
    // `BufferTabs` already has, but for the top-level Island strip.
    // ---------------------------------------------------------------

    /// Arm a drag on `tab_ix`. `mouse_x`/`mouse_y` are logical-px window
    /// coordinates; `strip_left` is the x of the first tab; `tab_width`
    /// is the equal-width tab slot used by `render`. No motion happens
    /// until `update_drag` reports the cursor moved past
    /// [`DRAG_ACTIVATION_PX`], so a normal click stays a click.
    pub fn begin_drag(
        &mut self,
        tab_ix: usize,
        mouse_x: f32,
        mouse_y: f32,
        strip_left: f32,
        tab_width: f32,
    ) {
        let tab_x = strip_left + tab_ix as f32 * tab_width;
        let grab_offset_x = (mouse_x - tab_x).clamp(0.0, tab_width);
        self.drag = Some(IslandDragState {
            source_index: tab_ix,
            grab_offset_x,
            start_x: mouse_x,
            start_y: mouse_y,
            current_x: mouse_x,
            current_y: mouse_y,
            live: false,
            detach_armed: false,
        });
    }

    /// Drive a live drag. Returns `Some((from, to))` when the dragged
    /// tab should be reordered in the source-of-truth (caller swaps
    /// `context_manager.contexts`). Also flips the internal "detach
    /// armed" flag when the cursor leaves the strip by more than
    /// [`DETACH_THRESHOLD_PX`].
    pub fn update_drag(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        strip_left: f32,
        strip_top: f32,
        tab_width: f32,
        num_tabs: usize,
    ) -> Option<(usize, usize)> {
        let Some(drag) = self.drag.as_mut() else {
            return None;
        };
        drag.current_x = mouse_x;
        drag.current_y = mouse_y;
        if !drag.live {
            let dx = mouse_x - drag.start_x;
            let dy = mouse_y - drag.start_y;
            if (dx * dx + dy * dy).sqrt() >= DRAG_ACTIVATION_PX {
                drag.live = true;
            } else {
                return None;
            }
        }
        // Vertical-out: cursor above the strip OR below it by more than
        // the detach threshold. Below-strip is the natural "pull down"
        // tear-off; above-strip is the natural "pull up" gesture and
        // only happens with negative coords on most platforms, so we
        // accept either side symmetrically.
        let below = mouse_y - (strip_top + ISLAND_HEIGHT);
        let above = strip_top - mouse_y;
        drag.detach_armed = below > DETACH_THRESHOLD_PX || above > DETACH_THRESHOLD_PX;

        if drag.detach_armed || tab_width <= 0.0 || num_tabs == 0 {
            // While detach is armed we don't reorder — the user is
            // telling us they want to lift the workspace out of the
            // strip, not slide it sideways.
            return None;
        }

        // Insertion index of the dragged tab: where its CENTER currently
        // sits inside the strip, clamped to a valid slot.
        let center_x = mouse_x - drag.grab_offset_x + tab_width * 0.5;
        let local = (center_x - strip_left).max(0.0);
        let target = ((local / tab_width) as usize).min(num_tabs.saturating_sub(1));
        if target != drag.source_index {
            let from = drag.source_index;
            drag.source_index = target;
            return Some((from, target));
        }
        None
    }

    /// Finalize the drag. Caller checks the return value to decide
    /// commit/detach/no-op semantics. Always clears internal state.
    pub fn end_drag(&mut self) -> IslandDragRelease {
        let Some(drag) = self.drag.take() else {
            return IslandDragRelease::None;
        };
        if !drag.live {
            return IslandDragRelease::None;
        }
        if drag.detach_armed {
            return IslandDragRelease::Detach {
                source_index: drag.source_index,
            };
        }
        IslandDragRelease::Reorder
    }

    /// Abort a drag without committing — used when the gesture is
    /// canceled by something else (e.g. a modal opening).
    #[allow(dead_code)]
    pub fn cancel_drag(&mut self) {
        self.drag = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.map(|d| d.live).unwrap_or(false)
    }

    pub fn is_detach_armed(&self) -> bool {
        self.drag.map(|d| d.detach_armed).unwrap_or(false)
    }

    /// Current source-index of the dragged tab. Tracks live during the
    /// drag — render uses this to know which slot is the "picked" one
    /// and which slot to paint the floating tab over.
    pub fn drag_source_index(&self) -> Option<usize> {
        self.drag.map(|d| d.source_index)
    }

    /// Re-target the drag's source index, e.g. after the caller has
    /// done a multi-step rearrangement that left the dragged tab at a
    /// different position than `update_drag` reported. Most callers
    /// don't need this; reorder consumers should match `update_drag`'s
    /// `from/to` directly.
    #[allow(dead_code)]
    pub fn retarget_drag_source(&mut self, new_source: usize) {
        if let Some(drag) = self.drag.as_mut() {
            drag.source_index = new_source;
        }
    }

    /// Rebase the per-tab state (colors / custom titles) for a swap.
    /// Mirrors `remove_tab_state` but for an in-place reorder.
    pub fn swap_tab_state(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        fn rebase_map<V: Clone>(map: &mut FxHashMap<usize, V>, from: usize, to: usize) {
            let mut next = FxHashMap::default();
            for (idx, value) in map.iter() {
                let new_idx = if *idx == from {
                    to
                } else if from < to {
                    if *idx > from && *idx <= to {
                        idx - 1
                    } else {
                        *idx
                    }
                } else if *idx >= to && *idx < from {
                    idx + 1
                } else {
                    *idx
                };
                next.insert(new_idx, value.clone());
            }
            *map = next;
        }
        rebase_map(&mut self.tab_colors, from, to);
        rebase_map(&mut self.tab_custom_titles, from, to);
        if let Some(picker) = self.color_picker_tab {
            self.color_picker_tab = Some(if picker == from {
                to
            } else if from < to && picker > from && picker <= to {
                picker - 1
            } else if from > to && picker >= to && picker < from {
                picker + 1
            } else {
                picker
            });
        }
    }
}
