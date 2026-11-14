//! Timing curves for tweens: CSS `cubic-bezier()`, `steps()` and `linear()`.

/// A CSS timing function: maps progress `x ∈ [0, 1]` to eased progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    Linear,
    /// `cubic-bezier(x1, y1, x2, y2)`, solved exactly like browsers do.
    CubicBezier(CubicBezier),
    /// `steps(n, position)`.
    Steps(u16, StepPosition),
    /// CSS `linear(...)`: piecewise-linear stops `[x, y]`, sorted by `x`, from `x = 0` to `x = 1`.
    /// Use [`crate::Spring::css_linear`] to bake a spring for the web.
    Piecewise(&'static [[f32; 2]]),
}

/// Where a `steps()` jump happens (CSS `jump-start`, `jump-end`, `jump-none`, `jump-both`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepPosition {
    Start,
    End,
    None,
    Both,
}

impl Easing {
    pub const LINEAR: Easing = Easing::Linear;
    pub const EASE: Easing = Easing::cubic(0.25, 0.1, 0.25, 1.0);
    pub const EASE_IN: Easing = Easing::cubic(0.42, 0.0, 1.0, 1.0);
    pub const EASE_OUT: Easing = Easing::cubic(0.0, 0.0, 0.58, 1.0);
    pub const EASE_IN_OUT: Easing = Easing::cubic(0.42, 0.0, 0.58, 1.0);
    /// Kit's CSS enter: `cubic-bezier(.2,.8,.2,1)`, 150 ms.
    pub const ENTER: Easing = Easing::cubic(0.2, 0.8, 0.2, 1.0);
    /// Kit's CSS exit: `cubic-bezier(.4,0,1,1)`, 110 ms.
    pub const EXIT: Easing = Easing::cubic(0.4, 0.0, 1.0, 1.0);
    /// Kit's overshoot for marks: `cubic-bezier(.34,1.45,.64,1)`.
    pub const OVERSHOOT: Easing = Easing::cubic(0.34, 1.45, 0.64, 1.0);
    /// Kit's draw-on: `cubic-bezier(.45,0,.2,1)`.
    pub const DRAW_ON: Easing = Easing::cubic(0.45, 0.0, 0.2, 1.0);

    /// `cubic-bezier(x1, y1, x2, y2)`. `x1` and `x2` are clamped to `[0, 1]` like CSS requires.
    pub const fn cubic(x1: f32, y1: f32, x2: f32, y2: f32) -> Easing {
        Easing::CubicBezier(CubicBezier::new(x1, y1, x2, y2))
    }

    /// Eased progress at `x`.
    #[inline]
    pub fn eval(&self, x: f32) -> f32 {
        self.eval_with_slope(x).0
    }

    /// `(y, dy/dx)` at `x`. The slope turns into velocity when a tween is retargeted.
    #[inline]
    pub fn eval_with_slope(&self, x: f32) -> (f32, f32) {
        match self {
            Easing::Linear => (x, 1.0),
            Easing::CubicBezier(b) => b.eval_with_slope(x),
            Easing::Steps(n, pos) => (steps(x, *n, *pos), 0.0),
            Easing::Piecewise(stops) => piecewise(x, stops),
        }
    }
}

/// A unit cubic Bézier through (0,0), (x1,y1), (x2,y2), (1,1), stored as polynomial coefficients.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl CubicBezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        let x1 = if x1 < 0.0 { 0.0 } else if x1 > 1.0 { 1.0 } else { x1 };
        let x2 = if x2 < 0.0 { 0.0 } else if x2 > 1.0 { 1.0 } else { x2 };
        CubicBezier { x1, y1, x2, y2 }
    }

    #[inline]
    fn coefficients(a1: f32, a2: f32) -> (f32, f32, f32) {
        let c = 3.0 * a1;
        let b = 3.0 * (a2 - a1) - c;
        let a = 1.0 - c - b;
        (a, b, c)
    }

    /// Solves `x(t) = x` the way WebKit's `UnitBezier` does (Newton, then bisection), then returns
    /// `y(t)` and `dy/dx`.
    pub fn eval_with_slope(&self, x: f32) -> (f32, f32) {
        if x <= 0.0 {
            return (0.0, self.slope_at(0.0));
        }
        if x >= 1.0 {
            return (1.0, self.slope_at(1.0));
        }
        let t = self.solve_t(x);
        let (ay, by, cy) = Self::coefficients(self.y1, self.y2);
        let y = ((ay * t + by) * t + cy) * t;
        (y, self.slope_at(t))
    }

    fn slope_at(&self, t: f32) -> f32 {
        let (ax, bx, cx) = Self::coefficients(self.x1, self.x2);
        let (ay, by, cy) = Self::coefficients(self.y1, self.y2);
        let dx = (3.0 * ax * t + 2.0 * bx) * t + cx;
        let dy = (3.0 * ay * t + 2.0 * by) * t + cy;
        if dx.abs() < 1e-6 {
            // Vertical tangent (x1 = 0 at t = 0): report a steep but finite slope.
            return if dy == 0.0 { 0.0 } else { dy.signum() * 1e3 };
        }
        dy / dx
    }

    fn solve_t(&self, x: f32) -> f32 {
        const EPS: f32 = 1e-7;
        let (ax, bx, cx) = Self::coefficients(self.x1, self.x2);
        let sample = |t: f32| ((ax * t + bx) * t + cx) * t;
        let mut t = x;
        for _ in 0..8 {
            let err = sample(t) - x;
            if err.abs() < EPS {
                return t;
            }
            let d = (3.0 * ax * t + 2.0 * bx) * t + cx;
            if d.abs() < 1e-6 {
                break;
            }
            t -= err / d;
        }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        t = x;
        for _ in 0..40 {
            let v = sample(t);
            if (v - x).abs() < EPS {
                break;
            }
            if x > v {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) * 0.5;
        }
        t
    }
}

fn steps(x: f32, n: u16, pos: StepPosition) -> f32 {
    if x >= 1.0 {
        return 1.0;
    }
    if x <= 0.0 && !matches!(pos, StepPosition::Start | StepPosition::Both) {
        return 0.0;
    }
    let n = n.max(1) as f32;
    let mut step = (x.max(0.0) * n).floor();
    if matches!(pos, StepPosition::Start | StepPosition::Both) {
        step += 1.0;
    }
    let jumps = match pos {
        StepPosition::Start | StepPosition::End => n,
        StepPosition::Both => n + 1.0,
        StepPosition::None => (n - 1.0).max(1.0),
    };
    (step.min(jumps) / jumps).clamp(0.0, 1.0)
}

fn piecewise(x: f32, stops: &[[f32; 2]]) -> (f32, f32) {
    match stops {
        [] => (x, 1.0),
        [only] => (only[1], 0.0),
        _ => {
            let x = x.clamp(stops[0][0], stops[stops.len() - 1][0]);
            let i = stops.partition_point(|s| s[0] <= x).clamp(1, stops.len() - 1);
            let [x0, y0] = stops[i - 1];
            let [x1, y1] = stops[i];
            let dx = x1 - x0;
            if dx <= 0.0 {
                return (y1, 0.0);
            }
            let slope = (y1 - y0) / dx;
            (y0 + (x - x0) * slope, slope)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_bezier_known_values() {
        // Chrome: getComputedTiming for `ease` at 50 % progress is ~0.8024.
        let y = Easing::EASE.eval(0.5);
        assert!((y - 0.802_4).abs() < 1e-3, "{y}");
        let y = Easing::EASE_IN_OUT.eval(0.5);
        assert!((y - 0.5).abs() < 1e-5);
        assert_eq!(Easing::EASE_OUT.eval(0.0), 0.0);
        assert_eq!(Easing::EASE_OUT.eval(1.0), 1.0);
        // The overshoot curve rises above 1.
        assert!((0..100).any(|i| Easing::OVERSHOOT.eval(i as f32 / 100.0) > 1.05));
    }

    #[test]
    fn bezier_is_monotone_in_x_and_solves_exactly() {
        let b = CubicBezier::new(0.2, 0.8, 0.2, 1.0);
        let (ax, bx, cx) = CubicBezier::coefficients(b.x1, b.x2);
        for i in 1..1000 {
            let x = i as f32 / 1000.0;
            let t = b.solve_t(x);
            let back = ((ax * t + bx) * t + cx) * t;
            assert!((back - x).abs() < 1e-5);
        }
    }

    #[test]
    fn steps_follow_css() {
        assert_eq!(steps(0.0, 4, StepPosition::End), 0.0);
        assert_eq!(steps(0.3, 4, StepPosition::End), 0.25);
        assert_eq!(steps(0.3, 4, StepPosition::Start), 0.5);
        assert_eq!(steps(0.5, 2, StepPosition::None), 1.0);
        assert_eq!(steps(0.4, 2, StepPosition::Both), 1.0 / 3.0);
        assert_eq!(steps(1.0, 3, StepPosition::Both), 1.0);
    }

    #[test]
    fn piecewise_linear() {
        let e = Easing::Piecewise(&[[0.0, 0.0], [0.5, 0.8], [1.0, 1.0]]);
        assert!((e.eval(0.25) - 0.4).abs() < 1e-6);
        assert!((e.eval(0.75) - 0.9).abs() < 1e-6);
        assert_eq!(e.eval_with_slope(0.25).1, 1.6);
    }
}
