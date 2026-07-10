use neoism_ui::chrome_policy::{fit_editor_rows, EditorRowFitInput};

#[test]
fn complete_rows_reach_status_after_all_chrome_is_reserved() {
    let fit = fit_editor_rows(EditorRowFitInput {
        scaled_margin_top: 140.0,
        layout_top: 0.0,
        layout_height: 564.0,
        window_height: 752.0,
        status_line_height: 47.0,
        nominal_cell_height: 35.0,
    });

    assert_eq!(fit.rows, 16);
    assert_eq!(fit.usable_height, 564.0);
    assert_eq!(fit.cell_height, 35.25);
    assert_eq!(f32::from(fit.rows) * fit.cell_height, fit.usable_height);
}

#[test]
fn status_boundary_clamps_a_layout_rect_that_extends_under_it() {
    let fit = fit_editor_rows(EditorRowFitInput {
        scaled_margin_top: 110.0,
        layout_top: 0.0,
        layout_height: 613.7143,
        window_height: 752.0,
        status_line_height: 47.142857,
        nominal_cell_height: 35.0,
    });

    assert_eq!(fit.rows, 17);
    assert_eq!(fit.usable_height, 595.0);
    assert_eq!(fit.cell_height, 35.0);
}
