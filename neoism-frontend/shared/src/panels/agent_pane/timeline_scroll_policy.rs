//! Shared timeline scroll policy for the agent pane.
//!
//! Wheel/touchpad scrolling, automatic stream-follow, and keyboard half-page
//! scrolling share the same timeline state. Wheel/touchpad and stream-follow
//! reuse the same damped spring, but Ctrl+U/Ctrl+D should feel like the markdown
//! renderer/nvim path: a fast half-page jump with the existing kinetic tail.

/// Fraction of the visible timeline that Ctrl+U / Ctrl+D should travel.
pub const CTRL_U_D_VIEWPORT_FRACTION: f32 = 0.5;

/// Convert the current viewport height into an agent timeline half-page step.
///
/// The sign mirrors the timeline's scroll offset convention: positive reveals
/// older history above the viewport (Ctrl+U), negative moves toward the bottom
/// / newer messages (Ctrl+D).
pub fn ctrl_u_d_scroll_delta(viewport_height_px: f32, older_history: bool) -> f32 {
    let magnitude = viewport_height_px.max(0.0) * CTRL_U_D_VIEWPORT_FRACTION;
    if older_history {
        magnitude
    } else {
        -magnitude
    }
}

/// Automatic follow retains its latch while the spring is catching up;
/// wheel motion keeps the existing proximity-based follow rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimelineScrollOwner {
    #[default]
    Wheel,
    FollowBottom,
}

/// One clock-free step of the existing wheel spring, also used by stream-follow.
/// Keep the aggressive settle threshold so neither path acquires a slow crawl.
/// Frames are capped at 50 ms, with exact settling when error is below 0.5 px and speed is below 30 px/s.
pub fn step_timeline_spring(
    mut position: f32,
    mut velocity: f32,
    target: f32,
    max_scroll: f32,
    dt: f32,
) -> (f32, f32, bool) {
    let max_scroll = max_scroll.max(0.0);
    if max_scroll == 0.0 {
        return (0.0, 0.0, true);
    }
    let target = target.clamp(0.0, max_scroll);
    let mut remaining = dt.clamp(0.0, 0.05);
    const OMEGA: f32 = 16.0;
    const MAX_SUBSTEP: f32 = 1.0 / 240.0;
    while remaining > 0.0 {
        let step = remaining.min(MAX_SUBSTEP);
        let delta = target - position;
        let accel = OMEGA * OMEGA * delta - 2.0 * OMEGA * velocity;
        velocity += accel * step;
        position += velocity * step;
        remaining -= step;
    }
    position = position.clamp(0.0, max_scroll);
    let settled = (target - position).abs() < 0.5 && velocity.abs() < 30.0;
    if settled {
        (target, 0.0, true)
    } else {
        (position, velocity, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_matches_existing_wheel_integration() {
        for (mut position, mut velocity, target, max_scroll, dt) in [
            (200.0_f32, 0.0_f32, 224.0_f32, 1000.0_f32, 1.0_f32 / 60.0),
            (224.0, 120.0, 248.0, 1000.0, 1.0 / 120.0),
            (50.0, -300.0, 0.0, 1000.0, 0.05),
            (1.0, -20.0, 0.0, 1000.0, 1.0 / 60.0),
            (99.9, 0.0, 100.0, 100.0, 1.0 / 60.0),
        ] {
            let actual = step_timeline_spring(position, velocity, target, max_scroll, dt);
            let mut remaining = dt.min(0.05);
            while remaining > 0.0 {
                let step = remaining.min(1.0 / 240.0);
                let delta = target - position;
                let accel = 16.0 * 16.0 * delta - 2.0 * 16.0 * velocity;
                velocity += accel * step;
                position += velocity * step;
                remaining -= step;
            }
            position = position.clamp(0.0, max_scroll);
            let settled = (target - position).abs() < 0.5 && velocity.abs() < 30.0;
            let expected = if settled {
                (target, 0.0, true)
            } else {
                (position, velocity, false)
            };
            assert_eq!(actual, expected);
        }
        assert_eq!(TimelineScrollOwner::default(), TimelineScrollOwner::Wheel);
    }

    #[test]
    fn spring_follow_moves_monotonically_and_settles_without_a_tail() {
        let mut position = 500.0;
        let mut velocity = 0.0;
        let mut settled = false;
        for _ in 0..60 {
            let before = position;
            (position, velocity, settled) =
                step_timeline_spring(position, velocity, 0.0, 2000.0, 1.0 / 60.0);
            assert!(position >= 0.0 && position <= before);
            if settled {
                break;
            }
        }
        assert!(settled, "follow must settle exactly in under one second");
        assert_eq!(position, 0.0);
        assert_eq!(velocity, 0.0);
        assert_eq!(
            step_timeline_spring(position, velocity, 0.0, 2000.0, 1.0 / 60.0),
            (0.0, 0.0, true)
        );
    }

    #[test]
    fn spring_tracks_continuous_growth_without_resetting_velocity() {
        let mut position = 0.0;
        let mut velocity = 0.0;
        for _ in 0..90 {
            position += 4.0;
            let before = position;
            (position, velocity, _) =
                step_timeline_spring(position, velocity, 0.0, 2000.0, 1.0 / 60.0);
            assert!(position > 0.0 && position < before);
            assert!(
                position < 40.0,
                "follow lag must not accumulate indefinitely"
            );
            assert!(velocity < 0.0);
        }
        let mut settled = false;
        for _ in 0..60 {
            (position, velocity, settled) =
                step_timeline_spring(position, velocity, 0.0, 2000.0, 1.0 / 60.0);
            if settled {
                break;
            }
        }
        assert!(settled);
        assert_eq!((position, velocity), (0.0, 0.0));
    }

    #[test]
    fn spring_bounds_frame_steps_and_clears_an_empty_range() {
        assert_eq!(
            step_timeline_spring(500.0, -100.0, 0.0, 1000.0, 2.0),
            step_timeline_spring(500.0, -100.0, 0.0, 1000.0, 0.05)
        );
        assert_eq!(
            step_timeline_spring(40.0, -500.0, 0.0, 0.0, 1.0 / 60.0),
            (0.0, 0.0, true)
        );
        assert_eq!(
            step_timeline_spring(40.0, 0.0, 0.0, 100.0, 0.0),
            (40.0, 0.0, false)
        );
    }

    #[test]
    fn ctrl_u_d_scroll_delta_is_signed_half_viewport() {
        assert_eq!(ctrl_u_d_scroll_delta(640.0, true), 320.0);
        assert_eq!(ctrl_u_d_scroll_delta(640.0, false), -320.0);
        assert_eq!(ctrl_u_d_scroll_delta(-12.0, true), 0.0);
    }
}
