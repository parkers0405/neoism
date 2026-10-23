use super::{BatchManager, Rect};

#[test]
fn inverse_scope_non_quad_triangles_preserve_vertices_and_interpolation() {
    let mut batches = BatchManager::new();
    let checkpoint = batches.occlusion_checkpoint();
    batches.add_triangle(0.0, 0.0, 20.0, 0.0, 10.0, 20.0, 0.5, [1.0; 4]);
    let original = batches.active[0].vertices.clone();
    batches.occlude_since(&checkpoint, &[[8.0, 0.0, 4.0, 20.0]]);
    let vertices = &batches.active[0].vertices;
    assert_eq!(vertices.len(), 6);
    for triangle in vertices.chunks_exact(3) {
        for (actual, expected) in triangle.iter().zip(&original) {
            assert_eq!(actual.pos, expected.pos);
            assert_eq!(actual.uv, expected.uv);
            assert_eq!(actual.color, expected.color);
            assert!(crate::occlusion::intersection(
                actual.clip_rect,
                [8.0, 0.0, 4.0, 20.0]
            )
            .is_none());
        }
    }
}

#[test]
fn inverse_scope_atlas_image_keeps_original_uv_and_geometry() {
    let mut batches = BatchManager::new();
    let checkpoint = batches.occlusion_checkpoint();
    let rect = Rect::from([10.0, 20.0, 100.0, 50.0]);
    let uv = [0.2, 0.3, 0.8, 0.9];
    batches.add_image_rect(&rect, 0.5, &[1.0; 4], &uv, 3);
    batches.occlude_since(&checkpoint, &[[35.0, 20.0, 50.0, 50.0]]);
    let mut instances = Vec::new();
    batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
    assert_eq!(instances.len(), 2);
    for instance in instances {
        assert_eq!(instance.pos, [10.0, 20.0, 0.5]);
        assert_eq!(instance.size, [100.0, 50.0]);
        assert_eq!(instance.uv_rect, uv);
        assert_eq!(instance.layers[0], 3);
        assert!(crate::occlusion::intersection(
            instance.clip_rect,
            [35.0, 20.0, 50.0, 50.0]
        )
        .is_none());
    }
}

#[test]
fn inverse_scope_checkpoints_restore_without_reclipping_prior_or_later_content() {
    for scale in [0.75, 1.0, 1.5, 2.0] {
        let mut batches = BatchManager::new();
        let rect = Rect::from([0.0, 0.0, 20.0, 20.0].map(|v| v * scale));
        let corners = [4.0 * scale; 4];
        let outer = [[8.0, 0.0, 4.0, 20.0].map(|v| v * scale)];
        let inner = [[0.0, 8.0, 20.0, 4.0].map(|v| v * scale)];
        batches.quad(&rect, 0.0, &[1.0; 4], corners, 5); // Before scope.
        let start = batches.occlusion_checkpoint();
        batches.quad(&rect, 0.1, &[1.0; 4], corners, 5);
        batches.occlude_since(&start, &outer); // Enter nested replacement.
        let nested = batches.occlusion_checkpoint();
        batches.quad(&rect, 0.2, &[1.0; 4], corners, 5);
        batches.occlude_since(&nested, &inner); // Restore outer.
        let restored = batches.occlusion_checkpoint();
        // Different draw order forces a new batch; checkpoint must cover it too.
        batches.quad(&rect, 0.3, &[1.0; 4], corners, 6);
        batches.occlude_since(&restored, &outer); // Restore empty scope.
        batches.quad(&rect, 0.4, &[1.0; 4], corners, 6); // Composer.
        let mut instances = Vec::new();
        batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
        assert_eq!(instances.len(), 8);
        assert_eq!(instances[0].clip_rect, [0.0; 4]);
        assert_eq!(instances[7].clip_rect, [0.0; 4]);
        for instance in &instances {
            assert_eq!(instance.size, [20.0 * scale; 2]);
            assert_eq!(instance.pos[..2], [0.0; 2]);
            assert_eq!(instance.corner_radii, corners);
        }
        for instance in &instances[1..7] {
            let hole = if instance.pos[2] == 0.2 {
                inner[0]
            } else {
                outer[0]
            };
            assert!(crate::occlusion::intersection(instance.clip_rect, hole).is_none());
        }
        assert_eq!(batches.clip_rect, [0.0; 4]);
    }
}

#[test]
fn inverse_scope_intersects_explicit_quad_clip() {
    let mut batches = BatchManager::new();
    let rect = Rect::from([0.0, 0.0, 20.0, 20.0]);
    let checkpoint = batches.occlusion_checkpoint();
    let clip = [2.0, 2.0, 16.0, 16.0];
    batches.quad_clipped(&rect, 0.0, &[1.0; 4], [4.0; 4], 0, clip);
    batches.occlude_since(&checkpoint, &[[8.0, 0.0, 4.0, 20.0]]);
    let mut instances = Vec::new();
    batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
    assert_eq!(instances.len(), 2);
    for instance in instances {
        assert_eq!(
            crate::occlusion::intersection(instance.clip_rect, clip),
            Some(instance.clip_rect)
        );
        assert_eq!(instance.size, [20.0; 2]);
        assert_eq!(instance.corner_radii, [4.0; 4]);
    }
}

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
