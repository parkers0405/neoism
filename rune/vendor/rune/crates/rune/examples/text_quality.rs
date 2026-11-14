//! Headless S5 raster oracle. Export the exact shaped glyph IDs/positions for CoreText.
use parley::PositionedLayoutItem;
use rune::color::hex;
use rune::render::{Gpu, Instance, NO_CLIP, OFFSCREEN_FORMAT, Offscreen, Renderer, write_png};
use rune::text::{ATLAS_SIZE, TextSystem};
use rune::{Family, TextStyle};
use serde_json::json;
use std::path::Path;

// Experimental swash oracle, independent of the production atlas/cache. Used only when
// OUT FONT FAMILY HINT EMBOLDEN is supplied; HINT is auto/on/off, EMBOLDEN in device px.
fn experimental(
    layout: &parley::Layout<rune::text::Ink>,
    scale: f32,
    origin: [f32; 2],
    hint: &str,
    embolden: f32,
    atlas: &mut rune::text::Atlas,
    cursor: &mut [u32; 3],
    out: &mut Vec<rune::text::GlyphQuad>,
) {
    use swash::scale::{Render, ScaleContext, Source};
    use swash::zeno::{Format, Vector};
    let mut cx = ScaleContext::new();
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(gr) = item else {
                continue;
            };
            let run = gr.run();
            let font = run.font();
            let font = swash::FontRef::from_index(font.data.as_ref(), font.index as usize).unwrap();
            let mut scaler = cx
                .builder(font)
                .size(run.font_size())
                .hint(match hint {
                    "on" => true,
                    "off" => false,
                    _ => scale < 1.5,
                })
                .normalized_coords(run.normalized_coords().iter().map(|c| c.to_bits()))
                .build();
            let mut rel = gr.offset();
            for g in gr.glyphs() {
                let x = ((origin[0] + rel + g.x) * 4.0).round() / 4.0;
                rel += g.advance;
                let Some(image) = Render::new(&[Source::Outline])
                    .format(Format::Alpha)
                    .embolden(embolden)
                    .offset(Vector::new(x.fract() - embolden, -embolden))
                    .render(&mut scaler, g.id as u16)
                else {
                    continue;
                };
                let p = image.placement;
                if p.width == 0 || p.height == 0 {
                    continue;
                }
                if cursor[0] + p.width + 1 >= ATLAS_SIZE {
                    cursor[0] = 1;
                    cursor[1] += cursor[2] + 1;
                    cursor[2] = 0;
                }
                assert!(
                    cursor[1] + p.height + 1 < ATLAS_SIZE,
                    "experimental atlas full"
                );
                for y in 0..p.height {
                    let dst = ((cursor[1] + y) * ATLAS_SIZE + cursor[0]) as usize;
                    atlas.data[dst..dst + p.width as usize].copy_from_slice(
                        &image.data[(y * p.width) as usize..((y + 1) * p.width) as usize],
                    );
                }
                out.push(rune::text::GlyphQuad {
                    rect: [
                        x.floor() + p.left as f32,
                        ((origin[1] + gr.baseline()).round() + g.y).round() - p.top as f32,
                        p.width as f32,
                        p.height as f32,
                    ],
                    uv: [cursor[0] as f32, cursor[1] as f32],
                    ink: 0,
                    byte: 0,
                    color: 0,
                    outline: None,
                });
                cursor[0] += p.width + 1;
                cursor[2] = cursor[2].max(p.height);
            }
        }
    }
    atlas.dirty = Some((0, cursor[1] + cursor[2] + 1));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = Path::new(args.get(1).expect("text_quality OUT [FONT FAMILY]"));
    std::fs::create_dir_all(out).unwrap();
    let path = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("crates/rune/assets/Inter-Regular.ttf");
    let family = args.get(3).map(String::as_str).unwrap_or("Inter");
    let experiment = args
        .get(4)
        .map(|hint| (hint.as_str(), args.get(5).unwrap().parse::<f32>().unwrap()));
    let data: &'static [u8] = Box::leak(std::fs::read(path).unwrap().into_boxed_slice());
    let gpu = Gpu::new(Gpu::instance(), None);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let strings = [
        "Hamburgefontsiv 0123456789",
        "The quick brown fox jumps over the lazy dog.",
        "Settings / Model / Working / Copy",
        "Il1 O0 rn m ffi fi fl -> != <=",
        "lHEximnoAVWgjpq",
    ];
    for scale in [1.0f32, 2.0] {
        let (w, h) = ((800.0 * scale) as u32, (1100.0 * scale) as u32);
        let target = Offscreen::new(&gpu.device, w, h);
        let mut text = TextSystem::new(&[data], (family, family));
        let mut quads = Vec::new();
        let mut rows = Vec::new();
        let mut cursor = [1, 1, 0];
        for (si, size) in [11.0, 12.0, 13.0, 15.0, 24.0].into_iter().enumerate() {
            for (ti, string) in strings.iter().enumerate() {
                let origin = [16.0 * scale, (8.0 + (si * 5 + ti) as f32 * 42.0) * scale];
                let mut style = TextStyle {
                    size,
                    weight: 400.0,
                    family: Family::Sans,
                    line_height: 0.0,
                    tracking: 0.0,
                    ..TextStyle::new(size, 400.0)
                };
                if std::env::var("RUNE_TEXT_OPSZ").as_deref() == Ok("point") { style = style.optical_size(size); }
                if std::env::var("RUNE_TEXT_OPSZ").as_deref() == Ok("legacy") { style = style.optical_sizing(rune::OpticalSize::FontDefault); }
                let layout = text.layout_rich(string, &[], style, scale, None);
                if let Some((hint, embolden)) = experiment {
                    experimental(
                        &layout,
                        scale,
                        origin,
                        hint,
                        embolden,
                        &mut text.atlas,
                        &mut cursor,
                        &mut quads,
                    );
                } else {
                    text.glyphs_for_layout(&layout, scale, origin, false, &mut quads);
                }
                let mut glyphs = Vec::new();
                for line in layout.lines() {
                    for item in line.items() {
                        let PositionedLayoutItem::GlyphRun(gr) = item else {
                            continue;
                        };
                        let mut rel = gr.offset();
                        for g in gr.glyphs() {
                            let x = ((origin[0] + rel + g.x) * 4.0).round() / 4.0;
                            let y = ((origin[1] + gr.baseline()).round() + g.y).round();
                            glyphs.push(json!({"id":g.id,"x":x,"y":y,"advance":g.advance}));
                            rel += g.advance;
                        }
                    }
                }
                rows.push(json!({"size":size,"text":string,"top":origin[1],"glyphs":glyphs,
                    "advance":layout.full_width(),"opsz":match style.optical_size {
                        rune::OpticalSize::Auto => Some(size), rune::OpticalSize::Fixed(value) => Some(value),
                        rune::OpticalSize::FontDefault => None,
                    }}));
            }
        }
        for dark in [false, true] {
            let (fg, bg) = if dark {
                (hex("#eeeeee").unwrap(), hex("#181818").unwrap())
            } else {
                (hex("#181818").unwrap(), hex("#eeeeee").unwrap())
            };
            let gamma = 1.0;
            let instances: Vec<_> = quads
                .iter()
                .map(|q| Instance {
                    rect: q.rect,
                    radii: [0.0; 4],
                    fill: fg.premul(1.0),
                    border: [0.0; 4],
                    params: [0.0, 2.0, q.atlas_page() as f32, gamma],
                    uv: [q.atlas_uv()[0], q.atlas_uv()[1], 0.0, 0.0],
                    clip: NO_CLIP,
                    fade: [0; 4],
                })
                .collect();
            renderer.prepare(&gpu.device, &gpu.queue, &mut text.atlas, &instances, (w, h));
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut encoder, &target.view, bg.premul(1.0));
            target.copy(&mut encoder);
            gpu.queue.submit([encoder.finish()]);
            let name = format!("{}-{}x", if dark { "dark" } else { "light" }, scale);
            write_png(
                &out.join(format!("rune-{name}.png")),
                w,
                h,
                &target.read(&gpu.device),
            )
            .unwrap();
            std::fs::write(out.join(format!("{name}.json")),serde_json::to_vec_pretty(&json!({
                "font":std::fs::canonicalize(path).unwrap(),"family":family,"scale":scale,"width":w,"height":h,
                "fg":if dark {238} else {24},"bg":if dark {24} else {238},"gamma":gamma,"rows":rows
            })).unwrap()).unwrap();
        }
        // Raw coverage, independent of gamma/blending, for offline experiments.
        let mut raw = vec![0u8; (w * h * 4) as usize];
        for p in raw.chunks_exact_mut(4) {
            p[3] = 255;
        }
        for q in &quads {
            let uv = q.atlas_uv();
            let data = text.atlas.page_data(q.atlas_page()).unwrap();
            for y in 0..q.rect[3] as u32 {
                for x in 0..q.rect[2] as u32 {
                    let dx = q.rect[0] as u32 + x;
                    let dy = q.rect[1] as u32 + y;
                    if dx >= w || dy >= h {
                        continue;
                    }
                    let v = data
                        [((uv[1] as u32 + y) * ATLAS_SIZE + uv[0] as u32 + x) as usize];
                    let p = ((dy * w + dx) * 4) as usize;
                    for c in 0..3 {
                        raw[p + c] = v;
                    }
                }
            }
        }
        write_png(&out.join(format!("coverage-{scale}x.png")), w, h, &raw).unwrap();
        // 64 phases per device pixel across two pixels; each row is a frame.
        let (sw, sh) = (320, 128 * 40);
        let sweep = Offscreen::new(&gpu.device, sw, sh);
        let style = TextStyle {
            size: 13.0,
            weight: 400.0,
            family: Family::Sans,
            line_height: 0.0,
            tracking: 0.0,
            ..TextStyle::new(13.0, 400.0)
        };
        let layout = text.layout_rich("lHEximno", &[], style, scale, None);
        let mut frames = Vec::new();
        let mut instances = Vec::new();
        for frame in 0..128 {
            let start = instances.len();
            let x = 16.0 + frame as f32 / 64.0;
            quads.clear();
            let origin = [x, frame as f32 * 40.0 + 2.0];
            if let Some((hint, embolden)) = experiment {
                experimental(
                    &layout,
                    scale,
                    origin,
                    hint,
                    embolden,
                    &mut text.atlas,
                    &mut cursor,
                    &mut quads,
                );
            } else {
                text.glyphs_for_layout(&layout, scale, origin, false, &mut quads);
            }
            for q in &quads {
                instances.push(Instance {
                    rect: q.rect,
                    radii: [0.0; 4],
                    fill: [1.0; 4],
                    border: [0.0; 4],
                    params: [0.0, 2.0, q.atlas_page() as f32, 1.0],
                    uv: [q.atlas_uv()[0], q.atlas_uv()[1], 0.0, 0.0],
                    clip: NO_CLIP,
                    fade: [0; 4],
                });
            }
            frames.push(json!({"x":x,"instances":instances.len()-start}));
        }
        renderer.prepare(
            &gpu.device,
            &gpu.queue,
            &mut text.atlas,
            &instances,
            (sw, sh),
        );
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &sweep.view, [0.0, 0.0, 0.0, 1.0]);
        sweep.copy(&mut encoder);
        gpu.queue.submit([encoder.finish()]);
        write_png(
            &out.join(format!("phases-{scale}x.png")),
            sw,
            sh,
            &sweep.read(&gpu.device),
        )
        .unwrap();
        std::fs::write(
            out.join(format!("phases-{scale}x.json")),
            serde_json::to_vec(&frames).unwrap(),
        )
        .unwrap();
    }
}
