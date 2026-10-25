use crate::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Toggle(&'static str),
    Other,
}

fn font() -> TextStyle {
    TextStyle {
        size: 10.0,
        line_height: 12.0,
        color: [0.8, 0.8, 0.8, 1.0],
    }
}
fn mono(text: &str, _: &TextStyle) -> f32 {
    text.chars().count() as f32 * 5.0
}
fn viewport() -> Rect {
    Rect::new(-1000.0, -1000.0, 10000.0, 10000.0)
}
fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 0.01, "{a} != {b}");
}
fn button(key: &str, action: Action) -> Element<Action> {
    Element::quad(key, [0.2, 0.3, 0.4, 1.0], 4.0)
        .height(Size::Px(20.0))
        .on_pointer_down(action)
        .semantics(Semantics::new(Role::Button).label(key))
}
fn lines(frame: &Frame<Action>) -> Vec<&str> {
    frame
        .paint
        .iter()
        .filter_map(|command| match command {
            PaintCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn rectangles_are_half_open_and_intersect() {
    let rect = Rect::new(1.0, 2.0, 3.0, 4.0);
    assert!(rect.contains(1.0, 2.0));
    assert!(rect.contains(3.99, 5.99));
    assert!(!rect.contains(4.0, 3.0));
    assert!(!rect.contains(2.0, 6.0));
    assert!(!rect.contains(f32::NAN, 3.0));
    assert_eq!(
        rect.intersection(Rect::new(3.0, 3.0, 4.0, 4.0)),
        Rect::new(3.0, 3.0, 1.0, 3.0)
    );
    assert!(rect.intersection(Rect::new(4.0, 2.0, 1.0, 1.0)).is_empty());
    assert!(Rect::new(0.0, 0.0, -1.0, 2.0).is_empty());
    assert!(Rect::new(f32::NAN, 0.0, 1.0, 1.0)
        .intersection(rect)
        .is_empty());
    assert_eq!(rect.translated(4.0, -1.0), Rect::new(5.0, 1.0, 3.0, 4.0));
}

#[test]
fn column_intrinsic_height_padding_and_gap() {
    let root = Element::<Action>::column(
        "root",
        [
            Element::text("one", "hello", font()),
            Element::text("two", "world", font()),
        ],
    )
    .padding(Insets::symmetric(8.0, 3.0))
    .gap(4.0);
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [10.0, 20.0], 100.0, viewport(), &mut mono)
        .unwrap();
    close(frame.bounds.height, 34.0);
    assert_eq!(
        frame.bounds_for("one"),
        Some(Rect::new(18.0, 23.0, 84.0, 12.0))
    );
    assert_eq!(
        frame.bounds_for("two"),
        Some(Rect::new(18.0, 39.0, 84.0, 12.0))
    );
    assert_eq!(lines(frame), ["hello", "world"]);
    assert_eq!(frame.bounds_for("missing"), None);
}

#[test]
fn row_flex_grow_wraps_and_measures_auto_height() {
    let root = Element::<Action>::row(
        "root",
        [
            Element::text("label", "hello world again", font())
                .grow(1.0)
                .basis(Size::Px(0.0)),
            Element::quad("toggle", [1.0; 4], 4.0)
                .width(Size::Px(20.0))
                .height(Size::Px(10.0))
                .shrink(0.0),
        ],
    )
    .gap(10.0)
    .align_items(Align::Center);
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [0.0, 0.0], 80.0, viewport(), &mut mono)
        .unwrap();
    close(frame.bounds.height, 36.0);
    close(frame.bounds_for("label").unwrap().width, 50.0);
    close(frame.bounds_for("toggle").unwrap().y, 13.0);
    assert_eq!(lines(frame), ["hello", "world", "again"]);
}

#[test]
fn intrinsic_text_width_is_measured_not_full_row_width() {
    let root = Element::<Action>::row(
        "root",
        [
            Element::text("label", "hello", font()),
            Element::spacer("space").grow(1.0),
            Element::spacer("fixed").width(Size::Px(10.0)),
        ],
    );
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    close(frame.bounds_for("label").unwrap().width, 25.0);
    close(frame.bounds_for("space").unwrap().width, 65.0);
    close(frame.bounds.height, 12.0);
}

#[test]
fn explicit_newlines_empty_and_trailing_lines_have_height() {
    let mut runtime = Runtime::<Action>::new();
    let frame = runtime
        .layout(
            Element::text("text", "a\n\nβ\r\n", font()),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    close(frame.bounds.height, 48.0);
    assert_eq!(lines(frame), ["a", "β"]);
    let positions: Vec<_> = frame
        .paint
        .iter()
        .filter_map(|p| match p {
            PaintCommand::Text { bounds, .. } => Some(bounds.y),
            _ => None,
        })
        .collect();
    assert_eq!(positions, [0.0, 24.0]);
    let frame = runtime
        .layout(
            Element::text("empty", "", font()),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    close(frame.bounds.height, 12.0);
    assert!(frame.paint.is_empty());
}

#[test]
fn unicode_long_tokens_and_zero_width_terminate() {
    let mut runtime = Runtime::<Action>::new();
    let frame = runtime
        .layout(
            Element::text("text", "é猫😀字abc", font()),
            [0.0, 0.0],
            10.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    assert_eq!(lines(frame), ["é猫", "😀字", "ab", "c"]);
    close(frame.bounds.height, 48.0);
    let frame = runtime
        .layout(
            Element::text("text", "é猫😀", font()),
            [0.0, 0.0],
            0.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    assert!(frame.paint.is_empty());
    close(frame.bounds.height, 36.0);
}

#[test]
fn wrap_uses_exact_candidate_measurement_and_emits_exact_widths() {
    let mut shaped = |text: &str, _: &TextStyle| {
        // Simulate kerning: AV is narrower than A + V.
        text.chars().count() as f32 * 5.0 - text.matches("AV").count() as f32 * 3.0
    };
    let mut runtime = Runtime::<Action>::new();
    let frame = runtime
        .layout(
            Element::text("text", "AV AV", font()),
            [0.0, 0.0],
            20.0,
            viewport(),
            &mut shaped,
        )
        .unwrap();
    assert_eq!(lines(frame), ["AV AV"]);
    match &frame.paint[0] {
        PaintCommand::Text {
            bounds,
            text,
            style,
            ..
        } => close(bounds.width, shaped(text, style)),
        _ => panic!("expected text"),
    }
}

#[test]
fn wrapping_normalizes_whitespace_without_losing_newlines() {
    let mut runtime = Runtime::<Action>::new();
    let frame = runtime
        .layout(
            Element::text("text", "  hello\t world  \n  next ", font()),
            [0.0, 0.0],
            200.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    assert_eq!(lines(frame), ["hello world", "next"]);
}

#[test]
fn text_padding_uses_content_width_and_clips_to_content_box() {
    let mut runtime = Runtime::<Action>::new();
    let frame = runtime
        .layout(
            Element::text("text", "hello world", font()).padding(Insets::all(5.0)),
            [3.0, 4.0],
            40.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    close(frame.bounds.height, 34.0);
    assert_eq!(lines(frame), ["hello", "world"]);
    match &frame.paint[0] {
        PaintCommand::Text { bounds, clip, .. } => {
            close(bounds.x, 8.0);
            close(bounds.y, 9.0);
            assert_eq!(*clip, Rect::new(8.0, 9.0, 30.0, 24.0));
        }
        _ => panic!("expected text"),
    }
}

#[test]
fn stack_intrinsic_size_and_last_child_is_topmost() {
    let root = Element::stack(
        "root",
        [
            button("bottom", Action::Other).height(Size::Px(40.0)),
            button("top", Action::Toggle("top")),
        ],
    );
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [10.0, 10.0], 100.0, viewport(), &mut mono)
        .unwrap();
    close(frame.bounds.height, 40.0);
    assert_eq!(frame.hits.len(), 2);
    assert_eq!(frame.hit_test(12.0, 12.0).unwrap().key, "top");
    assert_eq!(
        runtime.dispatch_pointer_down(12.0, 12.0),
        Some(Action::Toggle("top"))
    );
    assert_eq!(runtime.focused_key(), Some("top"));
    assert_eq!(runtime.pressed_key(), Some("top"));
    runtime.release_pointer();
    assert_eq!(runtime.pressed_key(), None);
    assert_eq!(
        runtime.dispatch_pointer_down(12.0, 40.0),
        Some(Action::Other)
    );
}

#[test]
fn nested_child_target_beats_parent_target() {
    let root = Element::column("root", [button("child", Action::Toggle("child"))])
        .padding(Insets::all(5.0))
        .on_pointer_down(Action::Other);
    let mut runtime = Runtime::new();
    runtime
        .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    assert_eq!(
        runtime.dispatch_pointer_down(10.0, 10.0),
        Some(Action::Toggle("child"))
    );
    assert_eq!(runtime.dispatch_pointer_down(1.0, 1.0), Some(Action::Other));
}

#[test]
fn clipping_matches_hits_semantics_and_paint() {
    let root = Element::column(
        "root",
        [button("target", Action::Other)
            .height(Size::Px(50.0))
            .shrink(0.0)],
    )
    .height(Size::Px(20.0))
    .clip(true);
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(
            root,
            [10.0, 10.0],
            100.0,
            Rect::new(15.0, 15.0, 50.0, 100.0),
            &mut mono,
        )
        .unwrap();
    let expected_clip = Rect::new(15.0, 15.0, 50.0, 15.0);
    assert_eq!(frame.hits[0].clip, expected_clip);
    assert_eq!(frame.semantics[0].clip, expected_clip);
    match frame.paint[0] {
        PaintCommand::Quad { bounds, clip, .. } => {
            assert_eq!(bounds, frame.hits[0].bounds);
            assert_eq!(clip, expected_clip);
        }
        _ => panic!("expected quad"),
    }
    assert_eq!(
        runtime.dispatch_pointer_down(20.0, 20.0),
        Some(Action::Other)
    );
    assert_eq!(runtime.dispatch_pointer_down(20.0, 30.0), None);
    assert_eq!(runtime.dispatch_pointer_down(14.0, 20.0), None);
    assert_eq!(runtime.dispatch_pointer_down(65.0, 20.0), None);
}

#[test]
fn disabled_controls_block_underlying_targets_and_cannot_focus() {
    let root = Element::stack(
        "root",
        [
            button("bottom", Action::Other),
            button("disabled", Action::Toggle("disabled"))
                .semantics(Semantics::new(Role::Checkbox).checked(true).disabled(true)),
        ],
    );
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    assert!(frame.hit_test(1.0, 1.0).unwrap().action.is_none());
    assert_eq!(runtime.dispatch_pointer_down(1.0, 1.0), None);
    assert!(!runtime.focus("disabled"));
    assert!(runtime.focus("bottom"));
    assert_eq!(runtime.focus_next(false), Some("bottom"));
}

#[test]
fn disabled_is_inherited_and_semantic_parent_is_nearest_semantic_ancestor() {
    let root = Element::column(
        "root",
        [Element::column("wrapper", [button("child", Action::Other)])],
    )
    .semantics(Semantics::new(Role::Group).disabled(true));
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    let child = frame.semantics.iter().find(|s| s.key == "child").unwrap();
    assert_eq!(child.parent.as_deref(), Some("root"));
    assert!(child.semantics.disabled);
    assert_eq!(runtime.dispatch_pointer_down(1.0, 1.0), None);
    assert_eq!(runtime.focus_next(false), None);
}

#[test]
fn focus_and_pressed_identity_survive_reorder_but_not_unmount() {
    let mut runtime = Runtime::new();
    let tree = || {
        Element::column(
            "root",
            [button("a", Action::Other), button("b", Action::Toggle("b"))],
        )
    };
    runtime
        .layout(tree(), [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    assert_eq!(runtime.focus_next(false), Some("a"));
    assert_eq!(runtime.focus_next(false), Some("b"));
    assert_eq!(
        runtime.dispatch_pointer_down(1.0, 21.0),
        Some(Action::Toggle("b"))
    );
    runtime
        .layout(
            Element::column(
                "root",
                [
                    button("b", Action::Toggle("new")),
                    button("a", Action::Other),
                ],
            ),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    assert_eq!(runtime.focused_key(), Some("b"));
    assert_eq!(runtime.pressed_key(), Some("b"));
    assert!(
        runtime
            .frame()
            .semantics
            .iter()
            .find(|s| s.key == "b")
            .unwrap()
            .focused
    );
    assert_eq!(
        runtime.dispatch_pointer_down(1.0, 1.0),
        Some(Action::Toggle("new"))
    );
    runtime
        .layout(
            button("a", Action::Other),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    assert_eq!(runtime.focused_key(), None);
    assert_eq!(runtime.pressed_key(), None);
    assert!(!runtime.focus("b"));
}

#[test]
fn semantic_interactive_role_focuses_without_an_action() {
    let root = Element::<Action>::quad("checkbox", [1.0; 4], 0.0)
        .height(Size::Px(20.0))
        .semantics(
            Semantics::new(Role::Checkbox)
                .label("Enabled")
                .checked(false),
        );
    let mut runtime = Runtime::new();
    runtime
        .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    assert!(runtime.focus("checkbox"));
    assert!(runtime.frame().semantics[0].focused);
    runtime.blur();
    assert!(!runtime.frame().semantics[0].focused);
    assert_eq!(runtime.focus_next(true), Some("checkbox"));
}

#[test]
fn focus_traversal_wraps_and_uses_preorder() {
    let root = Element::column(
        "root",
        [button("a", Action::Other), button("z", Action::Other)],
    )
    .on_pointer_down(Action::Other);
    let mut runtime = Runtime::new();
    runtime
        .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    assert_eq!(runtime.focus_next(true), Some("z"));
    assert_eq!(runtime.focus_next(false), Some("root"));
    assert_eq!(runtime.focus_next(false), Some("a"));
    assert_eq!(runtime.focus_next(true), Some("root"));
}

#[test]
fn clear_when_hidden_invalidates_owned_snapshots_and_all_state() {
    let mut runtime = Runtime::new();
    let snapshot = runtime
        .layout(
            button("button", Action::Other),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap()
        .clone();
    runtime.dispatch_pointer_down(1.0, 1.0);
    runtime.clear();
    assert!(runtime.frame().paint.is_empty());
    assert!(runtime.frame().hits.is_empty());
    assert!(runtime.frame().semantics.is_empty());
    assert_eq!(runtime.frame().bounds_for("button"), None);
    assert_eq!(runtime.focused_key(), None);
    assert_eq!(runtime.pressed_key(), None);
    assert_eq!(
        runtime.dispatch_pointer_down_in(snapshot.id, 1.0, 1.0),
        None
    );
    assert_eq!(runtime.dispatch_pointer_down(1.0, 1.0), None);
    assert!(!snapshot.paint.is_empty()); // old frame is owned, not aliased
}

#[test]
fn relayout_replaces_geometry_and_rejects_stale_scoped_events() {
    let mut runtime = Runtime::new();
    let old = runtime
        .layout(
            button("a", Action::Other),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap()
        .id;
    let new = runtime
        .layout(
            button("a", Action::Toggle("new")),
            [200.0, 200.0],
            50.0,
            viewport(),
            &mut mono,
        )
        .unwrap()
        .id;
    assert_ne!(old, new);
    assert_eq!(runtime.dispatch_pointer_down(1.0, 1.0), None);
    assert_eq!(runtime.dispatch_pointer_down_in(old, 201.0, 201.0), None);
    assert_eq!(
        runtime.dispatch_pointer_down_in(new, 201.0, 201.0),
        Some(Action::Toggle("new"))
    );
}

#[test]
fn measurement_pass_and_viewport_pass_have_identical_geometry() {
    let tree = || {
        Element::<Action>::column(
            "root",
            [
                Element::text(
                    "description",
                    "this is a longer description that wraps",
                    font(),
                ),
                button("control", Action::Other),
            ],
        )
        .gap(5.0)
        .padding(Insets::all(8.0))
    };
    let mut runtime = Runtime::new();
    let measured = runtime
        .layout(tree(), [0.0, 0.0], 80.0, viewport(), &mut mono)
        .unwrap()
        .clone();
    let painted = runtime
        .layout(
            tree(),
            [100.0, -20.0],
            80.0,
            Rect::new(100.0, 0.0, 80.0, 40.0),
            &mut mono,
        )
        .unwrap();
    close(measured.bounds.height, painted.bounds.height);
    for key in ["root", "description", "control"] {
        assert_eq!(
            measured.bounds_for(key).unwrap().translated(100.0, -20.0),
            painted.bounds_for(key).unwrap()
        );
    }
}

#[test]
fn offscreen_parent_does_not_cull_visible_overflowing_child() {
    let root = Element::column(
        "root",
        [
            Element::spacer("offset").height(Size::Px(50.0)).shrink(0.0),
            button("child", Action::Other).shrink(0.0),
        ],
    )
    .height(Size::Px(10.0));
    let mut runtime = Runtime::new();
    runtime
        .layout(
            root,
            [0.0, 0.0],
            100.0,
            Rect::new(0.0, 50.0, 100.0, 20.0),
            &mut mono,
        )
        .unwrap();
    assert_eq!(
        runtime.dispatch_pointer_down(1.0, 51.0),
        Some(Action::Other)
    );
}

#[test]
fn empty_clip_retains_measurement_and_focus_but_has_no_visible_output() {
    let mut runtime = Runtime::new();
    runtime
        .layout(
            button("button", Action::Other),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    runtime.focus("button");
    let frame = runtime
        .layout(
            button("button", Action::Other),
            [0.0, 0.0],
            100.0,
            Rect::ZERO,
            &mut mono,
        )
        .unwrap();
    close(frame.bounds.height, 20.0);
    assert!(frame.paint.is_empty());
    assert!(frame.hits.is_empty());
    assert!(frame.semantics.is_empty());
    assert_eq!(runtime.focused_key(), Some("button"));
}

#[test]
fn duplicate_or_empty_keys_fail_and_clear_previous_hits() {
    let mut runtime = Runtime::new();
    runtime
        .layout(
            button("button", Action::Other),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    runtime.focus("button");
    let duplicate = Element::column(
        "root",
        [button("same", Action::Other), button("same", Action::Other)],
    );
    let error = runtime
        .layout(duplicate, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap_err();
    assert_eq!(error, LayoutError::DuplicateKey("same".into()));
    assert_eq!(runtime.focused_key(), None);
    assert_eq!(runtime.dispatch_pointer_down(1.0, 1.0), None);
    assert_eq!(
        runtime
            .layout(
                button("", Action::Other),
                [0.0, 0.0],
                100.0,
                viewport(),
                &mut mono
            )
            .unwrap_err(),
        LayoutError::EmptyKey
    );
}

#[test]
fn invalid_inputs_are_reported_not_forwarded_to_taffy() {
    let mut runtime = Runtime::new();
    for width in [-1.0, f32::INFINITY, f32::NAN] {
        assert!(runtime
            .layout(
                button("button", Action::Other),
                [0.0, 0.0],
                width,
                viewport(),
                &mut mono
            )
            .is_err());
    }
    assert!(runtime
        .layout(
            button("button", Action::Other),
            [f32::NAN, 0.0],
            100.0,
            viewport(),
            &mut mono
        )
        .is_err());
    assert!(runtime
        .layout(
            button("button", Action::Other),
            [0.0, 0.0],
            100.0,
            Rect::new(0.0, 0.0, -1.0, 1.0),
            &mut mono
        )
        .is_err());
    for root in [
        button("button", Action::Other).gap(-1.0),
        button("button", Action::Other).grow(f32::NAN),
        button("button", Action::Other).width(Size::Percent(2.0)),
        Element::quad("quad", [2.0; 4], 0.0),
        Element::quad("quad", [1.0; 4], -1.0),
        Element::text(
            "text",
            "abc",
            TextStyle {
                line_height: 0.0,
                ..font()
            },
        ),
        Element::text(
            "text",
            "abc",
            TextStyle {
                size: f32::NAN,
                ..font()
            },
        ),
    ] {
        assert!(runtime
            .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
            .is_err());
        assert!(runtime.frame().hits.is_empty());
    }
}

#[test]
fn bad_measurements_fail_and_clear_old_frame() {
    let mut runtime = Runtime::new();
    for value in [f32::NAN, f32::INFINITY, -1.0] {
        runtime
            .layout(
                button("old", Action::Other),
                [0.0, 0.0],
                100.0,
                viewport(),
                &mut mono,
            )
            .unwrap();
        let mut bad = move |_: &str, _: &TextStyle| value;
        let error = runtime
            .layout(
                Element::text("text", "not empty", font()),
                [0.0, 0.0],
                100.0,
                viewport(),
                &mut bad,
            )
            .unwrap_err();
        assert_eq!(error, LayoutError::InvalidMeasurement);
        assert!(runtime.frame().paint.is_empty());
    }
}

#[test]
fn tree_depth_is_bounded() {
    let mut root = Element::<Action>::spacer("leaf");
    for index in 0..130 {
        root = Element::column(format!("node/{index}"), [root]);
    }
    let mut runtime = Runtime::new();
    assert_eq!(
        runtime
            .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
            .unwrap_err(),
        LayoutError::TreeTooDeep
    );
}

#[test]
fn quad_radius_is_clamped_to_box() {
    let mut runtime = Runtime::<Action>::new();
    let frame = runtime
        .layout(
            Element::quad("quad", [1.0; 4], 1000.0).height(Size::Px(10.0)),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    assert!(matches!(
        frame.paint[0],
        PaintCommand::Quad { radius: 5.0, .. }
    ));
}

#[test]
fn fractional_layout_preserves_logical_coordinates_and_percentages() {
    let root = Element::<Action>::row(
        "root",
        [
            Element::spacer("half")
                .width(Size::Percent(0.5))
                .height(Size::Px(7.25)),
            Element::spacer("rest").grow(1.0),
        ],
    );
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [0.25, 0.75], 101.5, viewport(), &mut mono)
        .unwrap();
    assert_eq!(frame.bounds, Rect::new(0.25, 0.75, 101.5, 7.25));
    assert_eq!(
        frame.bounds_for("half"),
        Some(Rect::new(0.25, 0.75, 50.75, 7.25))
    );
    assert_eq!(
        frame.bounds_for("rest"),
        Some(Rect::new(51.0, 0.75, 50.75, 7.25))
    );
}

#[test]
fn root_width_is_exact_but_child_min_max_are_honored() {
    let root = Element::<Action>::row(
        "root",
        [
            Element::spacer("capped")
                .grow(1.0)
                .max_width(Size::Px(30.0)),
            Element::spacer("rest").grow(1.0).min_width(Size::Px(20.0)),
        ],
    )
    .width(Size::Px(500.0))
    .min_width(Size::Px(500.0))
    .max_width(Size::Px(500.0));
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    close(frame.bounds.width, 100.0);
    close(frame.bounds_for("capped").unwrap().width, 30.0);
    close(frame.bounds_for("rest").unwrap().width, 70.0);
}

#[test]
fn disabling_mounted_control_reconciles_focus_and_pressed_state() {
    let mut runtime = Runtime::new();
    runtime
        .layout(
            button("same", Action::Other),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    runtime.dispatch_pointer_down(1.0, 1.0);
    assert_eq!(runtime.focused_key(), Some("same"));
    assert_eq!(runtime.pressed_key(), Some("same"));
    runtime
        .layout(
            button("same", Action::Other)
                .semantics(Semantics::new(Role::Button).disabled(true)),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap();
    assert_eq!(runtime.focused_key(), None);
    assert_eq!(runtime.pressed_key(), None);
    assert!(!runtime.frame().semantics[0].focused);
}

#[test]
fn fixed_text_height_clips_emitted_lines_and_hit_uses_same_box() {
    let mut runtime = Runtime::new();
    let root = Element::text("text", "one\ntwo\nthree", font())
        .height(Size::Px(15.0))
        .on_pointer_down(Action::Other);
    let frame = runtime
        .layout(root, [0.0, 0.0], 100.0, viewport(), &mut mono)
        .unwrap();
    assert_eq!(lines(frame), ["one", "two"]);
    close(frame.bounds.height, 15.0);
    for command in &frame.paint {
        if let PaintCommand::Text { clip, .. } = command {
            assert_eq!(*clip, Rect::new(0.0, 0.0, 100.0, 15.0));
        }
    }
    assert_eq!(
        runtime.dispatch_pointer_down(1.0, 14.99),
        Some(Action::Other)
    );
    assert_eq!(runtime.dispatch_pointer_down(1.0, 15.0), None);
}

#[test]
fn many_widths_produce_exact_measured_lines_and_clip_safe_hits() {
    for width in [
        0.0, 0.5, 3.0, 5.0, 9.0, 10.0, 10.5, 30.0, 42.75, 100.0, 1000.0,
    ] {
        let mut runtime = Runtime::new();
        let frame = runtime
            .layout(
                Element::text("text", "é猫😀 abcdefghijklmnop next\n\nlast", font())
                    .on_pointer_down(Action::Other),
                [0.25, -3.5],
                width,
                Rect::new(0.0, 0.0, 2000.0, 2000.0),
                &mut mono,
            )
            .unwrap();
        for command in &frame.paint {
            if let PaintCommand::Text {
                bounds,
                text,
                style,
                clip,
            } = command
            {
                close(bounds.width, mono(text, style));
                assert!(!bounds.intersection(*clip).is_empty());
                assert!(bounds.is_valid());
            }
        }
        for hit in &frame.hits {
            assert!(!hit.bounds.intersection(hit.clip).is_empty());
            assert!(!hit.contains(hit.bounds.right(), hit.bounds.y));
        }
        assert!(frame.bounds.height.is_finite());
    }
}

#[test]
fn padded_row_text_auto_height_uses_final_content_width() {
    // Exercise both intrinsic flex basis and the zero-basis/growing settings-row
    // pattern, plus fractional widths and explicit newlines.
    for grow in [false, true] {
        for (width, text, expected_height, expected_lines) in [
            (55.0, "hello world", 34.0, vec!["hello", "world"]),
            (54.75, "hello world", 34.0, vec!["hello", "world"]),
            (65.0, "hello world", 22.0, vec!["hello world"]),
            (
                55.0,
                "hello world\nagain",
                46.0,
                vec!["hello", "world", "again"],
            ),
        ] {
            let mut text =
                Element::<Action>::text("text", text, font()).padding(Insets::all(5.0));
            if grow {
                text = text.grow(1.0).basis(Size::Px(0.0));
            }
            let mut runtime = Runtime::new();
            let frame = runtime
                .layout(
                    Element::row("row", [text]),
                    [0.0, 0.0],
                    width,
                    viewport(),
                    &mut mono,
                )
                .unwrap();
            close(frame.bounds.height, expected_height);
            let bounds = frame.bounds_for("text").unwrap();
            close(bounds.height, expected_height);
            assert_eq!(lines(frame), expected_lines);
            for command in &frame.paint {
                if let PaintCommand::Text { bounds, clip, .. } = command {
                    // No auto-height line may be vertically truncated.
                    assert!(bounds.y >= clip.y);
                    assert!(bounds.bottom() <= clip.bottom());
                }
            }
        }
    }
}

#[test]
fn root_padding_floor_is_rejected_and_clears_previous_interaction() {
    let mut runtime = Runtime::new();
    let old_id = runtime
        .layout(
            button("old", Action::Other),
            [0.0, 0.0],
            100.0,
            viewport(),
            &mut mono,
        )
        .unwrap()
        .id;
    runtime.dispatch_pointer_down(1.0, 1.0);
    let root = Element::text("text", "abc", font()).padding(Insets::all(10.0));
    let error = runtime
        .layout(root, [0.0, 0.0], 5.0, viewport(), &mut mono)
        .unwrap_err();
    assert_eq!(error, LayoutError::IncompatibleRootWidth);
    assert!(error.to_string().contains("exact available width"));
    assert!(runtime.frame().paint.is_empty());
    assert!(runtime.frame().hits.is_empty());
    assert!(runtime.frame().semantics.is_empty());
    assert_eq!(runtime.focused_key(), None);
    assert_eq!(runtime.pressed_key(), None);
    assert_eq!(runtime.dispatch_pointer_down_in(old_id, 1.0, 1.0), None);
}

#[test]
fn successful_roots_never_exceed_exact_width_even_at_padding_floor() {
    for width in [0.0, 0.25, 5.0, 19.75, 20.0, 20.25, 55.0, 100.0] {
        for padding in [0.0, 0.125, 5.0, 10.0] {
            for root in [
                Element::<Action>::text("root", "abc", font()),
                Element::column("root", [Element::text("text", "abc", font())]),
                Element::row("root", [Element::text("text", "abc", font())]),
                Element::stack("root", [Element::text("text", "abc", font())]),
                Element::quad("root", [1.0; 4], 0.0),
            ] {
                let mut runtime = Runtime::new();
                match runtime.layout(
                    root.padding(Insets::all(padding)),
                    [0.0, 0.0],
                    width,
                    viewport(),
                    &mut mono,
                ) {
                    Ok(frame) => assert_eq!(frame.bounds.width, width),
                    Err(error) => {
                        assert_eq!(error, LayoutError::IncompatibleRootWidth);
                        assert!(padding * 2.0 > width);
                    }
                }
            }
        }
    }
}

#[test]
fn production_boolean_row_and_mixed_spacers_have_one_shared_runtime() {
    fn row(
        key: &str,
        title: &str,
        description: &str,
        checked: bool,
        action: Action,
    ) -> Element<Action> {
        Element::row(
            key,
            [
                Element::column(
                    format!("{key}/copy"),
                    [
                        Element::text(format!("{key}/title"), title, font()),
                        Element::text(format!("{key}/description"), description, font()),
                    ],
                )
                .grow(1.0)
                .basis(Size::Px(0.0))
                .gap(3.0),
                Element::stack(
                    format!("{key}/switch"),
                    [
                        Element::quad(format!("{key}/track"), [0.1, 0.5, 0.7, 1.0], 10.0),
                        Element::quad(format!("{key}/knob"), [1.0; 4], 8.0)
                            .width(Size::Px(16.0))
                            .height(Size::Px(16.0))
                            .align_self(Align::Center),
                    ],
                )
                .width(Size::Px(36.0))
                .height(Size::Px(20.0))
                .shrink(0.0),
            ],
        )
        .gap(12.0)
        .padding(Insets::all(8.0))
        .align_items(Align::Center)
        .on_pointer_down(action)
        .semantics(Semantics::new(Role::Switch).label(title).checked(checked))
    }
    let root = Element::column(
        "settings",
        [
            row(
                "editor/wrap",
                "Wrap",
                "Wrap long lines to fit the viewport",
                true,
                Action::Toggle("wrap"),
            ),
            Element::spacer("foreign-row-reserved-height").height(Size::Px(31.0)),
            row(
                "editor/gutter",
                "Gutter",
                "Display line numbers",
                false,
                Action::Toggle("gutter"),
            ),
        ],
    );
    let mut runtime = Runtime::new();
    let frame = runtime
        .layout(root, [30.0, 50.0], 160.0, viewport(), &mut mono)
        .unwrap();
    let first = frame.bounds_for("editor/wrap").unwrap();
    let second = frame.bounds_for("editor/gutter").unwrap();
    assert!(first.height > 40.0);
    close(second.y, first.bottom() + 31.0);
    assert_eq!(frame.semantics[0].semantics.checked, Some(true));
    assert_eq!(frame.semantics[1].semantics.checked, Some(false));
    assert_eq!(
        runtime.dispatch_pointer_down(first.x + 2.0, first.y + 2.0),
        Some(Action::Toggle("wrap"))
    );
    assert_eq!(
        runtime.dispatch_pointer_down(second.x + 2.0, second.y + 2.0),
        Some(Action::Toggle("gutter"))
    );
}
