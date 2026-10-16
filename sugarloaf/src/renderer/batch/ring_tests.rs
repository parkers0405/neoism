use super::{BatchManager, Rect};

#[test]
fn ring_clipped_preserves_geometry_order_and_following_quad() {
    for scale in [0.75, 1.0, 1.5, 2.0] {
        let mut batches = BatchManager::new();
        let inherited = [1.0, 2.0, 100.0, 100.0];
        batches.clip_rect = inherited;
        let clip = [20.25, 20.75, 6.0, 7.0].map(|v| v * scale);
        let color = [0.2, 0.4, 0.6, 0.5];
        batches.ring_clipped(
            [20.0 * scale; 2],
            8.0 * scale,
            2.0 * scale,
            &color,
            0.25,
            9,
            clip,
        );
        let rect = Rect::from([0.0, 0.0, 2.0, 2.0]);
        batches.quad(&rect, 0.0, &[1.0; 4], [0.0; 4], 9);
        batches.quad(&rect, 0.0, &[1.0; 4], [0.0; 4], 2);
        assert_eq!(
            batches.active.iter().map(|b| b.order).collect::<Vec<_>>(),
            [9, 2]
        );
        let mut instances = Vec::new();
        batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
        assert_eq!(instances.len(), 3);
        assert_eq!(instances[0].underline_style, 0); // order 2 first
        let ring = instances[1];
        let extent = 9.0 * scale + 0.5;
        assert_eq!(
            ring.pos,
            [20.0 * scale - extent, 20.0 * scale - extent, 0.25]
        );
        assert_eq!(ring.size, [2.0 * extent; 2]);
        assert_eq!(ring.corner_radii, [extent, extent, 8.0 * scale, scale]);
        assert_eq!(ring.underline_style, -1);
        assert_eq!(ring.layers, [0, 0]);
        assert_eq!(ring.color, color);
        assert_eq!(ring.clip_rect, clip);
        assert_eq!(instances[2].underline_style, 0);
        assert_eq!(instances[2].corner_radii, [0.0; 4]);
        assert_eq!(instances[2].clip_rect, inherited);
        assert_eq!(batches.clip_rect, inherited);
    }
}

#[test]
fn ring_clipped_rejects_invalid_inputs() {
    let mut batches = BatchManager::new();
    for bad in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        -1.0,
        f32::MAX,
    ] {
        batches.ring_clipped(
            [10.0; 2],
            bad,
            1.0,
            &[1.0; 4],
            0.0,
            0,
            [0.0, 0.0, 20.0, 20.0],
        );
        batches.ring_clipped(
            [10.0; 2],
            4.0,
            bad,
            &[1.0; 4],
            0.0,
            0,
            [0.0, 0.0, 20.0, 20.0],
        );
        batches.ring_clipped(
            [10.0; 2],
            4.0,
            1.0,
            &[1.0; 4],
            0.0,
            0,
            [bad, 0.0, bad, 20.0],
        );
    }
    batches.ring_clipped(
        [f32::NAN; 2],
        4.0,
        1.0,
        &[1.0; 4],
        0.0,
        0,
        [0.0, 0.0, 20.0, 20.0],
    );
    batches.ring_clipped(
        [10.0; 2],
        4.0,
        1.0,
        &[1.0; 4],
        f32::NAN,
        0,
        [0.0, 0.0, 20.0, 20.0],
    );
    batches.ring_clipped(
        [10.0; 2],
        4.0,
        1.0,
        &[f32::NAN; 4],
        0.0,
        0,
        [0.0, 0.0, 20.0, 20.0],
    );
    assert!(batches.active.is_empty());
}

#[test]
fn ring_webgpu_shader_parses_and_validates() {
    let module = naga::front::wgsl::parse_str(include_str!("../renderer.wgsl"))
        .expect("renderer WGSL must parse");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("renderer WGSL must validate");
}
