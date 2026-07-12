// Animated cursorline overlay — Rust-side replacement for nvim's
// built-in `cursorline` painting. The original cursorline was a row
// background nvim re-emitted as `grid_line` cells whenever the cursor
// moved, so a held-arrow scroll painted the highlight on a fresh row
// every frame in 1-cell jumps while the content underneath slid
// smoothly with the spring — the visual disconnect read as "spazzing".
//
// This module owns a per-pane spring keyed by `rich_text_id` that
// tracks the cursor's destination y. Each frame the renderer pushes a
// target y (already in physical pixels, already including the editor
// scroll spring's `pixel_offset_y`), the spring lerps `current` toward
// `target`, and `render()` paints a full-pane-width rectangle.
//
// Mirrors the layout/state pattern of `yank_flash` so the renderer
// wiring is symmetric.

use std::collections::HashMap;

use web_time::Instant;

use sugarloaf::Sugarloaf;

use crate::animation::CriticallyDampedSpring;
use crate::primitives::IdeTheme;

// Render depth + order — sits just above the editor cells but below
// chrome panels and modals. Matches yank_flash's layering choice so
// both overlays coexist cleanly.
const DEPTH: f32 = 0.04;
const ORDER: u8 = 21;

// Translucent foreground tint — matches what plain nvim's
// `cursorline` reads as (a soft semi-transparent lift, NOT a flat
// band). The previous flat `#1f1f1f` looked opaque/black because it
// was the same family as the editor bg; a low-alpha fg tint gives the
// "see-through" feel the user asked for, and stays visible on light
// themes (a white tint would vanish on a light bg).
const TINT_ALPHA: f32 = 0.06;

// Hover animation length — quick enough that the highlight tracks
// the cursor (no laggy trail on j/k) but long enough that the
// transition reads as a glide rather than a hard snap. ~70ms is
// roughly the perceptual threshold for "this moved smoothly" vs
// "this teleported".
const HOVER_ANIMATION_LENGTH: f32 = 0.07;

struct PaneState {
    /// Most recent target y from the renderer (physical pixels).
    target_y: f32,
    /// Current animated y (physical pixels) — only diverges from
    /// `target_y` while the hover spring is mid-glide.
    current_y: f32,
    /// Critically-damped spring tracking `(current_y - target_y)`.
    /// Decays to zero over `HOVER_ANIMATION_LENGTH`.
    spring: CriticallyDampedSpring,
    last_tick_at: Instant,
    first: bool,
}

impl PaneState {
    fn new() -> Self {
        Self {
            target_y: 0.0,
            current_y: 0.0,
            spring: CriticallyDampedSpring::new(),
            last_tick_at: Instant::now(),
            first: true,
        }
    }
}

#[derive(Default)]
pub struct CursorlineOverlay {
    panes: HashMap<usize, PaneState>,
}

impl CursorlineOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the destination y for the given pane (physical pixels,
    /// top of the rectangle). Called every frame by the renderer with
    /// the cursor's spring-adjusted output position.
    ///
    /// `snap`: when `true`, force the highlight to land at `target_y`
    /// with no glide. The renderer passes `true` whenever the editor
    /// scroll spring is animating (`pixel_offset_y != 0` or
    /// `source_line_offset != 0`), so during smooth scroll the
    /// highlight tracks the cell grid 1-for-1 instead of running its
    /// own second spring that lagged the grid by a few pixels and
    /// painted a band drifting behind the text. The hover-glide
    /// spring only fires for cursor *row* changes that aren't
    /// accompanied by a scroll — the original purpose.
    pub fn set_target(&mut self, rich_text_id: usize, target_y: f32, snap: bool) {
        let pane = self
            .panes
            .entry(rich_text_id)
            .or_insert_with(PaneState::new);
        if pane.first {
            pane.current_y = target_y;
            pane.target_y = target_y;
            pane.spring.reset();
            pane.first = false;
            pane.last_tick_at = Instant::now();
            return;
        }
        if snap {
            // During scroll-spring animation: pin the highlight to
            // the cell row exactly. No spring residue, no drift.
            pane.current_y = target_y;
            pane.target_y = target_y;
            pane.spring.reset();
            pane.last_tick_at = Instant::now();
            return;
        }
        if (target_y - pane.target_y).abs() < f32::EPSILON {
            return;
        }
        // Fold the existing (current - old_target) delta into the
        // spring so the glide picks up from where the previous one
        // was, not from a fresh full-distance kick.
        let pending_delta = pane.current_y - target_y;
        pane.spring.position = pending_delta;
        pane.target_y = target_y;
    }

    /// Per-frame step. Returns `true` if any pane is still gliding.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut still = false;
        for pane in self.panes.values_mut() {
            let dt = now
                .saturating_duration_since(pane.last_tick_at)
                .as_secs_f32()
                .min(0.05);
            pane.last_tick_at = now;
            if pane.spring.position.abs() < 0.01 {
                pane.spring.position = 0.0;
                pane.current_y = pane.target_y;
                continue;
            }
            let moving = pane.spring.update(dt, HOVER_ANIMATION_LENGTH);
            pane.current_y = pane.target_y + pane.spring.position;
            if moving {
                still = true;
            }
        }
        still
    }

    pub fn is_animating(&self) -> bool {
        self.panes.values().any(|p| p.spring.position.abs() >= 0.01)
    }

    /// Forget a pane — call when the editor pane is closed/destroyed
    /// so the stored target doesn't outlive the pane id.
    pub fn forget(&mut self, rich_text_id: usize) {
        self.panes.remove(&rich_text_id);
    }

    /// Paint the cursorline rectangle for the given pane. Coordinates
    /// arrive in PHYSICAL pixels; `sugarloaf.rect` expects logical, so
    /// the caller's `scale_factor` undoes the multiply (matches the
    /// `yank_flash::render` and `trail_cursor::draw` conventions).
    pub fn render(
        &self,
        sugarloaf: &mut Sugarloaf,
        rich_text_id: usize,
        pane_x: f32,
        pane_w: f32,
        cell_h: f32,
        scale_factor: f32,
        theme: &IdeTheme,
    ) {
        let Some(pane) = self.panes.get(&rich_text_id) else {
            return;
        };
        if pane_w <= 0.0 || cell_h <= 0.0 {
            return;
        }
        let inv = if scale_factor > 0.0 {
            1.0 / scale_factor
        } else {
            1.0
        };
        let x = pane_x * inv;
        let y = pane.current_y * inv;
        let w = pane_w * inv;
        let h = cell_h * inv;

        // See-through fg at low alpha — a soft lift over the bg rather
        // than a flat band, on both dark and light themes.
        let color = theme.f32_alpha(theme.fg, TINT_ALPHA);
        sugarloaf.rect(None, x, y, w, h, color, DEPTH, ORDER);
    }

    pub fn render_all(
        &self,
        sugarloaf: &mut Sugarloaf,
        pane_x: f32,
        pane_w: f32,
        cell_h: f32,
        scale_factor: f32,
        theme: &IdeTheme,
    ) {
        for rich_text_id in self.panes.keys().copied() {
            self.render(
                sugarloaf,
                rich_text_id,
                pane_x,
                pane_w,
                cell_h,
                scale_factor,
                theme,
            );
        }
    }
}
