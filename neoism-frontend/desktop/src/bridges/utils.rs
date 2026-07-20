//! Bridge helpers between the backend's strongly-typed config and the
//! POD shims exposed by `neoism-ui::utils`.
//!
//! The shared chrome (`neoism-ui`) deliberately avoids depending on the
//! backend crate, so it operates on `NavigationShim`, `ContextDimensionShim`,
//! `ThemeHint`, and `WinsizeShim`. The native frontend (this crate) still
//! owns the real `Navigation`, `ContextDimension`, `Theme`, `Config`, and
//! `WinsizeBuilder` types, and we translate at the call site.

use neoism_backend::config::navigation::{Navigation, NavigationMode};
use neoism_backend::config::Config;
use neoism_ui::utils::{
    update_colors_based_on_theme as shared_update_colors_based_on_theme,
    ContextDimensionShim, ContextMarginShim, NavigationMode as NavigationModeShim,
    NavigationShim, ThemeHint, WinsizeShim,
};
use neoism_window::window::Theme;

use crate::layout::dimensions::ContextDimension;

#[derive(Copy, Clone, Debug)]
pub struct ResizeDimensions {
    pub columns: usize,
    pub lines: usize,
}

impl neoism_terminal_core::crosswords::grid::Dimensions for ResizeDimensions {
    #[inline]
    fn total_lines(&self) -> usize {
        self.lines
    }

    #[inline]
    fn screen_lines(&self) -> usize {
        self.lines
    }

    #[inline]
    fn columns(&self) -> usize {
        self.columns
    }
}

#[inline]
pub fn resize_dimensions(columns: u16, rows: u16) -> ResizeDimensions {
    ResizeDimensions {
        columns: columns.max(1) as usize,
        lines: rows.max(1) as usize,
    }
}

/// Convert a backend `Navigation` into the shared POD shim.
#[inline]
pub fn nav_shim(nav: &Navigation) -> NavigationShim {
    let mode = match nav.mode {
        NavigationMode::Plain | NavigationMode::Tab => NavigationModeShim::Tab,
        #[cfg(target_os = "macos")]
        NavigationMode::NativeTab => NavigationModeShim::NativeTab,
    };
    NavigationShim {
        enabled: nav.is_enabled(),
        hide_if_single: nav.hide_if_single,
        mode,
    }
}

/// Convert a `ContextDimension` into the shared POD shim.
#[inline]
pub fn dim_shim(dim: &ContextDimension) -> ContextDimensionShim {
    ContextDimensionShim {
        width: dim.width,
        height: dim.height,
        columns: dim.columns,
        lines: dim.lines,
        margin: ContextMarginShim {
            left: dim.margin.left,
            right: dim.margin.right,
            top: dim.margin.top,
            bottom: dim.margin.bottom,
        },
    }
}

/// Convert the shared `WinsizeShim` into the native `WinsizeBuilder`.
#[inline]
pub fn winsize_from_shim(shim: WinsizeShim) -> teletypewriter::WinsizeBuilder {
    teletypewriter::WinsizeBuilder {
        rows: shim.rows,
        cols: shim.cols,
        width: shim.width,
        height: shim.height,
    }
}

/// Compute terminal PTY dimensions in `WinsizeBuilder` form via the shared
/// helper. This wraps the shim round-trip at the call site.
#[inline]
pub fn terminal_dimensions(layout: &ContextDimension) -> teletypewriter::WinsizeBuilder {
    winsize_from_shim(neoism_ui::utils::terminal_dimensions(&dim_shim(layout)))
}

/// Rows an nvim editor pane renders for a PTY/grid of `rows` fully-visible
/// rows.
///
/// This used to add one extra "overshoot" row so nvim's grid met the
/// status line. That row was necessarily partial. Editors now keep the
/// conservative complete-row count; the pane-specific renderer distributes
/// Translate a `neoism_window::window::Theme` into the shared `ThemeHint`.
#[inline]
pub fn theme_hint_from_window(theme: Theme) -> ThemeHint {
    match theme {
        Theme::Light => ThemeHint::Light,
        Theme::Dark => ThemeHint::Dark,
    }
}

/// Mirror of the native fork's old `update_colors_based_on_theme` that
/// mutated `Config` directly: invoke the shared (no-op) hook to keep
/// behaviour symmetric with the web host, then swap the live `Config`'s
/// `colors` from the `adaptive_colors` palette based on the platform hint.
///
/// Returns the resolved `ThemeHint` so callers can chain further work.
#[inline]
pub fn apply_theme_to_config(
    config: &mut Config,
    theme_opt: Option<Theme>,
) -> Option<ThemeHint> {
    let hint = theme_opt.map(theme_hint_from_window);
    let resolved = shared_update_colors_based_on_theme(hint);

    if let Some(hint) = resolved {
        if let Some(adaptive_colors) = &config.adaptive_colors {
            match hint {
                ThemeHint::Light => {
                    if let Some(light_colors) = adaptive_colors.light {
                        config.colors = light_colors;
                    }
                }
                ThemeHint::Dark => {
                    if let Some(dark_colors) = adaptive_colors.dark {
                        config.colors = dark_colors;
                    }
                }
            }
        }
    }

    resolved
}
