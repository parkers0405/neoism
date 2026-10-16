use crate::renderer::batch::{BatchManager, QuadInstance};

fn ring(center: [f32; 2], alpha: f32, clip: [f32; 4]) -> QuadInstance {
    let mut batches = BatchManager::new();
    batches.ring_clipped(center, 5.0, 2.0, &[1.0, 1.0, 1.0, alpha], 0.0, 0, clip);
    let mut instances = Vec::new();
    batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
    instances[0]
}

#[test]
fn ring_pixels_have_hole_analytic_aa_and_translucent_coverage() {
    for center in [[10.5, 10.5], [10.25, 10.75], [10.125, 10.375]] {
        for alpha in [1.0, 0.5, 0.2, 0.002] {
            let inst = ring(center, alpha, [0.0, 0.0, 24.0, 24.0]);
            let mut pixels = vec![0; 24 * 24];
            super::draw_quad_instance(&mut pixels, 24, 24, &inst);
            let mut partial = 0;
            for y in 0..24 {
                for x in 0..24 {
                    let dx = x as f32 + 0.5 - center[0];
                    let dy = y as f32 + 0.5 - center[1];
                    let coverage = (0.5
                        - (((dx * dx + dy * dy).sqrt() - 5.0).abs() - 1.0))
                        .clamp(0.0, 1.0);
                    let expected = (alpha * coverage * 255.0).round() as i32;
                    let actual = (pixels[y * 24 + x] & 255) as i32;
                    assert!(
                        (actual - expected).abs() <= 1,
                        "({x},{y}) actual={actual} expected={expected}"
                    );
                    if coverage > 0.0 && coverage < 1.0 {
                        partial += 1;
                    }
                }
            }
            assert!(partial > 0);
            assert_eq!(pixels[10 * 24 + 10], 0, "transparent hole");
            assert_eq!(pixels[0], 0, "outside stays transparent");
        }
    }
}

#[test]
fn ring_fractional_clips_select_original_geometry() {
    let original = ring([10.25, 10.75], 0.5, [0.0, 0.0, 24.0, 24.0]);
    let mut full = vec![0; 24 * 24];
    super::draw_quad_instance(&mut full, 24, 24, &original);
    for clip in [
        [10.25, 10.75, 10.0, 10.0],
        [0.0, 0.0, 10.25, 24.0],
        [4.6, 5.3, 2.0, 7.0],
    ] {
        let inst = ring([10.25, 10.75], 0.5, clip);
        assert_eq!(inst.pos, original.pos);
        assert_eq!(inst.size, original.size);
        assert_eq!(inst.corner_radii, original.corner_radii);
        let mut clipped = vec![0; full.len()];
        super::draw_quad_instance(&mut clipped, 24, 24, &inst);
        for y in 0..24 {
            for x in 0..24 {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let inside = px >= clip[0]
                    && px < clip[0] + clip[2]
                    && py >= clip[1]
                    && py < clip[1] + clip[3];
                assert_eq!(
                    clipped[y * 24 + x],
                    if inside { full[y * 24 + x] } else { 0 }
                );
            }
        }
    }
}
