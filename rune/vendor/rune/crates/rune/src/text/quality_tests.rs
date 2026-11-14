use super::*;

#[test]
fn centred_wrapped_lines_centre_each_line_in_the_wrap_width() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let content = "a much longer first line of words that wraps\nshort";
    let width = 400.0;
    let start = text.layout_rich(content, &[], style, 2.0, Some(width));
    let centred = text.layout_aligned(content, &[], style, 2.0, Some(width), Alignment::Center);
    assert!(!Rc::ptr_eq(&start, &centred), "alignment is part of the cache key");
    let offsets = |l: &Layout<Ink>| {
        l.lines().map(|line| (line.metrics().offset, line.metrics().advance - line.metrics().hanging_advance)).collect::<Vec<_>>()
    };
    assert!(offsets(&start).iter().all(|(offset, _)| *offset == 0.0));
    let lines = offsets(&centred);
    assert!(lines.len() >= 2);
    for (offset, advance) in lines {
        assert!((offset - (width - advance) / 2.0).abs() < 1.0, "offset {offset} advance {advance}");
    }
}

/// Glyph extents per drawn line, in device pixels, for one text element 300 pt wide at 2×.
fn line_spans(content: &str, align: crate::Align, justify: bool) -> Vec<(f32, f32)> {
    use crate::{Cx, El, Env, Runtime};
    type S = (String, crate::Align, bool);
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = crate::el::text(s.0.clone()).w(300.0).wrap().align(s.1);
        if s.2 { t.justify() } else { t }
    }
    let mut rt = Runtime::new((content.to_string(), align, justify), view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let mut quads: Vec<[f32; 4]> =
        rt.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH).map(|i| i.rect).collect();
    quads.sort_by(|a, b| (a[1] + a[3] / 2.0).total_cmp(&(b[1] + b[3] / 2.0)));
    let mut lines: Vec<(f32, f32, f32)> = Vec::new();
    for q in quads {
        let cy = q[1] + q[3] / 2.0;
        match lines.last_mut() {
            Some(l) if cy - l.0 < 16.0 => { l.1 = l.1.min(q[0]); l.2 = l.2.max(q[0] + q[2]); }
            _ => lines.push((cy, q[0], q[0] + q[2])),
        }
    }
    lines.into_iter().map(|(_, l, r)| (l, r)).collect()
}

#[test]
fn justified_text_fills_each_wrapped_line_but_the_last() {
    use crate::Align;
    let content = "the quick brown fox jumps over the lazy dog while five boxing wizards jump quickly and pack my box with five dozen liquor jugs";
    let justified = line_spans(content, Align::Start, true);
    assert!(justified.len() >= 3, "{justified:?}");
    for &(l, r) in &justified[..justified.len() - 1] {
        assert!(l < 8.0 && r > 592.0, "justified line spans {l}..{r}");
    }
    let start = line_spans(content, Align::Start, false);
    assert_eq!(start.len(), justified.len(), "justifying never rewraps");
    assert!(start[..start.len() - 1].iter().any(|&(_, r)| r < 592.0), "start lines stay ragged: {start:?}");
}

#[test]
fn wrapped_right_to_left_text_aligns_and_justifies_from_the_right() {
    use crate::Align;
    let hebrew = "שלום עולם זהו טקסט ארוך שנשבר לכמה שורות כדי לבדוק יישור מימין לשמאל בתוך רוחב קבוע של האלמנט";
    let start = line_spans(hebrew, Align::Start, false);
    if start.is_empty() {
        eprintln!("skipping: no Hebrew glyphs");
        return;
    }
    assert!(start.len() >= 2, "{start:?}");
    assert!(start.iter().all(|&(_, r)| r > 592.0), "wrapped RTL start hugs the right edge: {start:?}");
    let end = line_spans(hebrew, Align::End, false);
    assert!(end.iter().all(|&(l, _)| l < 8.0), "wrapped RTL end hugs the left edge: {end:?}");
    let justified = line_spans(hebrew, Align::Start, true);
    let last = justified.len() - 1;
    assert!(justified[..last].iter().all(|&(l, r)| l < 8.0 && r > 592.0), "{justified:?}");
    assert!(justified[last].1 > 592.0, "justified RTL last line starts at the right: {justified:?}");
}

/// Inline code between padding spaces in the body font draws one chip at the code's height,
/// with no separate sliver for each padding run.
#[test]
fn inline_code_and_its_padding_share_one_chip() {
    let mut text = TextSystem::default();
    let bg = Color::BLACK.alpha(0.08);
    let content = "say \u{202f}code\u{202f} now";
    let (a, b) = (content.find('c').unwrap(), content.find(" now").unwrap());
    let pad = |s: usize, e: usize| Span { bg: Some(bg), ..Span::new(s, e) };
    let code = Span { mono: true, size: Some(13.0 * 0.9), bg: Some(bg), ..Span::new(a, b - 3) };
    let spans = [pad(4, a), code.clone(), pad(b - 3, b)];
    let style = TextStyle::new(13.0, 400.0);
    let layout = text.layout_rich(content, &spans, style, 2.0, None);
    let mut decos = Vec::new();
    text.decorations(&layout, &spans, 2.0, [0.0, 0.0], &mut decos);
    assert_eq!(decos.len(), 1, "{:?}", decos.iter().map(|d| d.rect).collect::<Vec<_>>());
    let mut alone = Vec::new();
    let code_only = text.layout_rich(content, std::slice::from_ref(&code), style, 2.0, None);
    text.decorations(&code_only, std::slice::from_ref(&code), 2.0, [0.0, 0.0], &mut alone);
    assert_eq!((decos[0].rect[1], decos[0].rect[3]), (alone[0].rect[1], alone[0].rect[3]), "the code's height");
    assert!(decos[0].rect[2] > alone[0].rect[2], "padding widens it");
}

/// A pill's chip has room of its own: its label never wraps inside while it fits a line, no
/// other glyph or chip overlaps it at any such width, its icon fits inside it, and a caret at its edge sits outside.
#[test]
fn pill_chips_keep_room_and_never_overlap_at_wraps() {
    static DOT: Icon = Icon { name: "dot", view: 24.0, body: "<circle cx=\"12\" cy=\"12\" r=\"8\"/>" };
    let mut text = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let content = "ask @code-reviewer about @src/main.rs and @notes today please";
    let pills: Vec<PillChip> = ["@code-reviewer", "@src/main.rs", "@notes"]
        .iter()
        .enumerate()
        .map(|(k, label)| {
            let start = content.find(label).unwrap() as u32;
            PillChip { start, end: start + label.len() as u32, icon: (k == 1).then_some(&DOT), bg: None, fg: None, bare: None }
        })
        .collect();
    let scale = 2.0;
    let bare = text.layout_blocks(content, &[], &[], &[], style, scale, None, Alignment::Start);
    let one = text.layout_blocks(content, &[], &[], &pills, style, scale, None, Alignment::Start);
    let pm = PillMetrics::new(14.0);
    let room = (pm.lead(false) + pm.lead(true) + pm.lead(false) + 3.0 * pm.trail()) * scale;
    assert!((one.full_width() - bare.full_width() - room).abs() < 4.0, "{} vs {}", one.full_width(), bare.full_width());

    let glyph_boxes = |layout: &Layout<Ink>| {
        let mut out = Vec::new();
        for (li, line) in layout.lines().enumerate() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
                let mut x = gr.offset();
                for cluster in gr.run().visual_clusters() {
                    let w = cluster.advance();
                    out.push((li, cluster.text_range(), x, x + w));
                    x += w;
                }
            }
        }
        out
    };
    for width in (123..=420).step_by(9) {
        let max = width as f32 * scale;
        let layout = text.layout_blocks(content, &[], &[], &pills, style, scale, Some(max), Alignment::Start);
        let (mut chips, mut icons) = (Vec::new(), Vec::new());
        pill_chips(&layout, &pills, 14.0, scale, [0.0, 0.0], Color::BLACK, &mut chips, &mut icons);
        assert_eq!(chips.len(), 3, "each pill draws one chip at width {width}");
        assert_eq!(icons.len(), 1);
        let (_, px, at, _) = icons[0];
        let chip = chips[1].rect;
        assert!(at[0] >= chip[0] && at[0] + px as f32 <= chip[0] + chip[2] + 0.5, "icon inside its chip at width {width}");
        let line_of = |byte: usize| layout.lines().position(|l| l.text_range().contains(&byte)).unwrap();
        for p in &pills {
            assert_eq!(line_of(p.start as usize), line_of(p.end as usize - 1), "no wrap inside a pill at width {width}");
        }
        for (k, c) in chips.iter().enumerate() {
            let r = c.rect;
            let fits = r[0] + r[2] <= max + 0.5 || r[0] <= pm.margin * scale + 0.5;
            assert!(fits, "chip {k} {r:?} overflows {max} at width {width}");
            for (j, d) in chips.iter().enumerate().filter(|&(j, _)| j != k) {
                let same_row = (r[1] - d.rect[1]).abs() < 1.0;
                assert!(!same_row || r[0] + r[2] <= d.rect[0] + 0.5 || d.rect[0] + d.rect[2] <= r[0] + 0.5, "chips {k} and {j} overlap at width {width}");
            }
        }
        for (li, range, x0, x1) in glyph_boxes(&layout) {
            for (p, c) in pills.iter().zip(&chips) {
                let inside = (p.start as usize) <= range.start && range.end <= p.end as usize;
                let row = line_of(p.start as usize) == li;
                let ws = content[range.clone()].trim().is_empty();
                if row && !inside && !ws {
                    assert!(x1 <= c.rect[0] + 0.5 || c.rect[0] + c.rect[2] <= x0 + 0.5, "glyph {:?} overlaps a chip at width {width}", &content[range.clone()]);
                }
                if inside {
                    assert!(x0 >= c.rect[0] && x1 <= c.rect[0] + c.rect[2] + 0.5, "label {:?} {x0}..{x1} inside its chip {:?} at width {width}: {:?}", &content[range.clone()], c.rect, layout.lines().map(|l| l.text_range()).collect::<Vec<_>>());
                }
            }
        }
        for (p, c) in pills.iter().zip(&chips) {
            for (at, before) in [(p.start as usize, true), (p.end as usize, false)] {
                let cur = parley::editing::Cursor::from_byte_index(&layout, at, parley::Affinity::Downstream);
                let x = cursor_geometry(&cur, &layout, 1.0).x0 as f32;
                let line = line_of(at.min(content.len() - 1));
                if line == line_of(p.start as usize) {
                    let outside = if before { x <= c.rect[0] + 0.5 } else { x >= c.rect[0] + c.rect[2] - 0.5 };
                    assert!(outside, "caret at {at} ({x}) inside chip {:?} at width {width}", c.rect);
                }
            }
        }
    }
}

/// A pill wider than its line wraps like the text it labels (as a browser wraps a span) instead
/// of running past the line: one chip per line, each inside the line, over its part of the label.
#[test]
fn a_pill_wider_than_its_line_wraps_inside_it() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let label = "@Migrate config loader to the new schema";
    let content = format!("as in {label} did");
    let start = content.find(label).unwrap() as u32;
    let pills = [PillChip { start, end: start + label.len() as u32, icon: None, bg: None, fg: None, bare: None }];
    let scale = 2.0;
    for width in (90..=200).step_by(7) {
        let max = width as f32 * scale;
        let layout = text.layout_blocks(&content, &[], &[], &pills, style, scale, Some(max), Alignment::Start);
        let (mut chips, mut icons) = (Vec::new(), Vec::new());
        pill_chips(&layout, &pills, 14.0, scale, [0.0, 0.0], Color::BLACK, &mut chips, &mut icons);
        let rows: Vec<_> = layout.lines().filter(|l| { let r = l.text_range(); r.start < pills[0].end as usize && (pills[0].start as usize) < r.end }).collect();
        assert!(rows.len() >= 2, "the label wraps at width {width}");
        assert_eq!(chips.len(), rows.len(), "one chip per line at width {width}");
        assert!(layout.width() <= max + 0.5, "the text stays inside {max} at width {width}: {}", layout.width());
        for c in &chips {
            let r = c.deco.rect;
            assert!(r[0] >= -0.5 && r[0] + r[2] <= max + 0.5, "chip {r:?} inside {max} at width {width}");
        }
    }
}

#[test]
fn live_layouts_do_not_accumulate_between_commits() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let stable = text.layout_rich("stable label", &[], style, 2.0, None);
    for tick in 0..256 { text.layout_live(&tick.to_string(), style, 2.0); }
    assert_eq!(text.layouts.len(), 1);
    assert!(Rc::ptr_eq(&stable, &text.layout_rich("stable label", &[], style, 2.0, None)));
}

#[test]
fn a_view_shown_again_after_a_while_does_not_reshape() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let row = text.layout_rich("Back to the list", &[], style, 2.0, None);
    for _ in 0..8 { text.end_commit(); }
    let shaped = text.shaped;
    assert!(Rc::ptr_eq(&row, &text.layout_rich("Back to the list", &[], style, 2.0, None)));
    assert_eq!(text.shaped, shaped);
}

#[test]
fn stale_layouts_stay_within_their_budget_newest_kept() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let labels: Vec<String> = (0..1200).map(|i| format!("label {i:04}")).collect();
    for label in &labels {
        text.layout_rich(label, &[], style, 2.0, None);
        text.end_commit();
    }
    // Two more, so even the last label is stale.
    for _ in 0..2 { text.end_commit(); }
    let cost: usize = text.layouts.values().map(|e| e.cost as usize).sum();
    assert!(cost <= STALE_BYTES, "{cost}");
    assert!(cost > STALE_BYTES * 9 / 10, "{cost}");
    let key = |label: &str| text_hash(label, &[], style, 2.0, None);
    assert!(text.layouts.contains_key(&key(&labels[1199])));
    assert!(!text.layouts.contains_key(&key(&labels[0])));
}

/// A view's labels all share the commit it was last shown in.
#[test]
fn a_view_over_the_budget_keeps_a_budget_of_its_layouts() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let labels: Vec<String> = (0..1200).map(|i| format!("label {i:04}")).collect();
    let show = |text: &mut TextSystem| for label in &labels { text.layout_rich(label, &[], style, 2.0, None); };
    show(&mut text);
    for _ in 0..2 {
        text.end_commit();
        assert_eq!(text.layouts.len(), 1200, "the current and previous commits are never evicted");
    }
    text.end_commit();
    let cost: usize = text.layouts.values().map(|e| e.cost as usize).sum();
    assert!(cost <= STALE_BYTES, "{cost}");
    assert!(cost > STALE_BYTES * 9 / 10, "{cost}");
    let kept = text.layouts.len();
    let shaped = text.shaped;
    show(&mut text);
    assert_eq!((text.shaped - shaped) as usize, 1200 - kept, "shown again, only what was dropped reshapes");
    for _ in 0..2 {
        text.end_commit();
        assert_eq!(text.layouts.len(), 1200, "the current and previous commits are never evicted");
    }
}

#[test]
fn optical_size_default_keeps_static_font_glyphs_byte_identical() {
    let mut a = TextSystem::default(); let mut b = TextSystem::default();
    for scale in [1.0, 2.0] {
        for size in [11.0, 13.0, 24.0] {
            let style = TextStyle::new(size, 400.0);
            let mut qa = Vec::new(); let mut qb = Vec::new();
            a.glyphs("Hello ffi 0123456789", style, scale, [0.0, 0.0], &mut qa);
            b.glyphs("Hello ffi 0123456789", style.optical_sizing(OpticalSize::FontDefault), scale, [0.0, 0.0], &mut qb);
            assert_eq!(qa.iter().map(|q| (q.rect, q.uv)).collect::<Vec<_>>(), qb.iter().map(|q| (q.rect, q.uv)).collect::<Vec<_>>());
            assert_eq!(a.atlas.data, b.atlas.data);
        }
    }
}

#[test]
fn moving_glyphs_reuse_four_phases_and_land_without_atlas_work() {
    let mut text = TextSystem::default();
    let layout = text.layout_rich("lHEx ", &[], TextStyle::new(13.0, 400.0), 2.0, None);
    let mut quads = Vec::new();
    // Negative origins exercise phase carry as well as both sides of zero.
    for step in 0..256 {
        quads.clear();
        text.glyphs_for_layout(&layout, 2.0, [-2.0 + step as f32 / 64.0, 12.37], false, &mut quads);
        assert_eq!(quads.len(), 4, "dilation must not turn whitespace into empty draw instances");
        assert!(quads.iter().all(|g| g.rect[0].fract() == 0.0 && g.rect[1].fract() == 0.0));
    }
    let entries = text.atlas.map.len();
    assert!(entries <= 5 * 4, "four glyphs plus cached whitespace have at most four x phases");
    let mut track = rune_motion::Animated::new(-2.0f32).with_class(rune_motion::PropClass::Spatial);
    track.animate_to(8.0, rune_motion::Spring::new(0.3, 0.0), 0.0);
    let end = track.settle_time();
    assert!(end.is_finite());
    text.atlas.dirty = None;
    quads.clear();
    text.glyphs_for_layout(&layout, 2.0, [8.0, 12.37], false, &mut quads);
    let landed: Vec<_> = quads.iter().map(|q| (q.rect, q.uv)).collect();
    for frame in 0..120 {
        let x = track.sample(end + frame as f64 / 120.0);
        assert_eq!(x, 8.0, "the analytic tail lands on its target exactly");
        quads.clear();
        text.glyphs_for_layout(&layout, 2.0, [x, 12.37], false, &mut quads);
        assert!(quads.iter().map(|q| (q.rect, q.uv)).eq(landed.iter().copied()));
    }
    assert_eq!(text.atlas.map.len(), entries);
    assert!(text.atlas.dirty.is_none());
}

#[test]
fn glyphs_at_the_same_device_ppem_share_a_mask() {
    let mut text = TextSystem::default();
    text.set_font_smoothing(FontSmoothing::On);
    let small = text.layout_rich("H", &[], TextStyle::new(12.0, 400.0), 2.0, None);
    let large = text.layout_rich("H", &[], TextStyle::new(24.0, 400.0), 1.0, None);
    let mut a = Vec::new(); let mut b = Vec::new();
    // CoreGraphics dilates by device ppem alone, so the two share a mask.
    text.glyphs_for_layout(&small, 2.0, [0.0, 0.0], false, &mut a);
    text.glyphs_for_layout(&large, 1.0, [0.0, 0.0], false, &mut b);
    assert_eq!(a[0].uv, b[0].uv, "same device ppem and ink, same CoreGraphics mask");
}

#[test]
fn atlas_capacity_never_overwrites_current_or_pinned_placements() {
    let mut atlas = Atlas::new();
    let side = ATLAS_SIZE - 2;
    let bitmap = vec![137; (side * side) as usize];
    for page in 0..MAX_OVERFLOW_PAGES {
        let g = atlas.put(side, side, &bitmap, 0, 0).unwrap();
        assert_eq!(g.page as usize, page);
    }
    assert!(atlas.put(side, side, &bitmap, 0, 0).is_none());
    assert_eq!(atlas.resets(), 0, "capacity must not invalidate emitted quads");
    // Simulate the next frame retaining all eight pages in text flights.
    atlas.begin_frame((1 << MAX_OVERFLOW_PAGES) - 1);
    assert!(atlas.put(side, side, &bitmap, 0, 0).is_none());
    assert_eq!(atlas.resets(), 0, "pinned pages cannot be recycled either");
    for page in 0..MAX_OVERFLOW_PAGES {
        assert_eq!(atlas.page_data(page).unwrap()[ATLAS_SIZE as usize + 1], 137);
    }
    atlas.begin_frame(0);
    assert!(atlas.put(side, side, &bitmap, 0, 0).is_some(), "failure is retryable next frame");
}

#[test]
fn atlas_eviction_preserves_cached_empty_outlines() {
    let mut atlas = Atlas::new();
    let key = GlyphKey { font: 0, index: 0, glyph: 0, size: 0, coords: 0, subpx: 0, subpy: 0, dilation: 0, apple: 0, skew: 0 };
    atlas.map.insert(key, super::atlas_fill::Residency::Blank);
    atlas.icons.insert((0, 0), None);
    atlas.evict_page(0);
    assert!(atlas.map.contains_key(&key));
    assert!(atlas.icons.contains_key(&(0, 0)));
}

#[test]
fn colour_budget_retires_unused_scales_without_flushing_active_images() {
    let mut text = TextSystem::default();
    let key = GlyphKey { font: 0, index: 0, glyph: 0, size: 0, coords: 0, subpx: 0, subpy: 0, dilation: 0, apple: 0, skew: 0 };
    let pixels = crate::pixels::Pixels::new();
    pixels.set(1, 1, vec![255; 4]);
    text.color_pixels = vec![pixels.clone(), pixels];
    let active = ColorGlyph { index: 2, used: true, left: 0, top: 0, w: 4096, h: 4096 };
    text.color_glyphs.insert(key, ColorGlyph { index: 1, used: false, ..active });
    let active_key = GlyphKey { glyph: 1, ..key };
    text.color_glyphs.insert(active_key, active);
    text.color_bytes = COLOR_GLYPH_BUDGET + 1;
    text.begin_frame();
    assert!(!text.color_glyphs.contains_key(&key));
    assert_eq!(text.color_pixels.len(), 1);
    assert_eq!(text.color_glyphs[&active_key].index, 1);
    assert_eq!(text.color_glyph(1).rgba(), &[255; 4]);
    // An over-budget visible working set is reused, not flushed every frame.
    text.color_glyphs.get_mut(&active_key).unwrap().used = true;
    text.begin_frame();
    assert_eq!(text.color_glyphs.len(), 1);
    text.begin_frame();
    assert!(text.color_glyphs.is_empty(), "unused working set can be retired");
    assert_eq!(text.color_bytes, 0);
}

#[test]
fn four_hundred_scales_never_exhaust_the_atlas_or_poison_glyphs() {
    const ICON: Icon = Icon {
        name: "box",
        view: 16.0,
        body: r#"<rect x="2" y="2" width="12" height="12" fill="currentColor"/>"#,
    };
    let mut text = TextSystem::default();
    // Unplaceable request must not poison the icon cache with `Some(None)`.
    assert!(text.icon_quad(&ICON, ATLAS_SIZE, [0.0, 0.0]).is_none());
    assert!(!text.atlas.icons.contains_key(&(&ICON as *const Icon as usize, ATLAS_SIZE)));

    let sample = "The quick brown fox jumps over the lazy dog 0123456789";
    let mut quads = Vec::new();
    for i in 0..400 {
        text.begin_frame();
        quads.clear();
        let size = 18.0 + (i as f32) * 0.45;
        text.glyphs(sample, TextStyle::new(size, 400.0), 2.0, [0.0, 0.0], &mut quads);
        let expected = sample.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(quads.len(), expected, "missing glyph at step {i} (size={size})");
        let icon_px = 24 + (i % 96) as u32;
        assert!(text.icon_quad(&ICON, icon_px, [0.0, 0.0]).is_some(), "missing icon at step {i}");
    }
    assert!(text.atlas.resets() >= 4, "400 scales must cycle the atlas multiple times (resets={})", text.atlas.resets());
    assert!(text.atlas.page_count() <= MAX_STEADY_PAGES, "steady single-page frames stay within 2 pages");

    // Fresh string at scale 1.0 after 400 scales must have every glyph present and non-empty.
    text.begin_frame();
    let fresh = "Sphinx of black quartz, judge my vow! 9876543210";
    quads.clear();
    text.glyphs(fresh, TextStyle::new(16.0, 400.0), 1.0, [0.0, 0.0], &mut quads);
    let expected_fresh = fresh.chars().filter(|c| !c.is_whitespace()).count();
    assert_eq!(quads.len(), expected_fresh, "every glyph in the fresh string at scale 1 must be present");
    for (idx, q) in quads.iter().enumerate() {
        let page = (q.uv[1] as u32 / ATLAS_SIZE) as usize;
        let (ax, ay) = (q.uv[0] as u32, q.uv[1] as u32 % ATLAS_SIZE);
        let (w, h) = (q.rect[2] as u32, q.rect[3] as u32);
        let buf = if page == 0 { &text.atlas.data } else { &text.atlas.pages[page - 1].data };
        let mut sum = 0u64;
        for row in 0..h {
            let start = ((ay + row) * ATLAS_SIZE + ax) as usize;
            sum += buf[start..start + w as usize].iter().map(|&b| b as u64).sum::<u64>();
        }
        assert!(sum > 0, "fresh glyph {idx} must have nonzero coverage in page {page}");
    }
    // Whitespace is still cached as `Some(None)` so spaces don't re-enter swash every frame.
    assert!(text.atlas.map.values().any(|v| v.mask().is_none()), "empty space glyph is cached as None");
}

#[test]
fn single_frame_exceeding_one_page_places_every_glyph_across_overflow_pages() {
    const ICON: Icon = Icon {
        name: "box",
        view: 16.0,
        body: r#"<rect x="1" y="1" width="14" height="14" fill="currentColor"/>"#,
    };
    let mut text = TextSystem::default();
    text.begin_frame();
    let mut placed = Vec::new();
    // 32 icons around 400×400 px each (~5.1M texels) exceed one 2048×2048 page (4.19M texels) in a single frame.
    for i in 0..32u32 {
        let px = 380 + i;
        let q = text.icon_quad(&ICON, px, [0.0, 0.0]).expect("overflow page must accommodate single-frame burst");
        placed.push((px, q));
    }
    assert!(text.atlas.page_count() >= 2, "single frame exceeding 4M texels must spill to a second page");
    assert_eq!(text.atlas.resets(), 0, "must not evict a page already touched in the current frame");
    for (px, q) in &placed {
        let page = (q.uv[1] as u32 / ATLAS_SIZE) as usize;
        let (ax, ay) = (q.uv[0] as u32, q.uv[1] as u32 % ATLAS_SIZE);
        let buf = if page == 0 { &text.atlas.data } else { &text.atlas.pages[page - 1].data };
        let mid = ((ay + px / 2) * ATLAS_SIZE + ax + px / 2) as usize;
        assert_eq!(buf[mid], 255, "glyph at {px}px on page {page} must retain its coverage");
    }
}

#[test]
fn span_ring_subranges_case_elision_and_cache_changes() {
    use crate::{Cx, El, Env, Link, LinkPaint, Runtime, TextCase, col, rich};
    use crate::render::KIND_RECT;

    fn view(color: &Color, _: &mut Cx<Color>) -> El {
        col().child(rich("aa Straße zz", vec![Span::new(0, 13).bg(Color::WHITE, 3.0)])
            .span_ring(3, 10, *color, 1.0).text_case(TextCase::Uppercase)
            .link(Link::new(0..13, "whole").hover(LinkPaint::new().bg(Color::BLACK)))
            .on_link(|_: &mut Color, _, _| {}))
    }
    let mut rt = Runtime::new(Color::BLACK, view, (300.0, 80.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let ring = *rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap();
    let rects: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_RECT).collect();
    assert_eq!(rects.len(), 3, "ring boundaries split one background into three pieces");
    assert!(ring.rect[0] > rects[0].rect[0] && ring.rect[2] < rects.iter().map(|i| i.rect[2]).sum());
    let bounds = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("whole")).unwrap().rect;
    rt.pointer_move(bounds[0] + 2.0, bounds[1] + 2.0);
    rt.frame(0.01);
    let hovered: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_RECT).collect();
    assert_eq!(hovered.len(), 3, "hover recolors split backgrounds in place");
    assert_eq!(hovered.iter().filter(|i| i.params[0] == 1.0).count(), 1, "hover does not extend the ring outside its range");
    assert_eq!(hovered.iter().find(|i| i.params[0] == 1.0).unwrap().rect, ring.rect);
    assert!(hovered.iter().all(|i| i.fill == Color::BLACK.premul(1.0)));
    rt.pointer_move(299.0, 79.0);
    rt.state = Color::WHITE;
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.0);
    let changed = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap();
    assert_eq!(changed.border, Color::WHITE.premul(1.0), "ring color invalidates cached paint");
    assert_eq!(changed.rect, ring.rect);

    let mut cased = rich("İ xy", vec![Span::new(0, 5).bg(Color::WHITE, 2.0)])
        .span_ring(3, 5, Color::BLACK, 1.0).text_case(TextCase::Lowercase).text.unwrap();
    cased.apply_case();
    assert_eq!(cased.content, "i\u{307} xy");
    assert_eq!((cased.rings[0].0, cased.rings[0].1), (4, 6));
    assert_eq!((cased.spans[1].start, cased.spans[1].end), (4, 6));

    fn truncated(_: &(), _: &mut Cx<()>) -> El {
        col().w(60.0).child(rich("prefix middle suffix", vec![Span::new(14, 20).bg(Color::WHITE, 2.0)])
            .w(60.0).truncate_head().span_ring(14, 20, Color::BLACK, 1.0))
    }
    let mut cut = Runtime::new((), truncated, (60.0, 40.0), Env::new(false, 1.0));
    cut.frame(0.0);
    let rings: Vec<_> = cut.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).collect();
    assert_eq!(rings.len(), 1, "head truncation remaps the suffix ring");
    assert!(rings[0].rect[0] >= 0.0 && rings[0].rect[2] <= 64.0);
}

#[test]
fn span_ring_without_background_is_empty_and_reveal_fades_the_chip() {
    use crate::{Cx, El, Env, Runtime, col, rich, text};
    use crate::render::KIND_RECT;
    fn plain(_: &(), _: &mut Cx<()>) -> El { text("plain").span_ring(0, 5, Color::BLACK, 1.0) }
    let mut plain = Runtime::new((), plain, (100.0, 40.0), Env::new(false, 1.0));
    plain.frame(0.0);
    assert!(!plain.instances.iter().any(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0));

    fn view(s: &String, _: &mut Cx<String>) -> El {
        col().key("root").streaming(true).child(rich(s, vec![Span::new(0, s.len()).bg(Color::WHITE, 3.0)])
            .key("body").span_ring(0, s.len(), Color::BLACK, 1.0).reveal(0.2))
    }
    let mut rt = Runtime::new("one".to_string(), view, (300.0, 80.0), Env::new(false, 1.0));
    rt.frame(0.0);
    rt.state.push_str(" two");
    rt.invalidate();
    rt.frame(0.01);
    let initial = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap().border[3];
    assert!(initial < 0.2, "new chip is not opaque before its final word: {initial}");
    rt.frame(0.11);
    let middle = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap();
    assert!(middle.border[3] > initial && middle.border[3] < 1.0);
    assert_eq!(middle.border[3], middle.fill[3]);
    rt.frame(1.0);
    assert_eq!(rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap().border[3], 1.0);
}

#[test]
fn span_ring_draws_inset_border_on_light_page_merges_runs_wraps_and_stays_on_hover() {
    use crate::render::{Gpu, KIND_RECT, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::{Cx, El, Env, Link, LinkPaint, Runtime, col, rich};

    const PAGE: Color = Color { r: 0.96, g: 0.96, b: 0.97, a: 1.0 };
    const FILL: Color = Color { r: 0.95, g: 0.95, b: 0.96, a: 1.0 };
    const HOVER: Color = Color { r: 0.88, g: 0.90, b: 0.96, a: 1.0 };
    const RING: Color = Color { r: 0.12, g: 0.38, b: 0.88, a: 1.0 };

    fn view(alpha: &f32, _: &mut Cx<f32>) -> El {
        let content = " Try  bold code  now";
        let (a, b) = (1, content.find(" now").unwrap());
        let bold_a = content.find("bold").unwrap();
        let code_a = content.find("code").unwrap();
        let spans = vec![
            Span::new(a, b).bg(FILL, 6.0),
            Span::new(bold_a, bold_a + 4).bold(),
            Span::new(code_a, code_a + 4).mono(),
        ];
        col()
            .size(200.0, 44.0)
            .fill(PAGE)
            .padding(8.0)
            .child(
                rich(content, spans)
                    .font(TextStyle::new(14.0, 400.0))
                    .color(Color::BLACK)
                    .opacity(*alpha)
                    .span_ring(a, b, RING, 1.0)
                    .link(Link::new(a..b, "chip").hover(LinkPaint::new().bg(HOVER)))
                    .on_link(|_: &mut f32, _, _| {}),
            )
    }

    let mut rt = Runtime::new(1.0f32, view, (200.0, 44.0), Env::new(false, 2.0));
    rt.frame(0.0);
    let chips: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).copied().collect();
    assert_eq!(chips.len(), 1, "plain, bold and mono runs merge into one ringed chip: {chips:?}");
    let chip = chips[0];
    assert_eq!((chip.radii, chip.params[0]), ([8.0; 4], 2.0), "4 pt radius and 1 pt ring at 2x");
    assert_eq!((chip.fill, chip.border), (FILL.premul(1.0), RING.premul(1.0)));

    // Offscreen GPU readback in a light theme where fill ≈ page colour: the ring is visible
    // at the inset edge while the interior shows the fill.
    let gpu = Gpu::headless();
    let size = (400u32, 88u32);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    let out = target.read(&gpu.device);
    let px = |x: usize, y: usize| -> [u8; 4] { out[(y * size.0 as usize + x) * 4..][..4].try_into().unwrap() };

    let [x0, y0, w, h] = chip.rect.map(|v| v as usize);
    let mid_x = x0 + w / 2;
    let mid_y = y0 + h / 2;
    let outside = px(mid_x, y0 - 2);
    let top_ring = px(mid_x, y0 + 1);
    let bot_ring = px(mid_x, y0 + h - 2);
    let left_ring = px(x0 + 1, mid_y);
    // Inside the left padding (past the 2 px ring, before the first glyph): fill ≈ page, not ring.
    let inside_fill = px(x0 + 5, mid_y);
    assert!(outside[0] > 235 && outside[2] > 235 && (outside[0] as i32 - outside[2] as i32).abs() <= 5, "page outside: {outside:?}");
    for (edge, p) in [("top", top_ring), ("bottom", bot_ring), ("left", left_ring)] {
        assert!(
            (p[0] as i32 - 97).abs() <= 2 && (p[1] as i32 - 166).abs() <= 2 && (p[2] as i32 - 241).abs() <= 2 && p[3] == 255,
            "{edge} inset ring pixel is ring-coloured: {p:?}"
        );
    }
    assert!(inside_fill[0] > 230 && inside_fill[2] > 230 && (inside_fill[2] as i32 - inside_fill[0] as i32) < 10, "interior pixel is fill, not ring: {inside_fill:?}");
    // Outside the 8 px rounded corner radius, the corner pixel stays on the page background.
    let corner_out = px(x0, y0);
    assert!(corner_out[0] > 235 && (corner_out[0] as i32 - corner_out[2] as i32).abs() <= 5, "rounded corner cuts outside pixel: {corner_out:?}");

    // A partial-range ring at fractional scale keeps its right edge even though the
    // following unringed background overlaps it by bg_pad.
    fn partial(_: &(), _: &mut Cx<()>) -> El {
        col().fill(PAGE).padding(8.0).child(
            rich("aa ring zz", vec![Span::new(0, 10).bg(FILL, 6.0)])
                .span_ring(3, 7, RING, 1.0),
        )
    }
    let mut partial_rt = Runtime::new((), partial, (200.0, 44.0), Env::new(false, 1.5));
    partial_rt.frame(0.0);
    let partial_chip = partial_rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).unwrap();
    renderer.prepare(&gpu.device, &gpu.queue, &mut partial_rt.text.atlas, &partial_rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    let partial_out = target.read(&gpu.device);
    let x = (partial_chip.rect[0] + partial_chip.rect[2] - 1.0) as usize;
    let y = (partial_chip.rect[1] + partial_chip.rect[3] * 0.5) as usize;
    let edge = &partial_out[(y * size.0 as usize + x) * 4..][..4];
    assert!(edge[2] as i32 - edge[0] as i32 > 50, "fractional right ring survives adjacent fill: {edge:?}");

    // Hover replaces the fill with HOVER while keeping the resting ring on the same rect.
    let link_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("chip")).unwrap();
    rt.pointer_move(link_el.rect[0] + 4.0, link_el.rect[1] + 4.0);
    rt.frame(1.0 / 60.0);
    let hovered = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).unwrap();
    assert_eq!((hovered.rect, hovered.fill, hovered.border, hovered.params[0]), (chip.rect, HOVER.premul(1.0), RING.premul(1.0), 2.0));

    // Fading the text element scales both the fill and the ring border alpha.
    rt.pointer_move(0.0, 0.0);
    rt.state = 0.5;
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.0);
    let faded = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).unwrap();
    assert!((faded.fill[3] - FILL.premul(0.5)[3]).abs() < 1e-3 && (faded.border[3] - RING.premul(0.5)[3]).abs() < 1e-3);

    // Fractional scale (1.5x) and wrapping across lines: each wrapped line gets its own ringed chip.
    fn wrapped(_: &(), _: &mut Cx<()>) -> El {
        let s = "one long ringed link that wraps across lines";
        col().w(90.0).child(
            rich(s, vec![Span::new(4, s.len() - 13).bg(FILL, 3.0)])
                .font(TextStyle::new(14.0, 400.0))
                .span_rings([(4, s.len() - 13, RING, 1.0)]),
        )
    }
    let mut wrap_rt = Runtime::new((), wrapped, (90.0, 120.0), Env::new(false, 1.5));
    wrap_rt.frame(0.0);
    let lines: Vec<_> = wrap_rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).collect();
    assert!(lines.len() >= 2, "wrapped span splits into one ringed rect per line: {lines:?}");
    for d in &lines {
        assert_eq!((d.radii, d.params[0], d.border), ([6.0; 4], 1.5, RING.premul(1.0)));
    }
}

/// Underline rects drawn at 2× for `words`, each its own underlined text element side by side
/// in a row; device px `(left, right)`, left to right.
fn underlines(words: &[&str]) -> Vec<(f32, f32)> {
    use crate::{Cx, El, Env, Runtime};
    fn view(s: &Vec<String>, _: &mut Cx<Vec<String>>) -> El {
        let line = crate::oklcha(0.6, 0.15, 260.0, 0.4);
        crate::row().children(s.iter().map(|w| crate::el::rich(w.clone(), vec![Span::new(0, w.len()).underline(line)])))
    }
    let mut rt = Runtime::new(words.iter().map(|w| w.to_string()).collect(), view, (600.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let mut out: Vec<(f32, f32)> = rt.instances.iter()
        .filter(|i| i.params[1] == crate::render::KIND_RECT && i.rect[3] == 2.0)
        .map(|i| (i.rect[0], i.rect[0] + i.rect[2]))
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// A link flowed a word an element underlines as one line: each word's line
/// meets the next one's at the space, with no gap where an element's width rounds up (t942).
#[test]
fn underlined_words_in_a_row_meet_without_gaps() {
    let lines = underlines(&["Watch ", "Kaizen ", "× ", "OpenCode ", "(4:12, ", "1080p60) ", "→"]);
    assert_eq!(lines.len(), 7, "{lines:?}");
    for w in lines.windows(2) {
        assert_eq!(w[0].1, w[1].0, "each line meets the next: {lines:?}");
    }
}
