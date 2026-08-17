// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Per-cell fg / bg color resolution. Pure functions — no atlas state,
//! no row geometry. The selection / hint overrides happen at the
//! caller (`bg.rs`, `fg.rs`).

use neoism_terminal_core::colors::term::TermColors;
use neoism_terminal_core::crosswords::square::{ContentTag, Square};
use neoism_terminal_core::crosswords::style::{StyleFlags, StyleSet};

use crate::host::Renderer;

pub fn cell_fg(
    sq: Square,
    style_set: &StyleSet,
    renderer: &Renderer,
    term_colors: &TermColors,
) -> [u8; 4] {
    if sq.is_bg_only() {
        return normalized_to_u8(renderer.named_colors.foreground);
    }
    let style = style_set.get(sq.style_id());
    let color = if style.flags.contains(StyleFlags::HIDDEN) {
        renderer.compute_bg_color(&style, term_colors)
    } else if style.flags.contains(StyleFlags::INVERSE) {
        renderer.compute_bg_color(&style, term_colors)
    } else {
        renderer.compute_color(&style.fg, style.flags, term_colors)
    };
    normalized_to_u8(color)
}

/// Foreground for a selected cell. selection-fg
/// rule: use the configured `selection-foreground`
/// unless the user asked to keep the cell's own fg (Rio's
/// `ignore-selection-foreground-color`). falls back to
/// `state.colors.background` when no color is configured; Rio always
/// has a default selection_foreground populated in its theme, so we
/// use it directly.
#[inline]
pub fn cell_fg_selected(
    sq: Square,
    style_set: &StyleSet,
    renderer: &Renderer,
    term_colors: &TermColors,
) -> [u8; 4] {
    if renderer.ignore_selection_fg_color {
        cell_fg(sq, style_set, renderer, term_colors)
    } else {
        normalized_to_u8(renderer.named_colors.selection_foreground)
    }
}

pub fn cell_bg(
    sq: Square,
    style_set: &StyleSet,
    renderer: &Renderer,
    term_colors: &TermColors,
) -> [u8; 4] {
    let (color, is_default_background) = match sq.content_tag() {
        ContentTag::BgRgb => {
            let (r, g, b) = sq.bg_rgb();
            return [r, g, b, 255];
        }
        ContentTag::BgPalette => {
            let idx = sq.bg_palette_index() as usize;
            (renderer.color(idx, term_colors), false)
        }
        ContentTag::Codepoint => {
            let style = style_set.get(sq.style_id());
            if style.flags.contains(StyleFlags::INVERSE) {
                (
                    renderer.compute_color(&style.fg, style.flags, term_colors),
                    false,
                )
            } else {
                // The window clear pass already owns the terminal's default
                // background. Emitting that same color as an opaque cell
                // hides window transparency and background images. Only an
                // explicit ANSI background should produce a cell fill.
                let is_default = matches!(
                    style.bg,
                    neoism_terminal_core::colors::AnsiColor::Named(
                        neoism_terminal_core::colors::NamedColor::Background
                    )
                );
                (renderer.compute_bg_color(&style, term_colors), is_default)
            }
        }
    };
    resolved_cell_background(color, is_default_background)
}

#[inline]
fn resolved_cell_background(color: [f32; 4], is_default_background: bool) -> [u8; 4] {
    if is_default_background {
        [0, 0, 0, 0]
    } else {
        normalized_to_u8(color)
    }
}

#[inline]
pub(super) fn normalized_to_u8(c: [f32; 4]) -> [u8; 4] {
    neoism_ui::terminal_grid_emit::rgba_f32_to_u8(c)
}

#[cfg(test)]
mod tests {
    use super::resolved_cell_background;

    #[test]
    fn default_background_leaves_the_window_surface_visible() {
        assert_eq!(
            resolved_cell_background([0.10, 0.20, 0.30, 1.0], true),
            [0, 0, 0, 0]
        );
    }

    #[test]
    fn explicit_background_remains_an_opaque_cell_fill() {
        assert_eq!(
            resolved_cell_background([0.10, 0.20, 0.30, 1.0], false),
            [25, 51, 76, 255]
        );
    }
}
