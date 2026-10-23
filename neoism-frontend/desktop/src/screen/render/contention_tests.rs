use super::*;
use neoism_terminal_core::damage::TerminalDamage;
use std::sync::Arc;

#[test]
fn idle_and_busy_frames_advance_blink_without_terminal_damage() {
    let mut content = crate::context::renderable::RenderableContent::default();
    content.has_blinking_enabled = true;
    let now = Instant::now();
    tick_terminal_cursor_blink(&mut content, 500, now);
    assert!(content.is_blinking_cursor_visible);
    tick_terminal_cursor_blink(
        &mut content,
        500,
        now + std::time::Duration::from_millis(500),
    );
    assert!(!content.is_blinking_cursor_visible);
    // A second tick in the same frame (after Renderer::run) must not double-toggle.
    tick_terminal_cursor_blink(
        &mut content,
        500,
        now + std::time::Duration::from_millis(500),
    );
    assert!(!content.is_blinking_cursor_visible);
    content.last_typing = Some(now + std::time::Duration::from_millis(600));
    tick_terminal_cursor_blink(
        &mut content,
        500,
        now + std::time::Duration::from_millis(700),
    );
    assert!(content.is_blinking_cursor_visible);
    tick_terminal_cursor_blink(
        &mut content,
        500,
        now + std::time::Duration::from_millis(1100),
    );
    assert!(!content.is_blinking_cursor_visible);
}

fn panel(damage: TerminalDamage) -> PanelFrame {
    let mut row = Row::new(8);
    row.inner[0] = Square::from_char('好');
    PanelFrame {
        captured_at: Instant::now(),
        block_headers: None,
        prompt_animating: true,
        image_overlays: Vec::new(),
        route_id: 7,
        rich_text_id: 11,
        terminal_scroll_offset_y: 0.0,
        terminal_reserved_bottom_rows: 0,
        terminal_snapshot_above: None,
        terminal_snapshot_below: None,
        layout_rect: [10.0, 20.0, 80.0, 20.0],
        cols: 8,
        rows: 1,
        cell_w: 10.0,
        cell_h: 20.0,
        font_px: 14.0,
        visible_rows: vec![row],
        source_row_indices: vec![Some(0)],
        style_set: Default::default(),
        term_colors: Default::default(),
        cursor_policy: TerminalCursorVisibilityInput {
            block_footer_active: false,
            is_active: true,
            hide_running_command_cursor: false,
            block_input_cursor_present: false,
            cursor_state_visible: true,
            tree_focused: false,
            trail_cursor_enabled: false,
        },
        cursor_col: 0,
        cursor_row: 0,
        cursor_visible: true,
        cursor_shape: neoism_terminal_core::ansi::CursorShape::Block,
        cursor_blinking: true,
        cursor_blink_visible: true,
        cursor_preedit: false,
        cursor_color: [1.0, 0.0, 0.0, 1.0],
        has_non_terminal_surface: false,
        is_active: true,
        damage,
        selection: None,
        display_offset: 0,
        hint_matches: None,
        focused_match: None,
        hovered_hyperlink: None,
    }
}

#[test]
fn contention_retains_exact_rows_without_copying_or_replaying_full_damage() {
    let fresh = PanelFrameRef::fresh(panel(TerminalDamage::Full));
    assert!(fresh.requires_full_rebuild(false));
    let previous = fresh.frame.clone();
    let rows_ptr = previous.visible_rows.as_ptr();
    let terminal = crate::event::sync::FairMutex::new(());
    let guard = terminal.lock();
    for _ in 0..3 {
        assert!(terminal.try_lock_unfair().is_none());
        let retained = PanelFrameRef::retained(previous.clone());
        assert!(Arc::ptr_eq(&previous, &retained.frame));
        assert_eq!(rows_ptr, retained.visible_rows.as_ptr());
        assert_eq!(retained.visible_rows[0].inner[0].c(), '好');
        assert!(matches!(retained.damage, TerminalDamage::Noop));
        assert!(!retained.requires_full_rebuild(false));
        // A genuinely recreated grid still rebuilds from retained full-fidelity rows.
        assert!(retained.requires_full_rebuild(true));
        assert_eq!((retained.cols, retained.rows), (8, 1));
    }
    drop(guard);
    assert!(terminal.try_lock_unfair().is_some());
}

#[test]
fn consumed_but_unpresented_damage_is_deferred_until_a_fresh_snapshot() {
    let mut pending = crate::context::renderable::PendingUpdate::default();
    let mut last_damage = TerminalDamage::Full;
    defer_panel_damage(&mut pending, &mut last_damage);
    assert!(pending.is_dirty());
    assert!(matches!(last_damage, TerminalDamage::Noop));
    // A second busy frame must not erase the earlier, unpresented damage.
    defer_panel_damage(&mut pending, &mut last_damage);
    assert!(matches!(
        pending.take_terminal_damage(),
        Some(TerminalDamage::Full)
    ));
    assert!(pending.take_terminal_damage().is_none());
}

#[test]
fn effect_repaints_and_cursor_blink_do_not_replay_partial_damage() {
    let fresh = PanelFrameRef::fresh(panel(TerminalDamage::Partial(Default::default())));
    assert!(matches!(fresh.damage, TerminalDamage::Partial(_)));
    assert!(matches!(fresh.frame.damage, TerminalDamage::Noop));
    let mut retained = PanelFrameRef::retained(fresh.frame.clone());
    // These are presentation-only changes, not cell content damage.
    retained.cursor_blink_visible = false;
    retained.cursor_color = [0.0, 1.0, 0.0, 1.0];
    retained.terminal_scroll_offset_y = 0.5;
    retained.layout_rect[0] = 30.0;
    assert!(!retained.cursor_blink_visible);
    assert_ne!(retained.cursor_color, fresh.cursor_color);
    assert!(retained.prompt_animating);
    assert!(!retained.requires_full_rebuild(false));
    assert!(matches!(retained.damage, TerminalDamage::Noop));
    assert!(Arc::ptr_eq(&fresh.frame, &retained.frame));
    // Presentation updates cannot mutate cached geometry or disable its next blink.
    assert!(fresh.frame.cursor_blink_visible);
    assert_eq!(fresh.frame.layout_rect[0], 10.0);
    assert_eq!(fresh.frame.terminal_scroll_offset_y, 0.0);
}
