//! Ordered rail material. Unlike the cursor's final Metal pass, this effect slot is
//! BELOW the original rail children: no glyph copies, overlay views or extra hit targets.
use crate::app::{App, Sidebar};
use rune::{El, el};

const MARK: u32 = 0x4e475301;

pub fn active(app: &App) -> Option<usize> {
    if app.menu.is_some() { return Some(0); }
    if app.finder.is_some() { return Some(4); }
    if app.picker.as_ref().is_some_and(|p| p.mode == crate::palette::Mode::Systems) { return Some(5); }
    match app.sidebar {
        Some(Sidebar::Files) => Some(1),
        Some(Sidebar::Notes) => Some(2),
        Some(Sidebar::Conversations) => Some(3),
        None => None,
    }
}

fn optical() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" { fn neoism_glass_selection_allowed() -> bool; }
        (unsafe { neoism_glass_selection_allowed() })
            && std::env::var("RUNE_GPU_BACKEND").as_deref() != Ok("wgpu")
            && !std::env::args().any(|a| matches!(a.as_str(), "--snap" | "--tree"))
    }
    #[cfg(not(target_os = "macos"))]
    { false }
}

pub fn view(app: &App) -> El {
    let slot = active(app);
    let visible = app.liquid_glass_selection && slot.is_some();
    // Keep one stable identity, even while no panel is open. Offset uses Rune's render
    // thread spring; focus and cursor blink are deliberately not inputs.
    let y = 12.25 + slot.unwrap_or(0) as f32 * 51.5;
    let native = visible && app.glass_selection_policy.0;
    let light = ((app.pal.bg >> 16) & 255) + ((app.pal.bg >> 8) & 255) + (app.pal.bg & 255) > 384;
    let base = el().key("active-rail-material").absolute(7.75, 0.0).w(40.0).h(40.0)
        .offset(0.0, y).offset_motion(if app.glass_selection_policy.1 {
            rune::Anim::INSTANT
        } else { rune::Spring::new(0.42, 0.0).into() })
        .radius(14.0);
    if native {
        base.effect(MARK).effect_backdrop(true).effect_bleed(4.0)
            .effect_data([if light { 1.0 } else { 0.0 }, if app.glass_selection_policy.1 { 1.0 } else { 0.0 }, 0.0, 0.0])
    } else if visible {
        base.fill(crate::ui::theme::rgb(app.pal.bg))
            .border(1.0, crate::ui::theme::rgb(app.pal.dim))
    } else { base }
}

/// Accessibility queries stay on the UI thread, never inside the render callback.
/// A policy change commits once; the periodic idle check does not request a frame.
pub fn poll(app: &mut App, tx: &mut rune::Tx) -> bool {
    let policy = (app.liquid_glass_selection && optical(), crate::glass_cursor::reduce_motion());
    if app.liquid_glass_selection { tx.wake_after(std::time::Duration::from_secs(1)); }
    let changed = app.glass_selection_policy != policy;
    app.glass_selection_policy = policy;
    changed
}

#[derive(Default)]
pub struct Compositor {
    gpu: Option<(rune::compose::DeviceKey, wgpu::TextureFormat, Material)>,
    last: Option<f32>,
}

impl rune::compose::Compositor<App> for Compositor {
    fn render(&mut self, f: &mut rune::compose::Frame<'_, App>) {
        if !f.rt.instances.iter().any(|i| i.params[1] == rune::render::KIND_EFFECT) {
            self.last = None;
            f.draw_ui();
            return;
        }
        let clear = f.rt.clear_color();
        f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        let key = f.device_key();
        let mut seen = false;
        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            if site.mark != MARK { return; }
            let Some(backdrop) = site.backdrop else { return; };
            seen = true;
            if self.gpu.as_ref().is_none_or(|(k, format, _)| *k != key || *format != site.format) {
                if std::env::var_os("NEOISM_RUNE_TRACE_GLASS_SELECTION").is_some() {
                    eprintln!("[rail-glass] native slot rect={:?} scale={} format={:?}", site.instance.rect, site.scale, site.format);
                }
                self.gpu = Some((key, site.format, Material::new(f.device, site.format)));
            }
            let rect = site.instance.rect;
            let speed = self.last.map_or(0.0, |y| (rect[1] - y) / site.scale / (f.dt as f32).max(0.001));
            self.last = Some(rect[1]);
            let stretch = if site.data[1] > 0.0 { 0.0 } else { (speed.abs() / 1200.0).clamp(0.0, 1.0) };
            let data = [rect[0] - site.origin[0], rect[1] - site.origin[1], rect[2], rect[3],
                site.size[0] as f32, site.size[1] as f32, site.scale, stretch,
                site.data[0], if site.srgb_blend { 1.0 } else { 0.0 }, 0.0, 0.0];
            let material = &self.gpu.as_ref().unwrap().2;
            let mut bytes = [0u8; 48];
            for (chunk, value) in bytes.chunks_exact_mut(4).zip(data) { chunk.copy_from_slice(&value.to_ne_bytes()); }
            f.queue.write_buffer(&material.uniform, 0, &bytes);
            let bind = f.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rail glass"), layout: &material.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: material.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(backdrop) },
                ],
            });
            pass.set_pipeline(&material.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..6, 0..1);
        });
        if !seen { self.last = None; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_tracks_panel_not_keyboard_focus_or_cursor() {
        let mut app = App::new();
        app.menu = None;
        app.finder = None;
        app.picker = None;
        for (panel, index) in [(Sidebar::Files, 1), (Sidebar::Notes, 2), (Sidebar::Conversations, 3)] {
            app.sidebar = Some(panel);
            for focus in [crate::app::Focus::Content, crate::app::Focus::Sidebar, crate::app::Focus::Workspaces(0)] {
                app.focus = focus;
                app.cursor.liquid_glass = !app.cursor.liquid_glass;
                assert_eq!(active(&app), Some(index));
            }
        }
        app.sidebar = None;
        assert_eq!(active(&app), None);
    }
}

struct Material {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
}

impl Material {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rail glass"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rail glass"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rail glass"), source: wgpu::ShaderSource::Wgsl(include_str!("glass_selection.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rail glass"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None, cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rail glass"), size: 48, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        Self { pipeline, layout, uniform }
    }
}