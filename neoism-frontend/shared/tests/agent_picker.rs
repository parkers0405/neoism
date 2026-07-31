use neoism_ui::panels::agent_pane::command_controller::slash_options;
use neoism_ui::panels::agent_pane::state::picker::{
    NeoismAgentPicker, NeoismAgentPickerKind,
};
use neoism_ui::widgets::inline_picker::{layout_limited, row_limit_for_space};

#[test]
fn slash_command_prefix_outranks_description_matches() {
    let mut picker = NeoismAgentPicker::new(
        NeoismAgentPickerKind::Slash,
        "Commands",
        slash_options(),
        0,
    );

    picker.set_query("sess".to_string());

    assert_eq!(
        picker.options().first().map(|option| option.value.as_str()),
        Some("/sessions")
    );
    assert_eq!(
        picker.selected_option().map(|option| option.value.as_str()),
        Some("/sessions")
    );
    assert!(
        picker
            .options()
            .iter()
            .any(|option| option.value == "/compact"),
        "description matches should remain available below command-name matches"
    );
}

#[test]
fn compact_home_picker_stays_between_chrome_and_composer() {
    let input = [120.0, 640.0, 760.0, 100.0];
    let min_y = 90.0;
    let rows = row_limit_for_space(input[1], min_y, 1.0, false, 5);
    let rect = layout_limited(30, input, 1.0, false, rows, min_y).unwrap();

    assert!(rows <= 5);
    assert!(rect[1] >= min_y);
    assert!(rect[1] + rect[3] <= input[1] - 6.0);
}
