use super::{BatchManager, Rect};

#[test]
fn clipped_quad_preserves_original_bounds_and_corner_radii() {
    for scale in [0.75, 1.0, 1.5, 2.0] {
        for clip in [
            [10.0, 22.0, 100.0, 80.0],
            [10.0, 0.0, 100.0, 66.0],
            [22.0, 0.0, 100.0, 100.0],
            [0.0, 0.0, 82.0, 100.0],
            [10.0, 20.0, 100.0, 2.0],
            [10.0, 66.0, 100.0, 2.0],
        ] {
            for corners in [[8.0; 4], [8.0, 8.0, 0.0, 0.0]] {
                let mut batches = BatchManager::new();
                let rect = Rect::from([20.0, 20.0, 64.0, 48.0].map(|v| v * scale));
                let clip = clip.map(|v| v * scale);
                let corners = corners.map(|v| v * scale);
                batches.quad_clipped(&rect, 0.25, &[1.0; 4], corners, 5, clip);
                batches.quad(&rect, 0.5, &[1.0; 4], corners, 5);
                let mut instances = Vec::new();
                batches.build_display_list(
                    &mut instances,
                    &mut Vec::new(),
                    &mut Vec::new(),
                );
                assert_eq!(instances.len(), 2);
                assert_eq!(instances[0].pos, [rect.x, rect.y, 0.25]);
                assert_eq!(instances[0].size, [rect.width, rect.height]);
                assert_eq!(instances[0].corner_radii, corners);
                assert_eq!(instances[0].clip_rect, clip);
                assert_eq!(
                    instances[1].clip_rect, [0.0; 4],
                    "clip must not leak to the next primitive"
                );
                assert_eq!(batches.clip_rect, [0.0; 4]);
            }
        }
    }
}

#[test]
fn clipped_quad_does_not_mutate_an_existing_batch_clip() {
    let mut batches = BatchManager::new();
    let inherited = [1.0, 2.0, 100.0, 100.0];
    let explicit = [20.0, 24.0, 64.0, 10.0];
    batches.clip_rect = inherited;
    let rect = Rect::from([20.0, 20.0, 64.0, 48.0]);
    batches.quad_clipped(&rect, 0.0, &[1.0; 4], [8.0; 4], 5, explicit);
    batches.quad(&rect, 0.0, &[1.0; 4], [8.0; 4], 5);
    let mut instances = Vec::new();
    batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
    assert_eq!(instances[0].clip_rect, explicit);
    assert_eq!(instances[1].clip_rect, inherited);
    assert_eq!(batches.clip_rect, inherited);
}
