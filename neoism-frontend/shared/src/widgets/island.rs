// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.
//
// island.rs was originally retired from boo editor
// which is licensed under MIT license.

use rustc_hash::FxHashMap;
use std::borrow::Cow;
use sugarloaf::{Attributes, Sugarloaf};
use web_time::Instant;

// TODO(wave-cutover): the desktop fork imports its rich
// `crate::context::ContextManager`, `neoism_backend::event::EventProxy`,
// and `neoism_backend::event::{ProgressReport, ProgressState}` to drive
// the island's tab strip + OSC 9;4 progress bar. For the shared crate
// we mirror the API surface with POD shims so the widget keeps its
// shape — hosts translate their native types into these before calling
// `render` / `set_progress_report`.

/// Read-only view of the workspace tab strip the island renders. Hosts
/// implement this against whatever owns their context list.
pub trait IslandContexts {
    fn len(&self) -> usize;
    fn current_index(&self) -> usize;
    /// Title for the tab at `index`. Falls back to empty when absent —
    /// the island then skips drawing the tab.
    fn title(&self, index: usize) -> Option<IslandTabTitle>;
}

/// POD title payload the host hands the island. Mirrors the shape the
/// native `ContextManager::titles` exposes (content string + optional
/// program fallback). Both are owned so the host can compute them once
/// per frame.
#[derive(Clone, Debug, Default)]
pub struct IslandTabTitle {
    pub content: String,
    pub program: Option<String>,
    pub icon_kind: Option<String>,
}

/// Mirrors `neoism_backend::event::ProgressState` (OSC 9;4 values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressState {
    Remove,
    Set,
    Error,
    Indeterminate,
    Pause,
}

/// Mirrors `neoism_backend::event::ProgressReport`.
#[derive(Clone, Copy, Debug)]
pub struct ProgressReport {
    pub state: ProgressState,
    pub progress: Option<u8>,
}

// TODO(wave-cutover): native uses `neoism_window::event::KeyEvent` +
// `neoism_window::keyboard::{Key, NamedKey}` for the in-island rename
// field. Shared mirrors only the variants the rename handler matches
// against; hosts translate their winit event into this enum before
// calling `handle_rename_input`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandRenameKey {
    Escape,
    Enter,
    Backspace,
    Character(char),
}

/// Convert `[f32; 4]` colour to `[u8; 4]` for the `Text` API.
#[inline]
fn color_u8(c: [f32; 4]) -> [u8; 4] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
        (c[3].clamp(0.0, 1.0) * 255.0) as u8,
    ]
}

/// Height of the tab bar in pixels
pub const ISLAND_HEIGHT: f32 = 28.0;

/// Height of the progress bar in pixels
const PROGRESS_BAR_HEIGHT: f32 = 3.0;

/// Timeout in seconds for auto-dismissing stale progress bars
const PROGRESS_BAR_TIMEOUT_SECS: u64 = 15;

const TITLE_FONT_SIZE: f32 = 11.5;
const ISLAND_ORDER_BG: u8 = 3;
const ISLAND_ORDER_ELEMENT: u8 = 4;

/// Hover-grow animation duration for a workspace tab. Mirrors
/// `BufferTabs`' `TAB_HOVER_ANIM_MS` so the top-level strip's hover
/// highlight eases in/out exactly like the buffer-tab strip below it.
const TAB_HOVER_ANIM_MS: u64 = 150;
/// Hover scale factor for a workspace tab — same subtle 3.5% grow the
/// buffer-tab strip uses (`buffer_tabs::consts::TAB_HOVER_SCALE`).
const TAB_HOVER_SCALE: f32 = 1.035;

/// Left/right padding inside each tab — kept as breathing room around the
/// title text so it never butts against the tab separator lines.
const TAB_PADDING_X: f32 = 24.0;

/// Suffix used when truncating a title that doesn't fit in its tab.
const TITLE_ELLIPSIS: char = '…';

/// Truncate `title` to fit within `max_width` pixels at the tab font,
/// appending `…` when characters have to be dropped. Thin adapter that
/// asks sugarloaf's cached glyph advance for each char. Returns
/// `Cow::Borrowed(title)` when the full string fits so the common
/// "no truncation needed" path avoids allocating.
fn fit_title_to_width<'a>(
    sugarloaf: &mut Sugarloaf,
    title: &'a str,
    max_width: f32,
    font_size: f32,
) -> Cow<'a, str> {
    let attrs = Attributes::default();
    fit_title_with_widths(title, max_width, |c| {
        sugarloaf.char_advance(c, attrs, font_size)
    })
}

/// Pure-logic truncation: walks `title` left to right, summing per-char
/// widths from the supplied closure, appending `…` the first moment the
/// running total would exceed `max_width`. Separated from sugarloaf so
/// tests can feed synthetic widths without a GPU context.
///
/// Returns `Cow::Borrowed(title)` when the full string fits, so the
/// hot "no truncation needed" path does zero allocation.
///
/// `max_width <= 0.0` falls through the loop naturally: the first
/// char's accumulated width already exceeds the budget, `truncate_ix`
/// stays 0, and we return just `"…"` — a consistent sentinel that
/// at least signals "there was content here". Empty input returns
/// `Cow::Borrowed("")`.
///
/// Approximate (isolated per-char advances — no kerning, no ligatures,
/// no emoji cluster formation). Fine for short labels where a pixel or
/// two of slack is invisible.
fn fit_title_with_widths<'a>(
    title: &'a str,
    max_width: f32,
    mut char_width: impl FnMut(char) -> f32,
) -> Cow<'a, str> {
    let suffix_width = char_width(TITLE_ELLIPSIS);

    // `truncate_ix` tracks the last byte offset at which the prefix so
    // far still has room for the suffix. Updated before adding the next
    // char's width so the moment we detect overflow we already know
    // where to cut.
    let mut accumulated: f32 = 0.0;
    let mut truncate_ix: usize = 0;
    for (ix, c) in title.char_indices() {
        if accumulated + suffix_width <= max_width {
            truncate_ix = ix;
        }
        accumulated += char_width(c);
        if accumulated > max_width {
            let mut out = String::with_capacity(truncate_ix + TITLE_ELLIPSIS.len_utf8());
            out.push_str(&title[..truncate_ix]);
            out.push(TITLE_ELLIPSIS);
            return Cow::Owned(out);
        }
    }
    Cow::Borrowed(title)
}

/// Color picker constants
const PICKER_SWATCH_SIZE: f32 = 18.0;
const PICKER_SWATCH_GAP: f32 = 4.0;
const PICKER_PADDING: f32 = 6.0;
const PICKER_INPUT_HEIGHT: f32 = 26.0;
const PICKER_INPUT_FONT_SIZE: f32 = 12.0;
const PICKER_INPUT_MARGIN_TOP: f32 = 8.0;
const PICKER_TOP_PADDING: f32 = 4.0;
const PICKER_HEIGHT: f32 = PICKER_TOP_PADDING
    + PICKER_SWATCH_SIZE
    + PICKER_PADDING * 2.0
    + PICKER_INPUT_MARGIN_TOP
    + PICKER_INPUT_HEIGHT
    + PICKER_PADDING;
const PICKER_COLORS: [[f32; 4]; 6] = [
    [0.86, 0.26, 0.27, 1.0], // red
    [0.90, 0.57, 0.22, 1.0], // orange
    [0.85, 0.78, 0.25, 1.0], // yellow
    [0.34, 0.70, 0.38, 1.0], // green
    [0.30, 0.55, 0.85, 1.0], // blue
    [0.68, 0.40, 0.80, 1.0], // purple
];

/// Right margin after last tab
const ISLAND_MARGIN_RIGHT: f32 = 8.0;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandChromeSpec {
    pub height: f32,
    pub title_font_size: f32,
    pub tab_padding_x: f32,
    pub tab_radius: f32,
    pub margin_right: f32,
    pub icon_glyph: &'static str,
    pub icon_color: [u8; 4],
}

/// Shared top-level workspace strip contract. Native renders this through
/// `Island::render`; web renders a DOM host from the same constants so the
/// browser chrome does not drift into its own tab system.
pub fn island_chrome_spec(scale: f32) -> IslandChromeSpec {
    let scale = scale.max(0.1);
    let (icon_glyph, icon_color) = crate::panels::file_tree::icons::workspace_tab_icon();
    IslandChromeSpec {
        height: ISLAND_HEIGHT * scale,
        title_font_size: TITLE_FONT_SIZE * scale,
        tab_padding_x: TAB_PADDING_X * scale,
        tab_radius: 6.0 * scale,
        margin_right: ISLAND_MARGIN_RIGHT * scale,
        icon_glyph,
        icon_color,
    }
}

/// Shared Island title normalization: desktop calls the private equivalent
/// from `Island::get_title_for_tab`; web uses this exported helper before
/// rendering workspace labels.
pub fn island_tab_label(content: &str, program: Option<&str>) -> String {
    if !content.is_empty() {
        let content = content.trim_end_matches('/');
        let label = content
            .rsplit('/')
            .next()
            .filter(|component| !component.is_empty())
            .unwrap_or(content);
        return label.to_string();
    }

    if let Some(program) = program {
        if !program.is_empty() {
            return program.to_string();
        }
    }

    String::from("~")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandHit {
    Tab { index: usize },
    Strip,
}

/// Pixels the cursor must travel after press before a click "lifts" into a
/// real drag. Stays a plain click below this threshold so single-clicks
/// keep activating tabs.
const DRAG_ACTIVATION_PX: f32 = 5.0;

/// Vertical distance (in logical px) the cursor must leave the island
/// strip before the gesture flips from reorder into a detach preview.
/// Picked to be unambiguous — a careless dip below the strip doesn't
/// arm detach.
const DETACH_THRESHOLD_PX: f32 = 60.0;

/// Outcome of an `Island` drag, returned by [`Island::end_drag`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandDragRelease {
    /// No drag was active, or the drag never crossed the activation
    /// threshold (so the press should be treated as a plain click).
    None,
    /// The tab was reordered in place — caller has already mutated the
    /// underlying context list during `update_drag` swaps, so this just
    /// signals "the gesture committed".
    Reorder,
    /// The cursor exited the strip past the vertical detach threshold
    /// at release time. `source_index` is the original index of the
    /// dragged workspace, so the caller can lift the corresponding
    /// `ContextGrid` out of `context_manager.contexts`.
    Detach { source_index: usize },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct IslandDragState {
    /// Index in `context_manager.contexts` that was originally pressed.
    /// `update_drag` keeps this aligned with the live source index after
    /// each swap — the picked tab is the one currently at this index.
    source_index: usize,
    /// Distance from the tab's left edge to mouse x at press time, so
    /// the floating tab paints from the same pixel offset under the
    /// cursor (no "jump to center" on lift).
    grab_offset_x: f32,
    /// Press-time mouse position in logical px. Used to decide when
    /// motion has exceeded `DRAG_ACTIVATION_PX`.
    start_x: f32,
    start_y: f32,
    /// Latest mouse position in logical px.
    current_x: f32,
    current_y: f32,
    /// True once we've crossed `DRAG_ACTIVATION_PX`. Below this the
    /// release path returns `None` so a plain click keeps working.
    live: bool,
    /// True once the cursor has left the strip by more than
    /// `DETACH_THRESHOLD_PX` vertically. Renders the ghost preview and
    /// makes release return `Detach`.
    detach_armed: bool,
}

pub struct Island {
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub hide_if_single: bool,
    pub inactive_text_color: [f32; 4],
    pub active_text_color: [f32; 4],
    pub border_color: [f32; 4],
    /// Current progress bar state
    progress_state: Option<ProgressState>,
    /// Current progress value (0-100)
    progress_value: Option<u8>,
    /// When the *current* state began. Reset only when transitioning into a
    /// new state, so the indeterminate animation phase is not yanked back to
    /// zero by repeated identical OSC 9;4 reports (issue #1509).
    progress_started_at: Option<Instant>,
    /// Last time we saw an OSC 9;4 report — bumped on every report, used by
    /// the stale-bar dismissal timer. Decoupled from `progress_started_at`
    /// for the same reason.
    progress_last_seen: Option<Instant>,
    /// Progress bar color
    pub progress_bar_color: [f32; 4],
    /// Progress bar error color
    pub progress_bar_error_color: [f32; 4],
    /// Which tab has the color picker open (None = closed)
    color_picker_tab: Option<usize>,
    /// Per-tab background colors
    tab_colors: FxHashMap<usize, [f32; 4]>,
    /// Per-tab custom titles (user overrides)
    tab_custom_titles: FxHashMap<usize, String>,
    /// Current rename input text while picker is open
    rename_input: String,
    /// Caret blink timer
    rename_caret_time: Instant,
    /// Active workspace-tab drag, if any. `None` outside a drag.
    drag: Option<IslandDragState>,
    // ── Keyboard focus + animated hover ─────────────────────────────
    //
    // Mirror of `BufferTabs`' `focused`/`focused_index`/hover fields so
    // the top-level workspace strip behaves like a buffer-tab strip:
    // Alt+Up parks a focus CURSOR here (separate from the ACTIVE tab),
    // Left/Right move that cursor, Enter commits. Hover is the same
    // ease-out scale animation the buffer-tab strip uses.
    /// Whether the strip currently holds keyboard focus.
    focused: bool,
    /// Keyboard focus cursor — the tab the focus highlight sits on while
    /// `focused`. Separate from `current_index()` (the ACTIVE workspace);
    /// moved by `move_focus_cursor` and committed on Enter by the host.
    focus_cursor: usize,
    /// Focus-cursor rect in logical px, recomputed each `render`. Fed by
    /// the host into the shared animated trail cursor (the same path the
    /// buffer-tab strips use via `focused_cursor_rect`).
    focused_cursor_rect: Option<[f32; 4]>,
    /// Currently hovered tab index, or `None`.
    hover: Option<usize>,
    /// When the most recent hover transition began (drives the grow/shrink
    /// ease). `None` once the animation settles.
    hover_anim_started: Option<Instant>,
    /// Tab the hover animation is shrinking away from.
    hover_from: Option<usize>,
    /// Tab the hover animation is growing toward.
    hover_to: Option<usize>,
    /// Vertical offset (logical px) at which the strip paints. Lets the
    /// host place the workspace-tab strip *below* the chrome top bar
    /// instead of at y=0. All render geometry + the stored focus-cursor
    /// rect are shifted by this; host hit-tests add the same offset.
    top_offset: f32,
    /// Horizontal offset (logical px) at which the strip starts. Lets the
    /// host inset the workspace tabs to the content column (right of the
    /// file tree / sidebars) so they don't span over the tree. Host
    /// hit-tests add the same offset to their x math.
    left_offset: f32,
    /// Chrome zoom factor (Ctrl +/-), propagated by the host like every
    /// other chrome panel. All heights + font sizes multiply by this so
    /// the workspace strip zooms with the rest of the app. (The device
    /// HiDPI scale is applied by sugarloaf on top of this.)
    scale: f32,
}

impl Island {
    pub fn new(
        inactive_text_color: [f32; 4],
        active_text_color: [f32; 4],
        border_color: [f32; 4],
        hide_if_single: bool,
    ) -> Self {
        Self {
            hide_if_single,
            inactive_text_color,
            active_text_color,
            border_color,
            progress_state: None,
            progress_value: None,
            progress_started_at: None,
            progress_last_seen: None,
            // Default progress bar color (blue-ish)
            progress_bar_color: [0.3, 0.6, 1.0, 1.0],
            // Default error color (red-ish)
            progress_bar_error_color: [1.0, 0.3, 0.3, 1.0],
            color_picker_tab: None,
            tab_colors: FxHashMap::default(),
            tab_custom_titles: FxHashMap::default(),
            rename_input: String::new(),
            rename_caret_time: Instant::now(),
            drag: None,
            focused: false,
            focus_cursor: 0,
            focused_cursor_rect: None,
            hover: None,
            hover_anim_started: None,
            hover_from: None,
            hover_to: None,
            top_offset: 0.0,
            left_offset: 0.0,
            scale: 1.0,
        }
    }

    /// Set the chrome zoom factor (Ctrl +/-). Mirrors the other chrome
    /// panels' `set_scale` so the workspace strip zooms with them.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    /// Set the y at which the strip paints (logical px). The host passes
    /// the chrome top-bar height so the workspace tabs sit beneath it.
    pub fn set_top_offset(&mut self, top_offset: f32) {
        self.top_offset = top_offset;
    }

    pub fn top_offset(&self) -> f32 {
        self.top_offset
    }

    /// Set the x at which the strip starts (logical px). The host passes
    /// the file-tree / sidebar width so the tabs inset to the content
    /// column instead of spanning over the tree.
    pub fn set_left_offset(&mut self, left_offset: f32) {
        self.left_offset = left_offset;
    }

    pub fn left_offset(&self) -> f32 {
        self.left_offset
    }

    // ---------------------------------------------------------------
    // Keyboard focus + cursor — mirrors `BufferTabs::set_focused` /
    // `focused_index` / `move_focused`. The focus cursor is a separate
    // pointer from the active workspace; the host commits it (switches
    // the active workspace) on Enter.
    // ---------------------------------------------------------------

    /// Whether the strip currently holds keyboard focus.
    #[inline]
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Park (or drop) keyboard focus on the strip. On focus the cursor
    /// seeds to `active_index` so the highlight starts on the active
    /// workspace tab, exactly like `BufferTabs::set_focused` seeds the
    /// focus cursor to the active tab. `num_tabs == 0` refuses focus.
    pub fn set_focused(&mut self, focused: bool, active_index: usize, num_tabs: usize) {
        if focused && num_tabs > 0 {
            self.focus_cursor = active_index.min(num_tabs - 1);
            self.focused = true;
        } else {
            self.focused = false;
            self.focused_cursor_rect = None;
        }
    }

    /// Current keyboard focus cursor (clamped to the live tab count).
    #[inline]
    pub fn focus_cursor(&self, num_tabs: usize) -> usize {
        self.focus_cursor.min(num_tabs.saturating_sub(1))
    }

    /// Re-seat the focus cursor on a specific tab without moving focus
    /// state. Used by the host after committing so the cursor stays put.
    pub fn set_focus_cursor(&mut self, index: usize, num_tabs: usize) {
        if num_tabs > 0 {
            self.focus_cursor = index.min(num_tabs - 1);
        }
    }
}

mod drag;
mod picker;
mod render;

#[cfg(test)]
mod tests;
