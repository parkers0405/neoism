use neoism_ui::lifecycle_policy::{font_size_after_action, FontSizeAction};

#[test]
fn decrement_uses_window_zoom_instead_of_new_context_default() {
    assert_eq!(
        font_size_after_action(52.0, 14.0, FontSizeAction::Decrease),
        51.0
    );
}

#[test]
fn reset_returns_to_configured_size_and_steps_are_bounded() {
    assert_eq!(
        font_size_after_action(52.0, 16.0, FontSizeAction::Reset),
        16.0
    );
    assert_eq!(
        font_size_after_action(100.0, 16.0, FontSizeAction::Increase),
        100.0
    );
    assert_eq!(
        font_size_after_action(6.0, 16.0, FontSizeAction::Decrease),
        6.0
    );
}
