use super::*;
use crate::panels::completion_menu::ScrollDelta;
use crate::terminal_blocks::CommandBlockSnapshot;

#[test]
fn selection_autoscroll_is_zero_in_safe_middle() {
    let region = SelectionAutoscrollRegion::new(100.0, 20, 18.0);

    assert_eq!(region.drag_delta_pixels(220.0), 0.0);
}

#[test]
fn vertical_overlay_scroll_scales_lines_by_three_rows_per_notch() {
    assert_eq!(
        vertical_overlay_scroll_pixels(&ScrollDelta::Lines { x: 0.0, y: 1.0 }, 22.0),
        66.0
    );
    assert_eq!(
        vertical_overlay_scroll_pixels(&ScrollDelta::Lines { x: 0.0, y: -2.0 }, 16.0),
        -96.0
    );
}

#[test]
fn vertical_overlay_scroll_pixel_delta_passes_through() {
    assert_eq!(
        vertical_overlay_scroll_pixels(&ScrollDelta::Pixels { x: 0.0, y: -38.0 }, 22.0),
        -38.0
    );
}

#[test]
fn vertical_overlay_scroll_clamps_row_height_to_one() {
    // Degenerate row height shouldn't zero out the lines-delta scroll —
    // the user still expects motion proportional to the notch count.
    assert_eq!(
        vertical_overlay_scroll_pixels(&ScrollDelta::Lines { x: 0.0, y: 4.0 }, 0.0),
        12.0
    );
}

#[test]
fn agent_timeline_scroll_uses_calmer_line_step() {
    assert_eq!(
        agent_timeline_scroll_pixels(&ScrollDelta::Lines { x: 0.0, y: 1.0 }),
        24.0
    );
    assert_eq!(
        agent_timeline_wheel(&ScrollDelta::Lines { x: 0.0, y: 1.0 }),
        AgentTimelineWheel {
            pixels: 24.0,
            smooth: true,
        }
    );
    assert_eq!(
        agent_timeline_scroll_pixels(&ScrollDelta::Pixels { x: 0.0, y: -17.0 }),
        -17.0
    );
    assert_eq!(
        agent_timeline_wheel(&ScrollDelta::Pixels { x: 0.0, y: -17.0 }),
        AgentTimelineWheel {
            pixels: -17.0,
            smooth: false,
        }
    );
}

#[test]
fn agent_timeline_scroll_caps_mouse_wheel_bursts() {
    assert_eq!(
        agent_timeline_scroll_pixels(&ScrollDelta::Lines { x: 0.0, y: 8.0 }),
        72.0
    );
    assert_eq!(
        agent_timeline_scroll_pixels(&ScrollDelta::Lines { x: 0.0, y: -8.0 }),
        -72.0
    );
}

#[test]
fn diagnostics_popup_wheel_maps_lines_to_rows_and_inline_px() {
    let wheel =
        DiagnosticsPopupWheel::from_delta(&ScrollDelta::Lines { x: 2.0, y: -3.0 });

    assert_eq!(
        wheel,
        DiagnosticsPopupWheel {
            vertical_rows: 3,
            horizontal_px: -24.0,
        }
    );
}

#[test]
fn diagnostics_popup_wheel_rounds_pixels_to_whole_rows() {
    let wheel =
        DiagnosticsPopupWheel::from_delta(&ScrollDelta::Pixels { x: -11.0, y: 55.0 });

    assert_eq!(
        wheel,
        DiagnosticsPopupWheel {
            vertical_rows: -3,
            horizontal_px: 11.0,
        }
    );
}

#[test]
fn selection_autoscroll_scales_near_edges() {
    let region = SelectionAutoscrollRegion::new(100.0, 20, 20.0);

    assert_eq!(region.drag_delta_pixels(75.0), 34.0);
    assert_eq!(region.drag_delta_pixels(125.0), 20.5);
    assert_eq!(region.drag_delta_pixels(475.0), -20.5);
    assert_eq!(region.drag_delta_pixels(525.0), -34.0);
}

#[test]
fn editor_viewport_maps_to_terminal_style_scrollbar_state() {
    let viewport = EditorViewport {
        line_count: 1_000,
        topline: 200,
        botline: 240,
    };

    assert_eq!(
        viewport.scrollbar_model(),
        Some(EditorScrollbarModel {
            display_offset: 760,
            history_size: 960,
            screen_lines: 40,
        })
    );
}

#[test]
fn editor_viewport_suppresses_scrollbar_when_file_fits() {
    assert_eq!(
        EditorViewport {
            line_count: 40,
            topline: 0,
            botline: 40,
        }
        .scrollbar_model(),
        None
    );
}

#[test]
fn editor_viewport_converts_display_offset_to_topline() {
    let viewport = EditorViewport {
        line_count: 1_000,
        topline: 200,
        botline: 240,
    };

    assert_eq!(viewport.topline_for_display_offset(0), Some(960));
    assert_eq!(viewport.topline_for_display_offset(760), Some(200));
    assert_eq!(viewport.topline_for_display_offset(2_000), Some(0));
}

#[test]
fn editor_scrollbar_drag_target_is_nvim_one_indexed() {
    let viewport = EditorViewport {
        line_count: 1_000,
        topline: 200,
        botline: 240,
    };

    assert_eq!(
        viewport.scrollbar_drag_target(760),
        Some(EditorScrollbarDragTarget {
            topline: 200,
            nvim_topline: 201,
        })
    );
}

#[test]
fn editor_wheel_raw_delta_rejects_at_edges() {
    let top = EditorViewport {
        line_count: 500,
        topline: 0,
        botline: 40,
    };
    let bottom = EditorViewport {
        line_count: 500,
        topline: 460,
        botline: 500,
    };
    let middle = EditorViewport {
        line_count: 500,
        topline: 100,
        botline: 140,
    };

    assert_eq!(
        top.wheel_raw_action(12.0),
        EditorWheelAction::EdgeElastic {
            clear_top_snapshots: true,
            clear_bottom_snapshots: false,
            elastic_pixels: 12.0,
        }
    );
    assert_eq!(
        bottom.wheel_raw_action(-12.0),
        EditorWheelAction::EdgeElastic {
            clear_top_snapshots: false,
            clear_bottom_snapshots: true,
            elastic_pixels: -12.0,
        }
    );
    assert_eq!(middle.wheel_raw_action(12.0), EditorWheelAction::Idle);
}

#[test]
fn editor_wheel_commits_send_rows_or_edge_elastic() {
    let top = EditorViewport {
        line_count: 500,
        topline: 0,
        botline: 40,
    };
    let middle = EditorViewport {
        line_count: 500,
        topline: 100,
        botline: 140,
    };

    assert_eq!(
        top.wheel_commit_action(2, 20.0),
        EditorWheelAction::EdgeElastic {
            clear_top_snapshots: true,
            clear_bottom_snapshots: false,
            elastic_pixels: 40.0,
        }
    );
    assert_eq!(
        middle.wheel_commit_action(-3, 20.0),
        EditorWheelAction::SendRows {
            direction: EditorScrollDirection::Down,
            rows: 3,
        }
    );
}

#[test]
fn display_offset_delta_clamps_to_terminal_scroll_delta() {
    assert_eq!(display_offset_delta(20, 35), Some(15));
    assert_eq!(display_offset_delta(20, 20), None);
    assert_eq!(display_offset_delta(35, 20), Some(-15));
    assert_eq!(display_offset_delta(0, usize::MAX), Some(i32::MAX));
}

#[test]
fn scrollbar_click_band_maps_physical_pane_to_logical_hit_zone() {
    let band = ScrollbarClickBand {
        panel_rect: [200.0, 100.0, 600.0, 400.0],
        scale_factor: 2.0,
        hit_width_logical_px: 12.0,
    };

    assert!(band.contains_logical_point(394.0, 120.0));
    assert!(band.contains_logical_point(400.0, 250.0));
    assert!(!band.contains_logical_point(387.0, 120.0));
    assert!(!band.contains_logical_point(394.0, 40.0));
    assert!(!band.contains_logical_point(410.0, 120.0));
}

#[test]
fn scrollbar_click_band_rejects_invalid_scale() {
    let band = ScrollbarClickBand {
        panel_rect: [0.0, 0.0, 100.0, 100.0],
        scale_factor: 0.0,
        hit_width_logical_px: 12.0,
    };

    assert!(!band.contains_logical_point(95.0, 50.0));
}

#[test]
fn scrollbar_pane_kind_limits_global_scrollbar_ownership() {
    assert!(ScrollbarPaneKind::Editor.owns_global_scrollbar_band());
    assert!(ScrollbarPaneKind::Terminal.owns_global_scrollbar_band());
    assert!(!ScrollbarPaneKind::Markdown.owns_global_scrollbar_band());
    assert!(!ScrollbarPaneKind::Agent.can_show_global_scrollbar());
    assert!(!ScrollbarPaneKind::Tags.can_show_global_scrollbar());
}

#[test]
fn scrollbar_click_intent_swallows_empty_editor_band() {
    let intent = ScrollbarClickContext {
        pane_kind: ScrollbarPaneKind::Editor,
        band_contains_pointer: true,
        has_scroll_state: false,
        hit_scrollbar_geometry: false,
        grabbed_thumb: false,
    }
    .intent();

    assert_eq!(intent, ScrollbarClickIntent::SwallowEmptyBand);
}

#[test]
fn scrollbar_click_intent_ignores_overlay_empty_band() {
    let intent = ScrollbarClickContext {
        pane_kind: ScrollbarPaneKind::Agent,
        band_contains_pointer: true,
        has_scroll_state: false,
        hit_scrollbar_geometry: false,
        grabbed_thumb: false,
    }
    .intent();

    assert_eq!(intent, ScrollbarClickIntent::Ignore);
}

#[test]
fn scrollbar_click_intent_distinguishes_thumb_and_track() {
    let thumb = ScrollbarClickContext {
        pane_kind: ScrollbarPaneKind::Terminal,
        band_contains_pointer: true,
        has_scroll_state: true,
        hit_scrollbar_geometry: true,
        grabbed_thumb: true,
    }
    .intent();
    let track = ScrollbarClickContext {
        grabbed_thumb: false,
        ..ScrollbarClickContext {
            pane_kind: ScrollbarPaneKind::Terminal,
            band_contains_pointer: true,
            has_scroll_state: true,
            hit_scrollbar_geometry: true,
            grabbed_thumb: true,
        }
    }
    .intent();

    assert_eq!(
        thumb,
        ScrollbarClickIntent::StartDrag {
            jump_to_track: false
        }
    );
    assert_eq!(
        track,
        ScrollbarClickIntent::StartDrag {
            jump_to_track: true
        }
    );
}

#[test]
fn terminal_scrollbar_panel_state_reserves_footer_rows() {
    let state = TerminalScrollbarPanelContext {
        rich_text_id: 42,
        panel_rect: [10.0, 20.0, 800.0, 400.0],
        display_offset: 8,
        history_size: 120,
        screen_lines: 20,
        reserved_footer_rows: 3,
        cell_height_px: 17.6,
    }
    .panel_state();

    assert_eq!(state.rich_text_id, 42);
    assert_eq!(state.panel_rect, [10.0, 20.0, 800.0, 346.0]);
    assert_eq!(state.display_offset, 8);
    assert_eq!(state.history_size, 120);
    assert_eq!(state.screen_lines, 17);
}

#[test]
fn terminal_scrollbar_panel_state_keeps_one_visible_row() {
    let state = TerminalScrollbarPanelContext {
        rich_text_id: 7,
        panel_rect: [0.0, 0.0, 80.0, 12.0],
        display_offset: 0,
        history_size: 10,
        screen_lines: 1,
        reserved_footer_rows: 10,
        cell_height_px: 14.0,
    }
    .panel_state();

    assert_eq!(state.panel_rect, [0.0, 0.0, 80.0, 12.0]);
    assert_eq!(state.screen_lines, 1);
}

#[test]
fn terminal_wheel_edge_rejects_raw_terminal_edges() {
    assert_eq!(
        TerminalWheelEdgeContext {
            delta_pixels: 18.0,
            display_offset: 50,
            history_size: 50,
            use_block_scroll: false,
            block_cursor_can_scroll_at_bottom: false,
            block_at_composed_top: false,
        }
        .action(),
        TerminalWheelEdgeAction::Reject {
            clear_block_detached: false
        }
    );

    assert_eq!(
        TerminalWheelEdgeContext {
            delta_pixels: -18.0,
            display_offset: 0,
            history_size: 50,
            use_block_scroll: false,
            block_cursor_can_scroll_at_bottom: false,
            block_at_composed_top: false,
        }
        .action(),
        TerminalWheelEdgeAction::Reject {
            clear_block_detached: false
        }
    );
}

#[test]
fn terminal_wheel_edge_allows_block_cursor_to_continue_at_edges() {
    assert_eq!(
        TerminalWheelEdgeContext {
            delta_pixels: -18.0,
            display_offset: 0,
            history_size: 50,
            use_block_scroll: true,
            block_cursor_can_scroll_at_bottom: true,
            block_at_composed_top: false,
        }
        .action(),
        TerminalWheelEdgeAction::Continue
    );

    assert_eq!(
        TerminalWheelEdgeContext {
            delta_pixels: 18.0,
            display_offset: 50,
            history_size: 50,
            use_block_scroll: true,
            block_cursor_can_scroll_at_bottom: false,
            block_at_composed_top: false,
        }
        .action(),
        TerminalWheelEdgeAction::Continue
    );
}

#[test]
fn terminal_wheel_edge_rejects_block_scroll_at_composed_limits() {
    assert_eq!(
        TerminalWheelEdgeContext {
            delta_pixels: 18.0,
            display_offset: 50,
            history_size: 50,
            use_block_scroll: true,
            block_cursor_can_scroll_at_bottom: false,
            block_at_composed_top: true,
        }
        .action(),
        TerminalWheelEdgeAction::Reject {
            clear_block_detached: false
        }
    );

    assert_eq!(
        TerminalWheelEdgeContext {
            delta_pixels: -18.0,
            display_offset: 0,
            history_size: 50,
            use_block_scroll: true,
            block_cursor_can_scroll_at_bottom: false,
            block_at_composed_top: false,
        }
        .action(),
        TerminalWheelEdgeAction::Reject {
            clear_block_detached: true
        }
    );
}

#[test]
fn terminal_block_wheel_edge_allows_detached_cursor_below_raw_bottom() {
    let context = TerminalBlockWheelEdgeContext {
        delta_pixels: -18.0,
        display_offset: 0,
        history_size: 50,
        use_block_scroll: true,
        content_top_abs: Some(12),
        stored_cursor: Some(TerminalBlockScrollCursor {
            raw_top_abs: 20,
            chrome_row: 0,
        }),
        bottom_cursor: Some(TerminalBlockScrollCursor {
            raw_top_abs: 22,
            chrome_row: 0,
        }),
        block_detached: true,
    };

    assert!(context.raw_edge_rejected());
    assert!(context.block_cursor_can_scroll_at_bottom());
    assert_eq!(context.action(), TerminalWheelEdgeAction::Continue);
}

#[test]
fn terminal_block_wheel_edge_rejects_bottom_when_cursor_is_not_detached() {
    assert_eq!(
        TerminalBlockWheelEdgeContext {
            delta_pixels: -18.0,
            display_offset: 0,
            history_size: 50,
            use_block_scroll: true,
            content_top_abs: Some(12),
            stored_cursor: Some(TerminalBlockScrollCursor {
                raw_top_abs: 20,
                chrome_row: 0,
            }),
            bottom_cursor: Some(TerminalBlockScrollCursor {
                raw_top_abs: 22,
                chrome_row: 0,
            }),
            block_detached: false,
        }
        .action(),
        TerminalWheelEdgeAction::Reject {
            clear_block_detached: true
        }
    );
}

#[test]
fn terminal_block_wheel_edge_detects_composed_top_from_cursor_or_anchor() {
    let from_cursor = TerminalBlockWheelEdgeContext {
        delta_pixels: 18.0,
        display_offset: 50,
        history_size: 50,
        use_block_scroll: true,
        content_top_abs: Some(4),
        stored_cursor: Some(TerminalBlockScrollCursor {
            raw_top_abs: 0,
            chrome_row: 0,
        }),
        bottom_cursor: None,
        block_detached: true,
    };
    let from_anchor = TerminalBlockWheelEdgeContext {
        stored_cursor: None,
        content_top_abs: Some(0),
        ..from_cursor
    };

    assert!(from_cursor.block_at_composed_top());
    assert!(from_anchor.block_at_composed_top());
    assert_eq!(
        from_cursor.action(),
        TerminalWheelEdgeAction::Reject {
            clear_block_detached: false
        }
    );
}

fn block_snapshot(abs_row: usize) -> CommandBlockSnapshot {
    CommandBlockSnapshot {
        command: "echo ok".to_string(),
        cwd: None,
        status: crate::terminal_blocks::command::BlockStatusKind::Ok,
        favorite: false,
        output_start_row: Some(abs_row),
        duration_ms: 12.0,
    }
}

#[test]
fn block_scroll_plan_advances_through_command_chrome() {
    let plan = TerminalBlockScrollCommitContext {
        committed_rows: -2,
        display_offset: 6,
        history_size: 40,
        content_top_abs: Some(10),
        existing_cursor: None,
        bottom_cursor: None,
        snapshots: &[block_snapshot(10)],
        echo_rows: None,
    }
    .plan();

    assert_eq!(
        plan,
        TerminalBlockScrollPlan::CursorMoved(TerminalBlockScrollMoved {
            cursor: TerminalBlockScrollCursor {
                raw_top_abs: 11,
                chrome_row: 0,
            },
            direction: -1,
            raw_delta: -1,
            raw_scroll_delta: Some(-1),
            cursor_only_at_top: false,
            cursor_only_at_bottom: false,
            anchor_abs: 10,
        })
    );
}

#[test]
fn block_scroll_plan_recovers_to_raw_when_cursor_is_stale() {
    assert_eq!(
        TerminalBlockScrollCommitContext {
            committed_rows: 1,
            display_offset: 4,
            history_size: 40,
            content_top_abs: Some(0),
            existing_cursor: Some(TerminalBlockScrollCursor {
                raw_top_abs: 0,
                chrome_row: 0,
            }),
            bottom_cursor: None,
            snapshots: &[],
            echo_rows: None,
        }
        .plan(),
        TerminalBlockScrollPlan::CursorUnchanged {
            raw_scroll_delta: Some(1),
        }
    );
}

#[test]
fn block_scroll_finish_resets_at_composed_bottom() {
    let moved = TerminalBlockScrollMoved {
        cursor: TerminalBlockScrollCursor {
            raw_top_abs: 30,
            chrome_row: 0,
        },
        direction: -1,
        raw_delta: -1,
        raw_scroll_delta: None,
        cursor_only_at_top: false,
        cursor_only_at_bottom: true,
        anchor_abs: 29,
    };

    assert_eq!(
        moved.finish(
            true,
            0,
            40,
            Some(TerminalBlockScrollCursor {
                raw_top_abs: 30,
                chrome_row: 0,
            }),
        ),
        TerminalBlockScrollFinish::StoreCursor {
            cursor: TerminalBlockScrollCursor {
                raw_top_abs: 30,
                chrome_row: 0,
            },
            set_detached: Some(false),
            notify_scrollbar: true,
            reset_wheel: true,
            clear_accumulated_scroll: true,
            reached_top: false,
            reached_bottom: true,
        }
    );
}

// ---- TerminalScrollCommit (SideEffectPlan) tests --------------------
//
// The host (desktop / web) only needs to walk `plan.effects` in order,
// run the terminal mutation when it sees `ScrollDisplayRows`, then
// call `TerminalScrollPlan::resume` to get the trailing effects.
// These tests pin the ordering, the follow-up classification, and the
// trace payload so wasm and desktop end up applying the same writes.

#[test]
fn commit_raw_only_path_emits_scroll_display_and_requires_followup() {
    let commit = TerminalScrollCommit {
        use_block_scroll: false,
        block: TerminalBlockScrollCommitContext {
            committed_rows: -3,
            display_offset: 5,
            history_size: 40,
            content_top_abs: None,
            existing_cursor: None,
            bottom_cursor: None,
            snapshots: &[],
            echo_rows: None,
        },
    }
    .commit();

    assert_eq!(
        commit.effects,
        vec![TerminalScrollSideEffect::ScrollDisplayRows(-3)]
    );
    assert_eq!(
        commit.followup,
        TerminalScrollFollowup::RawOnlyRequiresScrollResult
    );
    assert!(commit.mark_dirty);
    assert!(commit.trace.is_none());

    // Host reports terminal actually scrolled -> notify only.
    assert_eq!(
        TerminalScrollPlan::resume(commit.followup, true, 2, 40, None),
        vec![TerminalScrollSideEffect::NotifyScrollbar],
    );
    // Host reports terminal stuck at edge -> reset + clear.
    assert_eq!(
        TerminalScrollPlan::resume(commit.followup, false, 5, 40, None),
        vec![
            TerminalScrollSideEffect::ResetWheel,
            TerminalScrollSideEffect::ClearAccumulatedScrollY,
        ],
    );
}

#[test]
fn commit_missing_anchor_clears_wheel_and_accumulator() {
    let commit = TerminalScrollCommit {
        use_block_scroll: true,
        block: TerminalBlockScrollCommitContext {
            committed_rows: 4,
            display_offset: 8,
            history_size: 40,
            content_top_abs: None,
            existing_cursor: None,
            bottom_cursor: None,
            snapshots: &[],
            echo_rows: None,
        },
    }
    .commit();

    assert_eq!(
        commit.effects,
        vec![
            TerminalScrollSideEffect::ResetWheel,
            TerminalScrollSideEffect::ClearAccumulatedScrollY,
        ]
    );
    assert_eq!(commit.followup, TerminalScrollFollowup::None);
    assert!(commit.mark_dirty);
    assert!(commit.trace.is_none());
}

#[test]
fn commit_cursor_unchanged_with_room_recovers_via_raw_scroll() {
    // raw scroll still has room -> clear block cursor, fall back to
    // raw, then resume with BlockRecoveryRequiresScrollResult.
    let commit = TerminalScrollCommit {
        use_block_scroll: true,
        block: TerminalBlockScrollCommitContext {
            committed_rows: 1,
            display_offset: 4,
            history_size: 40,
            content_top_abs: Some(0),
            existing_cursor: Some(TerminalBlockScrollCursor {
                raw_top_abs: 0,
                chrome_row: 0,
            }),
            bottom_cursor: None,
            snapshots: &[],
            echo_rows: None,
        },
    }
    .commit();

    assert_eq!(
        commit.effects,
        vec![
            TerminalScrollSideEffect::ClearBlockCursor,
            TerminalScrollSideEffect::ScrollDisplayRows(1),
        ]
    );
    assert_eq!(
        commit.followup,
        TerminalScrollFollowup::BlockRecoveryRequiresScrollResult
    );
    let trace = commit.trace.expect("trace present for cursor_unchanged");
    assert_eq!(trace.committed_rows, 1);
    assert_eq!(trace.direction, 1);
    assert_eq!(trace.raw_delta, 0);

    // Scrolled -> notify + reset + clear.
    assert_eq!(
        TerminalScrollPlan::resume(commit.followup, true, 5, 40, None),
        vec![
            TerminalScrollSideEffect::NotifyScrollbar,
            TerminalScrollSideEffect::ResetWheel,
            TerminalScrollSideEffect::ClearAccumulatedScrollY,
        ],
    );
    // Stuck -> reset + clear only.
    assert_eq!(
        TerminalScrollPlan::resume(commit.followup, false, 4, 40, None),
        vec![
            TerminalScrollSideEffect::ResetWheel,
            TerminalScrollSideEffect::ClearAccumulatedScrollY,
        ],
    );
}

#[test]
fn commit_cursor_unchanged_without_room_just_resets() {
    // raw scroll exhausted both ways -> no terminal mutation, no
    // follow-up; trailing reset_wheel + clear handle the parked
    // sub-row offset.
    let commit = TerminalScrollCommit {
        use_block_scroll: true,
        block: TerminalBlockScrollCommitContext {
            committed_rows: 1,
            display_offset: 40,
            history_size: 40,
            content_top_abs: Some(0),
            existing_cursor: Some(TerminalBlockScrollCursor {
                raw_top_abs: 0,
                chrome_row: 0,
            }),
            bottom_cursor: None,
            snapshots: &[],
            echo_rows: None,
        },
    }
    .commit();

    assert_eq!(
        commit.effects,
        vec![
            TerminalScrollSideEffect::ResetWheel,
            TerminalScrollSideEffect::ClearAccumulatedScrollY,
        ]
    );
    assert_eq!(commit.followup, TerminalScrollFollowup::None);
    assert!(commit.trace.is_some());
}

#[test]
fn commit_cursor_moved_emits_raw_scroll_then_finish_at_edge() {
    let commit = TerminalScrollCommit {
        use_block_scroll: true,
        block: TerminalBlockScrollCommitContext {
            committed_rows: -2,
            display_offset: 6,
            history_size: 40,
            content_top_abs: Some(10),
            existing_cursor: None,
            bottom_cursor: None,
            snapshots: &[block_snapshot(10)],
            echo_rows: None,
        },
    }
    .commit();

    // We always emit the raw scroll before computing finish because
    // desktop applies it before deciding edges.
    assert_eq!(
        commit.effects,
        vec![TerminalScrollSideEffect::ScrollDisplayRows(-1)],
    );
    let TerminalScrollFollowup::BlockMoveRequiresRawResult { moved } = commit.followup
    else {
        panic!(
            "expected BlockMoveRequiresRawResult, got {:?}",
            commit.followup
        );
    };
    assert_eq!(moved.direction, -1);
    assert_eq!(moved.raw_delta, -1);

    // Host scrolled, didn't reach bottom -> StoreCursor without
    // edge cleanup.
    let resumed = TerminalScrollPlan::resume(commit.followup, true, 5, 40, None);
    assert_eq!(
        resumed,
        vec![
            TerminalScrollSideEffect::SetBlockCursor(TerminalBlockScrollCursor {
                raw_top_abs: 11,
                chrome_row: 0,
            }),
            TerminalScrollSideEffect::NotifyScrollbar,
        ],
    );
}

#[test]
fn commit_cursor_moved_at_cursor_only_bottom_skips_raw_scroll() {
    // Display offset already at the live prompt (0) and we are
    // scrolling down within block chrome: raw_delta is non-zero but
    // raw_scroll_delta should be None because we don't touch the
    // alacritty viewport for cursor-only-at-bottom.
    let commit = TerminalScrollCommit {
        use_block_scroll: true,
        block: TerminalBlockScrollCommitContext {
            committed_rows: -1,
            display_offset: 0,
            history_size: 40,
            content_top_abs: Some(29),
            existing_cursor: Some(TerminalBlockScrollCursor {
                raw_top_abs: 29,
                chrome_row: 0,
            }),
            bottom_cursor: Some(TerminalBlockScrollCursor {
                raw_top_abs: 30,
                chrome_row: 0,
            }),
            snapshots: &[],
            echo_rows: None,
        },
    }
    .commit();

    assert!(
        commit.effects.is_empty(),
        "expected no raw ScrollDisplayRows at cursor_only_at_bottom, got {:?}",
        commit.effects
    );
    let TerminalScrollFollowup::BlockMoveRequiresRawResult { moved } = commit.followup
    else {
        panic!(
            "expected BlockMoveRequiresRawResult, got {:?}",
            commit.followup
        );
    };
    assert!(moved.cursor_only_at_bottom);
    // Desktop sets terminal_scrolled = true and display_after =
    // display_offset (pre-scroll) on the cursor_only path.
    let resumed = TerminalScrollPlan::resume(
        commit.followup,
        true,
        0,
        40,
        Some(TerminalBlockScrollCursor {
            raw_top_abs: 30,
            chrome_row: 0,
        }),
    );
    // We expect a SetBlockCursor + SetBlockDetached(false) (reached
    // bottom) + NotifyScrollbar + ResetWheel + ClearAccumulatedScrollY.
    assert_eq!(
        resumed,
        vec![
            TerminalScrollSideEffect::SetBlockCursor(TerminalBlockScrollCursor {
                raw_top_abs: 30,
                chrome_row: 0,
            }),
            TerminalScrollSideEffect::SetBlockDetached(false),
            TerminalScrollSideEffect::NotifyScrollbar,
            TerminalScrollSideEffect::ResetWheel,
            TerminalScrollSideEffect::ClearAccumulatedScrollY,
        ],
    );
}

#[test]
fn commit_cursor_moved_terminal_did_not_scroll_resets_to_anchor() {
    // Block cursor moved but the underlying alacritty terminal
    // refused to scroll (e.g. raw history exhausted). Desktop
    // collapses back to the anchor.
    let commit = TerminalScrollCommit {
        use_block_scroll: true,
        block: TerminalBlockScrollCommitContext {
            committed_rows: -2,
            display_offset: 6,
            history_size: 40,
            content_top_abs: Some(10),
            existing_cursor: None,
            bottom_cursor: None,
            snapshots: &[block_snapshot(10)],
            echo_rows: None,
        },
    }
    .commit();

    // terminal_scrolled=false: ResetToAnchor branch.
    let resumed = TerminalScrollPlan::resume(commit.followup, false, 6, 40, None);
    assert_eq!(
        resumed,
        vec![
            TerminalScrollSideEffect::SetBlockCursor(TerminalBlockScrollCursor {
                raw_top_abs: 10,
                chrome_row: 0,
            }),
            TerminalScrollSideEffect::SetBlockDetached(false),
            TerminalScrollSideEffect::ResetWheel,
            TerminalScrollSideEffect::ClearAccumulatedScrollY,
        ],
    );
}

#[test]
fn active_scrollbar_drag_rich_text_id_uses_drag_panel_current_order() {
    assert_eq!(
        active_scrollbar_drag_rich_text_id(Some(10), Some(20), 30),
        10
    );
    assert_eq!(active_scrollbar_drag_rich_text_id(None, Some(20), 30), 20);
    assert_eq!(active_scrollbar_drag_rich_text_id(None, None, 30), 30);
}

#[test]
fn diagnostics_popup_wheel_ignores_when_popup_hidden() {
    let decision = DiagnosticsPopupWheelContext {
        popup_visible: false,
        pointer_over_popup: true,
        row_under_pointer: Some(2),
        vertical_rows: 3,
        horizontal_px: 12.0,
    }
    .decide();
    assert_eq!(
        decision,
        DiagnosticsPopupWheelDecision {
            claimed: false,
            scroll_message: None,
            scroll_rows: None,
            mark_dirty: false,
        }
    );
}

#[test]
fn diagnostics_popup_wheel_claims_even_when_idle_over_popup() {
    let decision = DiagnosticsPopupWheelContext {
        popup_visible: true,
        pointer_over_popup: true,
        row_under_pointer: None,
        vertical_rows: 0,
        horizontal_px: 0.0,
    }
    .decide();
    assert!(decision.claimed);
    assert!(decision.scroll_message.is_none());
    assert!(decision.scroll_rows.is_none());
    assert!(!decision.mark_dirty);
}

#[test]
fn diagnostics_popup_wheel_scrolls_focused_message_inline() {
    let decision = DiagnosticsPopupWheelContext {
        popup_visible: true,
        pointer_over_popup: true,
        row_under_pointer: Some(4),
        vertical_rows: -2,
        horizontal_px: -18.0,
    }
    .decide();
    assert!(decision.claimed);
    assert_eq!(
        decision.scroll_message,
        Some(DiagnosticsMessageScroll {
            row_index: 4,
            horizontal_px: -18.0,
        })
    );
    assert_eq!(decision.scroll_rows, Some(-2));
    assert!(decision.mark_dirty);
}

#[test]
fn diagnostics_popup_wheel_skips_inline_scroll_under_threshold() {
    let decision = DiagnosticsPopupWheelContext {
        popup_visible: true,
        pointer_over_popup: true,
        row_under_pointer: Some(1),
        vertical_rows: 1,
        horizontal_px: 0.25,
    }
    .decide();
    assert!(decision.claimed);
    assert!(decision.scroll_message.is_none());
    assert_eq!(decision.scroll_rows, Some(1));
}

#[test]
fn block_content_top_picks_first_source_when_within_budget() {
    let pick = BlockContentTopPick {
        sources: &[42, 43, 44],
        row_is_empty: &[false, false, false],
        terminal_content_rows: 4,
        display_offset: 0,
        history_size: 10,
    };
    assert_eq!(pick.content_top_abs(), Some(42));
}

#[test]
fn block_content_top_shifts_when_overflow_exceeds_trailing_empties() {
    let pick = BlockContentTopPick {
        sources: &[10, 11, 12, 13, 14],
        row_is_empty: &[false, false, false, false, false],
        terminal_content_rows: 3,
        display_offset: 5,
        history_size: 50,
    };
    // overflow = 2, no trailing empties, not at top of history → skip 2 rows.
    assert_eq!(pick.content_top_abs(), Some(12));
}

#[test]
fn block_content_top_keeps_first_source_when_trailing_empties_absorb_overflow() {
    let pick = BlockContentTopPick {
        sources: &[10, 11, 12, 13, 14],
        row_is_empty: &[false, false, false, true, true],
        terminal_content_rows: 3,
        display_offset: 4,
        history_size: 50,
    };
    // overflow = 2, trailing empties = 2 → keep first source.
    assert_eq!(pick.content_top_abs(), Some(10));
}

#[test]
fn block_content_top_keeps_first_source_when_at_top_of_history() {
    let pick = BlockContentTopPick {
        sources: &[0, 1, 2, 3],
        row_is_empty: &[false, false, false, false],
        terminal_content_rows: 2,
        display_offset: 50,
        history_size: 50,
    };
    assert_eq!(pick.content_top_abs(), Some(0));
}

#[test]
fn drop_composer_prompt_row_strips_empty_prompt() {
    let mut rows: Vec<bool> = vec![false, true, false];
    let mut sources: Vec<usize> = vec![10, 11, 12];
    drop_composer_prompt_row(&mut rows, &mut sources, |row| *row, Some(11));
    assert_eq!(rows, vec![false, false]);
    assert_eq!(sources, vec![10, 12]);
}

#[test]
fn drop_composer_prompt_row_strips_non_empty_prompt_too() {
    let mut rows: Vec<bool> = vec![false, false, false];
    let mut sources: Vec<usize> = vec![10, 11, 12];
    drop_composer_prompt_row(&mut rows, &mut sources, |row| *row, Some(11));
    assert_eq!(rows, vec![false, false]);
    assert_eq!(sources, vec![10, 12]);
}

#[test]
fn drop_composer_prompt_row_no_op_without_prompt() {
    let mut rows: Vec<bool> = vec![true];
    let mut sources: Vec<usize> = vec![5];
    drop_composer_prompt_row(&mut rows, &mut sources, |row| *row, None);
    assert_eq!(rows, vec![true]);
    assert_eq!(sources, vec![5]);
}

#[test]
fn terminal_mouse_mode_emit_counts_vertical_and_horizontal_steps() {
    let report = TerminalMouseModeWheelReport {
        accumulated_x: -80.0,
        accumulated_y: 50.0,
        delta_x: -3.0,
        delta_y: 9.0,
        width: 12.0,
        height: 16.0,
    }
    .emit();
    assert_eq!(
        report,
        TerminalMouseModeWheelEmit {
            vertical_code: MOUSE_WHEEL_UP,
            vertical_count: 3,
            horizontal_code: MOUSE_WHEEL_RIGHT,
            horizontal_count: 6,
        }
    );
}

#[test]
fn terminal_mouse_mode_emit_picks_down_and_left_for_inverse_deltas() {
    let report = TerminalMouseModeWheelReport {
        accumulated_x: 30.0,
        accumulated_y: -40.0,
        delta_x: 5.0,
        delta_y: -2.0,
        width: 10.0,
        height: 20.0,
    }
    .emit();
    assert_eq!(report.vertical_code, MOUSE_WHEEL_DOWN);
    assert_eq!(report.horizontal_code, MOUSE_WHEEL_LEFT);
    assert_eq!(report.vertical_count, 2);
    assert_eq!(report.horizontal_count, 3);
}

#[test]
fn terminal_alternate_scroll_builds_csi_sequence_for_each_step() {
    let bytes = TerminalAlternateScrollCsi {
        accumulated_x: -40.0,
        accumulated_y: 60.0,
        delta_x: -3.0,
        delta_y: 5.0,
        width: 10.0,
        height: 20.0,
    }
    .build();
    // 60/20 = 3 lines up (A); 40/10 = 4 columns right (C).
    assert_eq!(bytes.line_count, 3);
    assert_eq!(bytes.column_count, 4);
    let expected: Vec<u8> = [
        0x1b, b'O', b'A', 0x1b, b'O', b'A', 0x1b, b'O', b'A', 0x1b, b'O', b'C', 0x1b,
        b'O', b'C', 0x1b, b'O', b'C', 0x1b, b'O', b'C',
    ]
    .to_vec();
    assert_eq!(bytes.bytes, expected);
}

#[test]
fn terminal_alternate_scroll_returns_empty_bytes_when_nothing_to_send() {
    let bytes = TerminalAlternateScrollCsi {
        accumulated_x: 1.0,
        accumulated_y: 1.0,
        delta_x: 1.0,
        delta_y: 1.0,
        width: 80.0,
        height: 80.0,
    }
    .build();
    assert_eq!(bytes.line_count, 0);
    assert_eq!(bytes.column_count, 0);
    assert!(bytes.bytes.is_empty());
}

// -----------------------------------------------------------------
// Editor key dispatch plan tests
// -----------------------------------------------------------------

fn key_ctx(mode: EditorModeClass) -> EditorKeyDispatchContext {
    EditorKeyDispatchContext {
        mode,
        editor_present: true,
        leader_pending: false,
        leader_age_ms: 0,
        finder_leader_pending: false,
        finder_leader_age_ms: 0,
        leader_timeout_ms: 750,
    }
}

#[test]
fn editor_key_esc_clears_search_highlight_when_editor_present() {
    let plan = EditorKeyDispatchPlan::classify("<Esc>", key_ctx(EditorModeClass::Normal));
    assert_eq!(plan, EditorKeyDispatchPlan::ClearSearchHighlightThenSend);
}

#[test]
fn editor_key_esc_passes_through_when_no_editor() {
    let mut ctx = key_ctx(EditorModeClass::Normal);
    ctx.editor_present = false;
    let plan = EditorKeyDispatchPlan::classify("<Esc>", ctx);
    assert_eq!(
        plan,
        EditorKeyDispatchPlan::PassThrough {
            notation: "<Esc>".to_string()
        }
    );
}

#[test]
fn editor_key_tab_cycles_buffers_in_normal_mode() {
    assert_eq!(
        EditorKeyDispatchPlan::classify("<Tab>", key_ctx(EditorModeClass::Normal)),
        EditorKeyDispatchPlan::BufferCycle { next: true }
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("<S-Tab>", key_ctx(EditorModeClass::Normal)),
        EditorKeyDispatchPlan::BufferCycle { next: false }
    );
}

#[test]
fn editor_key_tab_passes_through_in_insert_mode() {
    assert_eq!(
        EditorKeyDispatchPlan::classify("<Tab>", key_ctx(EditorModeClass::Insert)),
        EditorKeyDispatchPlan::PassThrough {
            notation: "<Tab>".to_string()
        }
    );
}

#[test]
fn editor_key_glyph_opens_palette_in_normal_mode() {
    assert_eq!(
        EditorKeyDispatchPlan::classify(":", key_ctx(EditorModeClass::Normal)),
        EditorKeyDispatchPlan::OpenCommandPalette
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("/", key_ctx(EditorModeClass::Normal)),
        EditorKeyDispatchPlan::OpenSearchPalette
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("?", key_ctx(EditorModeClass::Normal)),
        EditorKeyDispatchPlan::OpenSearchPaletteBackward
    );
}

#[test]
fn editor_key_glyph_passes_through_in_visual_mode() {
    // Visual mode passes `:` through so `:'<,'>` still works natively.
    assert_eq!(
        EditorKeyDispatchPlan::classify(":", key_ctx(EditorModeClass::Visual)),
        EditorKeyDispatchPlan::PassThrough {
            notation: ":".to_string()
        }
    );
}

#[test]
fn editor_key_question_mark_passes_through_in_insert_mode() {
    assert_eq!(
        EditorKeyDispatchPlan::classify("?", key_ctx(EditorModeClass::Insert)),
        EditorKeyDispatchPlan::PassThrough {
            notation: "?".to_string()
        }
    );
}

#[test]
fn editor_key_space_starts_leader_in_normal_mode() {
    assert_eq!(
        EditorKeyDispatchPlan::classify("<Space>", key_ctx(EditorModeClass::Normal)),
        EditorKeyDispatchPlan::StartLeader
    );
}

#[test]
fn editor_key_space_passes_through_in_insert_mode() {
    assert_eq!(
        EditorKeyDispatchPlan::classify("<Space>", key_ctx(EditorModeClass::Insert)),
        EditorKeyDispatchPlan::PassThrough {
            notation: "<Space>".to_string()
        }
    );
}

#[test]
fn editor_key_leader_second_key_matches_e_x_h_v_f() {
    let mut ctx = key_ctx(EditorModeClass::Normal);
    ctx.leader_pending = true;
    ctx.leader_age_ms = 100;

    assert_eq!(
        EditorKeyDispatchPlan::classify("e", ctx),
        EditorKeyDispatchPlan::LeaderToggleFileTree
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("x", ctx),
        EditorKeyDispatchPlan::LeaderCloseFocusedBuffer
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("h", ctx),
        EditorKeyDispatchPlan::LeaderSplitDown
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("v", ctx),
        EditorKeyDispatchPlan::LeaderSplitRight
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("f", ctx),
        EditorKeyDispatchPlan::LeaderStartFinder
    );
}

#[test]
fn editor_key_leader_unknown_key_flushes_and_sends() {
    let mut ctx = key_ctx(EditorModeClass::Normal);
    ctx.leader_pending = true;
    ctx.leader_age_ms = 50;
    assert_eq!(
        EditorKeyDispatchPlan::classify("k", ctx),
        EditorKeyDispatchPlan::LeaderFlushAndSend {
            notation: "k".to_string()
        }
    );
}

#[test]
fn editor_key_finder_leader_routes_f_and_w() {
    let mut ctx = key_ctx(EditorModeClass::Normal);
    ctx.finder_leader_pending = true;
    ctx.finder_leader_age_ms = 100;
    assert_eq!(
        EditorKeyDispatchPlan::classify("f", ctx),
        EditorKeyDispatchPlan::LeaderFinderFiles
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("w", ctx),
        EditorKeyDispatchPlan::LeaderFinderGrep
    );
    assert_eq!(
        EditorKeyDispatchPlan::classify("q", ctx),
        EditorKeyDispatchPlan::FinderLeaderFlushAndSend {
            notation: "q".to_string()
        }
    );
}

#[test]
fn editor_key_stale_leader_flushes_before_any_dispatch() {
    let mut ctx = key_ctx(EditorModeClass::Normal);
    ctx.leader_pending = true;
    ctx.leader_age_ms = 1000;
    ctx.leader_timeout_ms = 750;
    // Even though "e" would match leader, the stale flush wins.
    assert_eq!(
        EditorKeyDispatchPlan::classify("e", ctx),
        EditorKeyDispatchPlan::FlushStaleLeaderSpace
    );
}

#[test]
fn editor_key_plain_key_passes_through() {
    assert_eq!(
        EditorKeyDispatchPlan::classify("j", key_ctx(EditorModeClass::Normal)),
        EditorKeyDispatchPlan::PassThrough {
            notation: "j".to_string()
        }
    );
}

// -----------------------------------------------------------------
// Ex command intercept tests
// -----------------------------------------------------------------

#[test]
fn parse_ex_command_strips_colon_and_splits_head_tail() {
    assert_eq!(
        parse_ex_command(":theme dark"),
        Some(("theme".to_string(), "dark".to_string()))
    );
    assert_eq!(
        parse_ex_command(" :  q  "),
        Some(("q".to_string(), "".to_string()))
    );
    assert_eq!(parse_ex_command(""), None);
    assert_eq!(parse_ex_command(":"), None);
}

#[test]
fn markdown_ex_command_classifies_jumps_save_and_close() {
    assert_eq!(
        MarkdownExCommandPlan::classify("$"),
        MarkdownExCommandPlan::JumpToLastLine
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("42"),
        MarkdownExCommandPlan::JumpToLine(42)
    );
    // 0 saturates up to 1 (vim convention)
    assert_eq!(
        MarkdownExCommandPlan::classify("0"),
        MarkdownExCommandPlan::JumpToLine(1)
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("w"),
        MarkdownExCommandPlan::Save
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("wq"),
        MarkdownExCommandPlan::SaveAndCloseFocusedBuffer
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("runcell"),
        MarkdownExCommandPlan::RunNotebookCell
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("run-all"),
        MarkdownExCommandPlan::RunAllNotebookCells
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("runbelow"),
        MarkdownExCommandPlan::RunNotebookCellAndBelow
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("insert-code-above"),
        MarkdownExCommandPlan::InsertNotebookCodeCellAbove
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("insertcode"),
        MarkdownExCommandPlan::InsertNotebookCodeCellBelow
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("insert-markdown-above"),
        MarkdownExCommandPlan::InsertNotebookMarkdownCellAbove
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("insertmarkdown"),
        MarkdownExCommandPlan::InsertNotebookMarkdownCellBelow
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("deletecell"),
        MarkdownExCommandPlan::DeleteNotebookCell
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("move-cell-up"),
        MarkdownExCommandPlan::MoveNotebookCellUp
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("movecelldown"),
        MarkdownExCommandPlan::MoveNotebookCellDown
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("interruptkernel"),
        MarkdownExCommandPlan::InterruptNotebookKernel
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("clearoutputs"),
        MarkdownExCommandPlan::ClearNotebookOutputs
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("clearoutput"),
        MarkdownExCommandPlan::ClearNotebookCellOutput
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("restart-kernel"),
        MarkdownExCommandPlan::RestartNotebookKernel
    );
    assert_eq!(
        MarkdownExCommandPlan::classify("foo"),
        MarkdownExCommandPlan::PassThrough
    );
}

#[test]
fn global_ex_command_routes_pickers_and_finders() {
    assert_eq!(
        GlobalExCommandPlan::classify("shaderpicker", ""),
        GlobalExCommandPlan::Shaders
    );
    assert_eq!(
        GlobalExCommandPlan::classify("shader", "picker"),
        GlobalExCommandPlan::Shaders
    );
    assert_eq!(
        GlobalExCommandPlan::classify("shaders", ""),
        GlobalExCommandPlan::Shaders
    );
    assert_eq!(
        GlobalExCommandPlan::classify("theme", ""),
        GlobalExCommandPlan::ThemePicker
    );
    assert_eq!(
        GlobalExCommandPlan::classify("theme", "tokyo-night"),
        GlobalExCommandPlan::ApplyTheme("tokyo-night".to_string())
    );
    assert_eq!(
        GlobalExCommandPlan::classify("buffers", ""),
        GlobalExCommandPlan::OpenBuffersPicker
    );
    assert_eq!(
        GlobalExCommandPlan::classify("search", "files"),
        GlobalExCommandPlan::OpenFinderFiles
    );
    assert_eq!(
        GlobalExCommandPlan::classify("search", "Words"),
        GlobalExCommandPlan::OpenFinderGrep
    );
}

#[test]
fn global_ex_command_minimap_set_and_toggle() {
    assert_eq!(
        GlobalExCommandPlan::classify("minimap", "on"),
        GlobalExCommandPlan::SetMinimap(Some(true))
    );
    assert_eq!(
        GlobalExCommandPlan::classify("minimap", "off"),
        GlobalExCommandPlan::SetMinimap(Some(false))
    );
    assert_eq!(
        GlobalExCommandPlan::classify("minimap", ""),
        GlobalExCommandPlan::SetMinimap(None)
    );
    assert_eq!(
        GlobalExCommandPlan::classify("toggleminimap", ""),
        GlobalExCommandPlan::ToggleMinimap
    );
}

#[test]
fn global_ex_command_routes_agent_terminals() {
    assert_eq!(
        GlobalExCommandPlan::classify("claude", "--continue"),
        GlobalExCommandPlan::LaunchAgentTerminal {
            agent: AgentTag::Claude,
            tail: "--continue".to_string()
        }
    );
    assert_eq!(
        GlobalExCommandPlan::classify("opencode", ""),
        GlobalExCommandPlan::LaunchAgentTerminal {
            agent: AgentTag::OpenCode,
            tail: String::new()
        }
    );
    assert_eq!(
        GlobalExCommandPlan::classify("opencode-acp", "foo"),
        GlobalExCommandPlan::StartOpenCodeAcp {
            tail: "foo".to_string()
        }
    );
}

#[test]
fn global_ex_command_routes_splits_and_new_buffers() {
    assert_eq!(
        GlobalExCommandPlan::classify("split", ""),
        GlobalExCommandPlan::SplitDown
    );
    assert_eq!(
        GlobalExCommandPlan::classify("vsplit", ""),
        GlobalExCommandPlan::SplitRight
    );
    assert_eq!(
        GlobalExCommandPlan::classify("enew", ""),
        GlobalExCommandPlan::OpenEmptyBufferTab
    );
    assert_eq!(
        GlobalExCommandPlan::classify("tabnew", "src/main.rs"),
        GlobalExCommandPlan::OpenPathInEditor("src/main.rs".to_string())
    );
}

#[test]
fn global_ex_command_routes_quit_close_variants() {
    assert_eq!(
        GlobalExCommandPlan::classify("q", ""),
        GlobalExCommandPlan::CloseFocusedBufferTab
    );
    assert_eq!(
        GlobalExCommandPlan::classify("close!", ""),
        GlobalExCommandPlan::CloseFocusedBufferTab
    );
    assert_eq!(
        GlobalExCommandPlan::classify("wq", ""),
        GlobalExCommandPlan::WriteAndCloseFocusedBuffer
    );
    assert_eq!(
        GlobalExCommandPlan::classify("qa", ""),
        GlobalExCommandPlan::CloseAllBuffersInFocusedPaneOrWorkspace
    );
    assert_eq!(
        GlobalExCommandPlan::classify("wqa", ""),
        GlobalExCommandPlan::WriteAllAndCloseAllBuffers
    );
    assert_eq!(
        GlobalExCommandPlan::classify("not-a-command", ""),
        GlobalExCommandPlan::PassThrough
    );
}

// -----------------------------------------------------------------
// Scrollbar click plan tests
// -----------------------------------------------------------------

#[test]
fn scrollbar_click_plan_swallows_empty_band_for_editor_pane() {
    let plan = ScrollbarClickPlan::classify(ScrollbarClickContext {
        pane_kind: ScrollbarPaneKind::Editor,
        band_contains_pointer: true,
        has_scroll_state: false,
        hit_scrollbar_geometry: false,
        grabbed_thumb: false,
    });
    assert_eq!(plan, ScrollbarClickPlan::SwallowEmptyBand);
}

#[test]
fn scrollbar_click_plan_ignores_markdown_pane_empty_band() {
    // Markdown / agent / tags panes have their own click handlers;
    // the global scrollbar band must NOT swallow a click there.
    let plan = ScrollbarClickPlan::classify(ScrollbarClickContext {
        pane_kind: ScrollbarPaneKind::Markdown,
        band_contains_pointer: true,
        has_scroll_state: false,
        hit_scrollbar_geometry: false,
        grabbed_thumb: false,
    });
    assert_eq!(plan, ScrollbarClickPlan::Ignore);
}

#[test]
fn scrollbar_click_plan_starts_drag_on_thumb_and_jumps_on_track() {
    assert_eq!(
        ScrollbarClickPlan::classify(ScrollbarClickContext {
            pane_kind: ScrollbarPaneKind::Editor,
            band_contains_pointer: true,
            has_scroll_state: true,
            hit_scrollbar_geometry: true,
            grabbed_thumb: true,
        }),
        ScrollbarClickPlan::StartDragOnThumb
    );
    assert_eq!(
        ScrollbarClickPlan::classify(ScrollbarClickContext {
            pane_kind: ScrollbarPaneKind::Editor,
            band_contains_pointer: true,
            has_scroll_state: true,
            hit_scrollbar_geometry: true,
            grabbed_thumb: false,
        }),
        ScrollbarClickPlan::StartDragWithJumpToTrack
    );
}

#[test]
fn scrollbar_drag_target_prefers_drag_state_id_then_panel_then_current() {
    assert_eq!(scrollbar_drag_target_rich_text_id(Some(7), Some(3), 1), 7);
    assert_eq!(scrollbar_drag_target_rich_text_id(None, Some(3), 1), 3);
    assert_eq!(scrollbar_drag_target_rich_text_id(None, None, 1), 1);
}

// -----------------------------------------------------------------
// B5 wiring regression locks: cover the behavioural seams the
// desktop host relies on so a future shared-policy refactor can't
// silently shift the dispatch table out from under it.
// -----------------------------------------------------------------

#[test]
fn editor_key_stale_finder_leader_takes_precedence_over_finder_match() {
    // Even though "f" would otherwise route into `LeaderFinderFiles`,
    // a stale finder-leader flush must fire first so the held
    // `<Space>f` re-enters dispatch with a clean slate.
    let mut ctx = key_ctx(EditorModeClass::Normal);
    ctx.finder_leader_pending = true;
    ctx.finder_leader_age_ms = 1500;
    ctx.leader_timeout_ms = 750;
    assert_eq!(
        EditorKeyDispatchPlan::classify("f", ctx),
        EditorKeyDispatchPlan::FlushStaleFinderLeader
    );
}

#[test]
fn editor_key_esc_without_editor_present_does_not_clear_search() {
    // The plan only requests an hlsearch clear when an editor is
    // actually focused; without one, `<Esc>` must pass through so
    // the host doesn't synthesize an Ex command into nothing.
    let mut ctx = key_ctx(EditorModeClass::Normal);
    ctx.editor_present = false;
    assert_eq!(
        EditorKeyDispatchPlan::classify("<Esc>", ctx),
        EditorKeyDispatchPlan::PassThrough {
            notation: "<Esc>".to_string()
        }
    );
}

#[test]
fn markdown_ex_command_saturates_zero_line_to_one() {
    // `:0` is invalid in vim — the markdown plan saturates so the
    // host's `jump_to_line(plan_line)` always receives a 1-indexed
    // value, matching the previous inline `line.max(1)` guard.
    assert_eq!(
        MarkdownExCommandPlan::classify("0"),
        MarkdownExCommandPlan::JumpToLine(1)
    );
}

#[test]
fn global_ex_command_routes_quit_and_close_synonyms_uniformly() {
    // The host collapses every `q`/`close` synonym into one
    // close-tab call. Locking the plan output here means a typo
    // like `quite!` keeps mapping to the same branch instead of
    // silently passing through to nvim and quitting the editor.
    for head in [
        "q", "q!", "quit", "quit!", "quite", "quite!", "close", "close!",
    ] {
        assert_eq!(
            GlobalExCommandPlan::classify(head, ""),
            GlobalExCommandPlan::CloseFocusedBufferTab,
            "head `{head}` should classify as CloseFocusedBufferTab",
        );
    }

    for head in ["qa", "qa!", "quitall", "quitall!", "qall", "qall!"] {
        assert_eq!(
            GlobalExCommandPlan::classify(head, ""),
            GlobalExCommandPlan::CloseAllBuffersInFocusedPaneOrWorkspace,
            "head `{head}` should classify as CloseAllBuffersInFocusedPaneOrWorkspace",
        );
    }
}

#[test]
fn scrollbar_click_plan_terminal_pane_swallows_empty_band() {
    // Terminal panes own the global scrollbar band just like
    // editor panes — empty-band clicks there must also be
    // swallowed so they don't fall through to the terminal's
    // mouse handler.
    let plan = ScrollbarClickPlan::classify(ScrollbarClickContext {
        pane_kind: ScrollbarPaneKind::Terminal,
        band_contains_pointer: true,
        has_scroll_state: false,
        hit_scrollbar_geometry: false,
        grabbed_thumb: false,
    });
    assert_eq!(plan, ScrollbarClickPlan::SwallowEmptyBand);
}

#[test]
fn scrollbar_click_plan_ignores_misses_outside_geometry_with_active_scroll() {
    // Even with an active scrollbar (history > visible), a click
    // that misses the bar geometry must be ignored — otherwise
    // we'd start a drag from nowhere.
    let plan = ScrollbarClickPlan::classify(ScrollbarClickContext {
        pane_kind: ScrollbarPaneKind::Editor,
        band_contains_pointer: false,
        has_scroll_state: true,
        hit_scrollbar_geometry: false,
        grabbed_thumb: false,
    });
    assert_eq!(plan, ScrollbarClickPlan::Ignore);
}
