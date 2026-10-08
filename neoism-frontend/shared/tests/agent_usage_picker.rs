use neoism_ui::panels::agent_pane::command_controller::{
    plan_slash_command, slash_options, SlashCommandAction,
};
use neoism_ui::panels::agent_pane::state::picker::{
    NeoismAgentPicker, NeoismAgentPickerKind, NeoismAgentUsageAccount,
    NeoismAgentUsagePayload, NeoismAgentUsageWindow,
};

fn account(windows: usize) -> NeoismAgentUsageAccount {
    NeoismAgentUsageAccount {
        windows: (0..windows)
            .map(|_| NeoismAgentUsageWindow::default())
            .collect(),
        ..Default::default()
    }
}

#[test]
fn usage_is_an_explicit_local_slash_command() {
    assert_eq!(
        plan_slash_command(" /usage "),
        SlashCommandAction::OpenUsagePicker
    );
    assert!(slash_options()
        .iter()
        .any(|o| o.value == "/usage" && o.description.contains("Codex")));
    let mut picker = NeoismAgentPicker::new(
        NeoismAgentPickerKind::Slash,
        "Commands",
        slash_options(),
        0,
    );
    picker.set_query("usage".into());
    assert_eq!(picker.selected_option().unwrap().value, "/usage");
}

#[test]
fn usage_wire_contract_accepts_additional_fields_and_nullable_metadata() {
    let payload: NeoismAgentUsagePayload = serde_json::from_str(
        r#"{
        "accounts":[{"connection_id":"a","label":"Personal","is_default":true,
        "auth_type":"oauth","plan_type":null,"windows":[{"label":"Weekly",
        "used_percent":27.5,"reset_at":null,"limit_window_seconds":604800,"extra":true}],
        "error":null,"extra":"ignored"}],"extra":42}"#,
    )
    .unwrap();
    let a = &payload.accounts[0];
    assert_eq!(a.connection_id, "a");
    assert!(a.is_default);
    assert_eq!(a.windows[0].remaining_percent(), Some(72.5));
    assert_eq!(a.windows[0].reset_label(0), "Reset unavailable");
}

#[test]
fn remaining_limits_and_reset_times_are_normalized() {
    let mut window = NeoismAgentUsageWindow::default();
    for (used, remaining) in [
        (-10.0, Some(100.0)),
        (130.0, Some(0.0)),
        (25.25, Some(74.75)),
        (f64::NAN, None),
        (f64::INFINITY, None),
    ] {
        window.used_percent = used;
        assert_eq!(window.remaining_percent(), remaining);
    }
    for (reset, label) in [
        (999, "Reset due now"),
        (1001, "Resets in 1m · 01/01, 00:16 UTC"),
        (4600, "Resets in 1h 0m · 01/01, 01:16 UTC"),
        (91400, "Resets in 1d 1h · 01/02, 01:23 UTC"),
    ] {
        window.reset_at = Some(reset);
        assert_eq!(window.reset_label(1000), label);
    }
}

#[test]
fn usage_partial_rows_scroll_and_hit_test_identically_at_every_scale() {
    for scale in [0.5, 1.0, 2.0, 3.0] {
        let mut picker = NeoismAgentPicker::usage_loading();
        picker.set_usage_accounts(vec![account(1), account(2), account(0), account(2)]);
        let g = picker.usage_layout(
            [20.0, 400.0 * scale, 600.0 * scale, 80.0],
            scale,
            6,
            40.0 * scale,
        );
        assert!(!g.stacked);
        assert_eq!(picker.row_height(), 144.0);
        assert!(g.rect[1] >= 40.0 * scale);
        assert!(g.rect[1] + g.rect[3] <= 394.0 * scale + 0.001);
        assert_eq!(picker.max_visible_rows(), 2);
        assert_eq!(
            picker.usage_account_at(g.body[0] + 10.0, g.body[1] + 5.0 * scale),
            Some(0)
        );
        assert!(picker.scroll_touch_pixels(-150.5));
        let residual = picker.tick_list_scroll();
        assert_eq!(picker.scroll_offset, 1);
        assert!((residual + 6.5).abs() < 0.001);
        let second_boundary = g.body[1] + (144.0 - 6.5) * scale;
        assert_eq!(
            picker.usage_account_at(g.body[0] + 10.0, second_boundary - scale),
            Some(1)
        );
        assert_eq!(
            picker.usage_account_at(g.body[0] + 10.0, second_boundary + scale),
            Some(2)
        );
        assert_eq!(
            picker.usage_account_at(g.body[0] + 10.0, g.body[1] - scale),
            None
        );
        assert!(!picker.activate_row_at(g.body[0] + 10.0, g.body[1] + scale));
    }
}

#[test]
fn single_account_usage_panel_shrinks_to_its_content() {
    let mut picker = NeoismAgentPicker::usage_loading();
    picker.set_usage_accounts(vec![account(2)]);
    let g = picker.usage_layout([0.0, 500.0, 600.0, 100.0], 1.0, 6, 30.0);
    assert_eq!(g.body[3], picker.row_height());
    assert_eq!(g.rect[3], 210.0);
    assert!(!picker.scroll_pixels(-20.0));
}

#[test]
fn narrow_usage_panel_can_scroll_with_less_than_one_account_visible() {
    let mut picker = NeoismAgentPicker::usage_loading();
    picker.set_usage_accounts(vec![account(5), account(2)]);
    let g = picker.usage_layout([0.0, 230.0, 240.0, 100.0], 1.0, 6, 30.0);
    assert!(g.stacked);
    assert_eq!(picker.row_height(), 362.0);
    assert_eq!(picker.max_visible_rows(), 0);
    assert_eq!(g.rect[1], 30.0);
    assert!(picker.scroll_touch_pixels(-10_000.0));
    picker.tick_list_scroll();
    assert_eq!(
        picker.usage_account_at(10.0, g.body[1] + g.body[3] - 1.0),
        Some(1)
    );
    assert_eq!(picker.usage_account_at(10.0, g.body[1] + g.body[3]), None);
}

#[test]
fn usage_labels_are_human_readable() {
    let mut a = account(0);
    a.auth_type = "oauth".into();
    for (raw, expected) in [
        ("pro", "Pro"),
        ("PLUS", "Plus"),
        ("free", "Free"),
        ("business", "Business"),
        ("enterprise", "Enterprise"),
        ("team", "Team"),
        ("edu", "Edu"),
    ] {
        a.plan_type = Some(raw.into());
        assert_eq!(a.plan_label(), expected);
    }
    a.plan_type = None;
    assert_eq!(a.plan_label(), "ChatGPT");
    a.auth_type = "api".into();
    assert_eq!(a.plan_label(), "API key");
    a.plan_type = Some("unrecognized_backend_enum".into());
    assert_eq!(a.plan_label(), "API key");
    let mut window = NeoismAgentUsageWindow::default();
    for (raw, expected) in [
        ("Weekly", "Weekly limit"),
        ("5-hour", "5-hour limit"),
        ("Other", "Other"),
    ] {
        window.label = raw.into();
        assert_eq!(window.limit_label(), expected);
    }
}

#[test]
fn usage_loading_reserves_skeleton_rows_and_animates_until_completion() {
    for scale in [0.5, 1.0, 2.0, 3.0] {
        for width in [240.0, 600.0] {
            for max_rows in [1, 2, 6] {
                let mut picker = NeoismAgentPicker::usage_loading();
                assert!(picker.loading);
                assert!(picker.is_animating());
                let g = picker.usage_layout(
                    [0.0, 500.0 * scale, width * scale, 100.0],
                    scale,
                    max_rows,
                    30.0 * scale,
                );
                assert_eq!(g.body[3], 46.0 * max_rows.min(3) as f32 * scale);
                assert!(g.rect[1] >= 30.0 * scale);
                assert_eq!(picker.usage_account_at(10.0, g.body[1] + scale), None);
                picker.set_usage_accounts(vec![account(2)]);
                assert!(!picker.loading);
                assert!(!picker.is_animating());
            }
        }
    }
    let mut picker = NeoismAgentPicker::usage_loading();
    let g = picker.usage_layout([0.0, 130.0, 240.0, 100.0], 1.0, 6, 30.0);
    assert!(g.body[3] < 46.0 * 3.0);
    assert_eq!(g.rect[1], 30.0);
    picker.set_usage_error("Offline".into());
    assert!(!picker.is_animating());
}

#[test]
fn usage_query_is_ignored_and_payload_replacement_resets_loading_errors_and_scroll() {
    let mut picker = NeoismAgentPicker::usage_loading();
    assert!(picker.loading);
    picker.set_usage_error("Offline".into());
    assert!(!picker.loading);
    assert_eq!(picker.usage_error.as_deref(), Some("Offline"));
    picker.set_usage_accounts(vec![account(2); 6]);
    assert!(picker.usage_error.is_none());
    let g = picker.usage_layout([0.0, 500.0, 600.0, 100.0], 1.0, 6, 30.0);
    picker.set_query("nothing".into());
    assert!(picker.query.is_empty());
    assert!(picker.selected_option().is_none());
    picker.scroll_touch_pixels(-180.0);
    picker.tick_list_scroll();
    assert!(picker.scroll_offset > 0);
    picker.set_usage_accounts(vec![]);
    assert_eq!(picker.scroll_offset, 0);
    assert_eq!(picker.usage_account_at(10.0, g.body[1] + 1.0), None);
    assert!(!picker.loading);
}
