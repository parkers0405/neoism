
use super::*;

#[test]
fn test_island_constants() {
    // Verify all constants are set correctly. The workspace strip
    // mirrors the buffer-tab strip's height + font for a consistent
    // look (see BUFFER_TABS_HEIGHT = 28.0, FONT_SIZE = 11.5).
    assert_eq!(ISLAND_HEIGHT, 28.0);
    assert_eq!(TITLE_FONT_SIZE, 11.5);
    assert_eq!(TAB_PADDING_X, 24.0);
    assert_eq!(ISLAND_MARGIN_RIGHT, 8.0);
}

#[test]
fn test_island_initialization() {
    let inactive_color = [0.5, 0.5, 0.5, 1.0];
    let active_color = [0.9, 0.9, 0.9, 1.0];
    let border_color = [0.7, 0.7, 0.7, 1.0];

    let island = Island::new(inactive_color, active_color, border_color, true);

    assert_eq!(island.inactive_text_color, inactive_color);
    assert_eq!(island.active_text_color, active_color);
    assert_eq!(island.border_color, border_color);
    assert!(island.hide_if_single);
}

#[test]
fn test_island_height() {
    let island = Island::new(
        [0.8, 0.8, 0.8, 1.0],
        [1.0, 1.0, 1.0, 1.0],
        [0.8, 0.8, 0.8, 1.0],
        false,
    );
    assert_eq!(island.height(), ISLAND_HEIGHT);
}

fn test_island() -> Island {
    Island::new(
        [0.5, 0.5, 0.5, 1.0],
        [0.9, 0.9, 0.9, 1.0],
        [0.7, 0.7, 0.7, 1.0],
        false,
    )
}

#[test]
fn progress_first_report_seeds_started_and_seen() {
    let mut island = test_island();
    island.set_progress_report(ProgressReport {
        state: ProgressState::Indeterminate,
        progress: None,
    });
    assert!(island.progress_started_at.is_some());
    assert!(island.progress_last_seen.is_some());
    assert_eq!(island.progress_state, Some(ProgressState::Indeterminate));
}

#[test]
fn progress_repeated_same_state_keeps_started_at_stable() {
    // Issue #1509: a TUI that heartbeats `OSC 9;4;3` (or any same-state
    // report) must NOT restart the indeterminate animation phase, or the
    // pulsing block snaps back to the left edge on every report.
    let mut island = test_island();
    island.set_progress_report(ProgressReport {
        state: ProgressState::Indeterminate,
        progress: None,
    });
    let first_started = island.progress_started_at.unwrap();
    let first_seen = island.progress_last_seen.unwrap();

    // Sleep so a subsequent Instant::now() is observably later — the
    // started_at field must stay equal while last_seen advances.
    std::thread::sleep(web_time::Duration::from_millis(15));
    island.set_progress_report(ProgressReport {
        state: ProgressState::Indeterminate,
        progress: None,
    });

    assert_eq!(
        island.progress_started_at,
        Some(first_started),
        "started_at must not move on a same-state heartbeat"
    );
    assert!(
        island.progress_last_seen.unwrap() > first_seen,
        "last_seen must advance on every report"
    );
}

#[test]
fn progress_state_transition_resets_started_at() {
    // Set → Indeterminate is a real state change, so the animation
    // anchor should be reseated. (Set has no animation, but the
    // started_at field still becomes meaningful as soon as we hit
    // Indeterminate.)
    let mut island = test_island();
    island.set_progress_report(ProgressReport {
        state: ProgressState::Set,
        progress: Some(50),
    });
    let first = island.progress_started_at.unwrap();

    std::thread::sleep(web_time::Duration::from_millis(15));
    island.set_progress_report(ProgressReport {
        state: ProgressState::Indeterminate,
        progress: None,
    });

    assert!(
        island.progress_started_at.unwrap() > first,
        "transitioning into a new state must move started_at forward"
    );
    assert_eq!(island.progress_state, Some(ProgressState::Indeterminate));
}

#[test]
fn progress_set_value_change_does_not_reseat_started_at() {
    // Same `Set` state with a different percentage is still the same
    // state — only the value updates. started_at stays put; the bar
    // just redraws at the new fraction.
    let mut island = test_island();
    island.set_progress_report(ProgressReport {
        state: ProgressState::Set,
        progress: Some(20),
    });
    let first = island.progress_started_at.unwrap();

    std::thread::sleep(web_time::Duration::from_millis(15));
    island.set_progress_report(ProgressReport {
        state: ProgressState::Set,
        progress: Some(60),
    });

    assert_eq!(island.progress_started_at, Some(first));
    assert_eq!(island.progress_value, Some(60));
}

/// Each char = 1.0 wide, including the ellipsis. Easy arithmetic.
fn fixed_unit_width(_c: char) -> f32 {
    1.0
}

fn rendered_width(s: &str, char_width: impl FnMut(char) -> f32) -> f32 {
    s.chars().map(char_width).sum()
}

#[test]
fn title_fits_is_returned_unchanged() {
    assert_eq!(
        fit_title_with_widths("hello", 10.0, fixed_unit_width),
        "hello"
    );
    assert_eq!(fit_title_with_widths("hi", 2.0, fixed_unit_width), "hi");
}

#[test]
fn title_that_fits_borrows_without_allocating() {
    // Confirms the zero-allocation "no truncation" hot path: when the
    // full title fits, the returned Cow must stay Borrowed so the
    // render loop doesn't allocate a new String every frame.
    let out = fit_title_with_widths("ok", 10.0, fixed_unit_width);
    assert!(
        matches!(out, Cow::Borrowed(_)),
        "expected borrowed, got {out:?}"
    );
}

#[test]
fn title_zero_budget_returns_ellipsis() {
    // Historically this was short-circuited to return the full title;
    // now it falls through the loop and returns "…" consistently with
    // tiny-but-positive budgets.
    assert_eq!(fit_title_with_widths("abc", 0.0, fixed_unit_width), "…");
}

#[test]
fn title_overflow_gets_ellipsized_and_fits_budget() {
    // "hello world" budgeted at 5 → best we can do without exceeding
    // is "hell" (4) + "…" (1) = 5. Anything more overflows.
    let out = fit_title_with_widths("hello world", 5.0, fixed_unit_width);
    assert_eq!(out, "hell…");
    assert!(
        rendered_width(&out, fixed_unit_width) <= 5.0,
        "truncated width {} must be ≤ budget 5",
        rendered_width(&out, fixed_unit_width)
    );
}

#[test]
fn title_respects_budget_with_wide_chars() {
    // Mixed widths: 'W' = 2.0, others (including ellipsis) = 1.0.
    // Title "WxWxW", budget 4.0. Walk:
    // ix=0 W: before add, 0+1(suffix) ≤ 4 → truncate_ix=0; accum→2
    // ix=1 x: 2+1 ≤ 4 → truncate_ix=1; accum→3
    // ix=2 W: 3+1 ≤ 4 → truncate_ix=2; accum→5; 5>4 → cut.
    // Output: title[..2] + "…" = "Wx…", width 2+1+1 = 4 ≤ 4 ✓
    let widths = |c: char| if c == 'W' { 2.0 } else { 1.0 };
    let out = fit_title_with_widths("WxWxW", 4.0, widths);
    assert_eq!(out, "Wx…");
    assert!(rendered_width(&out, widths) <= 4.0);
}

#[test]
fn title_truncation_preserves_utf8_boundaries() {
    // Each emoji/char = 2.0 wide; ellipsis = 2.0.
    // Title "🎟🎟🎟" = 6.0. Budget 4.0 → one emoji + "…" = 4.0 ≤ 4 ✓.
    // Crucial: the byte index we cut at must be on a UTF-8 boundary.
    let w = |_c: char| 2.0;
    let out = fit_title_with_widths("🎟🎟🎟", 4.0, w);
    assert_eq!(out, "🎟…");
    assert!(out.chars().count() == 2, "{out:?} should be 2 graphemes");
}

#[test]
fn title_budget_smaller_than_ellipsis_still_returns_ellipsis() {
    // Budget 0.5 < ellipsis_width 1.0: first char overflows, prefix is
    // empty, we return just "…" so the user at least sees *something*
    // indicating truncation rather than a blank tab label.
    let out = fit_title_with_widths("abc", 0.5, fixed_unit_width);
    assert_eq!(out, "…");
}

#[test]
fn title_empty_input_returned_as_is() {
    assert_eq!(fit_title_with_widths("", 10.0, fixed_unit_width), "");
}

#[test]
fn title_exact_fit_not_truncated() {
    // Title "abcd" = 4.0, budget 4.0 → fits exactly, no truncation.
    assert_eq!(fit_title_with_widths("abcd", 4.0, fixed_unit_width), "abcd");
}

#[test]
fn progress_remove_clears_all_progress_state() {
    let mut island = test_island();
    island.set_progress_report(ProgressReport {
        state: ProgressState::Set,
        progress: Some(50),
    });
    island.set_progress_report(ProgressReport {
        state: ProgressState::Remove,
        progress: None,
    });
    assert!(island.progress_state.is_none());
    assert!(island.progress_value.is_none());
    assert!(island.progress_started_at.is_none());
    assert!(island.progress_last_seen.is_none());
}

// ── Keyboard focus cursor — mirrors BufferTabs::move_focused ────

#[test]
fn focus_seeds_cursor_on_active_tab() {
    let mut island = test_island();
    // Active workspace is tab 2 of 4 → focus cursor parks there.
    island.set_focused(true, 2, 4);
    assert!(island.is_focused());
    assert_eq!(island.focus_cursor(4), 2);
}

#[test]
fn focus_refused_when_no_tabs() {
    let mut island = test_island();
    island.set_focused(true, 0, 0);
    assert!(!island.is_focused());
    assert!(island.focused_cursor_rect().is_none());
}

#[test]
fn move_cursor_does_not_switch_active() {
    // The whole point: Left/Right move the CURSOR only. The widget
    // never changes the active workspace — that's the host's job on
    // Enter. So moving the cursor just updates `focus_cursor`.
    let mut island = test_island();
    island.set_focused(true, 1, 3);
    assert_eq!(island.focus_cursor(3), 1);

    assert!(island.move_focus_cursor(false, 3)); // right → 2
    assert_eq!(island.focus_cursor(3), 2);

    assert!(island.move_focus_cursor(false, 3)); // right wraps → 0
    assert_eq!(island.focus_cursor(3), 0);

    assert!(island.move_focus_cursor(true, 3)); // left wraps → 2
    assert_eq!(island.focus_cursor(3), 2);
}

#[test]
fn move_cursor_noop_when_unfocused_or_single() {
    let mut island = test_island();
    // Not focused → no movement.
    assert!(!island.move_focus_cursor(false, 3));
    // Focused but single tab → nowhere to move.
    island.set_focused(true, 0, 1);
    assert!(!island.move_focus_cursor(false, 1));
}

#[test]
fn clearing_focus_drops_cursor_rect() {
    let mut island = test_island();
    island.set_focused(true, 0, 2);
    // Pretend a render computed a rect.
    island.focused_cursor_rect = Some([1.0, 2.0, 3.0, 4.0]);
    assert!(island.focused_cursor_rect().is_some());
    island.set_focused(false, 0, 2);
    assert!(island.focused_cursor_rect().is_none());
}

// ── Animated hover — mirrors BufferTabs::set_hover ──────────────

#[test]
fn set_hover_reports_change_and_starts_anim() {
    let mut island = test_island();
    assert!(island.set_hover(Some(1), 3));
    assert_eq!(island.hover, Some(1));
    assert!(island.hover_anim_started.is_some());
    // Same hover again → no change.
    assert!(!island.set_hover(Some(1), 3));
}

#[test]
fn set_hover_filters_out_of_range() {
    let mut island = test_island();
    // ix >= num_tabs is filtered to None.
    assert!(!island.set_hover(Some(5), 3));
    assert_eq!(island.hover, None);
}

#[test]
fn clear_hover_immediate_wipes_anim_state() {
    let mut island = test_island();
    island.set_hover(Some(0), 2);
    assert!(island.clear_hover_immediate());
    assert_eq!(island.hover, None);
    assert!(island.hover_anim_started.is_none());
    assert!(island.hover_from.is_none());
    assert!(island.hover_to.is_none());
    // Already clear → no change reported.
    assert!(!island.clear_hover_immediate());
}
