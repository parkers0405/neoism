//! Inline diagnostics painted by Rust chrome on top of editor rows.
//!
//! This intentionally does not use Neovim virtual text/lines or
//! underlines. It mirrors Zed's inline diagnostic lens: first-line
//! message, one item per row, placed after the rendered line end plus
//! padding.

use std::cell::RefCell;
use std::collections::HashMap;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::ide_theme::IdeTheme;

const PAD_X: f32 = 7.0;
const PADDING_CELLS: u32 = 4;
const MIN_LENS_COLUMN: u32 = 0;
const MIN_LENS_WIDTH_PX: f32 = 44.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineDiagnosticSeverity {
    Error,
    Warn,
}

impl InlineDiagnosticSeverity {
    pub fn from_nvim(severity: u8) -> Option<Self> {
        match severity {
            1 => Some(Self::Error),
            2 => Some(Self::Warn),
            _ => None,
        }
    }

    fn color(self, theme: &IdeTheme) -> u32 {
        match self {
            Self::Error => theme.red,
            Self::Warn => theme.yellow,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warn => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineDiagnosticItem {
    /// Output row in the editor grid after applying the same source-line
    /// offset used by the smooth-scroll grid renderer.
    pub row: i32,
    pub severity: InlineDiagnosticSeverity,
    pub message: String,
    /// Absolute diagnostic location. `line` is one-based; `column` is
    /// zero-based, matching the editor diagnostic snapshot.
    pub line: u64,
    pub column: u32,
    pub end_line: u64,
    pub end_column: u32,
    pub source: Option<String>,
    pub code: Option<String>,
    pub code_description: Option<String>,
    pub tags: Vec<String>,
    pub related_information: Vec<InlineDiagnosticRelatedInformation>,
    /// Occupied text width for this visible editor row, in terminal
    /// columns. The host computes this from the rendered row so the
    /// diagnostic can sit after code when there is room.
    pub text_end_col: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineDiagnosticRelatedInformation {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineDiagnosticIdentity {
    pub severity: InlineDiagnosticSeverity,
    pub message: String,
    pub line: u64,
    pub column: u32,
    pub end_line: u64,
    pub end_column: u32,
    pub source: Option<String>,
    pub code: Option<String>,
}

impl From<&InlineDiagnosticItem> for InlineDiagnosticIdentity {
    fn from(item: &InlineDiagnosticItem) -> Self {
        Self {
            severity: item.severity,
            message: item.message.clone(),
            line: item.line,
            column: item.column,
            end_line: item.end_line,
            end_column: item.end_column,
            source: item.source.clone(),
            code: item.code.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct DiagnosticHit {
    rect: [f32; 4],
    item: InlineDiagnosticItem,
}

#[derive(Default)]
struct InlineDiagnosticsInteraction {
    hits: Vec<DiagnosticHit>,
    hovered: Option<InlineDiagnosticIdentity>,
    pinned: Option<InlineDiagnosticIdentity>,
    detail_geometry: super::diagnostic_detail::DiagnosticDetailGeometry,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineDiagnosticHoverOutcome {
    pub changed: bool,
    /// The pointer is over either a lens or its detail card. The host uses
    /// this to suppress the unrelated symbol-hover request underneath it.
    pub owns_pointer: bool,
    pub over_quick_fix: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineDiagnosticClickAction {
    None,
    /// A lens or the detail card consumed the click.
    Consumed,
    /// A click outside dismissed the pinned card but should continue to the
    /// editor surface underneath.
    Dismissed,
    QuickFix {
        line: u64,
        column: u32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct InlineDiagnosticsLayout {
    /// Physical-pixel left edge of the visible editor grid.
    pub pane_left_px: f32,
    /// Physical-pixel top edge of the first visible editor row.
    pub visible_top_px: f32,
    pub pane_width_px: f32,
    pub pane_height_px: f32,
    pub cell_width_px: f32,
    pub cell_height_px: f32,
    pub columns: u32,
    pub visible_rows: u32,
    /// Same pixel residual used by the grid smooth-scroll uniform.
    pub editor_pixel_offset_y: f32,
    pub scale_factor: f32,
    pub chrome_scale: f32,
}

/// Buffer/viewport state needed to place one absolute diagnostic into an
/// editor's retained smooth-scroll grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineDiagnosticViewport {
    /// Neovim-style 0-based absolute top buffer line.
    pub topline: u64,
    /// Integer source stride used by the resident grid this frame.
    pub source_line_offset: i32,
    /// Pixel residual applied to that grid after row sampling.
    pub pixel_offset_y: f32,
    pub cell_height_px: f32,
    pub visible_rows: u32,
    /// Retained physical rows outside the ordinary viewport band.
    pub buffer_above: u32,
    pub buffer_below: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InlineDiagnosticPlacement {
    /// Buffer row relative to the current (already-mutated) viewport.
    pub source_row: i32,
    /// Resident output row after inverting the grid's source sampler.
    pub output_row: i32,
}

#[derive(Default)]
pub struct InlineDiagnostics {
    interaction: RefCell<InlineDiagnosticsInteraction>,
}

impl InlineDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new paint frame. Hit regions are rebuilt from the exact lens
    /// geometry drawn this frame, so they move with smooth scrolling instead
    /// of retaining stale screen coordinates.
    pub fn begin_frame(&self, active: &[InlineDiagnosticIdentity]) {
        let mut interaction = self.interaction.borrow_mut();
        interaction.hits.clear();
        interaction.detail_geometry = Default::default();
        if interaction
            .hovered
            .as_ref()
            .is_some_and(|identity| !active.contains(identity))
        {
            interaction.hovered = None;
        }
        if interaction
            .pinned
            .as_ref()
            .is_some_and(|identity| !active.contains(identity))
        {
            interaction.pinned = None;
        }
    }

    pub fn render(
        &self,
        sugarloaf: &mut Sugarloaf,
        items: &[InlineDiagnosticItem],
        layout: InlineDiagnosticsLayout,
        paint_lenses: bool,
        theme: &IdeTheme,
    ) {
        if items.is_empty()
            || layout.cell_width_px <= 0.0
            || layout.cell_height_px <= 0.0
            || layout.scale_factor <= 0.0
            || layout.visible_rows == 0
            || layout.columns == 0
        {
            return;
        }

        let mut by_row: HashMap<i32, usize> = HashMap::new();
        for (idx, item) in items.iter().enumerate() {
            if !is_visible(item, layout) || item.message.trim().is_empty() {
                continue;
            }
            by_row
                .entry(item.row)
                .and_modify(|current| {
                    let current_item = &items[*current];
                    if item.severity.rank() < current_item.severity.rank() {
                        *current = idx;
                    }
                })
                .or_insert(idx);
        }
        if by_row.is_empty() {
            return;
        }

        let mut sorted: Vec<usize> = by_row.values().copied().collect();
        sorted.sort_by_key(|idx| {
            let item = &items[*idx];
            (item.row, item.severity.rank())
        });

        for idx in sorted {
            let item = &items[idx];
            if let Some(rect) =
                self.draw_item(sugarloaf, item, layout, paint_lenses, theme)
            {
                self.interaction.borrow_mut().hits.push(DiagnosticHit {
                    rect,
                    item: item.clone(),
                });
            }
        }
    }

    fn draw_item(
        &self,
        sugarloaf: &mut Sugarloaf,
        item: &InlineDiagnosticItem,
        layout: InlineDiagnosticsLayout,
        paint_lens: bool,
        theme: &IdeTheme,
    ) -> Option<[f32; 4]> {
        let inv = 1.0 / layout.scale_factor;
        let s = layout.chrome_scale.clamp(0.5, 3.0);
        let cell_w = layout.cell_width_px * inv;
        let cell_h = layout.cell_height_px * inv;
        let pane_left = layout.pane_left_px * inv;
        let pane_top = layout.visible_top_px * inv;
        let pane_w = layout.pane_width_px * inv;
        let pane_h = layout.pane_height_px * inv;
        let scroll_y = layout.editor_pixel_offset_y * inv;
        let y = pane_top + item.row as f32 * cell_h + scroll_y;
        if y + cell_h < pane_top || y > pane_top + pane_h {
            return None;
        }

        let color = item.severity.color(theme);
        let clip = Some([pane_left, pane_top, pane_w, pane_h]);
        let font_size = (cell_h - 4.0 * s).clamp(9.5 * s, 12.5 * s);
        let text_opts = DrawOpts {
            font_size,
            color: theme.u8(color),
            bold: true,
            clip_rect: clip,
            ..DrawOpts::default()
        };

        let message = item
            .message
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if message.is_empty() {
            return None;
        }

        let right_pad = 8.0 * s;
        let pane_right = pane_left + pane_w - right_pad;
        let min_lens_width = MIN_LENS_WIDTH_PX * s;
        let x = lens_x(pane_left, cell_w, item.text_end_col, layout.columns);
        let available = (pane_right - x).max(0.0);
        if available < min_lens_width {
            return None;
        }

        let text_x = x + PAD_X * s;
        let message_max = (pane_right - PAD_X * s - text_x).max(0.0);
        let message = truncate_to_fit(sugarloaf, &message, message_max, &text_opts);
        if message.is_empty() {
            return None;
        }
        let text_y = y + (cell_h - font_size) * 0.5 - 1.0 * s;
        // Severity-colored text ONLY — no chip background and no accent bar.
        // Reads like a Zed/VS Code inline hint (red error text floating after
        // the code) instead of a filled pill.
        let drawn_width = if paint_lens {
            sugarloaf
                .overlay_text_mut()
                .draw(text_x, text_y, &message, &text_opts)
        } else {
            // Sugarloaf composites overlay text in a distinct pass above
            // sprite rectangles, so a popup's nominal depth/order cannot
            // occlude a lens reliably. Keep this frame's exact anchor/hit
            // geometry but omit its glyphs while a detail surface is open.
            sugarloaf.overlay_text_mut().measure(&message, &text_opts)
        };
        Some([text_x, y, drawn_width.max(1.0), cell_h])
    }

    /// Update hover state from logical-pixel pointer coordinates.
    pub fn hover(&self, x: f32, y: f32) -> InlineDiagnosticHoverOutcome {
        let mut interaction = self.interaction.borrow_mut();
        let over_quick_fix = interaction
            .detail_geometry
            .quick_fix_rect
            .is_some_and(|rect| super::diagnostic_detail::rect_contains(rect, x, y));
        let over_detail = interaction.detail_geometry.panel_rect[2] > 0.0
            && super::diagnostic_detail::rect_contains(
                interaction.detail_geometry.panel_rect,
                x,
                y,
            );
        let hit_key = interaction
            .hits
            .iter()
            .rev()
            .find(|hit| super::diagnostic_detail::rect_contains(hit.rect, x, y))
            .map(|hit| InlineDiagnosticIdentity::from(&hit.item));
        let over_lens = hit_key.is_some();

        let previous = interaction.hovered.clone();
        if interaction.pinned.is_none() {
            if let Some(hit_key) = hit_key {
                interaction.hovered = Some(hit_key);
            } else if !over_detail {
                interaction.hovered = None;
            }
        }

        InlineDiagnosticHoverOutcome {
            changed: previous != interaction.hovered,
            owns_pointer: over_lens || over_detail,
            over_quick_fix,
        }
    }

    /// Toggle pinning, dismiss on outside click, or activate the pinned
    /// Quick Fix button. Coordinates are logical pixels.
    pub fn click(&self, x: f32, y: f32) -> InlineDiagnosticClickAction {
        let mut interaction = self.interaction.borrow_mut();
        if interaction
            .detail_geometry
            .quick_fix_rect
            .is_some_and(|rect| super::diagnostic_detail::rect_contains(rect, x, y))
        {
            if let Some(key) = interaction.pinned.as_ref() {
                return InlineDiagnosticClickAction::QuickFix {
                    line: key.line,
                    column: key.column,
                };
            }
        }
        if interaction.detail_geometry.panel_rect[2] > 0.0
            && super::diagnostic_detail::rect_contains(
                interaction.detail_geometry.panel_rect,
                x,
                y,
            )
        {
            return InlineDiagnosticClickAction::Consumed;
        }

        let hit_key = interaction
            .hits
            .iter()
            .rev()
            .find(|hit| super::diagnostic_detail::rect_contains(hit.rect, x, y))
            .map(|hit| InlineDiagnosticIdentity::from(&hit.item));
        if let Some(hit_key) = hit_key {
            if interaction.pinned.as_ref() == Some(&hit_key) {
                interaction.pinned = None;
                interaction.hovered = None;
            } else {
                interaction.pinned = Some(hit_key);
                interaction.hovered = None;
            }
            return InlineDiagnosticClickAction::Consumed;
        }

        if interaction.pinned.take().is_some() {
            interaction.hovered = None;
            return InlineDiagnosticClickAction::Dismissed;
        }
        InlineDiagnosticClickAction::None
    }

    pub fn dismiss_detail(&self) -> bool {
        let mut interaction = self.interaction.borrow_mut();
        let changed =
            interaction.pinned.take().is_some() || interaction.hovered.take().is_some();
        if changed {
            interaction.detail_geometry = Default::default();
        }
        changed
    }

    pub fn clear_hover(&self) -> bool {
        let mut interaction = self.interaction.borrow_mut();
        if interaction.hovered.take().is_some() {
            if interaction.pinned.is_none() {
                interaction.detail_geometry = Default::default();
            }
            true
        } else {
            false
        }
    }

    pub fn has_active_detail(&self) -> bool {
        let interaction = self.interaction.borrow();
        let Some(key) = interaction.pinned.as_ref().or(interaction.hovered.as_ref())
        else {
            return false;
        };
        interaction
            .hits
            .iter()
            .any(|hit| InlineDiagnosticIdentity::from(&hit.item) == *key)
    }

    /// Whether hover/pin state has selected a diagnostic, independent of
    /// whether its current-frame lens anchor has been rebuilt yet.
    pub fn has_selected_detail(&self) -> bool {
        let interaction = self.interaction.borrow();
        interaction.pinned.is_some() || interaction.hovered.is_some()
    }

    /// Paint the currently hovered or pinned diagnostic after the terminal
    /// grids, so the complete card sits above editor content. The anchor is
    /// resolved from this frame's hit list; if its line scrolls away the card
    /// does not float at an obsolete position.
    pub fn render_detail(
        &self,
        sugarloaf: &mut Sugarloaf,
        window_w: f32,
        window_h: f32,
        scale: f32,
        theme: &IdeTheme,
    ) {
        let detail = {
            let interaction = self.interaction.borrow();
            let (key, pinned) = if let Some(key) = interaction.pinned.as_ref() {
                (key, true)
            } else if let Some(key) = interaction.hovered.as_ref() {
                (key, false)
            } else {
                return;
            };
            interaction
                .hits
                .iter()
                .find(|hit| InlineDiagnosticIdentity::from(&hit.item) == *key)
                .map(|hit| (hit.clone(), pinned))
        };
        let Some((hit, pinned)) = detail else {
            self.interaction.borrow_mut().detail_geometry = Default::default();
            return;
        };

        let geometry = super::diagnostic_detail::render(
            sugarloaf,
            &super::diagnostic_detail::DiagnosticDetailContent {
                severity: hit.item.severity,
                message: hit.item.message,
                source: hit.item.source,
                line: hit.item.line,
                column: hit.item.column,
                end_line: hit.item.end_line,
                end_column: hit.item.end_column,
                code: hit.item.code,
                code_description: hit.item.code_description,
                tags: hit.item.tags,
                related_information: hit
                    .item
                    .related_information
                    .into_iter()
                    .map(|related| {
                        super::diagnostic_detail::DiagnosticDetailRelatedInformation {
                            path: related.path,
                            line: related.line,
                            column: related.column,
                            end_line: related.end_line,
                            end_column: related.end_column,
                            message: related.message,
                        }
                    })
                    .collect(),
                pinned,
            },
            super::diagnostic_detail::DiagnosticDetailLayout {
                anchor_x: hit.rect[0],
                anchor_y: hit.rect[1],
                cell_h: hit.rect[3],
                window_w,
                window_h,
                scale,
            },
            theme,
        );
        self.interaction.borrow_mut().detail_geometry = geometry;
    }
}

/// Popup backgrounds are sprites while lens glyphs live in Sugarloaf's text
/// overlay pass. Do not submit lens glyphs beneath either detail surface;
/// nominal sprite order alone cannot make that composition opaque.
pub fn inline_lenses_should_paint(
    diagnostic_detail_selected: bool,
    lsp_hover_visible: bool,
) -> bool {
    !diagnostic_detail_selected && !lsp_hover_visible
}

/// Project a 1-based absolute buffer diagnostic into the exact resident row
/// sampled by the smooth-scroll grid for this frame.
///
/// The grid paints source `output + source_line_offset`, then applies the
/// shared pixel residual. Diagnostics must invert the integer relation and
/// carry the same residual or their text visibly slides independently of the
/// code beneath it.
pub fn inline_diagnostic_placement(
    line_one_based: u64,
    viewport: InlineDiagnosticViewport,
) -> Option<InlineDiagnosticPlacement> {
    if line_one_based == 0 {
        return None;
    }

    let source_row = i128::from(line_one_based)
        .checked_sub(1)?
        .checked_sub(i128::from(viewport.topline))?;
    let source_row = i32::try_from(source_row).ok()?;
    let output_row = crate::render_policy::editor_output_row_for_source(
        source_row,
        viewport.source_line_offset,
    );

    // Do not create an overlay for a physical row the host's resident grid
    // cannot paint. This matters during elastic overscroll, whose offset can
    // exceed the single fractional edge row retained by the desktop host.
    let visible_rows = i32::try_from(viewport.visible_rows).ok()?;
    let buffer_above = i32::try_from(viewport.buffer_above).ok()?;
    let buffer_below = i32::try_from(viewport.buffer_below).ok()?;
    let first_resident_row = buffer_above.checked_neg()?;
    let resident_end = visible_rows.checked_add(buffer_below)?;
    if output_row < first_resident_row || output_row >= resident_end {
        return None;
    }

    inline_diagnostic_row_is_visible(
        output_row,
        viewport.visible_rows,
        viewport.cell_height_px,
        viewport.pixel_offset_y,
    )
    .then_some(InlineDiagnosticPlacement {
        source_row,
        output_row,
    })
}

fn is_visible(item: &InlineDiagnosticItem, layout: InlineDiagnosticsLayout) -> bool {
    inline_diagnostic_row_is_visible(
        item.row,
        layout.visible_rows,
        layout.cell_height_px,
        layout.editor_pixel_offset_y,
    )
}

/// Whether an output row intersects the visible editor clip after the
/// smooth-scroll pixel residual is applied.
///
/// A fractional scroll exposes one retained row outside the ordinary
/// `0..visible_rows` band: row `-1` while content moves down and row
/// `visible_rows` while it moves up. Filtering only by the integer row
/// discarded that edge diagnostic even though the matching grid row was
/// visibly sliding through the clip.
pub fn inline_diagnostic_row_is_visible(
    row: i32,
    visible_rows: u32,
    cell_height_px: f32,
    editor_pixel_offset_y: f32,
) -> bool {
    if visible_rows == 0
        || !cell_height_px.is_finite()
        || cell_height_px <= 0.0
        || !editor_pixel_offset_y.is_finite()
    {
        return false;
    }

    let row_top = row as f32 * cell_height_px + editor_pixel_offset_y;
    let row_bottom = row_top + cell_height_px;
    let viewport_bottom = visible_rows as f32 * cell_height_px;
    row_bottom > 0.0 && row_top < viewport_bottom
}

fn lens_x(pane_left: f32, cell_w: f32, text_end_col: u32, columns: u32) -> f32 {
    let start_col = text_end_col
        .saturating_add(PADDING_CELLS)
        .max(MIN_LENS_COLUMN)
        .min(columns);
    pane_left + start_col as f32 * cell_w
}

fn truncate_to_fit(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    max_width: f32,
    opts: &DrawOpts,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }
    if sugarloaf.overlay_text_mut().measure(text, opts) <= max_width {
        return text.to_string();
    }
    let suffix = "...";
    if sugarloaf.overlay_text_mut().measure(suffix, opts) > max_width {
        return String::new();
    }
    let char_count = text.chars().count();
    let mut low = 0usize;
    let mut high = char_count;
    while low < high {
        let mid = (low + high + 1) / 2;
        let mut candidate: String = text.chars().take(mid).collect();
        candidate.push_str(suffix);
        if sugarloaf.overlay_text_mut().measure(&candidate, opts) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut out: String = text.chars().take(low).collect();
    out.push_str(suffix);
    out
}

#[cfg(test)]
mod tests {
    use super::{
        inline_diagnostic_placement, inline_diagnostic_row_is_visible,
        inline_lenses_should_paint, DiagnosticHit, InlineDiagnosticClickAction,
        InlineDiagnosticItem, InlineDiagnosticPlacement, InlineDiagnosticSeverity,
        InlineDiagnosticViewport, InlineDiagnostics,
    };

    fn interactive_diagnostics() -> InlineDiagnostics {
        let diagnostics = InlineDiagnostics::new();
        diagnostics
            .interaction
            .borrow_mut()
            .hits
            .push(DiagnosticHit {
                rect: [100.0, 40.0, 180.0, 20.0],
                item: InlineDiagnosticItem {
                    row: 2,
                    severity: InlineDiagnosticSeverity::Error,
                    message: "complete diagnostic message".to_string(),
                    line: 51,
                    column: 7,
                    end_line: 50,
                    end_column: 12,
                    source: Some("test-lsp".to_string()),
                    code: Some("E0001".to_string()),
                    code_description: None,
                    tags: Vec::new(),
                    related_information: Vec::new(),
                    text_end_col: 10,
                },
            });
        diagnostics
    }

    fn viewport(
        topline: u64,
        source_line_offset: i32,
        pixel_offset_y: f32,
    ) -> InlineDiagnosticViewport {
        InlineDiagnosticViewport {
            topline,
            source_line_offset,
            pixel_offset_y,
            cell_height_px: 20.0,
            visible_rows: 10,
            buffer_above: 1,
            buffer_below: 1,
        }
    }

    #[test]
    fn placement_inverts_the_grid_source_row_during_fractional_scroll() {
        assert_eq!(
            inline_diagnostic_placement(101, viewport(100, -1, -8.0)),
            Some(InlineDiagnosticPlacement {
                source_row: 0,
                output_row: 1,
            }),
        );
        assert_eq!(
            inline_diagnostic_placement(101, viewport(100, 0, 0.0)),
            Some(InlineDiagnosticPlacement {
                source_row: 0,
                output_row: 0,
            }),
        );
    }

    #[test]
    fn placement_keeps_fractional_top_and_bottom_edge_rows() {
        assert_eq!(
            inline_diagnostic_placement(101, viewport(100, 1, 8.0))
                .unwrap()
                .output_row,
            -1,
        );
        assert_eq!(
            inline_diagnostic_placement(110, viewport(100, -1, -8.0))
                .unwrap()
                .output_row,
            10,
        );
    }

    #[test]
    fn placement_culls_offscreen_without_mutating_reappearance() {
        let before = inline_diagnostic_placement(123, viewport(120, 0, 0.0)).unwrap();
        assert!(inline_diagnostic_placement(123, viewport(150, 0, 0.0)).is_none());
        let after = inline_diagnostic_placement(123, viewport(120, 0, 0.0)).unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn fractional_scroll_keeps_the_entering_edge_row_visible() {
        let rows = 10;
        let cell_h = 20.0;

        // Content moving down exposes the retained row above row zero.
        assert!(inline_diagnostic_row_is_visible(-1, rows, cell_h, 6.0));
        assert!(!inline_diagnostic_row_is_visible(
            rows as i32,
            rows,
            cell_h,
            6.0,
        ));

        // Content moving up exposes the retained row below the last row.
        assert!(inline_diagnostic_row_is_visible(
            rows as i32,
            rows,
            cell_h,
            -6.0,
        ));
        assert!(!inline_diagnostic_row_is_visible(-1, rows, cell_h, -6.0));
    }

    #[test]
    fn settled_scroll_excludes_rows_touching_only_the_clip_boundary() {
        assert!(!inline_diagnostic_row_is_visible(-1, 10, 20.0, 0.0));
        assert!(inline_diagnostic_row_is_visible(0, 10, 20.0, 0.0));
        assert!(inline_diagnostic_row_is_visible(9, 10, 20.0, 0.0));
        assert!(!inline_diagnostic_row_is_visible(10, 10, 20.0, 0.0));
    }

    #[test]
    fn placement_rejects_invalid_or_unrepresentable_lines() {
        assert!(inline_diagnostic_placement(0, viewport(0, 0, 0.0)).is_none());
        assert!(inline_diagnostic_placement(u64::MAX, viewport(0, 0, 0.0)).is_none());
    }

    #[test]
    fn hover_then_click_pins_and_same_lens_click_dismisses() {
        let diagnostics = interactive_diagnostics();
        let hover = diagnostics.hover(150.0, 50.0);
        assert!(hover.changed);
        assert!(hover.owns_pointer);

        assert_eq!(
            diagnostics.click(150.0, 50.0),
            InlineDiagnosticClickAction::Consumed
        );
        assert!(diagnostics.interaction.borrow().pinned.is_some());
        assert_eq!(
            diagnostics.click(150.0, 50.0),
            InlineDiagnosticClickAction::Consumed
        );
        assert!(diagnostics.interaction.borrow().pinned.is_none());
    }

    #[test]
    fn pinned_quick_fix_reports_the_exact_diagnostic_position() {
        let diagnostics = interactive_diagnostics();
        diagnostics.click(150.0, 50.0);
        diagnostics.interaction.borrow_mut().detail_geometry =
            super::super::diagnostic_detail::DiagnosticDetailGeometry {
                panel_rect: [80.0, 70.0, 300.0, 160.0],
                quick_fix_rect: Some([100.0, 190.0, 260.0, 26.0]),
            };
        assert_eq!(
            diagnostics.click(120.0, 200.0),
            InlineDiagnosticClickAction::QuickFix {
                line: 51,
                column: 7,
            }
        );
    }

    #[test]
    fn outside_click_dismisses_without_consuming_the_editor_click() {
        let diagnostics = interactive_diagnostics();
        diagnostics.click(150.0, 50.0);
        assert_eq!(
            diagnostics.click(20.0, 20.0),
            InlineDiagnosticClickAction::Dismissed
        );
        assert!(diagnostics.interaction.borrow().pinned.is_none());
    }

    #[test]
    fn authoritative_removal_clears_a_pinned_detail_without_polling() {
        let diagnostics = interactive_diagnostics();
        diagnostics.click(150.0, 50.0);
        assert!(diagnostics.interaction.borrow().pinned.is_some());

        diagnostics.begin_frame(&[]);

        assert!(diagnostics.interaction.borrow().pinned.is_none());
        assert!(!diagnostics.has_active_detail());
    }

    #[test]
    fn detail_surfaces_occlude_inline_lens_glyphs() {
        assert!(inline_lenses_should_paint(false, false));
        assert!(!inline_lenses_should_paint(true, false));
        assert!(!inline_lenses_should_paint(false, true));
        assert!(!inline_lenses_should_paint(true, true));
    }
}
