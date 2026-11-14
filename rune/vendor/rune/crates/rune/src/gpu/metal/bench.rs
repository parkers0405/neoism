//! What `wgpu::custom` costs on top of the Metal calls it makes. The `metal_dispatch`
//! example runs it, in the perf profile (Rune's unit tests need debug assertions).

use objc2_metal::{MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLLoadAction, MTLRenderPassDescriptor, MTLStoreAction};

use super::resource::{MetalBuffer, MetalTexture};
use super::*;

// The libc crate declares it for macOS only; iOS's libSystem has it too.
unsafe extern "C" {
    fn proc_pid_rusage(pid: libc::c_int, flavor: libc::c_int, buffer: *mut libc::c_void) -> libc::c_int;
}

fn instructions() -> u64 {
    // SAFETY: proc_pid_rusage fills the rusage_info_v4 it is handed for RUSAGE_INFO_V4.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        assert_eq!(proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, (&raw mut info).cast()), 0);
        info.ri_instructions
    }
}

pub(super) fn gpu() -> (wgpu::Device, wgpu::Queue) {
    let adapter = pollster::block_on(instance().request_adapter(&Default::default())).unwrap();
    pollster::block_on(adapter.request_device(&Default::default())).unwrap()
}

pub(super) fn clear<'e>(encoder: &'e mut wgpu::CommandEncoder, view: &wgpu::TextureView) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("hud"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view, resolve_target: None, depth_slice: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.2, a: 0.4 }), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    })
}

/// The HUD's frame (a cleared pass, `writes` uniform writes, a submit) through wgpu's API into
/// this backend, against the same Metal calls made directly, offscreen. Prints the medians of 4
/// alternating rounds in instructions per frame.
#[doc(hidden)]
pub fn dispatch_overhead() {
    const FRAMES: u64 = 3000;
    let (device, queue) = gpu();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: 720, height: 160, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Bgra8UnormSrgb, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
    });
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let shared = device.as_custom::<MetalDevice>().unwrap().shared.clone();
    let raw_target = target.as_custom::<MetalTexture>().unwrap().raw.raw.clone();
    let raw_uniforms = uniforms.as_custom::<MetalBuffer>().unwrap().raw();
    let data = [0.5f32; 64];
    let bytes: &[u8] = bytemuck::cast_slice(&data);
    let via_wgpu = |writes: usize| {
        shared.pace();
        let view = target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        drop(clear(&mut encoder, &view));
        for _ in 0..writes { queue.write_buffer(&uniforms, 0, bytes); }
        queue.submit([encoder.finish()]);
    };
    let direct = |writes: usize| {
        shared.pace();
        let cb = shared.queue.commandBufferWithUnretainedReferences().unwrap();
        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: index 0 always exists.
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(&raw_target));
        color.setLoadAction(MTLLoadAction::Clear);
        color.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.2, alpha: 0.4 });
        color.setStoreAction(MTLStoreAction::Store);
        cb.renderCommandEncoderWithDescriptor(&pass).unwrap().endEncoding();
        for _ in 0..writes { raw_uniforms.write(0, bytes); }
        shared.commit(&cb);
    };
    let measure = |f: &dyn Fn(usize), writes| {
        for _ in 0..200 { f(writes); }
        let start = instructions();
        for _ in 0..FRAMES { f(writes); }
        (instructions() - start) as f64 / FRAMES as f64 / 1e3
    };
    let mut runs: [Vec<f64>; 4] = Default::default();
    for _ in 0..4 {
        for writes in [0, 1] {
            runs[writes * 2].push(measure(&via_wgpu, writes));
            runs[writes * 2 + 1].push(measure(&direct, writes));
        }
    }
    let median = |mut v: Vec<f64>| { v.sort_by(f64::total_cmp); (v[1] + v[2]) / 2.0 };
    for writes in [0, 1] {
        let (w, d) = (median(runs[writes * 2].clone()), median(runs[writes * 2 + 1].clone()));
        println!("writes={writes} kinstr/frame: wgpu::custom {w:.1}, direct {d:.1}, custom adds {:.1} (rounds {:.1?} vs {:.1?})",
            w - d, runs[writes * 2], runs[writes * 2 + 1]);
    }
}
