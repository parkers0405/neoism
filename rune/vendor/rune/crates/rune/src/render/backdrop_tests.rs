use super::*;

fn rect(rect: [f32; 4], fill: [f32; 4]) -> Instance {
    Instance { rect, fill, clip: NO_CLIP, ..bytemuck::Zeroable::zeroed() }
}

#[test]
fn backdrop_pool_is_lazy_reused_resized_and_follows_live_formats() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let mut slot = rect([0.0, 0.0, 16.0, 16.0], [0.0, 0.0, 0.0, 1.0]);
    slot.params = [0.0, KIND_EFFECT, 0.0, 1.0];
    for instances in [&[][..], &[slot][..]] {
        renderer.prepare(device, queue, &mut text.atlas, instances, (32, 32));
        assert!(renderer.backdrops.is_empty(), "ordinary slots allocate no backdrops");
    }
    slot.params[0] = 1.0;
    renderer.prepare(device, queue, &mut text.atlas, &[slot], (32, 32));
    let original = renderer.backdrops[0].view.clone();
    renderer.prepare(device, queue, &mut text.atlas, &[slot], (32, 32));
    assert_eq!(renderer.backdrops[0].view, original);

    // Switching after prepare is legal: the next draw must already have the new format's pool.
    let format = OFFSCREEN_FORMAT.remove_srgb_suffix();
    renderer.set_format(device, format);
    assert!(renderer.backdrops.iter().any(|b| b.format == format));
    for (format, size) in [(format, (48, 24)), (OFFSCREEN_FORMAT, (24, 48)), (format, (32, 32))] {
        renderer.set_format(device, format);
        renderer.prepare(device, queue, &mut text.atlas, &[slot, slot], size);
        let target = Offscreen::with_format(device, size.0, size.1, format);
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut backdrops = Vec::new();
        renderer.draw_with_backdrop(&mut encoder, &target.view, [0.0; 4], &mut |phase, site| {
            if let EffectPhase::Backdrop(_) = phase {
                let view = site.backdrop.unwrap();
                assert_eq!((view.texture().width(), view.texture().height()), size);
                assert_eq!(view.texture().format(), format);
                backdrops.push(view.clone());
            }
        });
        queue.submit([encoder.finish()]);
        assert_eq!(backdrops.len(), 2);
        assert_eq!(backdrops[0], backdrops[1], "same-format slots share a texture");
        assert_eq!(renderer.backdrops.len(), 2);
    }
}

#[test]
fn split_resets_the_hook_viewport_and_scissor_for_later_primitives() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let mut slot = rect([0.0, 0.0, 16.0, 16.0], [0.0, 0.0, 0.0, 1.0]);
    slot.params = [1.0, KIND_EFFECT, 0.0, 1.0];
    slot.clip = [0.0, 0.0, 16.0, 16.0];
    let instances = [rect([0.0, 0.0, 32.0, 32.0], [1.0, 0.0, 0.0, 1.0]), slot,
        rect([16.0, 0.0, 16.0, 32.0], [0.0, 1.0, 0.0, 1.0])];
    renderer.prepare(device, queue, &mut text.atlas, &instances, (32, 32));
    let target = Offscreen::new(device, 32, 32);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.draw_with(&mut encoder, &target.view, [0.0; 4], &mut |pass, site| {
        assert!(site.backdrop.is_some());
        pass.set_viewport(0.0, 0.0, 1.0, 1.0, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, 1, 1);
    });
    target.copy(&mut encoder);
    queue.submit([encoder.finish()]);
    let pixels = target.read(device);
    for (x, color) in [(8, [255, 0, 0, 255]), (24, [0, 255, 0, 255])] {
        let at = (24 * 32 + x) * 4;
        assert_eq!(&pixels[at..at + 4], &color);
    }
}

#[test]
fn backdrop_slot_degrades_when_the_target_cannot_be_copied() {
    let gpu = Gpu::headless();
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut renderer = Renderer::new(device, OFFSCREEN_FORMAT);
    let mut text = crate::text::TextSystem::new(&[], ("Inter", "Inter"));
    let mut slot = rect([0.0, 0.0, 16.0, 16.0], [0.0, 0.0, 0.0, 1.0]);
    slot.params = [1.0, KIND_EFFECT, 0.0, 1.0];
    slot.clip = [0.0, 0.0, 16.0, 16.0];
    renderer.prepare(device, queue, &mut text.atlas, &[slot], (32, 32));
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("no copy src"),
        size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut draws = 0;
    renderer.draw_with(&mut encoder, &view, [0.0; 4], &mut |_, site| {
        assert!(site.backdrop.is_none());
        draws += 1;
    });
    queue.submit([encoder.finish()]);
    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "{error:?}");
    assert_eq!(draws, 1);
}
