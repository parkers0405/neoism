//! A runtime the app draws itself (`Runtime::with_text`, a headless snapshot) can blend in sRGB
//! with `Runtime::set_srgb_blend`, hover and press tints included, as an `App::srgb_blend`
//! window does.
use rune::color::over;
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{Color, Cx, El, Env, Runtime, el};

const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };

fn view(_: &(), _: &mut Cx<()>) -> El {
    el().size(40.0, 40.0).fill(Color::BLACK)
        .child(el().size(20.0, 20.0).fill(RED).hover(Color::WHITE.alpha(0.5)).on_press(|_: &mut ()| {}))
}

fn hovered(srgb: bool) -> Runtime<()> {
    let text = TextSystem::new(&[], ("Inter", "Inter"));
    let mut rt = Runtime::with_text((), view, (40.0, 40.0), Env::new(true, 1.0), text);
    rt.set_srgb_blend(srgb);
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.frame(1.0 / 60.0);
    rt
}

/// The child's fill, with the hover tint composited over it.
fn tinted(rt: &Runtime<()>) -> [f32; 4] {
    rt.instances.iter().find(|i| i.rect[2] == 20.0 && i.rect[3] == 20.0).expect("the child's box").fill
}

fn pixel(gpu: &Gpu, rt: &mut Runtime<()>) -> [u8; 4] {
    let format = if rt.srgb_blend() { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(rt.srgb_blend());
    let target = Offscreen::with_format(&gpu.device, 40, 40, format);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (40, 40));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, rt.clear_color());
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    let px = target.read(&gpu.device);
    let at = (10 * 40 + 10) * 4;
    [px[at], px[at + 1], px[at + 2], px[at + 3]]
}

#[test]
fn hover_tints_follow_the_runtime_blend_mode() {
    let tint = Color::WHITE.alpha(0.5);
    let srgb = hovered(true);
    assert!(srgb.srgb_blend());
    assert_eq!(tinted(&srgb), over(tint.premul_srgb(1.0), RED.premul_srgb(1.0)), "composited in sRGB encoding");
    let linear = hovered(false);
    assert_eq!(tinted(&linear), over(tint.premul(1.0), RED.premul(1.0)), "composited linearly");
}

#[test]
fn a_self_drawn_runtime_renders_gamma_blended_pixels() {
    let gpu = Gpu::headless();
    let near = |got: [u8; 4], want: [u8; 4]| assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1), "{got:?} != {want:?}");
    // Half white over red: 128 in sRGB space, as CSS draws it; linear light reads 188.
    near(pixel(&gpu, &mut hovered(true)), [255, 128, 128, 255]);
    near(pixel(&gpu, &mut hovered(false)), [255, 188, 188, 255]);
}

#[test]
fn switching_blend_mode_redraws() {
    let mut rt = hovered(false);
    rt.frame(1.0);
    let commits = rt.stats.commits;
    rt.set_srgb_blend(true);
    rt.frame(1.0 / 60.0);
    assert!(rt.stats.commits > commits, "a change asks for a frame");
    assert_eq!(tinted(&rt), over(Color::WHITE.alpha(0.5).premul_srgb(1.0), RED.premul_srgb(1.0)));
    let commits = rt.stats.commits;
    rt.set_srgb_blend(true);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits, "the same mode is no change");
}

#[test]
fn a_handler_switches_blend_mode_live() {
    let mut rt = hovered(false);
    rt.update(|_, tx| tx.set_srgb_blend(true));
    assert!(rt.srgb_blend());
    rt.frame(1.0 / 60.0);
    assert_eq!(tinted(&rt), over(Color::WHITE.alpha(0.5).premul_srgb(1.0), RED.premul_srgb(1.0)));
    rt.update(|_, tx| { tx.set_srgb_blend(true); tx.set_srgb_blend(false); });
    assert!(!rt.srgb_blend(), "the last request wins");
}

/// A headless run follows a switch made by a handler: the renderer, its target and the
/// compositor's `Frame` change format on the frame that switches.
#[test]
fn a_headless_run_follows_a_live_switch() {
    use rune::compose::{Compositor, Frame};
    use rune::{App, AppSpec};
    use std::sync::Mutex;

    static SEEN: Mutex<Vec<(bool, wgpu::TextureFormat)>> = Mutex::new(Vec::new());
    struct Watch;
    impl Compositor<()> for Watch {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            SEEN.lock().unwrap().push((f.srgb_blend(), f.format));
            f.draw_ui();
        }
    }
    let dir = std::env::temp_dir().join(format!("rune-srgb-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, png) = (dir.join("script.json"), dir.join("snap.png"));
    std::fs::write(&script, r#"[{"t": 0.1, "type": "click", "x": 20, "y": 20}]"#).unwrap();
    let view = |_: &(), _: &mut Cx<()>| {
        el().size(40.0, 40.0).fill(RED)
            .child(el().size(40.0, 40.0).fill(Color::WHITE.alpha(0.5)))
            .on_press_tx(|_: &mut (), tx| tx.set_srgb_blend(true))
    };
    App::new(AppSpec::new("srgb-live", (40.0, 40.0), || (), view)).srgb_blend(false).compositor(Watch)
        .args(["--snap", png.to_str().unwrap(), "--scale", "1", "--script", script.to_str().unwrap(), "--at", "0.5"].map(String::from))
        .run();
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&png).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buf).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let at = (20 * 40 + 20) * 4;
    let px = &buf[at..at + 4];
    // Half white over red reads 128 blended in sRGB, 188 in linear light.
    assert!(px[1].abs_diff(128) <= 1 && px[0] == 255, "gamma-blended after the switch, got {px:?}");
    let seen = SEEN.lock().unwrap();
    assert_eq!(seen.first(), Some(&(false, OFFSCREEN_FORMAT)), "starts linear");
    assert_eq!(seen.last(), Some(&(true, OFFSCREEN_FORMAT.remove_srgb_suffix())), "ends gamma-space");
}

#[test]
fn switching_format_keeps_the_glyph_atlas_and_extra_pipelines() {
    let gpu = Gpu::headless();
    let text = TextSystem::new(&[], ("Inter", "Inter"));
    let view = |_: &(), _: &mut Cx<()>| {
        el().size(80.0, 40.0).fill(RED)
            .child(rune::text("hello"))
    };
    let mut rt = Runtime::with_text((), view, (80.0, 40.0), Env::new(true, 1.0), text);
    rt.frame(0.0);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.add_format(&gpu.device, wgpu::TextureFormat::Rgba16Float);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (80, 40));
    assert!(rt.text.atlas.dirty.is_none(), "initial prepare drained the atlas");

    rt.set_srgb_blend(true);
    rt.frame(1.0 / 60.0);
    let non_srgb = OFFSCREEN_FORMAT.remove_srgb_suffix();
    renderer.set_format(&gpu.device, non_srgb);
    renderer.set_srgb_blend(true);
    assert_eq!(renderer.format(), non_srgb);
    assert!(rt.text.atlas.dirty.is_none(), "switching blend format keeps the uploaded glyph atlas");

    let layer = Offscreen::with_format(&gpu.device, 80, 40, wgpu::TextureFormat::Rgba16Float);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (80, 40));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &layer.view, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        renderer.draw_range(&mut pass, wgpu::TextureFormat::Rgba16Float, 0..rt.instances.len() as u32);
    }
    renderer.submit(&gpu.queue, encoder);
}
