use crate::renderer::batch::{BatchManager, Rect};

#[test]
fn opaque_rounded_menu_preserves_surrounding_editor_pixels_at_all_scales() {
    use crate::font::{fonts::SugarloafFonts, FontLibrary};
    use crate::text::{DrawOpts, Text};

    let (library, _errors) = FontLibrary::new(SugarloafFonts::default());
    let (width, height) = (640, 240);
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let mut text = Text::new(&library);
        text.init_cpu();
        text.set_scale_factor(scale);
        let opts = DrawOpts {
            font_size: 16.0,
            color: [255; 4],
            ..DrawOpts::default()
        };
        for row in 0..5 {
            text.draw(
                8.0,
                8.0 + row as f32 * 20.0,
                "pub trait Renderer: Sized { fn render(&self); }",
                &opts,
            );
        }
        let mut editor = vec![0; (width * height) as usize];
        text.render_cpu(&mut editor, width as u32, height as u32);
        assert!(
            editor.iter().any(|p| *p != 0),
            "must rasterize actual editor text"
        );

        let menu = Rect::from([70.0, 18.0, 100.0, 72.0].map(|v| v * scale));
        let mut batches = BatchManager::new();
        batches.quad(&menu, 0.0, &[0.0, 0.0, 0.0, 1.0], [10.0 * scale; 4], 5);
        let mut instances = Vec::new();
        batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
        let black_menu = instances[0];
        let mut white_menu = black_menu;
        white_menu.color = [1.0; 4];
        let mut coverage = vec![0; editor.len()];
        super::draw_quad_instance(&mut coverage, width, height, &white_menu);
        // The production late pass draws material after normal grid/UI text.
        let mut composed = editor.clone();
        super::draw_quad_instance(&mut composed, width, height, &black_menu);
        let mut covered_ink = 0;
        let mut surviving_ink = 0;
        for i in 0..editor.len() {
            if coverage[i] != 0 {
                assert_eq!(
                    composed[i] & 0x00ff_ffff,
                    0,
                    "opaque menu leaked text at scale={scale}, index={i}"
                );
                covered_ink += usize::from(editor[i] != 0);
            } else {
                assert_eq!(
                    composed[i], editor[i],
                    "menu erased surrounding text at scale={scale}, index={i}"
                );
                surviving_ink += usize::from(editor[i] != 0);
            }
        }
        assert!(covered_ink > 0 && surviving_ink > 0);
        let corner = ((18.0 * scale) as i32 * width + (70.0 * scale) as i32) as usize;
        assert_eq!(
            coverage[corner], 0,
            "rounded corner must not punch a rectangular hole"
        );
    }
}

#[test]
fn clipped_quad_pixels_match_the_original_shape_inside_the_viewport() {
    let (width, height) = (200, 200);
    for scale in [0.75, 1.0, 1.5, 2.0] {
        for corners in [[8.0; 4], [8.0, 8.0, 0.0, 0.0]] {
            let rect = Rect::from([20.0, 20.0, 64.0, 48.0].map(|v| v * scale));
            let mut batches = BatchManager::new();
            batches.quad(&rect, 0.0, &[1.0; 4], corners.map(|v| v * scale), 5);
            let mut instances = Vec::new();
            batches.build_display_list(&mut instances, &mut Vec::new(), &mut Vec::new());
            let original = instances[0];
            let mut full = vec![0; (width * height) as usize];
            super::draw_quad_instance(&mut full, width, height, &original);
            for clip in [
                [10.0, 22.0, 100.0, 80.0],
                [10.0, 0.0, 100.0, 66.0],
                [22.0, 0.0, 100.0, 100.0],
                [0.0, 0.0, 82.0, 100.0],
                [10.0, 20.0, 100.0, 2.0],
                [10.0, 66.0, 100.0, 2.0],
                [0.0, 0.0, 4.0, 4.0],
            ] {
                let clip = clip.map(|v| v * scale);
                let mut instance = original;
                instance.clip_rect = clip;
                let mut pixels = vec![0; full.len()];
                super::draw_quad_instance(&mut pixels, width, height, &instance);
                for y in 0..height {
                    for x in 0..width {
                        let inside = x >= clip[0].round() as i32
                            && y >= clip[1].round() as i32
                            && x < (clip[0] + clip[2]).round() as i32
                            && y < (clip[1] + clip[3]).round() as i32;
                        let index = (y * width + x) as usize;
                        assert_eq!(pixels[index], if inside { full[index] } else { 0 },
                            "corner shape or viewport leaked: scale={scale}, clip={clip:?}, pixel=({x},{y})");
                    }
                }
            }
        }
    }
}
