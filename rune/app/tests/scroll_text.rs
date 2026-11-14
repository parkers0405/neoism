//! Observable slow-scroll glyph coverage, without a daemon, window, or input logging.
use rune::{Color, Cx, El, Env, FontSmoothing, Runtime, ScrollSpec, TextStyle, col, text};
use rune::render::{Gpu, KIND_GLYPH, OFFSCREEN_FORMAT, Offscreen, Renderer};

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().size(320.0, 240.0).scroll(ScrollSpec { stick_bottom: false }).child(
        col().key("paragraphs").w_full().pt(32.0).children((0..20).map(|i| {
            text(format!("{i}. Amber lantern quiet harbor"))
                .font(TextStyle::new(15.0, if i % 2 == 0 { 700.0 } else { 400.0 }).mono().lh(21.75)).color(Color::WHITE)
        })),
    )
}

#[test]
fn slow_scroll_preserves_text_coverage() {
    let gpu = Gpu::headless();
    // Optional synthetic-only snapshots for visual inspection; never captures user content.
    let images = std::env::var_os("RUNE_SCROLL_TEXT_IMAGES").map(std::path::PathBuf::from);
    if let Some(dir) = &images { std::fs::create_dir_all(dir).unwrap(); }
    for scale in [1.0, 2.0] {
        let size = ((320.0 * scale) as u32, (240.0 * scale) as u32);
        let format = OFFSCREEN_FORMAT;
        let target = Offscreen::with_format(&gpu.device, size.0, size.1, format);
        let mut renderer = Renderer::new(&gpu.device, format);
        let fonts: &[&'static [u8]] = &[
            include_bytes!("../assets/fonts/GeistMono-Regular.otf"),
            include_bytes!("../assets/fonts/GeistMono-Bold.otf"),
        ];
        let text_system = rune::text::TextSystem::new(fonts, ("Geist Mono", "Geist Mono"));
        let mut rt = Runtime::with_text((), view, (320.0, 240.0), Env::new(true, scale), text_system);
        rt.text.set_font_smoothing(FontSmoothing::On);
        let mut draw = |rt: &mut Runtime<()>| {
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
            target.copy(&mut enc);
            renderer.submit(&gpu.queue, enc);
            target.read(&gpu.device)
        };
        rt.frame(0.0);
        assert!(rt.instances.iter().filter(|q| q.params[1] == KIND_GLYPH).count() > 20);
        let first_y = |rt: &Runtime<()>| rt.instances.iter().find(|q| q.params[1] == KIND_GLYPH).unwrap().rect[1];
        let y0 = first_y(&rt);
        let before = draw(&mut rt);
        if let Some(dir) = &images {
            rune::render::write_png(&dir.join(format!("scale-{scale}-rest.png")), size.0, size.1, &before).unwrap();
        }
        let mut fractional = false;
        for step in 0..40 {
            rt.wheel(160.0, 120.0, -0.37 * 0.95_f32.powi(step));
            rt.frame(1.0 / 120.0);
            let top = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("paragraphs")).unwrap().rect[1] * scale;
            fractional |= (top - top.round()).abs() > 0.01;
            if step == 0 || step == 17 || step == 39 {
                let after = draw(&mut rt);
                if let Some(dir) = &images {
                    rune::render::write_png(&dir.join(format!("scale-{scale}-step-{step}.png")), size.0, size.1, &after).unwrap();
                }
                let dy = (y0 - first_y(&rt)) as usize;
                if step == 39 { assert!(dy > 0, "must compare a genuinely scrolled image"); }
                let stride = size.0 as usize * 4;
                let len = (size.1 as usize - dy) * stride;
                let changed = after[..len].iter().zip(&before[dy * stride..]).filter(|(a, b)| a != b).count();
                assert_eq!(changed, 0, "scroll only translates pixels: scale {scale}, step {step}, dy {dy}");
                eprintln!("scale={scale} step={step} translation={dy}px changed_channels={changed}");
            }
        }
        assert!(fractional, "must exercise fractional scroll state");
    }
}