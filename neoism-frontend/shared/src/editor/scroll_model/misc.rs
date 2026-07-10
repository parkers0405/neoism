use crate::panels::completion_menu::ScrollDelta;

/// Convert a host-neutral wheel delta into vertical pixels for an
/// overlay panel (modal, picker, finder, command palette, context
/// menu, git diff). LineDelta lines are scaled by `row_height * 3.0`
/// — three rows per notch matches the user's expectation of a "page
/// step" feel without crossing a full screen; PixelDelta passes
/// through 1:1 because high-res trackpads already report finger
/// motion in logical pixels.
pub fn vertical_overlay_scroll_pixels(delta: &ScrollDelta, row_height: f32) -> f32 {
    match delta {
        ScrollDelta::Lines { x: _, y } => *y * row_height.max(1.0) * 3.0,
        ScrollDelta::Pixels { x: _, y } => *y,
    }
}

/// Vertical pixels per wheel notch for the agent timeline / chat
/// pane. Calmer than overlay panels because the chat is dense and a
/// big step makes it hard to track the conversation. LineDelta lines
/// land at ~24px and PixelDelta passes through so high-resolution
/// trackpads preserve their native smoothness.
pub fn agent_timeline_scroll_pixels(delta: &ScrollDelta) -> f32 {
    agent_timeline_wheel(delta).pixels
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentTimelineWheel {
    pub pixels: f32,
    pub smooth: bool,
}

/// Convert agent timeline wheel input while preserving the device class.
/// Mouse wheels arrive as line deltas and should be animated; precision
/// trackpads arrive as pixel deltas and already feel smooth when applied 1:1.
pub fn agent_timeline_wheel(delta: &ScrollDelta) -> AgentTimelineWheel {
    match delta {
        ScrollDelta::Lines { x: _, y } => AgentTimelineWheel {
            pixels: y.clamp(-3.0, 3.0) * 24.0,
            smooth: true,
        },
        ScrollDelta::Pixels { x: _, y } => AgentTimelineWheel {
            pixels: *y,
            smooth: false,
        },
    }
}

/// Diagnostics popup wheel intent: vertical wheel notches scroll the
/// row list; horizontal wheel notches scroll the focused message
/// inline. Both axes are returned at once so the host can apply them
/// independently. Lines-delta horizontal: ~12 logical px per notch
/// reads at a comfortable pace without feeling sluggish. Pixels-delta
/// vertical: 22px maps to a single list row at the popup's default
/// row height; rounding keeps a partial notch from "almost"
/// scrolling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagnosticsPopupWheel {
    pub vertical_rows: i32,
    pub horizontal_px: f32,
}

impl DiagnosticsPopupWheel {
    pub fn from_delta(delta: &ScrollDelta) -> Self {
        match delta {
            ScrollDelta::Lines { x, y } => Self {
                vertical_rows: -(*y as i32),
                horizontal_px: -*x * 12.0,
            },
            ScrollDelta::Pixels { x, y } => Self {
                vertical_rows: (-*y / 22.0).round() as i32,
                horizontal_px: -*x,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionAutoscrollRegion {
    pub text_area_top_px: f64,
    pub text_area_bottom_px: f64,
    pub cell_height_px: f64,
}

impl SelectionAutoscrollRegion {
    pub fn new(text_area_top_px: f64, visible_lines: usize, cell_height_px: f64) -> Self {
        let cell_height_px = cell_height_px.max(1.0);
        Self {
            text_area_top_px,
            text_area_bottom_px: text_area_top_px + visible_lines as f64 * cell_height_px,
            cell_height_px,
        }
    }

    /// Pixel delta used while dragging a terminal selection past the
    /// visible text area. Positive means scroll up into history;
    /// negative means scroll down toward the live prompt.
    pub fn drag_delta_pixels(self, mouse_y_px: f64) -> f64 {
        selection_drag_scroll_pixels(self, mouse_y_px)
    }
}

pub fn selection_drag_scroll_pixels(
    region: SelectionAutoscrollRegion,
    mouse_y_px: f64,
) -> f64 {
    let cell_height = region.cell_height_px.max(1.0);
    let edge_zone = (cell_height * 2.5).max(32.0);

    if mouse_y_px < region.text_area_top_px + edge_zone {
        let distance = (region.text_area_top_px + edge_zone - mouse_y_px).max(0.0);
        let t = (distance / edge_zone).clamp(0.0, 1.0);
        cell_height * (0.35 + t * 1.35)
    } else if mouse_y_px > region.text_area_bottom_px - edge_zone {
        let distance = (mouse_y_px - (region.text_area_bottom_px - edge_zone)).max(0.0);
        let t = (distance / edge_zone).clamp(0.0, 1.0);
        -cell_height * (0.35 + t * 1.35)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Diagnostics popup wheel claim
// ---------------------------------------------------------------------------

/// Inputs to decide whether a wheel event over the diagnostics popup
/// is consumed and what side effects to apply. The host owns the popup
/// visibility / hit-test / row-at-pointer math; the shared decision
/// just turns those flags + the (vertical_rows, horizontal_px) wheel
/// into a set of actions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagnosticsPopupWheelContext {
    pub popup_visible: bool,
    pub pointer_over_popup: bool,
    pub row_under_pointer: Option<usize>,
    pub vertical_rows: i32,
    pub horizontal_px: f32,
}

/// Decision struct produced by [`DiagnosticsPopupWheelContext::decide`].
/// The host must apply both `scroll_message` and `scroll_by` if set, then
/// inspect `claimed` to know whether to forward the wheel to anything
/// underneath. Empirical rule: while the pointer is over the popup, we
/// always claim — even when both deltas are zero — to prevent phantom
/// scroll from leaking into the buffer behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagnosticsPopupWheelDecision {
    pub claimed: bool,
    pub scroll_message: Option<DiagnosticsMessageScroll>,
    pub scroll_rows: Option<i32>,
    pub mark_dirty: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagnosticsMessageScroll {
    pub row_index: usize,
    pub horizontal_px: f32,
}

impl DiagnosticsPopupWheelContext {
    pub fn decide(self) -> DiagnosticsPopupWheelDecision {
        if !self.popup_visible || !self.pointer_over_popup {
            return DiagnosticsPopupWheelDecision {
                claimed: false,
                scroll_message: None,
                scroll_rows: None,
                mark_dirty: false,
            };
        }

        let scroll_message = if self.horizontal_px.abs() > 0.5 {
            self.row_under_pointer
                .map(|row_index| DiagnosticsMessageScroll {
                    row_index,
                    horizontal_px: self.horizontal_px,
                })
        } else {
            None
        };
        let scroll_rows = (self.vertical_rows != 0).then_some(self.vertical_rows);
        let mark_dirty = scroll_message.is_some() || scroll_rows.is_some();

        DiagnosticsPopupWheelDecision {
            claimed: true,
            scroll_message,
            scroll_rows,
            mark_dirty,
        }
    }
}

// ---------------------------------------------------------------------------
// Block content top picker
// ---------------------------------------------------------------------------

/// Pure inputs for picking which absolute terminal row should be the
/// content top of a Warp-style block frame. The host flattens its
/// row/source/empty-row arrays before calling so the shared crate
/// never touches `Row<Square>` / `Crosswords`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockContentTopPick<'a> {
    /// Absolute row indices currently in the visible frame (post-drop).
    pub sources: &'a [usize],
    /// Parallel array: `row_is_empty[i]` is `true` when the visible
    /// row at index `i` is purely whitespace / default style.
    pub row_is_empty: &'a [bool],
    /// Visible content rows reserved for terminal output (chrome / footer excluded).
    pub terminal_content_rows: usize,
    pub display_offset: usize,
    pub history_size: usize,
}

impl<'a> BlockContentTopPick<'a> {
    pub fn content_top_abs(self) -> Option<usize> {
        let overflow = self
            .sources
            .len()
            .saturating_sub(self.terminal_content_rows);
        if overflow == 0 {
            return self.sources.first().copied();
        }
        let trailing_empty = self
            .row_is_empty
            .iter()
            .rev()
            .take_while(|empty| **empty)
            .count();
        if self.display_offset >= self.history_size || trailing_empty >= overflow {
            self.sources.first().copied()
        } else {
            self.sources.get(overflow).copied()
        }
    }
}

/// Pure variant of `drop_composer_owned_prompt_row` — drops the prompt
/// row from the parallel arrays when it corresponds to
/// `prompt_abs_row`. The host calls this against its own
/// `Vec<usize>` / `Vec<bool>` companion to its `Vec<Row<Square>>`
/// before invoking `BlockContentTopPick`.
pub fn drop_composer_prompt_row<R>(
    rows: &mut Vec<R>,
    sources: &mut Vec<usize>,
    _row_is_empty: impl Fn(&R) -> bool,
    prompt_abs_row: Option<usize>,
) {
    let Some(prompt_abs_row) = prompt_abs_row else {
        return;
    };
    let Some(index) = sources.iter().position(|&source| source == prompt_abs_row) else {
        return;
    };
    rows.remove(index);
    sources.remove(index);
}

// ---------------------------------------------------------------------------
// Terminal wheel: MOUSE_MODE / ALTERNATE_SCROLL emission
// ---------------------------------------------------------------------------

/// SGR mouse wheel codes the terminal expects in MOUSE_MODE.
pub const MOUSE_WHEEL_UP: u8 = 64;
pub const MOUSE_WHEEL_DOWN: u8 = 65;
pub const MOUSE_WHEEL_LEFT: u8 = 66;
pub const MOUSE_WHEEL_RIGHT: u8 = 67;

/// Pure decision for the MOUSE_MODE wheel branch: tell the host how
/// many vertical and horizontal mouse-report events to emit (and which
/// SGR codes) given the just-updated accumulated scroll state. Caller
/// is responsible for adding raw deltas into the accumulator before
/// calling, and for applying `% width` / `% height` afterwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalMouseModeWheelReport {
    pub accumulated_x: f64,
    pub accumulated_y: f64,
    pub delta_x: f64,
    pub delta_y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalMouseModeWheelEmit {
    pub vertical_code: u8,
    pub vertical_count: usize,
    pub horizontal_code: u8,
    pub horizontal_count: usize,
}

impl TerminalMouseModeWheelReport {
    pub fn emit(self) -> TerminalMouseModeWheelEmit {
        let vertical_code = if self.delta_y > 0.0 {
            MOUSE_WHEEL_UP
        } else {
            MOUSE_WHEEL_DOWN
        };
        let horizontal_code = if self.delta_x > 0.0 {
            MOUSE_WHEEL_LEFT
        } else {
            MOUSE_WHEEL_RIGHT
        };
        let vertical_count = if self.height > 0.0 {
            (self.accumulated_y / self.height).abs() as usize
        } else {
            0
        };
        let horizontal_count = if self.width > 0.0 {
            (self.accumulated_x / self.width).abs() as usize
        } else {
            0
        };
        TerminalMouseModeWheelEmit {
            vertical_code,
            vertical_count,
            horizontal_code,
            horizontal_count,
        }
    }
}

/// Pure decision for the ALT_SCREEN | ALTERNATE_SCROLL wheel branch:
/// build the CSI `ESC O <A|B|C|D>` byte stream and report how many
/// rows/cols it consumed from the accumulator. Caller still owns the
/// accumulator update (which uses `multiplier / divider`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalAlternateScrollCsi {
    pub accumulated_x: f64,
    pub accumulated_y: f64,
    pub delta_x: f64,
    pub delta_y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalAlternateScrollBytes {
    pub bytes: Vec<u8>,
    pub line_count: usize,
    pub column_count: usize,
}

impl TerminalAlternateScrollCsi {
    pub fn build(self) -> TerminalAlternateScrollBytes {
        let line_cmd = if self.delta_y > 0.0 { b'A' } else { b'B' };
        let column_cmd = if self.delta_x > 0.0 { b'D' } else { b'C' };
        let line_count = if self.height > 0.0 {
            (self.accumulated_y / self.height).abs() as usize
        } else {
            0
        };
        let column_count = if self.width > 0.0 {
            (self.accumulated_x / self.width).abs() as usize
        } else {
            0
        };
        let mut bytes = Vec::with_capacity(3 * (line_count + column_count));
        for _ in 0..line_count {
            bytes.push(0x1b);
            bytes.push(b'O');
            bytes.push(line_cmd);
        }
        for _ in 0..column_count {
            bytes.push(0x1b);
            bytes.push(b'O');
            bytes.push(column_cmd);
        }
        TerminalAlternateScrollBytes {
            bytes,
            line_count,
            column_count,
        }
    }
}
