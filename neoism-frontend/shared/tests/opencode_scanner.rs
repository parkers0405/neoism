use neoism_ui::render_policy::{opencode_scanner_frame, OPENCODE_SCANNER_WIDTH};

fn approx(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.0001,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn scanner_uses_the_opencode_tui_timing_and_trail() {
    let start = opencode_scanner_frame(0.0);
    assert_eq!(start.len(), OPENCODE_SCANNER_WIDTH);
    assert!(start[0].active);
    approx(start[0].alpha, 1.0);
    approx(start[1].alpha, 0.18);

    let end = opencode_scanner_frame(7.0 * 0.040);
    assert!(end[7].active);
    assert!(end[6].active);
    approx(end[6].alpha, 0.9);
    approx(end[6].brightness, 1.15);

    let backward = opencode_scanner_frame(17.0 * 0.040);
    assert!(backward[6].active);
    assert!(backward[7].active);

    let start_hold = opencode_scanner_frame(24.0 * 0.040);
    assert!(start_hold[0].active);
}

#[test]
fn scanner_holds_fades_and_wraps_after_54_frames() {
    let faded_hold = opencode_scanner_frame(30.0 * 0.040);
    assert!(faded_hold.iter().all(|cell| !cell.active));
    approx(faded_hold[0].alpha, 0.516);

    assert_eq!(
        opencode_scanner_frame(54.0 * 0.040),
        opencode_scanner_frame(0.0)
    );
}
