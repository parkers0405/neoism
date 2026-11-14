use super::*;
use super::tests::{pixels, rect};

// Apple's continuous corner for r = 1, corner at the origin: three cubics from (0, K) to (K, 0),
// as Core Animation and SwiftUI draw it.
const CURVE: [[f32; 2]; 10] = [
    [0.0, 1.528_664_8], [0.0, 1.088_493_2], [0.0, 0.868_407], [0.074_911_41, 0.631_494],
    [0.169_06, 0.372_824], [0.372_824, 0.169_06], [0.631_494, 0.074_911_41],
    [0.868_407, 0.0], [1.088_493_2, 0.0], [1.528_664_8, 0.0],
];

fn polyline(r: f32) -> Vec<[f32; 2]> {
    let mut points = Vec::new();
    for c in CURVE[..].windows(4).step_by(3) {
        for k in 0..64 {
            let t = k as f32 / 64.0;
            let b = [(1.0 - t).powi(3), 3.0 * t * (1.0 - t).powi(2), 3.0 * t * t * (1.0 - t), t.powi(3)];
            points.push(std::array::from_fn(|a| (0..4).map(|j| b[j] * c[j][a]).sum::<f32>() * r));
        }
    }
    points.push(CURVE[9].map(|v| v * r));
    points
}

/// Signed distance (negative inside) from `p`, relative to the corner, to the curve, extended
/// along both edges.
fn oracle(p: [f32; 2], line: &[[f32; 2]]) -> f32 {
    let e = line[line.len() - 1][0];
    if p[0] >= e || p[1] >= e { return (-p[0]).max(-p[1]); }
    let mut best = (f32::MAX, 0.0);
    for s in line.windows(2) {
        let (a, b) = (s[0], s[1]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let ap = [p[0] - a[0], p[1] - a[1]];
        let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1])).clamp(0.0, 1.0);
        let d = (ap[0] - ab[0] * t).hypot(ap[1] - ab[1] * t);
        if d < best.0 { best = (d, ab[0] * ap[1] - ab[1] * ap[0]); }
    }
    if best.1 > 0.0 { -best.0 } else { best.0 }
}

#[test]
fn continuous_corners_follow_apples_curve_in_fills_and_clips() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 96, 80);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let (bounds, r) = ([8.0, 8.0, 72.0, 56.0], 14.0);
    let line = polyline(r);
    let mut fill = rect(bounds, [1.0; 4]);
    fill.radii = [-r; 4];
    let mut clipped = rect([0.0, 0.0, 96.0, 80.0], [1.0; 4]);
    clipped.clip = [8.0, 8.0, 80.0, 64.0];
    clipped.round_clip(-r);
    for instance in [fill, clipped] {
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[instance], (96, 80));
        let out = pixels(&gpu, &renderer, &target);
        let mut worst = 0.0f32;
        for y in 0..80 {
            for x in 0..96 {
                // Fold each pixel centre into its own corner's frame.
                let c = [x as f32 + 0.5, y as f32 + 0.5];
                let p = [(c[0] - 8.0).min(80.0 - c[0]), (c[1] - 8.0).min(64.0 - c[1])];
                let want = (0.5 - oracle(p, &line)).clamp(0.0, 1.0);
                let got = out[(y * 96 + x) * 4 + 3] as f32 / 255.0;
                worst = worst.max((got - want).abs());
            }
        }
        assert!(worst < 0.04, "coverage off the curve by {worst}");
    }
}

// Past half the shorter side, Core Animation still draws each corner's whole curve, folded
// into its own quadrant; the shape stays inside the rect and covers its centre.
#[test]
fn continuous_corners_at_half_the_shorter_side_stay_closed() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 64, 32);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let mut capsule = rect([0.0, 0.0, 64.0, 32.0], [1.0; 4]);
    capsule.radii = [-100.0; 4];
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[capsule], (64, 32));
    let out = pixels(&gpu, &renderer, &target);
    let alpha = |x: usize, y: usize| out[(y * 64 + x) * 4 + 3];
    assert_eq!(alpha(32, 16), 255);
    assert_eq!(alpha(32, 0), 255, "the top edge's centre is flat");
    assert_eq!(alpha(0, 0), 0);
    assert!(alpha(0, 16) > 64 && alpha(0, 16) < 255, "the end reaches the rect's side");
}
