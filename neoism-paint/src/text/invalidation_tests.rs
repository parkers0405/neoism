//! Regression coverage for destructive invalidation across emission and font caches.
use super::*;
use crate::font::{FontData, FontLibraryData};

fn fixture_face(pixel: bool) -> FontData {
    let bytes: &'static [u8] = if pixel {
        include_bytes!("../font/resources/PressStart2P/PressStart2P-Regular.ttf")
    } else {
        include_bytes!("../font/resources/Geist/Geist-Variable.ttf")
    };
    FontData::from_static_slice(bytes).unwrap()
}

fn fixture_library(pixel: bool) -> FontLibrary {
    let mut data = FontLibraryData::default();
    data.insert(fixture_face(pixel));
    FontLibrary {
        inner: Arc::new(parking_lot::RwLock::new(data)),
    }
}

fn replace_primary_reusing_key(library: &FontLibrary, pixel: bool) {
    let mut replacement = fixture_face(pixel);
    let mut data = library.inner.write();
    replacement.key = data.inner[&0].key;
    data.inner.insert(0, replacement);
}

#[cfg(not(target_arch = "wasm32"))]
fn assert_same_emission_and_pixels(a: &Text, b: &Text) {
    assert!(!a.instances.is_empty());
    assert_eq!(a.instances.len(), b.instances.len());
    for (a, b) in a.instances.iter().zip(&b.instances) {
        assert_eq!(a.pos, b.pos);
        assert_eq!(a.glyph_pos, b.glyph_pos);
        assert_eq!(a.glyph_size, b.glyph_size);
        assert_eq!(a.bearings, b.bearings);
        assert_eq!(a.color, b.color);
        assert_eq!(a.atlas, b.atlas);
        assert_eq!(a.clip_rect, b.clip_rect);
        assert_eq!(a.raster_scale, b.raster_scale);
        assert_eq!(a.blur_radius, b.blur_radius);
    }
    // Compare only live glyph texels: unused atlas backing legitimately retains
    // old generations. This distinguishes rasterizer state from sampling bleed.
    for (a_inst, b_inst) in a.instances.iter().zip(&b.instances) {
        let a_state = a.cpu.as_ref().unwrap();
        let b_state = b.cpu.as_ref().unwrap();
        let (aa, ba) = if a_inst.atlas == 1 {
            (&a_state.atlas_color, &b_state.atlas_color)
        } else {
            (&a_state.atlas_grayscale, &b_state.atlas_grayscale)
        };
        let channels = if a_inst.atlas == 1 { 4 } else { 1 };
        for row in 0..a_inst.glyph_size[1] as usize {
            let start = |inst: &TextInstance, side: u16| {
                ((inst.glyph_pos[1] as usize + row) * side as usize
                    + inst.glyph_pos[0] as usize)
                    * channels
            };
            let len = a_inst.glyph_size[0] as usize * channels;
            assert!(
                aa.pixels()[start(a_inst, aa.side())..][..len]
                    == ba.pixels()[start(b_inst, ba.side())..][..len],
                "live glyph raster differs at row {row}"
            );
        }
    }
    let mut pixels_a = vec![0; 512 * 128];
    let mut pixels_b = vec![0; 512 * 128];
    a.render_cpu(&mut pixels_a, 512, 128);
    b.render_cpu(&mut pixels_b, 512, 128);
    assert!(pixels_a.iter().any(|pixel| *pixel != 0));
    if let Some(index) = pixels_a.iter().zip(&pixels_b).position(|(a, b)| a != b) {
        panic!(
            "CPU pixel differs at ({}, {}): {:#08x} != {:#08x}",
            index % 512,
            index / 512,
            pixels_a[index],
            pixels_b[index]
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn scaled_draw_after_hinting_change_transforms_all_new_instances() {
    let library = fixture_library(false);
    let mut text = Text::new(&library);
    text.init_cpu();
    text.set_scale_factor(1.25);
    let opts = DrawOpts {
        font_size: 17.3,
        clip_rect: Some([0.3, 1.7, 200.5, 90.7]),
        ..DrawOpts::default()
    };
    text.draw(
        3.0,
        4.0,
        "a long previously emitted label with many glyphs",
        &opts,
    );
    assert!(text.instances.len() > 2);
    library.inner.write().hinting ^= true;
    let width = text.draw_scaled(3.3, 4.7, "AV", &opts, 1.75);
    let mut fresh = Text::new(&library);
    fresh.init_cpu();
    fresh.set_scale_factor(1.25);
    let expected = fresh.draw_scaled(3.3, 4.7, "AV", &opts, 1.75);
    assert_eq!(width.to_bits(), expected.to_bits());
    assert!(text
        .instances
        .iter()
        .all(|instance| instance.raster_scale == 1.75));
    assert_same_emission_and_pixels(&text, &fresh);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn reused_font_key_replacement_matches_fresh_shaping_and_rasterization() {
    let library = fixture_library(true);
    let mut text = Text::new(&library);
    text.init_cpu();
    text.set_scale_factor(1.25);
    let opts = DrawOpts {
        font_size: 24.0,
        font_id: Some(0),
        ..DrawOpts::default()
    };
    let source = "AV office ffi 0123";
    // Populate both Swash contexts, not just the outer shape/atlas maps.
    text.draw(2.3, 5.7, source, &opts);
    assert!(!text.instances.is_empty());
    let token = text.font_cache_generation();
    replace_primary_reusing_key(&library, false);
    assert_ne!(token, text.font_cache_generation());
    let width = text.draw(2.3, 5.7, source, &opts);
    let actual = text.shape_for(source, &opts).unwrap();

    let mut fresh = Text::new(&library);
    fresh.init_cpu();
    fresh.set_scale_factor(1.25);
    let expected_width = fresh.draw(2.3, 5.7, source, &opts);
    let expected = fresh.shape_for(source, &opts).unwrap();
    assert_eq!(width.to_bits(), expected_width.to_bits());
    assert_eq!(actual.len(), expected.len());
    for (a, b) in actual.iter().zip(expected.iter()) {
        assert_eq!(a.baseline_px, b.baseline_px);
        assert_eq!(a.advance.to_bits(), b.advance.to_bits());
        assert_eq!(a.glyphs.len(), b.glyphs.len());
        for (a, b) in a.source_glyphs().zip(b.source_glyphs()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.x.to_bits(), b.x.to_bits());
            assert_eq!(a.y.to_bits(), b.y.to_bits());
            assert_eq!(a.advance.to_bits(), b.advance.to_bits());
            assert_eq!((a.cluster, a.cluster_end), (b.cluster, b.cluster_end));
        }
    }
    assert_same_emission_and_pixels(&text, &fresh);
}

#[test]
fn primary_metrics_follow_current_face_for_differently_observing_renderers() {
    let library = fixture_library(true);
    let mut early = Text::new(&library);
    let mut late = Text::new(&library);
    let opts = DrawOpts {
        font_size: 32.0,
        ..DrawOpts::default()
    };
    let old_metrics = library.inner.write().get_ui_font_metrics(&0, 32.0).unwrap();
    let old_baseline = early.baseline_offset(&opts);
    assert_eq!(late.baseline_offset(&opts), old_baseline);
    let late_token = late.font_cache_generation();

    replace_primary_reusing_key(&library, false);
    // The shared library must get correct metrics even before either Text
    // observes the replacement. A freshly allocated library is the reference.
    let independent = fixture_library(false);
    let expected_metrics = independent
        .inner
        .write()
        .get_ui_font_metrics(&0, 32.0)
        .unwrap();
    assert_ne!(
        old_metrics, expected_metrics,
        "fixtures need different line boxes"
    );
    assert_eq!(
        library.inner.write().get_ui_font_metrics(&0, 32.0).unwrap(),
        expected_metrics
    );
    let expected_baseline = Text::new(&independent).baseline_offset(&opts);
    assert_ne!(old_baseline, expected_baseline);
    assert_eq!(early.baseline_offset(&opts), expected_baseline);
    let early_token = early.font_cache_generation();
    assert_ne!(late.font_cache_generation(), late_token);
    assert_eq!(late.baseline_offset(&opts), expected_baseline);
    assert_ne!(late.font_cache_generation(), early_token);

    // Replacing again before the late renderer observes must remain correct;
    // no renderer-local invalidation controls the shared metrics cache.
    replace_primary_reusing_key(&library, true);
    assert_eq!(early.baseline_offset(&opts), old_baseline);
    assert_eq!(late.baseline_offset(&opts), old_baseline);
    assert_eq!(
        library.inner.write().get_ui_font_metrics(&0, 32.0).unwrap(),
        old_metrics
    );
}
