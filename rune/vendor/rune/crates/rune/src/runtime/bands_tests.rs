use super::*;
use crate::render::{KIND_RECT, KIND_SHADOW, NO_CLIP};

fn shadow(rect: [f32; 4], sigma: f32) -> Instance {
    Instance { rect, radii: [16.0; 4], fill: [0.0, 0.0, 0.0, 0.3], border: [0.0; 4], params: [0.0, KIND_SHADOW, sigma, 1.0], uv: [0.0; 4], clip: NO_CLIP, fade: [0; 4] }
}

/// Each band's window in absolute device px, as `base_vs` clamps the grown quad to it.
fn windows(out: &[Instance], grow: f32) -> Vec<[f32; 4]> {
    out.iter().map(|i| {
        let [x, y, w, h] = i.rect;
        let [u0, v0, u1, v1] = i.uv;
        assert!(u1 < 0.0, "every band carries the mark");
        [(x - grow).max(x + u0), (y - grow).max(y + v0), (x + w + grow).min(x - u1), (y + h + grow).min(y + v1)]
    }).collect()
}

fn area(r: [f32; 4]) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

#[test]
fn a_big_shadow_draws_as_four_bands_around_the_box() {
    let mut out = Vec::new();
    let caster = [100.0, 100.0, 1000.0, 600.0];
    push_shadow(&mut out, shadow([100.0, 104.0, 1000.0, 600.0], 4.0), caster, [16.0; 4]);
    assert_eq!(out.len(), 4);
    let grow = 4.0 * 3.0 + 1.0;
    let bands = windows(&out, grow);
    let quad = [100.0 - grow, 104.0 - grow, 1100.0 + grow, 704.0 + grow];
    let hole = [117.0, 117.0, 1083.0, 683.0];
    let total: f32 = bands.iter().map(|&b| area(b)).sum();
    assert_eq!(total, area(quad) - area(hole), "the bands tile the quad around the hole");
    for (a, b) in bands.iter().enumerate().flat_map(|(k, a)| bands[k + 1..].iter().map(move |b| (a, b))) {
        assert_eq!(area([a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]), 0.0, "bands never overlap");
    }
    let whole = shadow([100.0, 104.0, 1000.0, 600.0], 4.0);
    assert!(out.iter().all(|i| (i.rect, i.radii, i.fill, i.params, i.clip) == (whole.rect, whole.radii, whole.fill, whole.params, whole.clip)), "each band shades as the whole did");
}

#[test]
fn a_small_shadow_stays_one_quad() {
    let mut out = Vec::new();
    push_shadow(&mut out, shadow([0.0, 2.0, 120.0, 80.0], 4.0), [0.0, 0.0, 120.0, 80.0], [8.0; 4]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].uv, [0.0; 4]);
}

#[test]
fn a_shadow_with_a_hole_already_stays_one_quad() {
    let mut out = Vec::new();
    let inst = Instance { uv: [0.0, 0.0, 1000.0, 600.0], ..shadow([0.0, 0.0, 1000.0, 600.0], 4.0) };
    push_shadow(&mut out, inst, [0.0, 0.0, 1000.0, 600.0], [8.0; 4]);
    assert_eq!(out.len(), 1);
}

#[test]
fn a_big_border_ring_draws_as_bands_and_a_filled_box_never_asks() {
    let mut out = Vec::new();
    let ring = Instance { rect: [10.0, 10.0, 800.0, 500.0], radii: [-16.0; 4], fill: [0.0; 4], border: [1.0; 4], params: [1.0, KIND_RECT, 0.0, 1.0], uv: [0.0; 4], clip: NO_CLIP, fade: [0; 4] };
    push_ring(&mut out, ring);
    assert_eq!(out.len(), 4);
    let bands = windows(&out, 1.0);
    // Continuous corners reach K times their radius along each edge.
    let e = 16.0 * crate::render::CONTINUOUS_K + 1.0 + 0.5 + 1.0;
    let hole = [10.0 + e, 10.0 + e, 810.0 - e, 510.0 - e];
    let total: f32 = bands.iter().map(|&b| area(b)).sum();
    assert!((total - (area([9.0, 9.0, 811.0, 511.0]) - area(hole))).abs() < 1.0);
}
