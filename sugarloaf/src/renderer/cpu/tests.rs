use crate::renderer::batch::{BatchManager, Rect};

#[test]
fn clipped_quad_pixels_match_the_original_shape_inside_the_viewport() {
    let (width, height) = (200, 200);
    for scale in [0.75, 1.0, 1.5, 2.0] {
        for corners in [[8.0; 4], [8.0, 8.0, 0.0, 0.0]] {
            let rect = Rect::from([20.0, 20.0, 64.0, 48.0].map(|v| v * scale));
            let mut batches = BatchManager::new();
            batches.quad(&rect, 0.0, &[1.0; 4], corners.map(|v| v * scale), 5);
            let mut instances = Vec::new();
            batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
            let original = instances[0];
            let mut full = vec![0; (width * height) as usize];
            super::draw_quad_instance(&mut full, width, height, &original);
            for clip in [
                [10.0, 22.0, 100.0, 80.0],
                [10.0, 0.0, 100.0, 66.0],
                [22.0, 0.0, 100.0, 100.0],
                [0.0, 0.0, 82.0, 100.0],
                [10.0, 20.0, 100.0, 2.0],
                [10.0, 66.0, 100.0, 2.0],
                [0.0, 0.0, 4.0, 4.0],
            ] {
                let clip = clip.map(|v| v * scale);
                let mut instance = original;
                instance.clip_rect = clip;
                let mut pixels = vec![0; full.len()];
                super::draw_quad_instance(&mut pixels, width, height, &instance);
                for y in 0..height {
                    for x in 0..width {
                        let inside = x >= clip[0].round() as i32
                            && y >= clip[1].round() as i32
                            && x < (clip[0] + clip[2]).round() as i32
                            && y < (clip[1] + clip[3]).round() as i32;
                        let index = (y * width + x) as usize;
                        assert_eq!(pixels[index], if inside { full[index] } else { 0 },
                            "corner shape or viewport leaked: scale={scale}, clip={clip:?}, pixel=({x},{y})");
                    }
                }
            }
        }
    }
}
