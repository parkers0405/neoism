//! GPUiX's list motion for the command palette and finder (`command_palette/update.rs`,
//! `render.rs`). The list scrolls in whole rows: an integer top row moves to keep the selection
//! inside its scrolloff band, and the row delta becomes a residual offset that a critically
//! damped spring (`LIST_SCROLL_ANIMATION_LENGTH`, 0.30 s) settles back to zero, so the whole
//! list slides as one piece. The selection highlight rides its own, quicker spring
//! (`CURSOR_ANIMATION_LENGTH`, 0.12 s). A new query starts both at rest (`reset_motion`).
//!
//! In Rune the residual is the content's offset (`-top × row`) under `offset_motion`, so the
//! spring runs in the renderer with no view rebuilds; nothing here uses a native scroll view.

use super::follow::{CURSOR_SPRING, LIST_SPRING, Reveal, reveal_top};
use rune::{Color, El, col, el};

/// `fuzzy::scrolloff_for`: the band never exceeds half the visible rows.
pub fn scrolloff_for(rows: usize, visible: usize) -> usize { rows.min(visible.saturating_sub(1) / 2) }

/// The top row that keeps `selected` `scrolloff` rows inside a `visible`-row window.
pub fn follow_top(selected: usize, count: usize, visible: usize, top: usize, scrolloff: usize) -> usize {
    let visible = visible.max(1);
    let max_top = count.saturating_sub(visible);
    let scrolloff = scrolloff_for(scrolloff, visible);
    reveal_top(selected as i64, visible as i64, max_top as i64, top.min(max_top) as i64, Reveal::Margin(scrolloff as i64)).max(0) as usize
}

/// Wheel distance (`scroll_pixels`): it accumulates in `acc` and the window steps one row per
/// `row_h` of it, so notches and fine high-resolution wheels both move whole rows; the
/// selection is pulled back inside the band. Returns the new (top, selected).
#[allow(clippy::too_many_arguments)]
pub fn wheel(acc: &mut f32, dy: f32, row_h: f32, top: usize, selected: usize, count: usize, visible: usize, scrolloff: usize) -> (usize, usize) {
    let visible = visible.max(1);
    let max_top = count.saturating_sub(visible);
    *acc += dy;
    let steps = (*acc / row_h.max(1.0)).trunc();
    *acc -= steps * row_h.max(1.0);
    let top = (top as isize - steps as isize).clamp(0, max_top as isize) as usize;
    if steps == 0.0 { return (top, selected); }
    let band = scrolloff_for(scrolloff, visible);
    let low = if top == 0 { 0 } else { top + band };
    let high = if top == max_top { count.saturating_sub(1) } else { (top + visible - 1).saturating_sub(band) };
    (top, selected.clamp(low, high.max(low)))
}

/// The list viewport: `rows` (each `row_h` tall, in order) placed on content that slides to
/// `top`, with the highlight gliding to `selected`. `reset` names the query, so a new one
/// mounts fresh content at rest instead of sliding from the old position.
#[allow(clippy::too_many_arguments)]
pub fn view(key: &str, reset: &str, width: f32, row_h: f32, visible: usize, top: usize, selected: Option<usize>,
    highlight: Color, radius: f32, rows: Vec<El>) -> El {
    let count = rows.len();
    let mut content = col().key(format!("{key}:content:{reset}")).w(width).h(count as f32 * row_h)
        .absolute(0.0, 0.0).offset(0.0, -(top as f32) * row_h).offset_motion(LIST_SPRING);
    if let Some(selected) = selected.filter(|s| *s < count) {
        content = content.child(el().key("highlight").w(width).h(row_h).radius(radius).fill(highlight)
            .absolute(0.0, 0.0).offset(0.0, selected as f32 * row_h).offset_motion(CURSOR_SPRING));
    }
    for (index, row) in rows.into_iter().enumerate() {
        content = content.child(row.absolute(0.0, index as f32 * row_h));
    }
    el().key(key.to_owned()).w(width).h(visible.max(1) as f32 * row_h).clip_rounded(0.0).child(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_moves_only_at_the_band() {
        // 8 rows visible, band 2: rows 0..=5 keep the top at 0, row 6 moves it to 1.
        assert_eq!(follow_top(5, 40, 8, 0, 2), 0);
        assert_eq!(follow_top(6, 40, 8, 0, 2), 1);
        assert_eq!(follow_top(39, 40, 8, 1, 2), 32);
        assert_eq!(follow_top(0, 40, 8, 32, 2), 0);
    }

    #[test]
    fn a_wheel_notch_steps_a_row_and_pulls_the_selection_in() {
        // Down a notch from the top: the window steps to 1 and row 0 is pulled to 3 (1 + band).
        let mut acc = 0.0;
        assert_eq!(wheel(&mut acc, -40.0, 34.0, 0, 0, 40, 8, 2), (1, 3));
        assert_eq!(wheel(&mut acc, 40.0, 34.0, 0, 0, 40, 8, 2), (0, 0));
        // Fine wheel steps add up before the window moves.
        let mut fine = 0.0;
        assert_eq!(wheel(&mut fine, -10.0, 34.0, 0, 0, 40, 8, 2), (0, 0));
        assert_eq!(wheel(&mut fine, -30.0, 34.0, 0, 0, 40, 8, 2), (1, 3));
    }
}
