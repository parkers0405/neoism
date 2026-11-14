//! Effect slots (`El::effect`) and the compositor hooks around them: a compositor draws its own
//! pass at the slot, inside isolated layers, clipped like the primitives around it, in either
//! blend mode and in every format it is handed.

use std::sync::{Arc, Mutex};

use rune::color::{hex, srgb_decode, srgb_encode};
use rune::compose::{Compositor, Frame};
use rune::render::{CLIP_WGSL, EffectSite, KIND_EFFECT};
use rune::{App, AppSpec, Cx, El, effect, el};

const SHADER: &str = "
struct U { rect: vec4f, clip: vec4f, fade: vec4<u32>, frame: vec4f, color: vec4f, flags: vec4f }
@group(0) @binding(0) var<uniform> u: U;
struct V { @builtin(position) pos: vec4f }
@vertex fn vs(@builtin(vertex_index) vi: u32) -> V {
    let c = vec2f(f32(vi & 1u), f32(vi >> 1u));
    let l = (u.rect.xy + c * u.rect.zw - u.frame.xy) / u.frame.zw;
    return V(vec4f(l.x * 2.0 - 1.0, 1.0 - l.y * 2.0, 0.0, 1.0));
}
fn enc(x: vec3f) -> vec3f {
    return select(1.055 * pow(x, vec3f(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3f(0.0031308));
}
@fragment fn fs(v: V) -> @location(0) vec4f {
    let a = u.color.a * clip_alpha(v.pos.xy + u.frame.xy, u.clip, u.fade);
    var rgb = u.color.rgb;
    if (u.flags.x > 0.5) { rgb = enc(rgb); }
    return vec4f(rgb * a, a);
}
";

/// Paints every effect slot and its bleed solid red, clipped like the instance asks (or not at
/// all, with `ignore_clip`), with one pipeline per target format. It never applies an opacity:
/// Rune must.
struct Flat {
    pipes: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    bgl: Option<wgpu::BindGroupLayout>,
    formats: Arc<Mutex<Vec<wgpu::TextureFormat>>>,
    ignore_clip: bool,
}

impl Flat {
    fn new(formats: Arc<Mutex<Vec<wgpu::TextureFormat>>>) -> Self {
        Flat { pipes: Vec::new(), bgl: None, formats, ignore_clip: false }
    }

    fn draw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'_>, s: &EffectSite<'_>) {
        let bgl = self.bgl.get_or_insert_with(|| device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        }));
        if !self.pipes.iter().any(|(f, _)| *f == s.format) {
            self.formats.lock().unwrap().push(s.format);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(format!("{CLIP_WGSL}\n{SHADER}").into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[Some(bgl)], immediate_size: 0,
            });
            let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: s.format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.pipes.push((s.format, pipe));
        }
        let i = s.instance;
        assert_eq!(i.fill[3], 1.0, "a slot's opacity is Rune's to apply");
        assert_eq!(s.scale, i.params[3], "EffectSite::scale matches the slot's params[3]");
        let bleed = i.params[2];
        let mut data = [0.0f32; 24];
        data[0..4].copy_from_slice(&[i.rect[0] - bleed, i.rect[1] - bleed, i.rect[2] + 2.0 * bleed, i.rect[3] + 2.0 * bleed]);
        if self.ignore_clip {
            data[4..8].copy_from_slice(&rune::render::NO_CLIP);
        } else {
            data[4..8].copy_from_slice(&i.clip);
            for k in 0..4 { data[8 + k] = f32::from_bits(i.fade[k]); }
        }
        data[12..14].copy_from_slice(&s.origin);
        data[14] = s.size[0] as f32;
        data[15] = s.size[1] as f32;
        data[16..20].copy_from_slice(&[1.0, 0.0, 0.0, 1.0]);
        data[20] = if s.srgb_blend { 1.0 } else { 0.0 };
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: 96, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&data));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: bgl, entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        pass.set_pipeline(&self.pipes.iter().find(|(f, _)| *f == s.format).unwrap().1);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..4, 0..1);
    }
}

impl Compositor<()> for Flat {
    fn render(&mut self, f: &mut Frame<'_, ()>) {
        let (device, queue, size) = (f.device, f.queue, f.size);
        let clear = f.rt.clear_color();
        let count = f.rt.instances.len() as u32;
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
        let mut hook = |pass: &mut wgpu::RenderPass<'_>, s: &EffectSite<'_>| self.draw(device, queue, pass, s);
        f.ui.encode_layers_with(f.encoder, &mut hook);
        let mut pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: f.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        f.ui.draw_range_with(&mut pass, f.format, 0..count, &mut hook);
    }
}

/// Counts effect slots in the stream and draws the default frame, so nothing paints them.
struct Plain(Arc<Mutex<usize>>);

impl Compositor<()> for Plain {
    fn render(&mut self, f: &mut Frame<'_, ()>) {
        *self.0.lock().unwrap() = f.rt.instances.iter().filter(|i| i.params[1] == KIND_EFFECT).count();
        f.draw_ui();
    }
}

fn snap(name: &'static str, view: fn(&(), &mut Cx<()>) -> El, srgb: bool, comp: impl Compositor<()>) -> (u32, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("rune-effect-slot-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("snap.png");
    App::new(AppSpec::new(name, (96.0, 96.0), || (), view))
        .compositor(comp)
        .srgb_blend(srgb)
        .args(["--snap".to_string(), path.to_str().unwrap().to_string(), "--scale".to_string(), "1".to_string()])
        .run();
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap())).read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    std::fs::remove_dir_all(&dir).unwrap();
    (info.width, buf)
}

fn px(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let at = ((y * width + x) * 4) as usize;
    [rgba[at], rgba[at + 1], rgba[at + 2]]
}

/// What `top` at `alpha` over the grey background reads as, in the blend mode's own arithmetic.
fn over(top: [f32; 3], alpha: f32, srgb: bool) -> [u8; 3] {
    let bg = 32.0 / 255.0;
    std::array::from_fn(|c| {
        let v = if srgb { top[c] * alpha + bg * (1.0 - alpha) }
            else { srgb_encode(srgb_decode(top[c]) * alpha + srgb_decode(bg) * (1.0 - alpha)) };
        (v * 255.0).round() as u8
    })
}

fn near(a: [u8; 3], b: [u8; 3], what: &str) {
    assert!(a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 3), "{what}: got {a:?}, expected {b:?}");
}

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];

fn background() -> El {
    el().size(96.0, 96.0).fill(hex("#202020").unwrap())
}

fn faded(_: &(), _: &mut Cx<()>) -> El {
    background().child(
        el().size(64.0, 64.0).absolute(16.0, 16.0).opacity(0.5)
            .child(effect(1).absolute(0.0, 0.0).size(64.0, 64.0))
            .child(el().absolute(40.0, 40.0).size(24.0, 24.0).fill(hex("#0000ff").unwrap())),
    )
    .child(effect(2).absolute(84.0, 84.0).size(8.0, 8.0))
}

fn faded_rounded(_: &(), _: &mut Cx<()>) -> El {
    background().child(
        el().size(64.0, 64.0).absolute(16.0, 16.0).opacity(0.5).clip_rounded(24.0)
            .child(effect(1).absolute(0.0, 0.0).size(64.0, 64.0))
            .child(el().absolute(40.0, 40.0).size(24.0, 24.0).fill(hex("#0000ff").unwrap())),
    )
}

fn rounded(_: &(), _: &mut Cx<()>) -> El {
    background().child(el().size(64.0, 64.0).absolute(16.0, 16.0).clip_rounded(24.0).child(effect(1).size(64.0, 64.0)))
}

fn opaque(_: &(), _: &mut Cx<()>) -> El {
    background().child(effect(1).absolute(16.0, 16.0).size(64.0, 64.0))
}

#[test]
fn effect_in_a_faded_layer_is_faded_and_under_later_content() {
    for srgb in [false, true] {
        let formats = Arc::default();
        let (w, rgba) = snap("faded", faded, srgb, Flat::new(Arc::clone(&formats)));
        near(px(&rgba, w, 24, 24), over(RED, 0.5, srgb), "effect alone, half faded");
        near(px(&rgba, w, 68, 68), over(BLUE, 0.5, srgb), "later content covers the effect inside the layer");
        near(px(&rgba, w, 8, 8), over([32.0 / 255.0; 3], 1.0, srgb), "outside the card");
        near(px(&rgba, w, 88, 88), over(RED, 1.0, srgb), "a slot outside any layer");
        // One compositor drew into the layer's float target and the frame target.
        let formats = formats.lock().unwrap();
        assert_eq!(formats.len(), 2, "{formats:?}");
        assert!(formats.contains(&wgpu::TextureFormat::Rgba16Float));
    }
}

#[test]
fn rounded_clip_cuts_the_effect_corner() {
    for srgb in [false, true] {
        let (w, rgba) = snap("rounded", rounded, srgb, Flat::new(Arc::default()));
        near(px(&rgba, w, 17, 17), over([32.0 / 255.0; 3], 1.0, srgb), "corner is clipped away");
        near(px(&rgba, w, 48, 48), over(RED, 1.0, srgb), "centre");
        near(px(&rgba, w, 48, 17), over(RED, 1.0, srgb), "straight edge is not cut");
        let (w, rgba) = snap("faded-rounded", faded_rounded, srgb, Flat::new(Arc::default()));
        near(px(&rgba, w, 17, 17), over([32.0 / 255.0; 3], 1.0, srgb), "layer corner is clipped away");
        near(px(&rgba, w, 24, 48), over(RED, 0.5, srgb), "layer centre-left, half faded");
    }
}

#[test]
fn opaque_effects_match_across_blend_modes() {
    let (w, linear) = snap("opaque-linear", opaque, false, Flat::new(Arc::default()));
    let (_, gamma) = snap("opaque-gamma", opaque, true, Flat::new(Arc::default()));
    assert_eq!(linear.len(), gamma.len());
    let worst = linear.chunks(4).zip(gamma.chunks(4)).flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b))).max().unwrap();
    assert!(worst <= 2, "modes differ by {worst}");
    near(px(&linear, w, 48, 48), [255, 0, 0], "effect covers its rect");
}

fn faded_leaf(_: &(), _: &mut Cx<()>) -> El {
    background().child(effect(1).absolute(16.0, 16.0).size(64.0, 64.0).opacity(0.5))
}

fn faded_only_child(_: &(), _: &mut Cx<()>) -> El {
    background().child(el().absolute(16.0, 16.0).size(64.0, 64.0).opacity(0.5).child(effect(1).size(64.0, 64.0)))
}

fn dimmed(_: &(), _: &mut Cx<()>) -> El {
    background().child(effect(1).absolute(16.0, 16.0).size(64.0, 64.0).dim(0.5))
}

fn faded_rounded_leaf(_: &(), _: &mut Cx<()>) -> El {
    background().child(el().size(64.0, 64.0).absolute(16.0, 16.0).clip_rounded(24.0)
        .child(effect(1).size(64.0, 64.0).opacity(0.5)))
}

/// A faded slot gets a layer that applies its opacity, whichever way it was faded, so a
/// compositor that paints at full strength is still faded.
#[test]
fn a_faded_slot_is_faded_for_the_compositor() {
    for srgb in [false, true] {
        for (name, view) in [("leaf", faded_leaf as fn(&(), &mut Cx<()>) -> El), ("only-child", faded_only_child), ("dimmed", dimmed)] {
            let (w, rgba) = snap(name, view, srgb, Flat::new(Arc::default()));
            near(px(&rgba, w, 48, 48), over(RED, 0.5, srgb), name);
            near(px(&rgba, w, 8, 8), over([32.0 / 255.0; 3], 1.0, srgb), name);
        }
        let (w, rgba) = snap("faded-rounded-leaf", faded_rounded_leaf, srgb, Flat::new(Arc::default()));
        near(px(&rgba, w, 17, 17), over([32.0 / 255.0; 3], 1.0, srgb), "faded slot keeps its rounded clip");
        near(px(&rgba, w, 48, 48), over(RED, 0.5, srgb), "faded slot under a rounded clip");
    }
}

fn bleeding(_: &(), _: &mut Cx<()>) -> El {
    background().child(el().absolute(16.0, 16.0).size(64.0, 64.0).clip_fade(0.0, 0.0, 0.0, 0.0)
        .child(effect(1).effect_bleed(16.0).absolute(-12.0, 0.0).size(8.0, 8.0)))
}

/// A slot whose rect lies outside its clip still paints the bleed that reaches in, and a
/// compositor that ignores the clip is held to it by the scissor.
#[test]
fn bleed_reaches_in_and_the_scissor_holds_the_clip() {
    let (w, rgba) = snap("bleeding", bleeding, false, Flat { ignore_clip: true, ..Flat::new(Arc::default()) });
    near(px(&rgba, w, 20, 20), over(RED, 1.0, false), "bleed inside the clip");
    near(px(&rgba, w, 10, 20), over([32.0 / 255.0; 3], 1.0, false), "bleed outside the clip");
    near(px(&rgba, w, 20, 44), over([32.0 / 255.0; 3], 1.0, false), "past the bleed");
}

#[test]
fn an_unpainted_effect_slot_draws_nothing() {
    let slots = Arc::default();
    let (w, rgba) = snap("unpainted", opaque, false, Plain(Arc::clone(&slots)));
    assert_eq!(*slots.lock().unwrap(), 1);
    let grey = px(&rgba, w, 8, 8);
    for (x, y) in [(16, 16), (48, 48), (79, 79)] {
        assert_eq!(px(&rgba, w, x, y), grey, "({x}, {y})");
    }
}

struct ScaleProbe(Arc<Mutex<Vec<(u32, f32, f32)>>>);

impl Compositor<()> for ScaleProbe {
    fn render(&mut self, f: &mut Frame<'_, ()>) {
        let (device, queue, size) = (f.device, f.queue, f.size);
        let clear = f.rt.clear_color();
        let count = f.rt.instances.len() as u32;
        let marks = f.rt.marks.clone();
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
        let log = &self.0;
        let mut hook = |_: &mut wgpu::RenderPass<'_>, s: &EffectSite<'_>| {
            assert_eq!(s.scale, s.instance.params[3]);
            log.lock().unwrap().push((marks[s.index as usize], s.scale, s.instance.params[2]));
        };
        f.ui.encode_layers_with(f.encoder, &mut hook);
        let mut pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: f.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        f.ui.draw_range_with(&mut pass, f.format, 0..count, &mut hook);
    }
}

fn scaled_slots(_: &(), _: &mut Cx<()>) -> El {
    background()
        .child(effect(1).absolute(8.0, 8.0).size(16.0, 16.0))
        .child(
            el().absolute(32.0, 32.0).size(32.0, 32.0).scale(1.5).child(
                el().size(16.0, 16.0).live_scale(&rune::Live::new(2.0)).child(effect(2).size(16.0, 16.0)),
            ),
        )
}

#[test]
fn slot_reports_device_pixels_per_point_without_bleed() {
    let seen = Arc::default();
    let dir = std::env::temp_dir().join(format!("rune-effect-slot-scale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("snap.png");
    App::new(AppSpec::new("slot-scale", (96.0, 96.0), || (), scaled_slots))
        .compositor(ScaleProbe(Arc::clone(&seen)))
        .args(["--snap".to_string(), path.to_str().unwrap().to_string(), "--scale".to_string(), "2".to_string()])
        .run();
    std::fs::remove_dir_all(&dir).unwrap();
    let got = seen.lock().unwrap().clone();
    assert_eq!(got, vec![(1, 2.0, 0.0), (2, 6.0, 0.0)]);
}

struct BackdropProbe(Arc<Mutex<Vec<(u32, &'static str, bool, bool)>>>);

impl Compositor<()> for BackdropProbe {
    fn render(&mut self, f: &mut Frame<'_, ()>) {
        let (device, queue, size) = (f.device, f.queue, f.size);
        let clear = f.rt.clear_color();
        let marks = f.rt.marks.clone();
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
        let log = &self.0;
        f.ui.draw_with_backdrop(f.encoder, f.target, clear, &mut |phase, s| {
            let phase_name = match phase {
                rune::render::EffectPhase::Backdrop(_) => "backdrop",
                rune::render::EffectPhase::Draw(_) => "draw",
            };
            log.lock().unwrap().push((marks[s.index as usize], phase_name, s.backdrop.is_some(), s.in_layer));
        });
    }
}

fn mixed_backdrop_slots(_: &(), _: &mut Cx<()>) -> El {
    background()
        .child(effect(1).absolute(8.0, 8.0).size(16.0, 16.0))
        .child(effect(2).effect_backdrop(true).absolute(32.0, 8.0).size(16.0, 16.0))
        .child(
            el().absolute(8.0, 40.0).size(48.0, 32.0).opacity(0.5)
                .child(el().size(16.0, 16.0).fill(hex("#00ff00").unwrap()))
                .child(effect(3).effect_backdrop(true).size(16.0, 16.0)),
        )
}

#[test]
fn only_backdrop_slots_split_the_pass_and_receive_a_backdrop_view() {
    let seen = Arc::default();
    let _ = snap("backdrop-probe", mixed_backdrop_slots, false, BackdropProbe(Arc::clone(&seen)));
    let got = seen.lock().unwrap().clone();
    assert_eq!(
        got,
        vec![
            (3, "backdrop", true, true),
            (3, "draw", true, true),
            (1, "draw", false, false),
            (2, "backdrop", true, false),
            (2, "draw", true, false),
        ],
    );
}

#[test]
fn inert_backdrop_slots_draw_but_hidden_and_offscreen_subtrees_do_not() {
    let seen = Arc::default();
    let _ = snap("backdrop-visibility", |_, _| background()
        .child(el().inert().absolute(8.0, 8.0).size(24.0, 24.0)
            .child(effect(1).effect_backdrop(true).size(24.0, 24.0)))
        .child(el().absolute(200.0, 200.0).size(24.0, 24.0)
            .child(effect(2).effect_backdrop(true).size(24.0, 24.0)))
        .child(el().opacity(0.0).absolute(40.0, 8.0).size(24.0, 24.0)
            .child(effect(3).effect_backdrop(true).size(24.0, 24.0))),
        false, BackdropProbe(Arc::clone(&seen)));
    assert_eq!(*seen.lock().unwrap(), vec![(1, "backdrop", true, false), (1, "draw", true, false)]);
}

struct CacheSlotProbe {
    frame_no: u32,
    flat: Flat,
    log: Arc<Mutex<Vec<(u32, usize, bool, bool)>>>,
}

impl Compositor<()> for CacheSlotProbe {
    fn render(&mut self, f: &mut Frame<'_, ()>) {
        self.frame_no += 1;
        let frame_no = self.frame_no;
        let (device, queue, size) = (f.device, f.queue, f.size);
        let clear = f.rt.clear_color();
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
        let stats = f.ui.layer_stats();
        let flat = &mut self.flat;
        let log = &self.log;
        f.ui.draw_with_backdrop(f.encoder, f.target, clear, &mut |phase, s| {
            if let rune::render::EffectPhase::Draw(pass) = phase {
                log.lock().unwrap().push((frame_no, stats.targets, s.backdrop.is_some(), s.in_layer));
                // Frame 1 leaves the slot unpainted; frame 2 paints solid red, proving the
                // `.cache()` subtree was not frozen on frame 1's raster.
                if frame_no >= 2 {
                    flat.draw(device, queue, pass, s);
                }
            }
        });
    }
    fn animating(&self) -> bool {
        self.frame_no < 2
    }
}

fn cached_subtree_with_effect(_: &(), _: &mut Cx<()>) -> El {
    background().child(
        el().key("cached-card").cache().absolute(16.0, 16.0).size(64.0, 64.0)
            .fill(hex("#282828").unwrap())
            .child(effect(1).effect_backdrop(true).absolute(8.0, 8.0).size(48.0, 48.0)),
    )
}

#[test]
fn effect_slot_in_cached_subtree_draws_live_and_never_caches() {
    let dir = std::env::temp_dir().join(format!("rune-effect-slot-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = Arc::default();
    App::new(AppSpec::new("slot-in-cache", (96.0, 96.0), || (), cached_subtree_with_effect))
        .compositor(CacheSlotProbe { frame_no: 0, flat: Flat::new(Arc::default()), log: Arc::clone(&log) })
        .args([
            "--frames".to_string(),
            "2".to_string(),
            "--out".to_string(),
            dir.to_str().unwrap().to_string(),
            "--scale".to_string(),
            "1".to_string(),
        ])
        .run();
    let frame2 = dir.join("frame_0001.png");
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&frame2).unwrap())).read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    std::fs::remove_dir_all(&dir).unwrap();

    let got = log.lock().unwrap().clone();
    assert_eq!(got, vec![(1, 0, true, false), (2, 0, true, false)], "cached subtree with effect slot stays live with 0 cache targets");
    // Frame 2 must show the red paint from frame 2, not frame 1's unpainted #282828.
    let center = px(&buf, info.width, 40, 40);
    assert!(center[0] > 240 && center[1] < 15 && center[2] < 15, "frame 2 drew live red at (40, 40): {center:?}");
}
