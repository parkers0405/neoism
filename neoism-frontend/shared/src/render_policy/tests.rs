use super::*;

#[test]
fn editor_scroll_render_offset_uses_neovide_floor_split() {
    let cell = 20.0;

    assert_eq!(
        editor_scroll_render_offset(-1.0, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: -1,
            pixel_offset_y: 0.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset(-0.2, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: -1,
            pixel_offset_y: -16.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset(1.0, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: 1,
            pixel_offset_y: 0.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset(0.2, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: 0,
            pixel_offset_y: -4.0,
        }
    );
    // 0.125 lines * 39 px = 4.875 px, which the helper rounds
    // away from zero to keep the GPU uniform on whole pixels.
    assert_eq!(
        editor_scroll_render_offset(0.125, 0.0, 39.0, None),
        EditorScrollRenderOffset {
            source_line_offset: 0,
            pixel_offset_y: -5.0,
        }
    );
}

#[test]
fn editor_scroll_mutated_snapshot_split_mirrors_positive_offsets() {
    let cell = 20.0;

    assert_eq!(
        editor_scroll_render_offset_for_mutated_snapshot(-0.2, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: -1,
            pixel_offset_y: -16.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset_for_mutated_snapshot(0.2, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: 1,
            pixel_offset_y: 16.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset_for_mutated_snapshot(1.0, 0.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: 1,
            pixel_offset_y: 0.0,
        }
    );
}

#[test]
fn editor_scroll_elastic_does_not_change_source_row() {
    let cell = 20.0;

    assert_eq!(
        editor_scroll_render_offset(-1.0, 7.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: -1,
            pixel_offset_y: 7.0,
        }
    );
    assert_eq!(
        editor_scroll_render_offset(0.0, 7.0, cell, None),
        EditorScrollRenderOffset {
            source_line_offset: 0,
            pixel_offset_y: 7.0,
        }
    );
}

#[test]
fn editor_scroll_render_offset_ignores_invalid_cell_height() {
    assert_eq!(
        editor_scroll_render_offset(1.0, 7.0, 0.0, None),
        EditorScrollRenderOffset::default()
    );
    assert_eq!(
        editor_scroll_render_offset(1.0, 7.0, f32::NAN, None),
        EditorScrollRenderOffset::default()
    );
    assert_eq!(
        editor_scroll_render_offset(f32::INFINITY, 0.0, 20.0, None),
        EditorScrollRenderOffset::default()
    );
}

#[test]
fn editor_cursor_row_inverts_scroll_source_lookup() {
    let cursor_row = 10;
    for source_line_offset in [-3, -1, 0, 1, 3] {
        let output_row = editor_cursor_output_row(cursor_row, source_line_offset);
        assert_eq!(output_row + source_line_offset, cursor_row);
    }
}

#[test]
fn editor_cursor_grid_row_applies_hidden_buffer_and_clamps() {
    assert_eq!(editor_cursor_grid_row(10, 3, 30, 1, 1), 8);
    assert_eq!(editor_cursor_grid_row(0, 5, 30, 1, 1), 0);
    assert_eq!(editor_cursor_grid_row(80, 0, 30, 1, 1), 31);
}

#[test]
fn editor_source_row_for_output_matches_cursor_inverse_mapping() {
    for source_line_offset in [-4, -1, 0, 2, 5] {
        for output_row in [-1, 0, 10, 24] {
            let source = editor_source_row_for_output(output_row, source_line_offset);
            assert_eq!(
                editor_output_row_for_source(source, source_line_offset),
                output_row
            );
        }
    }
}

// ------------------------------------------------------------
// trail-cursor policy tests
// ------------------------------------------------------------

fn geom(panel: [f32; 4], margin_top: f32, cell_w: f32, cell_h: f32) -> GridPanelGeometry {
    GridPanelGeometry {
        panel_rect: panel,
        scaled_margin: ScaledMargin {
            top: margin_top,
            left: 4.0,
            right: 2.0,
            bottom: 0.0,
        },
        cell_width: cell_w,
        cell_height: cell_h,
        columns: 80,
    }
}

#[test]
fn trail_cursor_terminal_pane_uses_raw_row_no_scroll() {
    let plan = terminal_grid_trail_cursor_destination(TrailCursorPlanInput {
        geometry: geom([10.0, 20.0, 800.0, 600.0], 8.0, 10.0, 20.0),
        cursor_row: 5,
        cursor_col: 3,
        visible_rows: 30.0,
        editor_scroll: None,
        last_editor_trail_cursor_cell: None,
        rich_text_id: 0,
    });
    // origin_x = 10 + 4 = 14; pane_top = 20 + 8 = 28.
    assert_eq!(plan.x, 14.0 + 3.0 * 10.0);
    assert_eq!(plan.y, 28.0 + 5.0 * 20.0);
    assert_eq!(plan.width, 10.0);
    assert_eq!(plan.height, 20.0);
    assert!(!plan.no_jump);
    assert_eq!(plan.next_last_cell, None);
}

#[test]
fn trail_cursor_clamps_to_pane_top_when_row_zero_and_negative_scroll() {
    // A large negative spring residual would otherwise push the
    // destination above the pane top — clamp should snap it back.
    let plan = terminal_grid_trail_cursor_destination(TrailCursorPlanInput {
        geometry: geom([0.0, 0.0, 800.0, 600.0], 0.0, 10.0, 20.0),
        cursor_row: 0,
        cursor_col: 0,
        visible_rows: 30.0,
        editor_scroll: Some(EditorScrollState {
            scroll_position_lines: -5.0,
            elastic_offset_y: -200.0,
            previous_source_line_offset: None,
        }),
        last_editor_trail_cursor_cell: None,
        rich_text_id: 42,
    });
    assert_eq!(plan.y, 0.0); // clamped to pane_top
}

#[test]
fn trail_cursor_clamps_to_last_visible_row_when_spring_pushes_below() {
    // visible_rows=4, cell_h=20 -> pane bottom = pane_top + 80.
    // trail_top_max = bottom - cell_h = 60. A huge positive
    // spring offset must not paint past row index 3.
    let plan = terminal_grid_trail_cursor_destination(TrailCursorPlanInput {
        geometry: geom([0.0, 0.0, 800.0, 600.0], 0.0, 10.0, 20.0),
        cursor_row: 50,
        cursor_col: 0,
        visible_rows: 4.0,
        editor_scroll: Some(EditorScrollState {
            scroll_position_lines: 10.0,
            elastic_offset_y: 0.0,
            previous_source_line_offset: None,
        }),
        last_editor_trail_cursor_cell: None,
        rich_text_id: 7,
    });
    assert_eq!(plan.y, 60.0);
}

#[test]
fn trail_cursor_emits_no_jump_when_cell_unchanged() {
    let geom = geom([0.0, 0.0, 800.0, 600.0], 0.0, 10.0, 20.0);
    let scroll = EditorScrollState {
        scroll_position_lines: 0.0,
        elastic_offset_y: 0.0,
        previous_source_line_offset: None,
    };
    let cell = (7usize, 5usize, 9usize);
    let plan = terminal_grid_trail_cursor_destination(TrailCursorPlanInput {
        geometry: geom,
        cursor_row: 5,
        cursor_col: 9,
        visible_rows: 30.0,
        editor_scroll: Some(scroll),
        last_editor_trail_cursor_cell: Some(cell),
        rich_text_id: 7,
    });
    assert!(plan.no_jump);
    assert_eq!(plan.next_last_cell, Some(cell));
}

#[test]
fn trail_cursor_jumps_when_cell_differs() {
    let geom = geom([0.0, 0.0, 800.0, 600.0], 0.0, 10.0, 20.0);
    let scroll = EditorScrollState {
        scroll_position_lines: 0.0,
        elastic_offset_y: 0.0,
        previous_source_line_offset: None,
    };
    let plan = terminal_grid_trail_cursor_destination(TrailCursorPlanInput {
        geometry: geom,
        cursor_row: 6,
        cursor_col: 9,
        visible_rows: 30.0,
        editor_scroll: Some(scroll),
        last_editor_trail_cursor_cell: Some((7, 5, 9)),
        rich_text_id: 7,
    });
    assert!(!plan.no_jump);
    assert_eq!(plan.next_last_cell, Some((7, 6, 9)));
}

#[test]
fn trail_cursor_terminal_pane_never_remembers_cell() {
    let plan = terminal_grid_trail_cursor_destination(TrailCursorPlanInput {
        geometry: geom([0.0, 0.0, 800.0, 600.0], 0.0, 10.0, 20.0),
        cursor_row: 5,
        cursor_col: 9,
        visible_rows: 30.0,
        editor_scroll: None,
        last_editor_trail_cursor_cell: Some((7, 5, 9)),
        rich_text_id: 7,
    });
    // Non-editor panes must NOT echo back a cell — the host
    // clears `last_editor_trail_cursor_cell` to `None` in that
    // case, so a later editor focus is treated as a fresh jump.
    assert!(!plan.no_jump);
    assert_eq!(plan.next_last_cell, None);
}

#[test]
fn trail_cursor_zero_visible_rows_pins_to_pane_top() {
    // Edge case: terminal reports 0 rows momentarily mid-resize.
    // pane_bottom == pane_top, trail_top_max = pane_top.
    let plan = terminal_grid_trail_cursor_destination(TrailCursorPlanInput {
        geometry: geom([0.0, 100.0, 800.0, 600.0], 0.0, 10.0, 20.0),
        cursor_row: 5,
        cursor_col: 0,
        visible_rows: 0.0,
        editor_scroll: None,
        last_editor_trail_cursor_cell: None,
        rich_text_id: 0,
    });
    assert_eq!(plan.y, 100.0);
}

// -----------------------------------------------------------------
// Block-header chrome layout tests.
// -----------------------------------------------------------------

fn sample_block_header_grid() -> GridPanelGeometry {
    GridPanelGeometry {
        panel_rect: [100.0, 200.0, 800.0, 600.0],
        scaled_margin: ScaledMargin {
            top: 20.0,
            left: 10.0,
            right: 10.0,
            bottom: 30.0,
        },
        cell_width: 20.0,
        cell_height: 40.0,
        columns: 80,
    }
}

#[test]
fn block_header_panel_geometry_divides_by_scale() {
    let geom = block_header_panel_geometry(BlockHeaderPanelGeometryInput {
        grid: sample_block_header_grid(),
        terminal_scroll_offset_phys: 0.0,
        terminal_content_rows: 24,
        font_px_phys: 32.0,
        scale_factor: 2.0,
    });
    // (panel_rect[0] + margin.left) / 2.0 = (100 + 10) / 2 = 55
    assert_eq!(geom.panel_left_logical, 55.0);
    // (panel_rect[1] + margin.top) / 2.0 = (200 + 20) / 2 = 110
    // No scroll residual -> panel_top_logical == raw.
    assert_eq!(geom.panel_top_logical, 110.0);
    // cell w/h divided by scale, clamped to >= 1.0.
    assert_eq!(geom.cell_w_logical, 10.0);
    assert_eq!(geom.cell_h_logical, 20.0);
    // panel_right = panel_left + columns * cell_w = 55 + 80*10 = 855
    assert_eq!(geom.panel_right_logical, 855.0);
    // content_clip width = panel_right - panel_left = 800.
    assert_eq!(geom.content_clip_logical[2], 800.0);
    // content_clip height = terminal_content_rows * cell_h_logical = 24 * 20 = 480.
    assert_eq!(geom.content_clip_logical[3], 480.0);
    // font / 2.0 = 16.
    assert_eq!(geom.font_size_logical, 16.0);
}

#[test]
fn block_header_panel_geometry_shifts_with_terminal_scroll_residual() {
    // panel_top_logical should shift by scroll_offset / scale, but
    // content_clip stays anchored to the static viewport.
    let geom = block_header_panel_geometry(BlockHeaderPanelGeometryInput {
        grid: sample_block_header_grid(),
        terminal_scroll_offset_phys: 80.0,
        terminal_content_rows: 24,
        font_px_phys: 32.0,
        scale_factor: 2.0,
    });
    assert_eq!(geom.panel_top_logical, 110.0 + 40.0);
    assert_eq!(geom.content_clip_logical[1], 110.0);
}

#[test]
fn block_header_panel_geometry_guards_against_zero_scale() {
    let geom = block_header_panel_geometry(BlockHeaderPanelGeometryInput {
        grid: sample_block_header_grid(),
        terminal_scroll_offset_phys: 0.0,
        terminal_content_rows: 24,
        font_px_phys: 32.0,
        scale_factor: 0.0,
    });
    // Falls back to 1.0 -> no division by zero, no NaN spread.
    assert!(geom.panel_left_logical.is_finite());
    assert!(geom.panel_top_logical.is_finite());
    assert_eq!(geom.cell_w_logical, 20.0);
}

#[test]
fn block_header_panel_geometry_clamps_min_cell_size() {
    let mut g = sample_block_header_grid();
    g.cell_width = 0.0;
    g.cell_height = 0.0;
    let geom = block_header_panel_geometry(BlockHeaderPanelGeometryInput {
        grid: g,
        terminal_scroll_offset_phys: 0.0,
        terminal_content_rows: 24,
        font_px_phys: 0.0,
        scale_factor: 2.0,
    });
    assert_eq!(geom.cell_w_logical, 1.0);
    assert_eq!(geom.cell_h_logical, 1.0);
    assert_eq!(geom.font_size_logical, 1.0);
}

#[test]
fn block_header_row_metrics_centers_text_in_row() {
    let geom = block_header_panel_geometry(BlockHeaderPanelGeometryInput {
        grid: sample_block_header_grid(),
        terminal_scroll_offset_phys: 0.0,
        terminal_content_rows: 24,
        font_px_phys: 32.0,
        scale_factor: 2.0,
    });
    let metrics = block_header_row_metrics(geom, 3);
    // row_top = panel_top + 3 * cell_h = 110 + 60 = 170.
    assert_eq!(metrics.row_top, 170.0);
    // font_size 16 fits inside cell_h 20 -> clamp leaves it at 16.
    assert_eq!(metrics.clamped_font_size, 16.0);
    // text_y = row_top + (20 - 16) * 0.5 - 1 = 170 + 2 - 1 = 171.
    assert_eq!(metrics.text_y, 171.0);
    // action_reserve = min(cell_h * 3.4 + 24, width * 0.35)
    //                = min(20 * 3.4 + 24, 800 * 0.35)
    //                = min(92, 280) = 92.
    assert_eq!(metrics.action_reserve, 92.0);
}

#[test]
fn block_header_row_metrics_clamps_font_size_to_cell_height() {
    // Tiny cell forces the glyph to shrink to the 8px floor.
    let geom = BlockHeaderPanelGeometry {
        panel_top_logical: 0.0,
        panel_left_logical: 0.0,
        panel_right_logical: 200.0,
        cell_w_logical: 2.0,
        cell_h_logical: 4.0,
        font_size_logical: 64.0,
        content_clip_logical: [0.0, 0.0, 200.0, 96.0],
    };
    let m = block_header_row_metrics(geom, 0);
    // 8 is the floor; cell_h (4) is below 8, so clamp picks max(cell_h, 8) = 8.
    assert_eq!(m.clamped_font_size, 8.0);
}

#[test]
fn block_header_row_metrics_caps_action_reserve_at_third_of_width() {
    // Wide cell (200x200) -> 2.2*200+20=460 vs width*0.35.
    let geom = BlockHeaderPanelGeometry {
        panel_top_logical: 0.0,
        panel_left_logical: 0.0,
        panel_right_logical: 800.0,
        cell_w_logical: 200.0,
        cell_h_logical: 200.0,
        font_size_logical: 20.0,
        content_clip_logical: [0.0, 0.0, 800.0, 200.0],
    };
    let m = block_header_row_metrics(geom, 0);
    assert_eq!(m.action_reserve, 800.0 * 0.35);
}

#[test]
fn block_hover_icon_layout_places_filter_on_right_copy_left() {
    let layout = block_hover_icon_layout(BlockHoverIconLayoutInput {
        panel_top_logical: 100.0,
        panel_right_logical: 800.0,
        cell_h_logical: 20.0,
        anchor_display_row: 1,
    });
    // icon_size = 20 * 0.85 = 17.
    // icon_right = 800 - 8 = 792.
    // filter_rect.x = 792 - 17 = 775; w = 17.
    assert!((layout.filter_rect[0] - 775.0).abs() < 1e-3);
    assert!((layout.filter_rect[2] - 17.0).abs() < 1e-3);
    // favorite_rect sits 17 + 6 px to the left of the filter.
    assert!((layout.favorite_rect[0] - (775.0 - 17.0 - 6.0)).abs() < 1e-3);
    // copy_rect sits 17 + 6 px to the left of favorite.
    assert!((layout.copy_rect[0] - (775.0 - 17.0 * 2.0 - 6.0 * 2.0)).abs() < 1e-3);
    // Union covers both rects horizontally.
    let union_right = layout.icon_union[0] + layout.icon_union[2];
    let filter_right = layout.filter_rect[0] + layout.filter_rect[2];
    assert!((union_right - filter_right).abs() < 1e-3);
    // Icon Y centers a 17px box inside a 20px row (1.5px above row top).
    let icon_y = 100.0 + 20.0 + (20.0 - 17.0) * 0.5;
    assert!((layout.filter_rect[1] - icon_y).abs() < 1e-3);
    assert!((layout.favorite_rect[1] - icon_y).abs() < 1e-3);
    assert!((layout.copy_rect[1] - icon_y).abs() < 1e-3);
}

#[test]
fn block_hover_icon_anchor_row_clamps_to_span_end() {
    // command row at chrome offset 1 inside a 2-row span starting at 5.
    assert_eq!(block_hover_icon_anchor_row(5, 7, 0, 1), 6);
    // partial span where first_chrome_row is the COMMAND row -> 0 offset.
    assert_eq!(block_hover_icon_anchor_row(10, 11, 1, 1), 10);
    // Offset would land past the span end -> clamp to end - 1.
    assert_eq!(block_hover_icon_anchor_row(0, 2, 0, 5), 1);
}

#[test]
fn block_status_color_token_picks_yellow_for_running() {
    use crate::terminal_blocks::BlockStatusKind;
    assert_eq!(
        block_status_color_token(BlockStatusKind::Running),
        BlockStatusColorToken::Yellow,
    );
    assert_eq!(
        block_status_color_token(BlockStatusKind::Ok),
        BlockStatusColorToken::Green,
    );
    assert_eq!(
        block_status_color_token(BlockStatusKind::Error(1)),
        BlockStatusColorToken::Red,
    );
}

#[test]
fn block_status_glyph_running_returns_none_for_loader() {
    use crate::terminal_blocks::BlockStatusKind;
    assert_eq!(block_status_glyph(BlockStatusKind::Running), None);
    assert_eq!(block_status_glyph(BlockStatusKind::Ok), Some("\u{2022}"));
    assert_eq!(
        block_status_glyph(BlockStatusKind::Error(127)),
        Some("\u{2022}"),
    );
}

// -----------------------------------------------------------------
// Animation timing tests.
// -----------------------------------------------------------------

#[test]
fn animation_phase_wraps_seconds_at_ten_thousand() {
    // 9999 keeps its seconds intact, 10_000 rolls to 0, 10_001 -> 1.
    assert_eq!(animation_phase_from_unix_secs(9_999, 0), 9_999.0);
    assert_eq!(animation_phase_from_unix_secs(10_000, 0), 0.0);
    assert_eq!(animation_phase_from_unix_secs(10_001, 0), 1.0);
}

#[test]
fn animation_phase_adds_subsecond_fraction() {
    // 500_000_000 ns = 0.5s.
    let phase = animation_phase_from_unix_secs(5, 500_000_000);
    assert!((phase - 5.5).abs() < 1e-6);
}

#[test]
fn loader_orbit_position_walks_each_side_of_square() {
    let half = 10.0;
    // Start at top-left corner.
    let (x, y) = loader_orbit_position(0.0, half);
    assert!((x - (-half)).abs() < 1e-4);
    assert!((y - (-half)).abs() < 1e-4);
    // Quarter lap: top-right corner.
    let (x, y) = loader_orbit_position(0.25, half);
    assert!((x - half).abs() < 1e-4);
    assert!((y - (-half)).abs() < 1e-4);
    // Halfway: bottom-right corner.
    let (x, y) = loader_orbit_position(0.5, half);
    assert!((x - half).abs() < 1e-4);
    assert!((y - half).abs() < 1e-4);
    // Three quarters: bottom-left corner.
    let (x, y) = loader_orbit_position(0.75, half);
    assert!((x - (-half)).abs() < 1e-4);
    assert!((y - half).abs() < 1e-4);
    // Full lap -> back to start.
    let (x, y) = loader_orbit_position(1.0, half);
    assert!((x - (-half)).abs() < 1e-4);
    assert!((y - (-half)).abs() < 1e-4);
}

#[test]
fn loader_orbit_position_repeats_with_period_one() {
    // rem_euclid keeps negative phases inside the orbit too.
    let half = 7.0;
    let a = loader_orbit_position(0.4, half);
    let b = loader_orbit_position(1.4, half);
    let c = loader_orbit_position(-0.6, half);
    assert!((a.0 - b.0).abs() < 1e-4);
    assert!((a.1 - b.1).abs() < 1e-4);
    assert!((a.0 - c.0).abs() < 1e-4);
    assert!((a.1 - c.1).abs() < 1e-4);
}

#[test]
fn loader_pastel_color_carries_alpha_through() {
    let color = loader_pastel_color(3, 1, 0.42);
    assert!((color[3] - 0.42).abs() < 1e-6);
    // RGB stays in [0.0, 1.0].
    for c in &color[..3] {
        assert!(*c >= 0.0 && *c <= 1.0);
    }
}

#[test]
fn loader_pastel_color_palette_rotates_with_tick_and_trail() {
    // The shuffle is non-trivial: two different `tick`s shouldn't
    // always pick the same palette entry. Use the precomputed
    // mixed hash to find a deterministic guarantee.
    let a = loader_pastel_color(0, 0, 1.0);
    let b = loader_pastel_color(1, 0, 1.0);
    // Indices 0 and (5+0+0).rotate_left of 0 differ from
    // (0*5 + 0*3 + 0.rotate_left(3)) = 0 vs (5 + 0 + 8) % 7 = 6.
    assert_ne!(a[..3], b[..3]);
}

#[test]
fn loader_animation_frame_uses_135x_phase_and_12hz_tick() {
    let frame = loader_animation_frame(2.0);
    assert!((frame.phase - 2.7).abs() < 1e-5);
    assert_eq!(frame.tick, 24);
}

#[test]
fn loader_animation_frame_zero_phase_starts_at_origin() {
    let frame = loader_animation_frame(0.0);
    assert_eq!(frame.phase, 0.0);
    assert_eq!(frame.tick, 0);
}

#[test]
fn loader_animation_frame_clamps_invalid_phase_to_zero_tick() {
    // Bad input shouldn't NaN the palette index or panic on cast.
    let frame = loader_animation_frame(-1.0);
    assert_eq!(frame.tick, 0);
    let nan = loader_animation_frame(f32::NAN);
    assert_eq!(nan.tick, 0);
}

// --- present / pacing / pane-projection policy --------------------------

#[test]
fn should_present_frame_or_combines_dirty_and_animation() {
    assert!(!should_present_frame(false, false));
    assert!(should_present_frame(true, false));
    assert!(should_present_frame(false, true));
    assert!(should_present_frame(true, true));
}

#[test]
fn pane_logical_rect_divides_origin_and_grid_by_scale() {
    let rect = pane_logical_rect(PaneLogicalRectInput {
        scaled_margin_left: 20.0,
        scaled_margin_top: 10.0,
        layout_rect: [100.0, 60.0, 0.0, 0.0],
        cell_width_phys: 12.0,
        cell_height_phys: 24.0,
        columns: 80,
        rows: 24,
        scale_factor: 2.0,
    });
    assert_eq!(rect.x, (20.0 + 100.0) / 2.0);
    assert_eq!(rect.y, (10.0 + 60.0) / 2.0);
    assert_eq!(rect.width, (80.0 * 12.0) / 2.0);
    assert_eq!(rect.height, (24.0 * 24.0) / 2.0);
}

#[test]
fn pane_logical_rect_clamps_zero_scale_to_one() {
    // Pathological scale must not divide by zero.
    let rect = pane_logical_rect(PaneLogicalRectInput {
        scaled_margin_left: 0.0,
        scaled_margin_top: 0.0,
        layout_rect: [0.0, 0.0, 0.0, 0.0],
        cell_width_phys: 10.0,
        cell_height_phys: 20.0,
        columns: 4,
        rows: 2,
        scale_factor: 0.0,
    });
    assert_eq!(rect.width, 40.0);
    assert_eq!(rect.height, 40.0);
}

#[test]
fn pane_logical_rect_treats_non_finite_cell_as_zero() {
    let rect = pane_logical_rect(PaneLogicalRectInput {
        scaled_margin_left: 0.0,
        scaled_margin_top: 0.0,
        layout_rect: [0.0, 0.0, 0.0, 0.0],
        cell_width_phys: f32::NAN,
        cell_height_phys: f32::INFINITY,
        columns: 10,
        rows: 10,
        scale_factor: 1.0,
    });
    assert_eq!(rect.width, 0.0);
    assert_eq!(rect.height, 0.0);
}

#[test]
fn pane_overlay_paintable_requires_editor_role_and_positive_grid() {
    assert!(pane_overlay_is_paintable(true, 80, 24, 12.0, 24.0));
    // Terminal pane skipped (minimap is editor-only today).
    assert!(!pane_overlay_is_paintable(false, 80, 24, 12.0, 24.0));
    // Empty grid skipped.
    assert!(!pane_overlay_is_paintable(true, 0, 24, 12.0, 24.0));
    assert!(!pane_overlay_is_paintable(true, 80, 0, 12.0, 24.0));
    // Degenerate cell metrics skipped.
    assert!(!pane_overlay_is_paintable(true, 80, 24, 0.0, 24.0));
    assert!(!pane_overlay_is_paintable(true, 80, 24, 12.0, f32::NAN));
}

#[test]
fn frame_pacing_stats_computes_means_and_budget() {
    let stats = frame_pacing_stats(FramePacingCounters {
        frames: 30,
        elapsed_secs: 0.5,
        render_us_sum: 30_000,        // 1ms mean
        render_us_max: 2_000,         // 2ms max
        full_render_us_sum: 60_000,   // 2ms mean
        full_render_us_max: 4_000,    // 4ms max
        animation_dt_us_sum: 480_000, // 16ms mean
        animation_dt_us_max: 25_000,  // 25ms max
    });
    assert_eq!(stats.fps, 60.0);
    assert_eq!(stats.mean_render_ms, 1.0);
    assert_eq!(stats.max_render_ms, 2.0);
    assert_eq!(stats.mean_full_ms, 2.0);
    assert_eq!(stats.max_full_ms, 4.0);
    assert!((stats.mean_animation_dt_ms - 16.0).abs() < 1e-3);
    assert_eq!(stats.max_animation_dt_ms, 25.0);
    // 60 fps -> ~16.6ms budget; minus 2ms render = ~14.6ms idle.
    let expected_budget = 1000.0 / 60.0;
    assert!((stats.frame_budget_ms - expected_budget).abs() < 1e-3);
    assert!((stats.wait_outside_render_ms - (expected_budget - 2.0)).abs() < 1e-3);
    // 25 - 16 = 9.
    assert!((stats.pacing_jitter_ms - 9.0).abs() < 1e-3);
}

#[test]
fn frame_pacing_stats_clamps_negative_wait_and_jitter() {
    // Mean exceeds budget — wait must clamp to 0, never negative.
    // 60 fps -> 16.67ms budget; full_render mean = 25ms.
    let stats = frame_pacing_stats(FramePacingCounters {
        frames: 60,
        elapsed_secs: 1.0,
        render_us_sum: 0,
        render_us_max: 0,
        full_render_us_sum: 60 * 25_000, // 25ms mean — over budget
        full_render_us_max: 30_000,
        animation_dt_us_sum: 60 * 10_000, // 10ms mean
        animation_dt_us_max: 9_000,       // max (9) < mean (10) -> jitter clamps
    });
    assert_eq!(stats.fps, 60.0);
    // 25ms mean > ~16.67ms budget -> wait clamps to 0.
    assert_eq!(stats.wait_outside_render_ms, 0.0);
    // max (9) < mean (10) -> 0 (no negative jitter).
    assert_eq!(stats.pacing_jitter_ms, 0.0);
}

#[test]
fn frame_pacing_stats_guards_empty_frame_window() {
    // Zero frames / zero elapsed: still produce finite output.
    let stats = frame_pacing_stats(FramePacingCounters::default());
    assert!(stats.fps.is_finite());
    assert_eq!(stats.fps, 0.0);
    assert_eq!(stats.mean_render_ms, 0.0);
    assert_eq!(stats.frame_budget_ms, 0.0);
    assert_eq!(stats.wait_outside_render_ms, 0.0);
    assert_eq!(stats.pacing_jitter_ms, 0.0);
}

#[test]
fn grid_total_row_count_sums_visible_and_buffers() {
    assert_eq!(grid_total_row_count(40, 1, 1), 42);
    // Saturates instead of overflowing.
    assert_eq!(grid_total_row_count(u32::MAX, 1, 1), u32::MAX);
}

#[test]
fn editor_edge_slot_picks_above_when_offset_positive() {
    let (above, below) = editor_edge_slot_source_y(2.5, 4, 30);
    assert_eq!(above, Some(3)); // -1 + source_line_offset
    assert_eq!(below, None);
}

#[test]
fn editor_edge_slot_picks_below_when_offset_negative() {
    let (above, below) = editor_edge_slot_source_y(-1.5, 4, 30);
    assert_eq!(above, None);
    assert_eq!(below, Some(34)); // visible_rows + source_line_offset
}

#[test]
fn editor_edge_slot_zero_offset_leaves_both_empty() {
    let (above, below) = editor_edge_slot_source_y(0.0, 0, 30);
    assert_eq!(above, None);
    assert_eq!(below, None);
}

#[test]
fn editor_edge_slot_actions_leave_stable_fractional_rows() {
    let (above, below, desired_above, desired_below) =
        editor_edge_slot_actions(2.5, 4, 30, Some(3), None, false, false, false);
    assert_eq!(above, TerminalEdgeSlotAction::Leave);
    assert_eq!(below, TerminalEdgeSlotAction::Leave);
    assert_eq!(desired_above, Some(3));
    assert_eq!(desired_below, None);
}

#[test]
fn editor_edge_slot_actions_emit_changed_or_damaged_rows() {
    let (above, below, desired_above, desired_below) =
        editor_edge_slot_actions(-1.5, 4, 30, Some(3), None, false, true, false);
    assert_eq!(above, TerminalEdgeSlotAction::Clear);
    assert_eq!(below, TerminalEdgeSlotAction::Emit { source_y: 34 });
    assert_eq!(desired_above, None);
    assert_eq!(desired_below, Some(34));
}

#[test]
fn editor_edge_slot_actions_force_refresh_clears_zero_offset() {
    let (above, below, desired_above, desired_below) =
        editor_edge_slot_actions(0.0, 0, 30, None, None, false, false, true);
    assert_eq!(above, TerminalEdgeSlotAction::Clear);
    assert_eq!(below, TerminalEdgeSlotAction::Clear);
    assert_eq!(desired_above, None);
    assert_eq!(desired_below, None);
}

#[test]
fn terminal_edge_slot_emits_above_when_positive_offset() {
    let (above, below) = terminal_edge_slot_actions(1.0, 30, false);
    assert_eq!(above, TerminalEdgeSlotAction::Emit { source_y: -1 });
    assert_eq!(below, TerminalEdgeSlotAction::Clear);
}

#[test]
fn terminal_edge_slot_emits_below_when_negative_offset() {
    let (above, below) = terminal_edge_slot_actions(-1.0, 30, false);
    assert_eq!(above, TerminalEdgeSlotAction::Clear);
    assert_eq!(below, TerminalEdgeSlotAction::Emit { source_y: 30 });
}

#[test]
fn terminal_edge_slot_force_refresh_clears_zero_offset() {
    let (above, below) = terminal_edge_slot_actions(0.0, 30, true);
    assert_eq!(above, TerminalEdgeSlotAction::Clear);
    assert_eq!(below, TerminalEdgeSlotAction::Clear);
}

#[test]
fn terminal_cursor_visible_hides_when_block_footer_active() {
    let input = TerminalCursorVisibilityInput {
        block_footer_active: true,
        is_active: true,
        hide_running_command_cursor: false,
        block_input_cursor_present: false,
        cursor_state_visible: true,
        tree_focused: false,
        trail_cursor_enabled: false,
    };
    assert!(!terminal_cursor_visible(input));
}

#[test]
fn terminal_cursor_visible_hides_when_running_command_cursor_hidden() {
    let input = TerminalCursorVisibilityInput {
        block_footer_active: false,
        is_active: false,
        hide_running_command_cursor: true,
        block_input_cursor_present: false,
        cursor_state_visible: true,
        tree_focused: false,
        trail_cursor_enabled: false,
    };
    assert!(!terminal_cursor_visible(input));
}

#[test]
fn terminal_cursor_visible_block_input_ignores_state_visible() {
    // With a block input cursor present, visibility no longer depends on
    // `cursor_state_visible` — the composer caret reads as live regardless.
    let input = TerminalCursorVisibilityInput {
        block_footer_active: false,
        is_active: false,
        hide_running_command_cursor: false,
        block_input_cursor_present: true,
        cursor_state_visible: false,
        tree_focused: false,
        trail_cursor_enabled: false,
    };
    assert!(terminal_cursor_visible(input));
}

#[test]
fn terminal_cursor_visible_tree_focus_hides_cursor() {
    let input = TerminalCursorVisibilityInput {
        block_footer_active: false,
        is_active: true,
        hide_running_command_cursor: false,
        block_input_cursor_present: true,
        cursor_state_visible: true,
        tree_focused: true,
        trail_cursor_enabled: false,
    };
    assert!(!terminal_cursor_visible(input));
}

#[test]
fn terminal_cursor_visible_trail_overlay_hides_cursor() {
    let input = TerminalCursorVisibilityInput {
        block_footer_active: false,
        is_active: true,
        hide_running_command_cursor: false,
        block_input_cursor_present: false,
        cursor_state_visible: true,
        tree_focused: false,
        trail_cursor_enabled: true,
    };
    assert!(!terminal_cursor_visible(input));
}

#[test]
fn block_cursor_uniforms_off_when_not_block_style() {
    let u =
        block_cursor_uniforms(false, false, 4, 10, [0.1, 0.2, 0.3, 1.0], [0.9, 0.8, 0.7]);
    assert_eq!(u, BlockCursorUniforms::HIDDEN);
}

#[test]
fn block_cursor_uniforms_block_writes_position_and_colors() {
    // bg=(0.1,0.2,0.3), cursor=(0.9,0.8,0.7). The block-cursor
    // shader inverts the glyph under the cursor cell, so the GPU
    // `cursor_color` slot (= `cursor_color_u` here) is fed the
    // BG color and the `cursor_bg_color` slot (= `cursor_bg_u`)
    // is fed the CURSOR color. Asserted explicitly so a future
    // refactor that "fixes" the field names will trip this test
    // before it ships a visible cursor-color regression.
    let u =
        block_cursor_uniforms(true, false, 4, 10, [0.1, 0.2, 0.3, 1.0], [0.9, 0.8, 0.7]);
    assert_eq!(u.cursor_pos, [4, 10]);
    assert_eq!(u.cursor_color_u, [0.1, 0.2, 0.3, 1.0]);
    assert_eq!(u.cursor_bg_u, [0.9, 0.8, 0.7, 1.0]);
}

#[test]
fn block_cursor_uniforms_block_suppress_zeroes_colors_only() {
    let u =
        block_cursor_uniforms(true, true, 4, 10, [0.1, 0.2, 0.3, 1.0], [0.9, 0.8, 0.7]);
    // Position still set so the cursor cell stays in sync.
    assert_eq!(u.cursor_pos, [4, 10]);
    assert_eq!(u.cursor_bg_u, [0.0; 4]);
    assert_eq!(u.cursor_color_u, [0.0; 4]);
}

#[test]
fn lsp_status_from_state_maps_known_tokens() {
    assert_eq!(
        lsp_status_from_state(Some("active")),
        Some(LspStatusToken::Active)
    );
    assert_eq!(
        lsp_status_from_state(Some("missing")),
        Some(LspStatusToken::Missing)
    );
    assert_eq!(lsp_status_from_state(Some("none")), None);
}

#[test]
fn lsp_status_from_state_falls_back_to_initializing() {
    assert_eq!(
        lsp_status_from_state(None),
        Some(LspStatusToken::Initializing)
    );
    assert_eq!(
        lsp_status_from_state(Some("starting")),
        Some(LspStatusToken::Initializing)
    );
    assert_eq!(
        lsp_status_from_state(Some("")),
        Some(LspStatusToken::Initializing)
    );
}

#[test]
fn home_tilde_display_collapses_home() {
    assert_eq!(
        home_tilde_display("/home/parker", Some("/home/parker")),
        "~"
    );
}

#[test]
fn home_tilde_display_collapses_under_home() {
    assert_eq!(
        home_tilde_display("/home/parker/projects/neoism", Some("/home/parker")),
        "~/projects/neoism"
    );
}

#[test]
fn home_tilde_display_keeps_outside_home_verbatim() {
    assert_eq!(
        home_tilde_display("/etc/hosts", Some("/home/parker")),
        "/etc/hosts"
    );
    // Important: do NOT collapse a path whose prefix string-matches home but
    // ends mid-segment (`/home/parker2` is not under `/home/parker`).
    assert_eq!(
        home_tilde_display("/home/parker2/foo", Some("/home/parker")),
        "/home/parker2/foo"
    );
}

#[test]
fn home_tilde_display_no_home_returns_path() {
    assert_eq!(home_tilde_display("/etc/hosts", None), "/etc/hosts");
}

#[test]
fn scaled_margin_from_trbl_reorders_to_internal_layout() {
    // Backend `Margin` is `top, right, bottom, left` (CSS order).
    // `ScaledMargin` stores `top, left, right, bottom` so the
    // policy fns can address fields in the order they emit them.
    // The constructor must permute correctly so a downstream
    // refactor that swaps internal field order can't silently
    // break callers.
    let m = ScaledMargin::from_trbl(1.0, 2.0, 3.0, 4.0);
    assert_eq!(m.top, 1.0);
    assert_eq!(m.right, 2.0);
    assert_eq!(m.bottom, 3.0);
    assert_eq!(m.left, 4.0);
}
