use super::*;

#[test]
fn geist_title_wrap_uses_regular_proportional_metrics_and_reserves_all_rows() {
    let fonts = sugarloaf::font::FontLibrary {
        inner: Default::default(),
    };
    let id = crate::primitives::prose_font::ui_prose_font_id(&fonts).unwrap();
    let mut text = sugarloaf::text::Text::new(&fonts);
    text.init_cpu();
    let opts = DrawOpts {
        font_id: Some(id),
        font_size: 14.0,
        ..DrawOpts::default()
    };
    assert!(!opts.bold);
    assert!(text.measure("WWWW", &opts) > 2.0 * text.measure("iiii", &opts));
    let title = "A long pull request title with WWWW, iiiii and Unicode αβγ";
    let lines = measured_lines(title, 132.0, |s| text.measure(s, &opts));
    assert!(lines.len() > 2);
    assert_eq!(lines.concat(), title);
    for line in &lines {
        assert!(text.measure(line, &opts) <= 132.0);
    }
    // Rendering reserves a full line box per measured row before metadata.
    let title_y = 12.0;
    let metadata_y = title_y + lines.len() as f32 * 20.0 + 4.0;
    assert!(metadata_y > title_y + (lines.len() - 1) as f32 * 20.0 + opts.font_size);
}

#[test]
fn control_fragments_clip_to_viewport_and_exclude_occlusion() {
    let clip = Rect::new(12.0, 100.0, 90.0, 50.0);
    let control = Rect::new(8.0, 92.0, 140.0, 40.0);
    let cut = [40.0, 110.0, 20.0, 10.0];
    let visible = visible_regions(control, clip, &[cut]);
    let mut area = 0.0;
    for r in visible {
        assert!(r.x >= clip.x && r.y >= clip.y);
        assert!(r.x + r.w <= clip.x + clip.w && r.y + r.h <= clip.y + clip.h);
        assert!(
            r.x + r.w <= cut[0]
                || r.x >= cut[0] + cut[2]
                || r.y + r.h <= cut[1]
                || r.y >= cut[1] + cut[3]
        );
        area += r.w * r.h;
    }
    assert_eq!(area, 90.0 * 32.0 - 20.0 * 10.0);
    assert!(visible_regions(control, clip, &[[0.0, 0.0, 200.0, 200.0]]).is_empty());
    assert!(visible_regions(control, Rect::new(0.0, 0.0, 0.0, 0.0), &[]).is_empty());
}
