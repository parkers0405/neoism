// Yank-flash overlay — paints a quick fading highlight over the rows
// nvim's `TextYankPost` reported. Designed to read like a soft "blink"
// confirming the yank without obscuring the cell content underneath.
//
// State machine: each emission seeds a `Flash`; `tick()` evicts entries
// whose elapsed > FLASH_MS. Multiple flashes can stack — useful for
// rapid `yy yy yy` so each yank gets its own fade rather than the new
// one truncating the previous.

use web_time::Instant;

use crate::primitives::paint::Paint;

// TODO(wave6-cutover): switch from `crate::primitives::IdeTheme` to the
// neoism-ui `ChromeTheme` once that grows the syntax-color budget the
// yank flash needs. For now the lifted panel keeps the native theme
// shape — the parent will wire `IdeTheme` in via `crate::primitives`.
use crate::primitives::IdeTheme;

/// Total fade lifetime (logical ms). Long enough to be perceptible
/// without lingering during fast editing.
const FLASH_MS: f32 = 360.0;
/// Peak alpha at the start of the flash. Dims linearly (with cubic
/// ease-out shaping) to 0 over `FLASH_MS`.
const PEAK_ALPHA: f32 = 0.35;

const DEPTH: f32 = 0.04;
// Above the editor cells (rendered at lower order) but below status
// chrome and modals.
const ORDER: u8 = 22;

fn elapsed_ms(started: Instant) -> f32 {
    Instant::now()
        .saturating_duration_since(started)
        .as_secs_f32()
        * 1000.0
}

#[derive(Clone, Copy, Debug)]
struct Flash {
    started_at: Instant,
    /// 0-based screen rows the flash covers, both ends inclusive.
    /// These are screen rows (relative to win-top), not buffer lines —
    /// a scroll after the flash spawns doesn't try to track the
    /// content. Short fade keeps that lag invisible in practice.
    row_top: u32,
    row_bot: u32,
    /// Optional inclusive column span in grid cells. When absent we
    /// fall back to the old full-row flash for compatibility with
    /// older producers.
    col_left: Option<u32>,
    col_right: Option<u32>,
}

pub struct YankFlash {
    flashes: Vec<Flash>,
}

impl YankFlash {
    pub fn new() -> Self {
        Self {
            flashes: Vec::new(),
        }
    }

    /// Spawn a flash for the row range `[row_top, row_bot]` (inclusive).
    /// Caller is responsible for clamping to the visible window — the
    /// renderer just paints what it's told.
    pub fn push(&mut self, row_top: u32, row_bot: u32) {
        self.push_span(row_top, row_bot, None, None);
    }

    /// Spawn a flash for a row range with an optional inclusive column
    /// range. Column spans keep single-line and visual-block yanks from
    /// painting the whole editor width.
    pub fn push_span(
        &mut self,
        row_top: u32,
        row_bot: u32,
        col_left: Option<u32>,
        col_right: Option<u32>,
    ) {
        let (a, b) = if row_top <= row_bot {
            (row_top, row_bot)
        } else {
            (row_bot, row_top)
        };
        let (col_left, col_right) = match (col_left, col_right) {
            (Some(left), Some(right)) if left <= right => (Some(left), Some(right)),
            (Some(left), Some(right)) => (Some(right), Some(left)),
            _ => (None, None),
        };
        self.flashes.push(Flash {
            started_at: Instant::now(),
            row_top: a,
            row_bot: b,
            col_left,
            col_right,
        });
    }

    /// Drop expired flashes. Idempotent — safe to call every frame.
    pub fn tick(&mut self) {
        self.flashes.retain(|f| elapsed_ms(f.started_at) < FLASH_MS);
    }

    /// True while any flash is still in its fade window. Wired into
    /// the renderer's `needs_redraw()` so the loop keeps ticking until
    /// the alpha lands at zero — without this the fade would freeze
    /// after the single redraw triggered by the rpc notify.
    pub fn is_animating(&self) -> bool {
        self.flashes
            .iter()
            .any(|f| elapsed_ms(f.started_at) < FLASH_MS)
    }

    /// Paint every active flash for the editor pane described by the
    /// caller. Coordinates are physical pixels — the caller is
    /// expected to multiply through scale_factor before passing them
    /// in (matches how `trail_cursor::draw` is invoked, so the two
    /// land in the same coordinate space).
    pub fn render(
        &mut self,
        sugarloaf: &mut dyn Paint,
        pane_x: f32,
        pane_y: f32,
        pane_w: f32,
        cell_w: f32,
        cell_h: f32,
        scale_factor: f32,
        theme: &IdeTheme,
    ) {
        self.tick();
        if self.flashes.is_empty() || pane_w <= 0.0 || cell_w <= 0.0 || cell_h <= 0.0 {
            return;
        }
        // Use the theme's yellow as the flash tint — matches the
        // unsaved-tab pip and the warn pill so the chrome reads as a
        // single visual language. PEAK_ALPHA keeps the underlying
        // cells legible.
        for flash in &self.flashes {
            let elapsed = elapsed_ms(flash.started_at);
            let t = (elapsed / FLASH_MS).clamp(0.0, 1.0);
            // Eased-out — bright at the start, then a soft tail. Using
            // 1 - t^3 gives a punchy onset without a flat plateau.
            let fade = 1.0 - t * t * t;
            let alpha = PEAK_ALPHA * fade;
            if alpha <= 0.001 {
                continue;
            }

            // Caller passes physical-pixel pane geometry; sugarloaf's
            // rect() multiplies through scale_factor again, so divide
            // first to land in logical px (matches how trail_cursor
            // does it via `scale_factor` argument on draw_always).
            let inv = if scale_factor > 0.0 {
                1.0 / scale_factor
            } else {
                1.0
            };
            let row_count = flash.row_bot - flash.row_top + 1;
            for row_ix in 0..row_count {
                let row = flash.row_top + row_ix;
                let (x, w) = match (flash.col_left, flash.col_right) {
                    (Some(left), Some(right)) => {
                        let right_px =
                            (right.saturating_add(1) as f32 * cell_w).min(pane_w);
                        let (left_px, width) = if row_count == 1 {
                            let left_px = left as f32 * cell_w;
                            if left_px >= pane_w || right_px <= left_px {
                                continue;
                            }
                            (left_px, right_px - left_px)
                        } else if row_ix == 0 {
                            let left_px = left as f32 * cell_w;
                            if left_px >= pane_w {
                                continue;
                            }
                            (left_px, pane_w - left_px)
                        } else if row_ix == row_count - 1 {
                            if right_px <= 0.0 {
                                continue;
                            }
                            (0.0, right_px)
                        } else {
                            (0.0, pane_w)
                        };
                        ((pane_x + left_px) * inv, width * inv)
                    }
                    _ => (pane_x * inv, pane_w * inv),
                };
                let y = (pane_y + row as f32 * cell_h) * inv;
                let h = cell_h * inv;

                sugarloaf.rect(
                    None,
                    x,
                    y,
                    w,
                    h,
                    theme.f32_alpha(theme.yellow, alpha),
                    DEPTH,
                    ORDER,
                );
            }
        }
    }
}

impl Default for YankFlash {
    fn default() -> Self {
        Self::new()
    }
}
