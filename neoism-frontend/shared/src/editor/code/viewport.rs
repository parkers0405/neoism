//! Shared code viewport update. Both Sugarloaf and GPUIX call this exact
//! wrap, center-lock, caret-follow, and spring implementation.
use super::layout::*;
use super::types::CodePane;
use web_time::Instant;

pub struct CodeViewportState {
    pub text_view_w: f32,
    pub wrap: std::sync::Arc<WrapIndex>,
    pub total_rows: usize,
    pub cursor_line: usize,
    pub cursor_local_col: usize,
    pub cursor_vrow: usize,
    pub content_h: f32,
    pub max_scroll: f32,
    pub scroll_x: f32,
    pub scroll_animating: bool,
}

pub fn update_viewport(
    pane: &mut CodePane,
    x: f32,
    w: f32,
    text_x: f32,
    cell_w: f32,
    row_h: f32,
    h_content: f32,
) -> CodeViewportState {
    let line_count = pane.buffer.line_count();
    const TEXT_PAD_X: f32 = 8.0;
    const SCROLLBAR_W: f32 = 6.0;
    // Wrap layout pass (DisplayMap-lite): text columns available to
    // the buffer between gutter and scrollbar decide how many VISUAL
    // rows each buffer line occupies. The prefix-sum index is cached on
    // (buffer revision, cols) — resize and edits rebuild it (O(lines)),
    // steady frames pay nothing.
    let text_right_pad = TEXT_PAD_X + SCROLLBAR_W + 2.0;
    let text_view_w = (x + w - text_x - text_right_pad).max(cell_w);
    let cols = if pane.wrap {
        ((text_view_w / cell_w).floor() as usize).max(1)
    } else {
        0 // NoWrap sentinel: identity index, horizontal caret-follow.
    };
    let plugin_revision = pane
        .plugin_decorations
        .as_ref()
        .map_or(0, |snapshot| snapshot.revision);
    let wrap_key = (pane.buffer.revision, cols, plugin_revision);
    if pane.wrap_index_key != Some(wrap_key) || !pane.wrap_index.is_valid_for(line_count)
    {
        let hidden = pane
            .plugin_decorations
            .as_ref()
            .filter(|snapshot| !snapshot.hidden_lines.contains(&pane.buffer.cursor_line))
            .map(|snapshot| &snapshot.hidden_lines)
            .cloned()
            .unwrap_or_default();
        let virtual_rows = pane
            .plugin_decorations
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .virtual_lines
                    .iter()
                    .map(|(line, rows)| (*line, rows.len()))
                    .collect()
            })
            .unwrap_or_default();
        pane.wrap_index = std::sync::Arc::new(WrapIndex::build_with_projection(
            &pane.buffer.lines,
            cols,
            TAB_DISPLAY_WIDTH,
            &hidden,
            &virtual_rows,
        ));
        pane.wrap_index_key = Some(wrap_key);
    }
    let wrap = pane.wrap_index.clone();
    let total_rows = wrap.total_rows(line_count);

    // Cursor in visual-row space: segment + column-within-segment come
    // from the same cut math the painter draws with.
    let cursor_line = pane.buffer.cursor_line.min(line_count.saturating_sub(1));
    let (cursor_seg, cursor_local_col) = wrap.visual_position(
        cursor_line,
        &pane.buffer.lines[cursor_line],
        pane.buffer.cursor_col,
        TAB_DISPLAY_WIDTH,
    );
    let cursor_vrow = wrap.first_row_of_line(cursor_line) + cursor_seg;

    // Rebase the viewport by the same row displacement as a structural edit
    // before clamping/caret-follow. This keeps the painted caret and nearby
    // text stationary through repeated newline deletion.
    pane.apply_pending_cursor_view_anchor(row_h);

    // Content extent + scroll clamp + caret reveal (nvim scrolloff).
    let content_h = total_rows as f32 * row_h;
    pane.content_height = content_h;
    pane.scroll_viewport_height = h_content;
    let max_scroll = (content_h - h_content).max(0.0);
    if pane.buffer.follow_cursor {
        pane.touch_viewport_detached = false;
        pane.buffer.follow_cursor = false;
        pane.last_keyboard_reveal = Some(Instant::now());
        // nvim-style center-lock in PURE INTEGER ROWS (visual rows —
        // the wrap index already mapped the cursor): the cursor row
        // pins mid-viewport (clamps release at the buffer edges).
        // Integer math matters — the float version rounded a stretched
        // row height per keystroke and could flip one row up/down while
        // typing on one line (visible as a bounce). The dead-band means
        // an unchanged desired row NEVER touches the target.
        let rows_fit_i = (h_content / row_h).round().max(1.0) as i64;
        let max_top = ((content_h - h_content) / row_h).round().max(0.0) as i64;
        let half = (rows_fit_i - 1) / 2;
        let desired_center = cursor_vrow as i64 - half;
        let desired_top = desired_center.clamp(0, max_top);
        let current_top = (pane.target_scroll_y / row_h).round() as i64;
        // Centering achievable → hard center-lock (the loved mid-file
        // feel). Clamped at a buffer edge → centering would snap the
        // view (e.g. the first `j` after a top-of-file Ctrl-D dragged
        // everything back to row 0); switch to MINIMAL scroll there:
        // move only to keep the cursor a few rows off the view edge.
        let clamped = desired_top != desired_center;
        let new_top = if !clamped {
            desired_top
        } else {
            const EDGE_SCROLLOFF: i64 = 4;
            let cursor = cursor_vrow as i64;
            let margin = EDGE_SCROLLOFF.min((rows_fit_i - 1) / 2);
            if cursor < current_top + margin {
                (cursor - margin).clamp(0, max_top)
            } else if cursor > current_top + rows_fit_i - 1 - margin {
                (cursor + margin - rows_fit_i + 1).clamp(0, max_top)
            } else {
                current_top
            }
        };
        if std::env::var_os("NEOISM_SCROLL_LOG").is_some() {
            eprintln!(
                "neoism::scroll reveal cursor_vrow={cursor_vrow} desired_center={desired_center} clamped={clamped} current_top={current_top} new_top={new_top} max_top={max_top}"
            );
        }
        if new_top != current_top {
            pane.target_scroll_y = new_top as f32 * row_h;
            pane.target_scroll_raw = pane.target_scroll_y;
        }
    }
    pane.target_scroll_y = pane.target_scroll_y.clamp(0.0, max_scroll);

    // Horizontal caret-follow (NoWrap only): plain minimal adjustment
    // keeping the caret inside the text view with a 2-cell margin.
    if pane.wrap {
        pane.scroll_x = 0.0;
    } else {
        let caret_px = cursor_local_col as f32 * cell_w;
        let margin = (cell_w * 2.0).min(text_view_w * 0.25);
        let low = (caret_px + cell_w + margin - text_view_w).max(0.0);
        let high = (caret_px - margin).max(low);
        pane.scroll_x = pane.scroll_x.clamp(low, high);
    }
    let scroll_x = pane.scroll_x;

    // Scroll animation: critically-damped SPRING (the old nvim-era
    // editor_scroll feel). Unlike exponential decay — which teleports
    // to max velocity on the first frame and reads as a jerk — a
    // spring accelerates from rest, sweeps, and settles briskly with
    // no overshoot and no tail crawl. Velocity carries across target
    // changes, so held arrows and Ctrl-D chains stay one continuous
    // motion.
    let now = Instant::now();
    let dt = pane
        .scroll_last_tick_at
        .map(|at| (now - at).as_secs_f32().clamp(0.0, 0.05))
        .unwrap_or(0.0);
    // Neovide's long-jump rule (rendered_window.rs keeps only 2×height
    // of scrollback): the animated travel is capped to ~a viewport —
    // the far portion of a `:1`-from-line-300 jump TELEPORTS (that
    // content was never on screen) and only the final stretch springs.
    // Uncapped, every frame of a huge sweep redraws the full viewport
    // and the settle phase visibly tanks the frame rate.
    let max_travel = h_content * 1.25;
    let far = pane.target_scroll_y - pane.scroll_y;
    if far.abs() > max_travel {
        pane.scroll_y = pane.target_scroll_y - max_travel * far.signum();
        pane.scroll_velocity_px_s = 0.0;
    }
    let delta = pane.target_scroll_y - pane.scroll_y;
    let scroll_animating = delta.abs() > 0.5 || pane.scroll_velocity_px_s.abs() > 1.0;
    if scroll_animating {
        // A settled spring has no running clock. Its first new target starts
        // at rest; time spent idle must not become a 50 ms initial jump. Keep
        // this clock (and velocity) across targets while motion is active.
        pane.scroll_last_tick_at = Some(now);
        // ω of the critically-damped spring; higher = snappier.
        const OMEGA: f32 = 16.0;
        // Explicit Euler is only stable for small dt. When the frame rate
        // drops — e.g. heavy per-keystroke diagnostic work while typing —
        // `dt` climbs to its 0.05 clamp and a single big step makes this
        // spring OVERSHOOT and ring frame-to-frame: the visible up/down
        // bounce. Integrate in fixed sub-steps so the spring behaves the
        // same at 20fps as at 144fps and never overshoots, recomputing
        // `delta` each sub-step so it tracks a moving target cleanly.
        const MAX_SUBSTEP: f32 = 1.0 / 240.0;
        let mut remaining = dt;
        while remaining > 0.0 {
            let step = remaining.min(MAX_SUBSTEP);
            let delta = pane.target_scroll_y - pane.scroll_y;
            let accel = OMEGA * OMEGA * delta - 2.0 * OMEGA * pane.scroll_velocity_px_s;
            pane.scroll_velocity_px_s += accel * step;
            pane.scroll_y += pane.scroll_velocity_px_s * step;
            remaining -= step;
        }
        if (pane.target_scroll_y - pane.scroll_y).abs() < 0.5
            && pane.scroll_velocity_px_s.abs() < 30.0
        {
            pane.scroll_y = pane.target_scroll_y;
            pane.scroll_velocity_px_s = 0.0;
            pane.scroll_last_tick_at = None;
        }
    } else {
        pane.scroll_y = pane.target_scroll_y;
        pane.scroll_velocity_px_s = 0.0;
        pane.scroll_last_tick_at = None;
    }
    pane.scroll_y = pane.scroll_y.clamp(0.0, max_scroll);

    CodeViewportState {
        text_view_w,
        wrap,
        total_rows,
        cursor_line,
        cursor_local_col,
        cursor_vrow,
        content_h,
        max_scroll,
        scroll_x,
        scroll_animating,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn pane() -> CodePane {
        CodePane::new(
            PathBuf::from("viewport.txt"),
            &(0..500)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    #[test]
    fn integer_center_lock_and_long_jump_cap_stay_stable() {
        let mut pane = pane();
        update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
        pane.buffer.set_cursor_position(300, 0, false);
        pane.buffer.follow_cursor = true;
        let view = update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
        assert_eq!(view.cursor_vrow, 300);
        assert_eq!(pane.target_scroll_y, 296.0 * 21.0);
        assert!(pane.target_scroll_y - pane.scroll_y <= 210.0 * 1.25);
        assert!(view.scroll_animating);
        let target = pane.target_scroll_y;
        pane.buffer.follow_cursor = true;
        update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
        assert_eq!(pane.target_scroll_y, target);
    }

    #[test]
    fn held_arrow_start_does_not_integrate_idle_time() {
        let first_positions: Vec<_> = [Duration::from_millis(7), Duration::from_secs(5)]
            .into_iter()
            .map(|idle| {
                let mut pane = pane();
                update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
                // Advance the age of any retained idle clock without sleeping.
                // The old implementation kept Some(now) here, so the first
                // cursor-follow frame incorrectly integrated this idle time.
                pane.scroll_last_tick_at = pane.scroll_last_tick_at
                    .map(|_| Instant::now() - idle);
                pane.buffer.set_cursor_position(6, 0, false);
                pane.buffer.follow_cursor = true;
                let view = update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
                assert!(view.scroll_animating);
                assert!(pane.target_scroll_y > 0.0);
                let first = pane.scroll_y;
                assert_eq!(pane.scroll_velocity_px_s, 0.0);
                assert!(pane.scroll_last_tick_at.is_some());

                pane.scroll_last_tick_at = Some(Instant::now() - Duration::from_millis(7));
                update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
                assert!(pane.scroll_y > first);
                assert!(pane.scroll_velocity_px_s > 0.0);

                // Another held-arrow target must carry active velocity, not
                // restart at rest on every key repeat.
                let moving = pane.scroll_y;
                pane.buffer.set_cursor_position(7, 0, false);
                pane.buffer.follow_cursor = true;
                pane.scroll_last_tick_at = Some(Instant::now() - Duration::from_millis(7));
                update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
                assert!(pane.scroll_y > moving);
                assert!(pane.scroll_velocity_px_s > 0.0);
                first
            }).collect();
        assert_eq!(first_positions, vec![0.0, 0.0]);
    }

    #[test]
    fn spring_settles_without_overshoot_at_slow_frame_rates() {
        let mut pane = pane();
        update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
        pane.buffer.set_cursor_position(40, 0, false);
        pane.buffer.follow_cursor = true;
        update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
        for _ in 0..100 {
            let before = pane.scroll_y;
            pane.scroll_last_tick_at = Some(Instant::now() - Duration::from_millis(50));
            update_viewport(&mut pane, 0.0, 800.0, 50.0, 10.0, 21.0, 210.0);
            assert!(pane.scroll_y >= before);
            assert!(pane.scroll_y <= pane.target_scroll_y);
        }
        assert_eq!(pane.scroll_y, pane.target_scroll_y);
        assert_eq!(pane.scroll_velocity_px_s, 0.0);
        assert!(pane.scroll_last_tick_at.is_none());
    }
}
