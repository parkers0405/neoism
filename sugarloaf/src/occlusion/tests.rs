use super::*;
use crate::text::TextInstance;

fn area(r: Rect) -> f32 {
    r[2] * r[3]
}

#[test]
fn scanline_silhouette_keeps_corners_not_just_bounding_box() {
    let holes = [
        [3.0, 0.0, 4.0, 2.0],
        [1.0, 2.0, 8.0, 6.0],
        [3.0, 8.0, 4.0, 2.0],
    ];
    let parts = visible([0.0, 0.0, 10.0, 10.0], [0.0; 4], &holes).unwrap();
    assert_eq!(parts.iter().copied().map(area).sum::<f32>(), 36.0);
    for (i, p) in parts.iter().enumerate() {
        assert!(holes.iter().all(|h| intersection(*p, *h).is_none()));
        assert!(parts[i + 1..]
            .iter()
            .all(|r| intersection(*p, *r).is_none()));
    }
    assert!(parts
        .iter()
        .any(|r| intersection(*r, [0.0, 0.0, 2.0, 2.0]).is_some()));
}

#[test]
fn glyph_partial_intersection_preserves_atlas_and_reveal_clip_at_device_scales() {
    for scale in [0.75, 1.0, 1.5, 2.0] {
        let glyph = TextInstance {
            pos: [10.0 * scale, 20.0 * scale],
            glyph_pos: [123, 456],
            glyph_size: [12, 8],
            bearings: [-2, -3],
            raster_scale: scale,
            clip_rect: [9.0, 18.0, 8.0, 6.0].map(|v| v * scale),
            color: [128; 4],
            blur_radius: 1.5 * scale,
            ..TextInstance::default()
        };
        let hole = [12.0, 19.0, 3.0, 2.0].map(|v| v * scale);
        let parts = glyph_parts(&glyph, &[hole]).unwrap();
        assert_eq!(parts.len(), 4);
        for part in parts {
            assert_eq!(part.pos, glyph.pos);
            assert_eq!(part.bearings, glyph.bearings);
            assert_eq!(part.glyph_pos, glyph.glyph_pos);
            assert_eq!(part.glyph_size, glyph.glyph_size);
            assert_eq!(part.raster_scale, scale);
            assert_eq!(part.blur_radius, glyph.blur_radius);
            assert!(intersection(part.clip_rect, hole).is_none());
            assert_eq!(
                intersection(part.clip_rect, glyph.clip_rect),
                Some(part.clip_rect)
            );
        }
        assert!(glyph_parts(&glyph, &[[1000.0; 4]]).is_none());
        assert!(glyph_parts(&glyph, &[[0.0, 0.0, 100.0, 100.0]])
            .unwrap()
            .is_empty());
    }
}

#[test]
fn image_splits_preserve_cropped_or_flipped_uvs_and_metadata() {
    for uv in [[0.2, 0.3, 0.8, 0.9], [0.8, 0.9, 0.2, 0.3]] {
        let image = crate::sugarloaf::graphics::GraphicOverlay {
            image_id: 42,
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            z_index: 99,
            source_rect: uv,
        };
        let parts = image_parts(&image, &[[35.0, 20.0, 50.0, 50.0]]).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].width, 25.0);
        assert_eq!(parts[1].x, 85.0);
        for p in &parts {
            assert_eq!(p.image_id, image.image_id);
            assert_eq!(p.z_index, image.z_index);
            assert_eq!(p.source_rect[1], uv[1]);
            assert_eq!(p.source_rect[3], uv[3]);
        }
        assert!(
            (parts[0].source_rect[2] - (uv[0] + (uv[2] - uv[0]) * 0.25)).abs() < 1e-6
        );
        assert!(
            (parts[1].source_rect[0] - (uv[0] + (uv[2] - uv[0]) * 0.75)).abs() < 1e-6
        );
    }
}

#[test]
fn deferred_image_ranges_keep_prior_and_restored_output_untouched() {
    let image = crate::sugarloaf::graphics::GraphicOverlay {
        image_id: 7,
        x: 0.0,
        y: 0.0,
        width: 20.0,
        height: 20.0,
        source_rect: [0.0, 0.0, 1.0, 1.0],
        z_index: 1,
    };
    let mut images = vec![image.clone()];
    let first = images.len();
    images.push(image.clone());
    // Final placement after any producer-side adjustment is what gets clipped.
    images[first].x = 2.0;
    filter_tail(&mut images, first, |image| {
        image_parts(image, &[[8.0, 0.0, 4.0, 20.0]])
    });
    assert_eq!(images.len(), 3);
    assert_eq!(images[0].source_rect, image.source_rect);
    assert_eq!(images[0].width, image.width);
    let restored = images.len();
    filter_tail(&mut images, restored, |image| {
        image_parts(image, &[[8.0, 0.0, 4.0, 20.0]])
    });
    assert_eq!(images.len(), restored); // Repeated setter with no emissions.
    images.push(image.clone());
    assert_eq!(images.last().unwrap().width, image.width);
    assert_eq!(images.last().unwrap().source_rect, image.source_rect);
}

#[test]
fn overlaps_empty_and_touching_holes_are_safe() {
    let bounds = [0.0, 0.0, 10.0, 10.0];
    assert!(visible(bounds, [0.0; 4], &[]).is_none());
    assert!(visible(bounds, [0.0; 4], &[[10.0, 0.0, 3.0, 3.0]]).is_none());
    let holes = [[2.0, 2.0, 6.0, 6.0]; 2];
    assert_eq!(
        visible(bounds, [0.0; 4], &holes)
            .unwrap()
            .into_iter()
            .map(area)
            .sum::<f32>(),
        64.0
    );
    assert!(!valid([0.0, 0.0, f32::NAN, 1.0]));
    assert!(!valid([0.0; 4]));
}
