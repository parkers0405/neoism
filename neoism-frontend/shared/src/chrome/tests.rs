
use super::EditorScrollViewportBounds;

#[test]
fn editor_scroll_viewport_bounds_rejects_only_hard_edges() {
    let top = EditorScrollViewportBounds {
        topline: 0,
        botline: 24,
        line_count: 100,
    };
    assert!(top.rejects_delta(-1.0));
    assert!(!top.rejects_delta(1.0));

    let middle = EditorScrollViewportBounds {
        topline: 10,
        botline: 34,
        line_count: 100,
    };
    assert!(!middle.rejects_delta(-1.0));
    assert!(!middle.rejects_delta(1.0));

    let bottom = EditorScrollViewportBounds {
        topline: 76,
        botline: 100,
        line_count: 100,
    };
    assert!(!bottom.rejects_delta(-1.0));
    assert!(bottom.rejects_delta(1.0));
}

#[test]
fn editor_scroll_viewport_bounds_unknown_never_rejects() {
    let unknown = EditorScrollViewportBounds {
        topline: 0,
        botline: 0,
        line_count: 0,
    };
    assert!(!unknown.rejects_delta(-1.0));
    assert!(!unknown.rejects_delta(1.0));
}
