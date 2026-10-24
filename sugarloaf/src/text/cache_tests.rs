use super::*;
use crate::font::SugarloafFonts;

#[test]
fn warm_text_reuses_run_and_glyph_allocations_without_segmentation() {
    let (library, _) = FontLibrary::new(SugarloafFonts::default());
    let mut renderer = Text::new(&library);
    renderer.set_scale_factor(1.25);
    let source = "office\te\u{301} é \u{f07b} 🦀 tail";
    for preferred in [None, Some(0)] {
        let opts = DrawOpts {
            font_id: preferred,
            bold: true,
            italic: true,
            font_size: 13.3,
            ..DrawOpts::default()
        };
        // Lazy platform fallback discovery may mutate the library on the first
        // cold pass. Observe it before establishing the warm baseline.
        renderer.shape_for(source, &opts).unwrap();
        let runs = renderer.shape_for(source, &opts).unwrap();
        let counts = (
            renderer.segmentation_chars,
            renderer.shape_misses,
            renderer.full_text_cache.allocations,
            renderer.shape_cache.allocations,
        );
        let width = shaped_text_width(&runs) / renderer.scale_factor;
        for _ in 0..32 {
            let again = renderer.shape_for(source, &opts).unwrap();
            assert!(Arc::ptr_eq(&runs, &again));
            for (a, b) in runs.iter().zip(again.iter()) {
                assert!(Arc::ptr_eq(&a.glyphs, &b.glyphs));
                assert_eq!(a.source_offset, b.source_offset);
            }
            assert_eq!(renderer.measure(source, &opts).to_bits(), width.to_bits());
        }
        assert_eq!(
            counts,
            (
                renderer.segmentation_chars,
                renderer.shape_misses,
                renderer.full_text_cache.allocations,
                renderer.shape_cache.allocations
            )
        );
        // Color, clipping, extrusion and reveal are emission-only. They must
        // not fragment the shape cache or change any shaping geometry.
        let painted = DrawOpts {
            color: [10, 20, 30, 40],
            extrude: true,
            clip_rect: Some([0.3, 1.2, 17.1, 12.7]),
            ..opts
        };
        assert!(Arc::ptr_eq(
            &runs,
            &renderer.shape_for(source, &painted).unwrap()
        ));
        let effects = [TextReveal {
            range: 10..source.len(),
            opacity: 0.25,
            blur_radius: 1.7,
        }];
        let indexed = prepare_reveal_effects(&runs, &effects, 1.25);
        for g in runs.iter().flat_map(|r| r.source_glyphs()) {
            assert!(source.is_char_boundary(g.cluster as usize));
            assert!(source.is_char_boundary(g.cluster_end as usize));
            assert_eq!(
                indexed[&g.cluster],
                reveal_effect(g.cluster, g.cluster_end, &effects, 1.25)
            );
        }
    }
}

#[test]
fn rebasing_a_run_is_only_a_view_and_preserves_width_bits() {
    let (library, _) = FontLibrary::new(SugarloafFonts::default());
    let mut text = Text::new(&library);
    let run = text.shape_run_for("e\u{301}\tffi", 0, 56, 14, 0).unwrap();
    let view = offset_run_clusters(run.clone(), 17);
    assert!(Arc::ptr_eq(&run.glyphs, &view.glyphs));
    assert_eq!(shaped_width(&run).to_bits(), shaped_width(&view).to_bits());
    for (local, global) in run.source_glyphs().zip(view.source_glyphs()) {
        assert_eq!(global.cluster, local.cluster + 17);
        assert_eq!(global.cluster_end, local.cluster_end + 17);
    }
    let cached = text.shape_run_for("e\u{301}\tffi", 0, 56, 14, 0).unwrap();
    assert!(Arc::ptr_eq(&cached.glyphs, &view.glyphs));
    assert_eq!(cached.source_offset, 0);
}

#[test]
fn complete_style_and_scale_keys_do_not_alias() {
    let (library, _) = FontLibrary::new(SugarloafFonts::default());
    let mut text = Text::new(&library);
    let base = DrawOpts::default();
    let first = text.shape_for("AV ffi", &base).unwrap();
    for opts in [
        DrawOpts {
            font_size: 14.01,
            ..base
        },
        DrawOpts { bold: true, ..base },
        DrawOpts {
            italic: true,
            ..base
        },
        DrawOpts {
            font_id: Some(0),
            ..base
        },
    ] {
        let other = text.shape_for("AV ffi", &opts).unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
    }
    text.set_scale_factor(1.5);
    assert!(!Arc::ptr_eq(
        &first,
        &text.shape_for("AV ffi", &base).unwrap()
    ));
}

#[test]
fn generations_are_unique_and_observe_shared_mutations_before_measurement() {
    let (library, _) = FontLibrary::new(SugarloafFonts::default());
    let mut a = Text::new(&library);
    let mut b = Text::new(&library);
    let a0 = a.font_cache_generation();
    let b0 = b.font_cache_generation();
    assert_ne!(a0, b0);
    assert_eq!(a.font_cache_generation(), a0);
    library.inner.write().hinting ^= true;
    let a1 = a.font_cache_generation(); // no measure/draw needed
    assert_ne!(a1, a0);
    assert_eq!(a.font_cache_generation(), a1);
    let b1 = b.font_cache_generation();
    assert_ne!(b1, b0);
    assert_ne!(b1, a1);
    // A observes multiple intervening edits while B observes only the last.
    library
        .inner
        .write()
        .inner
        .get_mut(&0)
        .unwrap()
        .should_embolden ^= true;
    let a2 = a.font_cache_generation();
    library.inner.write().symbol_maps = Some(vec![crate::font::SymbolMap {
        font_index: 0,
        range: 'a'..'z',
    }]);
    let a3 = a.font_cache_generation();
    let b3 = b.font_cache_generation();
    let mut generations = vec![a0, b0, a1, b1, a2, a3, b3];
    let count = generations.len();
    generations.sort_unstable();
    generations.dedup();
    assert_eq!(generations.len(), count);
    // Same Arc and same public font key, but new immutable font allocation.
    let (mut replacement, old_key) = {
        let lib = library.inner.read();
        let bytes = lib
            .inner
            .values()
            .find_map(|font| font.data().as_ref())
            .expect("default library includes bundled symbol font bytes");
        (
            crate::font::FontData::from_slice(bytes.as_ref()).unwrap(),
            lib.inner[&0].key,
        )
    };
    replacement.key = old_key;
    library.inner.write().inner.insert(0, replacement);
    let a4 = a.font_cache_generation();
    assert_ne!(a4, a3);
    a.set_font_library(&library); // same-pointer explicit invalidation
    let a5 = a.font_cache_generation();
    assert_ne!(a4, a5);
    a.set_scale_factor(1.25);
    assert_ne!(a5, a.font_cache_generation());
    drop(a);
    let mut recreated = Text::new(&library);
    assert_ne!(recreated.font_cache_generation(), a5);
    assert_ne!(recreated.font_cache_generation(), b.font_cache_generation());
}

#[test]
fn pending_font_changes_invalidate_before_shape_and_prefix_hits() {
    let (library, _) = FontLibrary::new(SugarloafFonts::default());
    let mut text = Text::new(&library);
    let opts = DrawOpts::default();
    let before = text.shape_for("AV e\u{301} ffi", &opts).unwrap();
    text.measure_char_prefixes("AV", &opts);
    let token = text.font_cache_generation();
    assert!(!text.full_text_cache.is_empty());
    assert!(!text.prefix_measure_cache.is_empty());
    library.inner.write().hinting ^= true;
    assert_ne!(text.font_cache_generation(), token);
    assert!(text.full_text_cache.is_empty());
    assert!(text.shape_cache.is_empty());
    assert!(text.prefix_measure_cache.is_empty());
    assert!(text.font_coverage.is_empty());
    let after = text.shape_for("AV e\u{301} ffi", &opts).unwrap();
    assert!(!Arc::ptr_eq(&before, &after));
    for (a, b) in before.iter().zip(after.iter()) {
        assert!(!Arc::ptr_eq(&a.glyphs, &b.glyphs));
        assert_eq!(a.advance.to_bits(), b.advance.to_bits());
    }
}

#[test]
fn append_only_discovery_keeps_previously_emitted_instances() {
    let (library, _) = FontLibrary::new(SugarloafFonts::default());
    let mut text = Text::new(&library);
    let old = text.font_cache_generation();
    text.instances.push(TextInstance {
        pos: [3.0, 4.0],
        ..TextInstance::default()
    });
    let face = library.inner.read().inner[&0].clone();
    library.inner.write().insert(face);
    assert_ne!(old, text.font_cache_generation());
    assert_eq!(text.instances.len(), 1);
    assert_eq!(text.instances[0].pos, [3.0, 4.0]);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn fractional_dpi_clip_reveal_warm_and_cold_emission_match_exactly() {
    let (library, _) = FontLibrary::new(SugarloafFonts::default());
    let mut text = Text::new(&library);
    text.init_cpu();
    text.set_scale_factor(1.25);
    let source = "ffi\te\u{301} \u{f07b} tail";
    let opts = DrawOpts {
        font_size: 13.3,
        bold: true,
        italic: true,
        extrude: true,
        clip_rect: Some([2.3, 1.7, 45.9, 17.1]),
        ..DrawOpts::default()
    };
    let reveals = [TextReveal {
        range: 4..source.len(),
        opacity: 0.4,
        blur_radius: 1.3,
    }];
    let width = text.draw_revealed(0.3, 2.7, source, &opts, &reveals);
    let cold = text.instances.clone();
    text.clear();
    let warm_width = text.draw_revealed(0.3, 2.7, source, &opts, &reveals);
    assert_eq!(width.to_bits(), warm_width.to_bits());
    assert_eq!(cold.len(), text.instances.len());
    for (a, b) in cold.iter().zip(&text.instances) {
        assert_eq!(a.pos, b.pos);
        assert_eq!(a.bearings, b.bearings);
        assert_eq!(a.glyph_pos, b.glyph_pos);
        assert_eq!(a.glyph_size, b.glyph_size);
        assert_eq!(a.color, b.color);
        assert_eq!(a.clip_rect, b.clip_rect);
        assert_eq!(a.blur_radius, b.blur_radius);
        assert_eq!(a.atlas, b.atlas);
    }
}
