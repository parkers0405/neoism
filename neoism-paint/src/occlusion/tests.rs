use super::*;
use crate::text::TextInstance;

#[test]
fn unchanged_output_keeps_storage_without_clone_or_tail_allocation() {
    struct Instance(usize);
    let mut items: Vec<_> = (0..20_000).map(Instance).collect();
    let pointer = items.as_ptr();
    let capacity = items.capacity();
    let mut visits = 0;
    filter_tail(&mut items, 7, |_| {
        visits += 1;
        None
    });
    assert_eq!(visits, 19_993);
    assert_eq!(items.as_ptr(), pointer);
    assert_eq!(items.capacity(), capacity);
    assert!(items
        .iter()
        .enumerate()
        .all(|(index, item)| item.0 == index));
}

#[test]
fn in_place_compaction_preserves_order_across_removals_and_multiple_splits() {
    let mut items = vec![0, 1, 2, 3, 4, 5, 6];
    filter_tail(&mut items, 1, |item| match item {
        0 => panic!("output before the scope must not be inspected"),
        1 | 4 => Some(Vec::new()),
        2 => Some(vec![20, 21, 22]),
        3 => Some(vec![30]),
        5 => Some(vec![50, 51]),
        _ => None,
    });
    assert_eq!(items, [0, 20, 21, 22, 30, 50, 51, 6]);
    filter_tail(&mut items, usize::MAX, |_| panic!("empty tail"));
    assert_eq!(items, [0, 20, 21, 22, 30, 50, 51, 6]);
}

#[test]
fn prepared_composer_holes_leave_all_distant_glyphs_verbatim() {
    let holes: Vec<_> = (0..128)
        .map(|row| [100.0, 800.0 + row as f32, 500.0, 1.0])
        .collect();
    let prepared = Occlusions::new(&holes);
    for index in 0..20_000 {
        let glyph = TextInstance {
            pos: [(index % 100) as f32 * 8.0, (index / 100) as f32 * 3.0],
            glyph_size: [8, 12],
            ..TextInstance::default()
        };
        assert!(glyph_parts_with(&glyph, &prepared).is_none());
    }
    // Broad-phase rejection must not remove the silhouette's visible corners.
    assert_eq!(
        prepared.visible([90.0, 800.0, 20.0, 1.0], [0.0; 4]),
        Some(vec![[90.0, 800.0, 10.0, 1.0]])
    );
}

fn area(r: Rect) -> f32 {
    r[2] * r[3]
}

#[test]
#[ignore = "manual CPU microbenchmark, not a frame-rate assertion"]
fn composer_occlusion_microbenchmark() {
    use std::hint::black_box;
    use std::time::Instant;
    let holes: Vec<_> = (0..128)
        .map(|row| [100.0, 800.0 + row as f32, 500.0, 1.0])
        .collect();
    let bounds = [120.0, 100.0, 8.0, 12.0];
    let started = Instant::now();
    for _ in 0..100_000 {
        black_box(visible_intersecting(
            black_box(bounds),
            [0.0; 4],
            black_box(&holes),
        ));
    }
    let prior = started.elapsed();
    let prepared = Occlusions::new(&holes);
    let started = Instant::now();
    for _ in 0..100_000 {
        black_box(prepared.visible(black_box(bounds), [0.0; 4]));
    }
    eprintln!(
        "100k distant glyphs: prior={prior:?}, prepared={:?}",
        started.elapsed()
    );
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
        let image = crate::graphics::GraphicOverlay {
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
    let image = crate::graphics::GraphicOverlay {
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
