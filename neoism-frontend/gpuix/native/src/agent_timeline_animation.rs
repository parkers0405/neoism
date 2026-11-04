//! Continuation signal shared by both serialized and ordinary timeline frames.
//! Kept dependency-free so this exact production logic can also be tested with
//! `rustc --test` without compiling or loading the native graphics engine.

pub(crate) fn frame_animation(
    before: Option<&'static str>,
    after: Option<&'static str>,
    preparing: bool,
    scrolling: bool,
    status_visible: bool,
) -> (bool, Option<&'static str>) {
    // Incoming events invalidate the transcript; a busy provider alone must
    // not animate older messages while its status row is offscreen. Visible
    // status ink, tools and reveals keep their own animation ownership.
    let before = before.filter(|reason| *reason != "streaming" || status_visible);
    let after = after.filter(|reason| *reason != "streaming" || status_visible);
    let reason = after.or(before);
    (preparing || scrolling || reason.is_some(), reason)
}

#[cfg(test)]
mod tests {
    use super::frame_animation;

    #[test]
    fn idle_frame_does_not_schedule_more_work() {
        assert_eq!(frame_animation(None, None, false, false, false), (false, None));
    }

    #[test]
    fn expiry_during_paint_requests_one_confirmation_then_quiesces() {
        // Paint started with expansion progress < 1, then expensive work
        // consumed the remaining duration before metadata was assembled.
        assert_eq!(
            frame_animation(Some("tool_expansion"), None, false, false, false),
            (true, Some("tool_expansion")),
        );
        // No reason is retained between frames: the confirmation starts and
        // finishes idle, so it cannot create a permanent idle repaint loop.
        assert_eq!(frame_animation(None, None, false, false, false), (false, None));
    }

    #[test]
    fn current_reason_takes_precedence_over_initial_reason() {
        assert_eq!(
            frame_animation(Some("tool_expansion"), Some("syntax_compile"), false, false, false),
            (true, Some("syntax_compile")),
        );
        assert_eq!(
            frame_animation(None, Some("tool_expansion"), false, false, false),
            (true, Some("tool_expansion")),
        );
    }

    #[test]
    fn preparation_and_scroll_continuations_are_preserved() {
        assert_eq!(frame_animation(None, None, true, false, false), (true, None));
        assert_eq!(frame_animation(None, None, false, true, false), (true, None));
    }

    #[test]
    fn busy_provider_without_visible_animation_does_not_spin_the_transcript() {
        assert_eq!(frame_animation(Some("streaming"), Some("streaming"), false, false, false), (false, None));
        assert_eq!(frame_animation(Some("streaming"), None, false, false, false), (false, None));
        assert_eq!(frame_animation(None, Some("streaming"), true, false, false), (true, None));
        assert_eq!(frame_animation(None, Some("streaming"), false, true, false), (true, None));
        assert_eq!(frame_animation(None, Some("streaming"), false, false, true), (true, Some("streaming")));
    }

    #[test]
    fn visible_animation_finishes_even_when_busy_fallback_takes_over() {
        for reason in ["text_reveal", "tool_motion", "running_tool", "user_orb", "timeline_interaction_settle"] {
            assert_eq!(frame_animation(Some(reason), Some("streaming"), false, false, false), (true, Some(reason)));
            assert_eq!(frame_animation(Some("streaming"), Some(reason), false, false, false), (true, Some(reason)));
        }
    }
}