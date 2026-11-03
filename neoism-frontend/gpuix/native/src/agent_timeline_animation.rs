//! Continuation signal shared by both serialized and ordinary timeline frames.
//! Kept dependency-free so this exact production logic can also be tested with
//! `rustc --test` without compiling or loading the native graphics engine.

pub(crate) fn frame_animation(
    before: Option<&'static str>,
    after: Option<&'static str>,
    preparing: bool,
    scrolling: bool,
) -> (bool, Option<&'static str>) {
    let reason = after.or(before);
    (preparing || scrolling || reason.is_some(), reason)
}

#[cfg(test)]
mod tests {
    use super::frame_animation;

    #[test]
    fn idle_frame_does_not_schedule_more_work() {
        assert_eq!(frame_animation(None, None, false, false), (false, None));
    }

    #[test]
    fn expiry_during_paint_requests_one_confirmation_then_quiesces() {
        // Paint started with expansion progress < 1, then expensive work
        // consumed the remaining duration before metadata was assembled.
        assert_eq!(
            frame_animation(Some("tool_expansion"), None, false, false),
            (true, Some("tool_expansion")),
        );
        // No reason is retained between frames: the confirmation starts and
        // finishes idle, so it cannot create a permanent idle repaint loop.
        assert_eq!(frame_animation(None, None, false, false), (false, None));
    }

    #[test]
    fn current_reason_takes_precedence_over_initial_reason() {
        assert_eq!(
            frame_animation(Some("tool_expansion"), Some("syntax_compile"), false, false),
            (true, Some("syntax_compile")),
        );
        assert_eq!(
            frame_animation(None, Some("tool_expansion"), false, false),
            (true, Some("tool_expansion")),
        );
    }

    #[test]
    fn preparation_and_scroll_continuations_are_preserved() {
        assert_eq!(frame_animation(None, None, true, false), (true, None));
        assert_eq!(frame_animation(None, None, false, true), (true, None));
    }
}