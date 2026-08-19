//! Slim `Panel::draw`-shaped adapters for the second batch of lifted
//! panels (breadcrumbs, notifications, completion menu, search overlay,
//! yank flash, cursor surfaces). Sibling to `chrome_shim.rs`, kept in
//! its own file so two parallel agents can edit without stomping. The
//! shims supply conservative host-side defaults (empty snapshots) so
//! Chrome can call them per-frame without ever crashing; live cell
//! metrics flow in from `Chrome::draw` where the panel math needs them
//! (completion anchor, yank flash cells).
//!
//! Each shim has the same shape as the working ones in `chrome_shim.rs`:
//! a thin `draw(&[mut] self, sugarloaf, &PanelLayout, &PanelContext)`
//! that converts the slim args into the wider native call. Where a
//! panel renders nothing without snapshot data (completion menu without
//! a `PopupMenu`, minimap without a route subscription), the shim still
//! calls through — the native panels early-return on missing data, so
//! the wiring is harmless and lights up the moment the host pushes real
//! state.

use sugarloaf::Sugarloaf;

use crate::chrome::active_ide_theme;
use crate::editor_snapshot::PopupMenu;
use crate::layout::PanelLayout;
use crate::panels::breadcrumbs::Breadcrumbs;
use crate::panels::completion_menu::{CompletionMenu, EditorAnchor};
use crate::panels::editor_scroll::EditorScroll;
use crate::panels::notifications::Notifications;
use crate::panels::search::SearchOverlay;
use crate::panels::trail_cursor::TrailCursor;
use crate::panels::yank_flash::YankFlash;

impl Breadcrumbs {
    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        layout: &PanelLayout,
        _ctx: &crate::panels::PanelContext,
    ) {
        let theme = active_ide_theme();
        let bounds = layout.bounds;
        self.render(sugarloaf, bounds.x, bounds.y, bounds.w, &theme);
    }
}

impl Notifications {
    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        layout: &PanelLayout,
        _ctx: &crate::panels::PanelContext,
    ) {
        let theme = active_ide_theme();
        let bounds = layout.bounds;
        let scale = layout.scale.max(0.5);
        // Native `render` expects (window_w_phys, window_h_phys,
        // scale_factor) and anchors toasts at `window_w - toast_w -
        // margin`. Model the window edge as the BOUNDS' RIGHT EDGE in
        // window coordinates (x + w) — using just `bounds.w` dropped
        // the pane's left offset, parking web toasts a file-tree-width
        // short of the window's right side.
        let dimensions = ((bounds.x + bounds.w) * scale, bounds.h * scale, scale);
        self.render(sugarloaf, dimensions, bounds.y, &theme);
    }
}

impl CompletionMenu {
    /// `cell_w` / `cell_h` are the chrome's live cell metrics in the
    /// same logical units as `layout.bounds` (Chrome passes
    /// `Chrome::cell_metrics()`); the shim scales them to physical px
    /// alongside the panel origin, matching the native anchor build in
    /// `desktop/src/host/run.rs` (`geometry.row_h * scale_factor`).
    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        layout: &PanelLayout,
        _ctx: &crate::panels::PanelContext,
        cell_w: f32,
        cell_h: f32,
    ) {
        let theme = active_ide_theme();
        let bounds = layout.bounds;
        let scale = layout.scale.max(0.5);
        let cell_w = cell_w.max(1.0);
        let cell_h = cell_h.max(1.0);
        // Prefer the host-pushed snapshot (via `set_popup` / `set_anchor`).
        // Fall back to a default empty PopupMenu so the native render's
        // `layout()` returns None and nothing paints — keeping the
        // pre-state-push behaviour intact when no host data is available.
        let fallback_menu = PopupMenu::default();
        let fallback_anchor = EditorAnchor {
            cell_w: cell_w * scale,
            cell_h: cell_h * scale,
            panel_left_phys: bounds.x * scale,
            panel_top_phys: bounds.y * scale,
            panel_lines: (bounds.h / cell_h).floor().max(1.0) as u32,
            editor_focused: true,
        };
        let menu_owned: Option<PopupMenu> = self.stored_popup().cloned();
        let anchor_owned: Option<EditorAnchor> = self.stored_anchor().copied();
        // `menu_owned` / `anchor_owned` are independent of `self` so we
        // can pass references into the `&mut self` render call below
        // without retaining a self-borrow.
        let menu_ref = menu_owned.as_ref().unwrap_or(&fallback_menu);
        let anchor_ref = anchor_owned.as_ref().unwrap_or(&fallback_anchor);
        let dimensions = (bounds.w * scale, bounds.h * scale, scale);
        self.render(
            sugarloaf,
            Some(menu_ref),
            anchor_ref,
            dimensions,
            false,
            &theme,
        );
    }
}

impl SearchOverlay {
    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        layout: &PanelLayout,
        _ctx: &crate::panels::PanelContext,
    ) {
        let bounds = layout.bounds;
        let scale = layout.scale.max(0.5);
        let dimensions = (bounds.w * scale, bounds.h * scale, scale);
        self.render(sugarloaf, dimensions);
    }
}

// NOTE: `Minimap` has no draw shim — Chrome::draw renders it directly
// via `begin_frame()` + `render_pane(route_id, ...)` per visible pane
// (see `chrome/draw.rs`), mirroring the native per-route loop in
// `desktop/src/screen/render/cell_emit.rs`.

impl YankFlash {
    /// `cell_w` / `cell_h` are the chrome's live cell metrics in the
    /// same logical units as `layout.bounds`; scaled to physical px
    /// here to match the native call in
    /// `desktop/src/screen/render/overlays.rs` (which passes the
    /// grid's physical cell dimensions).
    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        layout: &PanelLayout,
        _ctx: &crate::panels::PanelContext,
        cell_w: f32,
        cell_h: f32,
    ) {
        let theme = active_ide_theme();
        let bounds = layout.bounds;
        let scale = layout.scale.max(0.5);
        // Paints over the pane in physical coords. `render` tick()s
        // every frame and early-returns when there are no active
        // flashes, so calling unconditionally is fine.
        self.render(
            sugarloaf,
            bounds.x * scale,
            bounds.y * scale,
            bounds.w * scale,
            cell_w.max(1.0) * scale,
            cell_h.max(1.0) * scale,
            scale,
            &theme,
        );
    }
}

impl TrailCursor {
    /// Slim `Panel`-shaped wrapper. The native panel already exposes a
    /// `pub fn draw(&self, sugarloaf, scale_factor, cursor_color)` with
    /// a different signature, so adding a same-named `draw` here would
    /// collide. We expose `draw_slim` instead and Chrome calls that.
    pub fn draw_slim(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        layout: &PanelLayout,
        _ctx: &crate::panels::PanelContext,
    ) {
        let theme = active_ide_theme();
        let scale = layout.scale.max(0.5);
        // Use the theme's fg as the cursor color until the host wires
        // a per-cell cursor color through the buffer snapshot. The
        // trail only paints while animating; idle frames are cheap.
        let color = theme.f32(theme.fg);
        self.draw(sugarloaf, scale, color);
    }
}

impl EditorScroll {
    /// Pure animation state — there's no native `render` to call. The
    /// editor body itself reads `current_scroll_offset` and applies the
    /// offset to its cell positions; this shim exists so Chrome can
    /// uniformly invoke `.draw()` on every panel without a special
    /// case. No-op by design.
    pub fn draw(
        &self,
        _sugarloaf: &mut Sugarloaf,
        _layout: &PanelLayout,
        _ctx: &crate::panels::PanelContext,
    ) {
        // TODO(wave6-cutover): once the editor body lives in this crate
        // it'll read scroll offsets here per-pane.
    }
}
