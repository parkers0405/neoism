//! Window frame changes app code asks for (`Tx::resize_window`, `Tx::center_window`): where
//! the frame lands, and the spring that carries it there one display frame at a time.

use rune_motion::Spring;

/// What handlers asked of their window's frame, merged across a commit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FrameRequest {
    /// Content size in points.
    pub size: Option<(f32, f32)>,
    /// Asked with `Tx::resize_window` rather than its `_instant` form.
    pub animate: bool,
    /// The spring the frame moves on; `None` jumps. Resolved when the runtime latches the
    /// request (`Tx::instant`, `Tx::motion`, reduced motion).
    pub spring: Option<Spring>,
    pub center: bool,
}

impl FrameRequest {
    /// A later request's size replaces an earlier one's; centring sticks.
    pub fn merge(self, later: FrameRequest) -> FrameRequest {
        let (size, animate, spring) = if later.size.is_some() { (later.size, later.animate, later.spring) } else { (self.size, self.animate, self.spring) };
        FrameRequest { size, animate, spring, center: self.center || later.center }
    }
}

/// Where a frame request lands, in top-down screen points `[x, y, w, h]`. `current` is the
/// window's outer frame, `outer` the outer size for the requested content size, `visible` the
/// usable area of its screen. The top edge stays put, as an AppKit window grows downward;
/// centring follows AppKit's `center` (centred across, a quarter of the spare height above).
/// A frame never moves further off the visible area than it already was.
pub(crate) fn target(current: [f32; 4], outer: Option<(f32, f32)>, center: bool, visible: [f32; 4]) -> [f32; 4] {
    let (w, h) = outer.unwrap_or((current[2], current[3]));
    let [vx, vy, vw, vh] = visible;
    let (mut x, mut y) = (current[0], current[1]);
    if center {
        x = vx + ((vw - w) / 2.0).floor();
        y = vy + ((vh - h) / 4.0).floor();
    }
    let (left, top) = if center { (vx, vy) } else { (current[0].min(vx), current[1].min(vy)) };
    let right = if center { vx + vw } else { (current[0] + current[2]).max(vx + vw) };
    let bottom = if center { vy + vh } else { (current[1] + current[3]).max(vy + vh) };
    x = x.min(right - w).max(left);
    y = y.min(bottom - h).max(top);
    [x, y, w, h]
}

/// A frame moving to `to` on a spring, sampled on the host's clock (seconds). Each component
/// is its own closed-form track, so a retarget keeps position and velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameAnim {
    pub spring: Option<Spring>,
    start: f64,
    from: [f32; 4],
    velocity: [f32; 4],
    pub to: [f32; 4],
    end: f64,
}

/// Half a device pixel at 2x: the frame rounds to whole pixels, so closer is landed.
const EPSILON: f32 = 0.25;

impl FrameAnim {
    pub fn new(spring: Option<Spring>, now: f64, from: [f32; 4], velocity: [f32; 4], to: [f32; 4]) -> FrameAnim {
        let end = spring.map_or(0.0, |s| (0..4).map(|i| s.settling_duration_from(from[i], velocity[i], to[i], EPSILON)).fold(0.0f32, f32::max));
        FrameAnim { spring, start: now, from, velocity, to, end: now + f64::from(end) }
    }

    /// Starts from where an earlier animation is now, at its speed, or from rest at `from`.
    pub fn retarget(previous: Option<&FrameAnim>, spring: Option<Spring>, now: f64, from: [f32; 4], to: [f32; 4]) -> FrameAnim {
        let (from, velocity) = previous.map_or((from, [0.0; 4]), |p| p.sample(now));
        FrameAnim::new(spring, now, from, if spring.is_some() { velocity } else { [0.0; 4] }, to)
    }

    pub fn sample(&self, now: f64) -> ([f32; 4], [f32; 4]) {
        let Some(spring) = self.spring.filter(|_| now < self.end) else { return (self.to, [0.0; 4]) };
        let t = (now - self.start).max(0.0) as f32;
        let mut at = [0.0; 4];
        let mut velocity = [0.0; 4];
        for i in 0..4 {
            (at[i], velocity[i]) = spring.sample(self.from[i], self.velocity[i], self.to[i], t);
        }
        (at, velocity)
    }

    pub fn done(&self, now: f64) -> bool { self.spring.is_none() || now >= self.end }

    /// Moves the whole path by `(dx, dy)`: someone dragged the window mid-flight, and the
    /// size carries on from where they put it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn shift(&mut self, dx: f32, dy: f32) {
        for rect in [&mut self.from, &mut self.to] {
            rect[0] += dx;
            rect[1] += dy;
        }
    }
}

/// A requested content size inside the window's minimum and maximum sizes.
pub(crate) fn clamp_size(size: (f32, f32), min: (f32, f32), max: Option<(f32, f32)>) -> (f32, f32) {
    let (max_w, max_h) = max.unwrap_or((f32::INFINITY, f32::INFINITY));
    (size.0.max(min.0).min(max_w), size.1.max(min.1).min(max_h))
}

/// Rounds a frame to whole device pixels, so content and drawable sizes agree.
pub(crate) fn snap(rect: [f32; 4], scale: f32) -> [f32; 4] {
    rect.map(|v| (v * scale).round() / scale)
}

/// The drawable size a window takes when its scale changes from `from` to `to` and it keeps its
/// logical size, rounded as winit rounds it. Windows sends `ScaleFactorChanged` before it resizes
/// the window, so its `inner_size` is still the old scale's then.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn rescaled(size: (u32, u32), from: f64, to: f64) -> (u32, u32) {
    let scale = |v: u32| (f64::from(v) / from * to).round() as u32;
    (scale(size.0), scale(size.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scale_change_keeps_the_logical_size() {
        // 980 x 760 points, dragged from a 150% laptop to a 100% display and back.
        assert_eq!(rescaled((1470, 1140), 1.5, 1.0), (980, 760));
        assert_eq!(rescaled((980, 760), 1.0, 1.5), (1470, 1140));
        // Odd scales round to the nearest pixel, and a round trip lands where it began.
        assert_eq!(rescaled((1225, 950), 1.25, 1.75), (1715, 1330));
        assert_eq!(rescaled(rescaled((1225, 950), 1.25, 1.75), 1.75, 1.25), (1225, 950));
        assert_eq!(rescaled((1001, 777), 1.0, 1.25), (1251, 971));
    }

    const SCREEN: [f32; 4] = [0.0, 33.0, 1728.0, 1084.0];

    #[test]
    fn a_resize_keeps_the_top_edge_and_stays_on_screen() {
        let window = [100.0, 200.0, 400.0, 432.0];
        assert_eq!(target(window, Some((400.0, 632.0)), false, SCREEN), [100.0, 200.0, 400.0, 632.0]);
        // Growing past the bottom of the screen lifts the window just enough.
        assert_eq!(target(window, Some((400.0, 1000.0)), false, SCREEN), [100.0, 117.0, 400.0, 1000.0]);
        // Taller than the screen: the top pins to the visible top.
        assert_eq!(target(window, Some((400.0, 2000.0)), false, SCREEN), [100.0, 33.0, 400.0, 2000.0]);
    }

    #[test]
    fn a_window_already_off_screen_is_not_pulled_back() {
        let window = [-200.0, 900.0, 400.0, 400.0];
        // Shrinking leaves it where the user put it.
        assert_eq!(target(window, Some((400.0, 300.0)), false, SCREEN), [-200.0, 900.0, 400.0, 300.0]);
        // Growing may go no further off than it already was.
        assert_eq!(target(window, Some((400.0, 600.0)), false, SCREEN), [-200.0, 700.0, 400.0, 600.0]);
    }

    #[test]
    fn centring_matches_appkit() {
        // Measured from NSWindow.center on a 1728 x 1084 visible frame: a quarter of the spare
        // height above, three quarters below.
        assert_eq!(target([0.0, 0.0, 400.0, 100.0], Some((400.0, 332.0)), true, [0.0, 0.0, 1728.0, 1084.0]), [664.0, 188.0, 400.0, 332.0]);
        assert_eq!(target([0.0, 0.0, 400.0, 100.0], Some((300.0, 932.0)), true, [0.0, 0.0, 1728.0, 1084.0]), [714.0, 38.0, 300.0, 932.0]);
        assert_eq!(target([5.0, 5.0, 800.0, 132.0], None, true, SCREEN), [464.0, 33.0 + 238.0, 800.0, 132.0]);
    }

    #[test]
    fn the_spring_lands_on_the_target_and_retargets_with_its_speed() {
        let spring = Some(Spring::new(0.3, 0.0));
        let anim = FrameAnim::new(spring, 1.0, [0.0, 0.0, 400.0, 400.0], [0.0; 4], [0.0, 0.0, 400.0, 600.0]);
        let (mid, v) = anim.sample(1.1);
        assert!(mid[3] > 400.0 && mid[3] < 600.0 && v[3] > 0.0, "{mid:?} {v:?}");
        assert_eq!(mid[0..3], [0.0, 0.0, 400.0]);
        assert!(!anim.done(1.1));
        assert!(anim.done(2.0));
        assert_eq!(anim.sample(2.0).0, anim.to);
        let back = FrameAnim::retarget(Some(&anim), spring, 1.1, [9.0; 4], [0.0, 0.0, 400.0, 400.0]);
        assert_eq!(back.sample(1.1).0, mid);
        assert!(back.sample(1.11).0[3] > mid[3], "keeps moving the way it was going for a moment");
        let jump = FrameAnim::retarget(Some(&anim), None, 1.1, [9.0; 4], [1.0; 4]);
        assert!(jump.done(1.1));
        assert_eq!(jump.sample(1.1).0, [1.0; 4]);
    }

    #[test]
    fn a_drag_mid_flight_keeps_the_new_origin_and_the_size_carries_on() {
        let spring = Some(Spring::new(0.3, 0.0));
        let mut anim = FrameAnim::new(spring, 0.0, [10.0, 20.0, 400.0, 400.0], [0.0; 4], [10.0, 20.0, 400.0, 600.0]);
        let before = anim.sample(0.1).0;
        anim.shift(50.0, -5.0);
        let after = anim.sample(0.1).0;
        assert_eq!(after, [before[0] + 50.0, before[1] - 5.0, before[2], before[3]]);
        assert_eq!(anim.to, [60.0, 15.0, 400.0, 600.0]);
    }

    #[test]
    fn requested_sizes_stay_inside_the_window_limits() {
        assert_eq!(clamp_size((300.0, 900.0), (420.0, 0.0), Some((800.0, 700.0))), (420.0, 700.0));
        assert_eq!(clamp_size((500.0, 100.0), (420.0, 200.0), None), (500.0, 200.0));
    }

    #[test]
    fn later_sizes_win_and_centring_sticks() {
        let a = FrameRequest { size: Some((1.0, 2.0)), animate: true, spring: None, center: true };
        let b = FrameRequest { size: Some((3.0, 4.0)), animate: false, spring: None, center: false };
        assert_eq!(a.merge(b), FrameRequest { size: Some((3.0, 4.0)), animate: false, spring: None, center: true });
        assert_eq!(b.merge(FrameRequest { center: true, ..Default::default() }), FrameRequest { center: true, ..b });
    }
}
